//! Bounded no-shell process execution and recoverable result journaling.

pub(crate) mod group;
pub(crate) mod probe;
mod process;
mod redaction;

use std::error::Error;
use std::fmt::{self, Display, Formatter};

pub use process::{CheckRunJournal, JournaledRun, ProcessRunner, RunDisposition, RunEnvironment};
pub use redaction::{RedactionRule, Redactor};

/// Stable categories for runner and check-journal failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RunnerErrorKind {
    /// The requested command, path, policy, or limit is invalid.
    InvalidRequest,
    /// Another live invocation owns the same exact check request.
    AlreadyRunning,
    /// A filesystem or process operation failed.
    Io,
    /// Persisted JSON could not be encoded or decoded.
    Serialization,
    /// A request identifier was reused with different leased inputs.
    JournalConflict,
    /// A persisted lease or result is incomplete, non-canonical, or inconsistent.
    CorruptJournal,
    /// A capture worker failed while reading process output.
    CaptureFailed,
}

/// A typed process-runner failure with a stable category.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunnerError {
    kind: RunnerErrorKind,
    message: String,
}

impl RunnerError {
    pub(crate) fn new(kind: RunnerErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    /// Returns the stable runner error category.
    #[must_use]
    pub const fn kind(&self) -> RunnerErrorKind {
        self.kind
    }

    /// Returns the explanatory failure message.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl Display for RunnerError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for RunnerError {}
