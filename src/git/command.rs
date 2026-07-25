//! Bounded, no-shell Git command execution for adapter-owned argument vectors.

use std::error::Error;
use std::ffi::OsStr;
use std::fmt::{self, Display, Formatter};
use std::io::Read;
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

const MAX_GIT_OUTPUT_BYTES: usize = 16 * 1024 * 1024;
const GIT_COMMAND_TIMEOUT: Duration = Duration::from_mins(5);
const GIT_POLL_INTERVAL: Duration = Duration::from_millis(10);
const GIT_CAPTURE_CHUNK_BYTES: usize = 8 * 1024;

/// Stable Git-adapter failure categories.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GitErrorKind {
    /// The Git executable, repository path, or required filesystem capability is unavailable.
    Unavailable,
    /// Git returned bytes that violate a stable machine-readable contract.
    InvalidOutput,
    /// A requested Git or binding operation violates Mino policy.
    PolicyViolation,
}

/// A typed Git-adapter failure with a stable category and explanatory message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitError {
    kind: GitErrorKind,
    message: String,
}

impl GitError {
    /// Creates a Git-adapter error.
    #[must_use]
    pub fn new(kind: GitErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    /// Returns the stable failure category.
    #[must_use]
    pub const fn kind(&self) -> GitErrorKind {
        self.kind
    }

    /// Returns the explanatory failure message.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl Display for GitError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for GitError {}

#[derive(Debug)]
pub(crate) struct GitCommandOutput {
    pub(crate) success: bool,
    pub(crate) exit_code: Option<i32>,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
}

/// Runs one read-only Git argument vector with lock creation disabled.
pub(crate) fn run_read_only<I, S>(root: &Path, arguments: I) -> Result<GitCommandOutput, GitError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    run(root, arguments, true)
}

/// Runs one explicitly authorized Git mutation argument vector.
pub(crate) fn run_mutating<I, S>(root: &Path, arguments: I) -> Result<GitCommandOutput, GitError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    run(root, arguments, false)
}

fn run<I, S>(
    root: &Path,
    arguments: I,
    disable_optional_locks: bool,
) -> Result<GitCommandOutput, GitError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(root)
        .args(arguments)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if disable_optional_locks {
        command.env("GIT_OPTIONAL_LOCKS", "0");
    }
    let started = Instant::now();
    let mut child = crate::runner::group::spawn(&mut command).map_err(|error| {
        GitError::new(
            GitErrorKind::Unavailable,
            format!("Failed to start Git at {}: {error}", root.display()),
        )
    })?;
    let stdout = crate::runner::group::take_stdout(&mut child)
        .ok_or_else(|| unavailable("Git process did not expose piped stdout"))?;
    let stderr = crate::runner::group::take_stderr(&mut child)
        .ok_or_else(|| unavailable("Git process did not expose piped stderr"))?;
    let captured_bytes = Arc::new(AtomicUsize::new(0));
    let output_exceeded = Arc::new(AtomicBool::new(false));
    let capture_failed = Arc::new(AtomicBool::new(false));
    let (stdout_handle, stdout_finished) = spawn_capture(
        stdout,
        Arc::clone(&captured_bytes),
        Arc::clone(&output_exceeded),
        Arc::clone(&capture_failed),
    );
    let (stderr_handle, stderr_finished) = spawn_capture(
        stderr,
        captured_bytes,
        Arc::clone(&output_exceeded),
        Arc::clone(&capture_failed),
    );
    let status = observe(
        &mut child,
        started,
        &output_exceeded,
        &capture_failed,
        [&stdout_finished, &stderr_finished],
    );
    let stdout = join_capture(stdout_handle, "stdout");
    let stderr = join_capture(stderr_handle, "stderr");
    let stdout = stdout?;
    let stderr = stderr?;
    if output_exceeded.load(Ordering::Acquire) {
        return Err(GitError::new(
            GitErrorKind::InvalidOutput,
            format!("Git output exceeded the {MAX_GIT_OUTPUT_BYTES}-byte adapter limit"),
        ));
    }
    if let Some(error) = stdout.error.or(stderr.error) {
        return Err(unavailable(format!(
            "Failed to capture Git output: {error}"
        )));
    }
    let status = status?;
    Ok(GitCommandOutput {
        success: status.success(),
        exit_code: status.code(),
        stdout: stdout.bytes,
        stderr: stderr.bytes,
    })
}

