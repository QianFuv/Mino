//! Error values returned by domain validation and transition operations.

use std::error::Error;
use std::fmt::{self, Display, Formatter};

/// Stable categories for domain failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DomainErrorKind {
    /// An identifier does not satisfy its required grammar.
    InvalidIdentifier,
    /// A timestamp is not a valid UTC RFC3339 value.
    InvalidTimestamp,
    /// The serialized schema version is unsupported.
    UnsupportedSchemaVersion,
    /// The serialized protocol version or revision is unsupported.
    UnsupportedProtocolVersion,
    /// A semantic state transition is not legal.
    InvalidTransition,
    /// A task identifier is duplicated within a plan.
    DuplicateTask,
    /// A requested task does not exist.
    TaskNotFound,
    /// A task is not the first eligible task in implementation order.
    TaskOrderViolation,
    /// One or more task dependencies are incomplete.
    UnmetDependencies,
    /// Another task already owns the active execution slot.
    ActiveTaskExists,
    /// A plan operation requires an approval record.
    ApprovalRequired,
    /// Serialized or in-memory data violates a domain invariant.
    InvariantViolation,
}

/// A typed domain failure with a stable kind and explanatory message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DomainError {
    kind: DomainErrorKind,
    message: String,
}

impl DomainError {
    /// Creates a domain failure.
    #[must_use]
    pub(crate) fn new(kind: DomainErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    /// Returns the stable failure kind.
    #[must_use]
    pub const fn kind(&self) -> DomainErrorKind {
        self.kind
    }

    /// Returns the explanatory failure message.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl Display for DomainError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for DomainError {}
