//! Storage error categories and values.

use std::error::Error;
use std::fmt::{self, Display, Formatter};

use crate::domain::DomainError;

/// Stable categories for recoverable storage failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoreErrorKind {
    /// A filesystem operation failed.
    Io,
    /// Serialized storage data could not be decoded.
    Serialization,
    /// The bounded plan lock could not be acquired in time.
    LockTimeout,
    /// The requested plan does not exist.
    PlanNotFound,
    /// A plan already exists at the requested identifier.
    PlanAlreadyExists,
    /// The caller supplied a revision other than the current revision.
    StaleRevision,
    /// A request identifier was reused for a different operation.
    RequestConflict,
    /// A domain mutation failed or did not advance exactly one revision.
    InvalidMutation,
    /// Persisted state, journal, event, or snapshot data is inconsistent.
    CorruptState,
    /// A deterministic test interruption was injected.
    InjectedFailure,
}

/// A typed storage failure with a stable category and explanatory message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoreError {
    kind: StoreErrorKind,
    message: String,
}

impl StoreError {
    /// Creates a storage failure.
    #[must_use]
    pub(crate) fn new(kind: StoreErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    /// Returns the stable storage failure category.
    #[must_use]
    pub const fn kind(&self) -> StoreErrorKind {
        self.kind
    }

    /// Returns the explanatory failure message.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl Display for StoreError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for StoreError {}

impl From<std::io::Error> for StoreError {
    fn from(error: std::io::Error) -> Self {
        Self::new(StoreErrorKind::Io, error.to_string())
    }
}

impl From<serde_json::Error> for StoreError {
    fn from(error: serde_json::Error) -> Self {
        Self::new(StoreErrorKind::Serialization, error.to_string())
    }
}

impl From<DomainError> for StoreError {
    fn from(error: DomainError) -> Self {
        Self::new(StoreErrorKind::InvalidMutation, error.to_string())
    }
}