fn observe(
    child: &mut command_group::GroupChild,
    started: Instant,
    output_exceeded: &AtomicBool,
    capture_failed: &AtomicBool,
    captures_finished: [&AtomicBool; 2],
) -> Result<ExitStatus, GitError> {
    let mut status = None;
    loop {
        let forced_reason = if output_exceeded.load(Ordering::Acquire) {
            Some("Git output exceeded its bounded capture limit")
        } else if capture_failed.load(Ordering::Acquire) {
            Some("Git output capture failed")
        } else if started.elapsed() >= GIT_COMMAND_TIMEOUT {
            Some("Git command exceeded its five-minute timeout")
        } else {
            None
        };
        if let Some(reason) = forced_reason {
            crate::runner::group::terminate(child)
                .map_err(|error| unavailable(format!("Failed to terminate Git: {error}")))?;
            let _ = crate::runner::group::wait(child);
            return Err(unavailable(reason));
        }
        if status.is_none() {
            status = crate::runner::group::try_wait(child)
                .map_err(|error| unavailable(format!("Failed to observe Git: {error}")))?;
        }
        if let Some(status) = status
            && captures_finished
                .iter()
                .all(|finished| finished.load(Ordering::Acquire))
        {
            return Ok(status);
        }
        thread::sleep(GIT_POLL_INTERVAL.min(GIT_COMMAND_TIMEOUT.saturating_sub(started.elapsed())));
    }
}

struct StreamCapture {
    bytes: Vec<u8>,
    error: Option<String>,
}

fn spawn_capture(
    mut stream: impl Read + Send + 'static,
    captured_bytes: Arc<AtomicUsize>,
    output_exceeded: Arc<AtomicBool>,
    capture_failed: Arc<AtomicBool>,
) -> (thread::JoinHandle<StreamCapture>, Arc<AtomicBool>) {
    let finished = Arc::new(AtomicBool::new(false));
    let worker_finished = Arc::clone(&finished);
    let handle = thread::spawn(move || {
        let capture = read_stream(
            &mut stream,
            &captured_bytes,
            &output_exceeded,
            &capture_failed,
        );
        worker_finished.store(true, Ordering::Release);
        capture
    });
    (handle, finished)
}

fn read_stream(
    stream: &mut impl Read,
    captured_bytes: &AtomicUsize,
    output_exceeded: &AtomicBool,
    capture_failed: &AtomicBool,
) -> StreamCapture {
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; GIT_CAPTURE_CHUNK_BYTES];
    loop {
        let count = match stream.read(&mut chunk) {
            Ok(0) => return StreamCapture { bytes, error: None },
            Ok(count) => count,
            Err(error) => {
                capture_failed.store(true, Ordering::Release);
                return StreamCapture {
                    bytes,
                    error: Some(error.to_string()),
                };
            }
        };
        let previous = captured_bytes.fetch_add(count, Ordering::AcqRel);
        let retained = MAX_GIT_OUTPUT_BYTES.saturating_sub(previous).min(count);
        bytes.extend_from_slice(&chunk[..retained]);
        if count > retained {
            output_exceeded.store(true, Ordering::Release);
            return StreamCapture { bytes, error: None };
        }
    }
}

fn join_capture(
    handle: thread::JoinHandle<StreamCapture>,
    stream: &str,
) -> Result<StreamCapture, GitError> {
    handle
        .join()
        .map_err(|_| unavailable(format!("Git {stream} capture worker panicked")))
}

fn unavailable(message: impl Into<String>) -> GitError {
    GitError::new(GitErrorKind::Unavailable, message)
}
