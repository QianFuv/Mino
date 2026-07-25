//! Embedded protocol resources, compatibility status, and explicit migration.

mod bundle;
mod migrate;

use std::error::Error;
use std::fmt::{self, Display, Formatter};

pub use bundle::{ProtocolBundle, ProtocolManifest, ProtocolRegistry, ProtocolResource};
pub use migrate::{
    MigrationDisposition, ProtocolMigrationReport, ProtocolMigrationRequest, ProtocolMigrator,
};

/// Stable categories for embedded protocol and migration failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProtocolErrorKind {
    /// Embedded bytes or their manifest are malformed or digest-mismatched.
    InvalidBundle,
    /// No explicit transform exists for the requested target.
    UnsupportedMigration,
}

/// A typed protocol registry or migration failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProtocolError {
    kind: ProtocolErrorKind,
    message: String,
}

impl ProtocolError {
    pub(crate) fn new(kind: ProtocolErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    /// Returns the stable protocol error category.
    #[must_use]
    pub const fn kind(&self) -> ProtocolErrorKind {
        self.kind
    }

    /// Returns the explanatory failure message.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl Display for ProtocolError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for ProtocolError {}
