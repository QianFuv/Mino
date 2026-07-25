//! Bounded, no-shell Git command execution for adapter-owned argument vectors.

use std::error::Error;
use std::ffi::OsStr;
use std::fmt::{self, Display, Formatter};
use std::path::Path;
use std::process::Command;

const MAX_GIT_OUTPUT_BYTES: usize = 16 * 1024 * 1024;

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
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
}

pub(crate) fn run_read_only<I, S>(root: &Path, arguments: I) -> Result<GitCommandOutput, GitError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(arguments)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .output()
        .map_err(|error| {
            GitError::new(
                GitErrorKind::Unavailable,
                format!("Failed to start Git at {}: {error}", root.display()),
            )
        })?;
    let total_bytes = output
        .stdout
        .len()
        .checked_add(output.stderr.len())
        .ok_or_else(|| {
            GitError::new(GitErrorKind::InvalidOutput, "Git output length overflowed")
        })?;
    if total_bytes > MAX_GIT_OUTPUT_BYTES {
        return Err(GitError::new(
            GitErrorKind::InvalidOutput,
            format!("Git output exceeded the {MAX_GIT_OUTPUT_BYTES}-byte adapter limit"),
        ));
    }
    Ok(GitCommandOutput {
        success: output.status.success(),
        stdout: output.stdout,
        stderr: output.stderr,
    })
}
