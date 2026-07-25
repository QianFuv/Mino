//! Immutable evidence capture, content-addressed blobs, and audit queries.

mod blob;
mod policy;
mod store;

use std::error::Error;
use std::fmt::{self, Display, Formatter};

pub use policy::{AddEvidenceRequest, EvidenceRequestContext, EvidenceSource};
pub use store::{
    CommandEvidenceRequest, EvidenceAddReport, EvidenceAudit, EvidenceFinding, EvidenceStore,
};

/// Stable categories for evidence-policy and persistence failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EvidenceErrorKind {
    /// The caller supplied incomplete, unsafe, or incompatible evidence.
    InvalidRequest,
    /// The expected plan revision is no longer current.
    RevisionConflict,
    /// A request identifier was reused with different evidence input.
    RequestConflict,
    /// The requested plan does not exist.
    PlanNotFound,
    /// The requested evidence record does not exist.
    EvidenceNotFound,
    /// A filesystem operation failed.
    Io,
    /// Evidence JSON could not be encoded or decoded.
    Serialization,
    /// Immutable records, index entries, or blob metadata disagree.
    CorruptStore,
    /// The bounded evidence lock could not be acquired.
    LockTimeout,
}

/// A typed evidence failure with a stable category and explanatory message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EvidenceError {
    kind: EvidenceErrorKind,
    message: String,
}

impl EvidenceError {
    pub(crate) fn new(kind: EvidenceErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    /// Returns the stable evidence-error category.
    #[must_use]
    pub const fn kind(&self) -> EvidenceErrorKind {
        self.kind
    }

    /// Returns the explanatory failure message.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    pub(crate) fn into_message(self) -> String {
        self.message
    }
}

impl Display for EvidenceError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for EvidenceError {}
