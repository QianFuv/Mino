//! Bounded, no-shell Git command execution for adapter-owned argument vectors.

use std::error::Error;
use std::ffi::{OsStr, OsString};
use std::fmt::{self, Display, Formatter};
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use crate::runner::probe::{BoundedCommandError, BoundedCommandErrorKind, BoundedCommandRunner};

const GENERAL_GIT_OUTPUT_BYTES: usize = 16 * 1024 * 1024;
const GENERAL_GIT_TIMEOUT: Duration = Duration::from_mins(5);
const PROBE_GIT_OUTPUT_BYTES: usize = 64 * 1024;
const PROBE_GIT_TIMEOUT: Duration = Duration::from_secs(5);
const GIT_ENVIRONMENT_ALLOWLIST: &[&str] = &[
    "PATH",
    "PATHEXT",
    "SYSTEMROOT",
    "WINDIR",
    "HOME",
    "USERPROFILE",
    "TMP",
    "TEMP",
    "TMPDIR",
];

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
    run(root, arguments, true, GitCommandProfile::General)
}

/// Runs one short read-only Git probe with discovery limits.
pub(crate) fn run_probe<I, S>(root: &Path, arguments: I) -> Result<GitCommandOutput, GitError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    run(root, arguments, true, GitCommandProfile::Probe)
}

/// Runs one explicitly authorized Git mutation argument vector.
pub(crate) fn run_mutating<I, S>(root: &Path, arguments: I) -> Result<GitCommandOutput, GitError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    run(root, arguments, false, GitCommandProfile::General)
}

#[derive(Clone, Copy)]
enum GitCommandProfile {
    General,
    Probe,
}

fn run<I, S>(
    root: &Path,
    arguments: I,
    disable_optional_locks: bool,
    profile: GitCommandProfile,
) -> Result<GitCommandOutput, GitError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let inherited = allowed_environment();
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(root)
        .args(arguments)
        .env_clear()
        .envs(inherited)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C");
    if disable_optional_locks {
        command.env("GIT_OPTIONAL_LOCKS", "0");
    }
    let runner = match profile {
        GitCommandProfile::General => {
            BoundedCommandRunner::new(GENERAL_GIT_TIMEOUT, GENERAL_GIT_OUTPUT_BYTES)
        }
        GitCommandProfile::Probe => {
            BoundedCommandRunner::new(PROBE_GIT_TIMEOUT, PROBE_GIT_OUTPUT_BYTES)
        }
    }
    .map_err(|error| map_bounded_error(&error))?;
    let output = runner
        .run(&mut command)
        .map_err(|error| map_bounded_error(&error))?;
    Ok(GitCommandOutput {
        success: output.status.success(),
        exit_code: output.status.code(),
        stdout: output.stdout,
        stderr: output.stderr,
    })
}

fn allowed_environment() -> Vec<(String, OsString)> {
    GIT_ENVIRONMENT_ALLOWLIST
        .iter()
        .filter_map(|name| std::env::var_os(name).map(|value| ((*name).to_owned(), value)))
        .collect()
}

fn map_bounded_error(error: &BoundedCommandError) -> GitError {
    let kind = if error.kind() == BoundedCommandErrorKind::OutputLimit {
        GitErrorKind::InvalidOutput
    } else {
        GitErrorKind::Unavailable
    };
    let message = match error.kind() {
        BoundedCommandErrorKind::OutputLimit | BoundedCommandErrorKind::Timeout => {
            format!("Git adapter command failed: {error}")
        }
        BoundedCommandErrorKind::Spawn => "Git executable is unavailable".to_owned(),
        BoundedCommandErrorKind::InvalidLimits
        | BoundedCommandErrorKind::Capture
        | BoundedCommandErrorKind::Observe
        | BoundedCommandErrorKind::Terminate => {
            "Git adapter command could not complete safely".to_owned()
        }
    };
    GitError::new(kind, message)
}
