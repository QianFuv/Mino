//! Reusable bounded no-shell subprocess capture for internal adapters.

use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::io::Read;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use super::group;

const CAPTURE_CHUNK_BYTES: usize = 8 * 1024;
const POLL_INTERVAL: Duration = Duration::from_millis(10);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BoundedCommandErrorKind {
    InvalidLimits,
    Spawn,
    Capture,
    OutputLimit,
    Timeout,
    Observe,
    Terminate,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct BoundedCommandError {
    kind: BoundedCommandErrorKind,
    message: String,
}

impl BoundedCommandError {
    pub(crate) const fn kind(&self) -> BoundedCommandErrorKind {
        self.kind
    }
}

impl Display for BoundedCommandError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for BoundedCommandError {}

#[derive(Debug)]
pub(crate) struct BoundedCommandOutput {
    pub(crate) status: ExitStatus,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct BoundedCommandRunner {
    timeout: Duration,
    output_limit: usize,
}

impl BoundedCommandRunner {
    pub(crate) fn new(timeout: Duration, output_limit: usize) -> Result<Self, BoundedCommandError> {
        if timeout.is_zero() || output_limit == 0 {
            return Err(command_error(
                BoundedCommandErrorKind::InvalidLimits,
                "Bounded command limits must be positive",
            ));
        }
        Ok(Self {
            timeout,
            output_limit,
        })
    }

    pub(crate) fn run(
        &self,
        command: &mut Command,
    ) -> Result<BoundedCommandOutput, BoundedCommandError> {
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let started = Instant::now();
        let mut child = group::spawn(command).map_err(|error| {
            command_error(
                BoundedCommandErrorKind::Spawn,
                format!("Failed to start bounded command: {error}"),
            )
        })?;
        let stdout = group::take_stdout(&mut child).ok_or_else(|| {
            command_error(
                BoundedCommandErrorKind::Capture,
                "Bounded command did not expose piped stdout",
            )
        })?;
        let stderr = group::take_stderr(&mut child).ok_or_else(|| {
            command_error(
                BoundedCommandErrorKind::Capture,
                "Bounded command did not expose piped stderr",
            )
        })?;
        let captured_bytes = Arc::new(AtomicUsize::new(0));
        let output_exceeded = Arc::new(AtomicBool::new(false));
        let capture_failed = Arc::new(AtomicBool::new(false));
        let (stdout_handle, stdout_finished) = spawn_capture(
            stdout,
            self.output_limit,
            Arc::clone(&captured_bytes),
            Arc::clone(&output_exceeded),
            Arc::clone(&capture_failed),
        );
        let (stderr_handle, stderr_finished) = spawn_capture(
            stderr,
            self.output_limit,
            captured_bytes,
            Arc::clone(&output_exceeded),
            Arc::clone(&capture_failed),
        );
        let status = self.observe(
            &mut child,
            started,
            &output_exceeded,
            &capture_failed,
            [&stdout_finished, &stderr_finished],
        );
        let stdout = join_capture(stdout_handle, "stdout")?;
        let stderr = join_capture(stderr_handle, "stderr")?;
        if output_exceeded.load(Ordering::Acquire) {
            return Err(command_error(
                BoundedCommandErrorKind::OutputLimit,
                format!(
                    "Command output exceeded the {}-byte limit",
                    self.output_limit
                ),
            ));
        }
        if let Some(error) = stdout.error.or(stderr.error) {
            return Err(command_error(
                BoundedCommandErrorKind::Capture,
                format!("Failed to capture command output: {error}"),
            ));
        }
        Ok(BoundedCommandOutput {
            status: status?,
            stdout: stdout.bytes,
            stderr: stderr.bytes,
        })
    }

    fn observe(
        &self,
        child: &mut command_group::GroupChild,
        started: Instant,
        output_exceeded: &AtomicBool,
        capture_failed: &AtomicBool,
        captures_finished: [&AtomicBool; 2],
    ) -> Result<ExitStatus, BoundedCommandError> {
        let mut status = None;
        loop {
            let forced = if output_exceeded.load(Ordering::Acquire) {
                Some(BoundedCommandErrorKind::OutputLimit)
            } else if capture_failed.load(Ordering::Acquire) {
                Some(BoundedCommandErrorKind::Capture)
            } else if started.elapsed() >= self.timeout {
                Some(BoundedCommandErrorKind::Timeout)
            } else {
                None
            };
            if let Some(kind) = forced {
                group::terminate(child).map_err(|error| {
                    command_error(
                        BoundedCommandErrorKind::Terminate,
                        format!("Failed to terminate bounded command: {error}"),
                    )
                })?;
                let _ = group::wait(child);
                return Err(command_error(
                    kind,
                    match kind {
                        BoundedCommandErrorKind::OutputLimit => {
                            "Command output exceeded its bounded capture limit"
                        }
                        BoundedCommandErrorKind::Capture => "Command output capture failed",
                        BoundedCommandErrorKind::Timeout => "Command exceeded its timeout",
                        BoundedCommandErrorKind::InvalidLimits
                        | BoundedCommandErrorKind::Spawn
                        | BoundedCommandErrorKind::Observe
                        | BoundedCommandErrorKind::Terminate => "Bounded command was terminated",
                    },
                ));
            }
            if status.is_none() {
                status = group::try_wait(child).map_err(|error| {
                    command_error(
                        BoundedCommandErrorKind::Observe,
                        format!("Failed to observe bounded command: {error}"),
                    )
                })?;
            }
            if let Some(status) = status
                && captures_finished
                    .iter()
                    .all(|finished| finished.load(Ordering::Acquire))
            {
                return Ok(status);
            }
            thread::sleep(POLL_INTERVAL.min(self.timeout.saturating_sub(started.elapsed())));
        }
    }
}

struct StreamCapture {
    bytes: Vec<u8>,
    error: Option<String>,
}

fn spawn_capture(
    mut stream: impl Read + Send + 'static,
    output_limit: usize,
    captured_bytes: Arc<AtomicUsize>,
    output_exceeded: Arc<AtomicBool>,
    capture_failed: Arc<AtomicBool>,
) -> (thread::JoinHandle<StreamCapture>, Arc<AtomicBool>) {
    let finished = Arc::new(AtomicBool::new(false));
    let worker_finished = Arc::clone(&finished);
    let handle = thread::spawn(move || {
        let capture = read_stream(
            &mut stream,
            output_limit,
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
    output_limit: usize,
    captured_bytes: &AtomicUsize,
    output_exceeded: &AtomicBool,
    capture_failed: &AtomicBool,
) -> StreamCapture {
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; CAPTURE_CHUNK_BYTES];
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
        let retained = output_limit.saturating_sub(previous).min(count);
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
) -> Result<StreamCapture, BoundedCommandError> {
    handle.join().map_err(|_| {
        command_error(
            BoundedCommandErrorKind::Capture,
            format!("Command {stream} capture worker panicked"),
        )
    })
}

fn command_error(kind: BoundedCommandErrorKind, message: impl Into<String>) -> BoundedCommandError {
    BoundedCommandError {
        kind,
        message: message.into(),
    }
}
