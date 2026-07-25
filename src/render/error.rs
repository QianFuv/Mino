//! Renderer error categories and values.

use std::error::Error;
use std::fmt::{self, Display, Formatter};

/// Stable categories for Markdown projection failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RenderErrorKind {
    /// A filesystem operation failed.
    Io,
    /// Plan data could not be serialized for deterministic rendering.
    Serialization,
    /// An existing managed projection differs from its expected bytes.
    Drift,
}

/// A typed projection failure with a stable category and explanatory message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderError {
    kind: RenderErrorKind,
    message: String,
}

impl RenderError {
    /// Creates a renderer failure.
    #[must_use]
    pub(crate) fn new(kind: RenderErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    /// Returns the stable renderer failure category.
    #[must_use]
    pub const fn kind(&self) -> RenderErrorKind {
        self.kind
    }

    /// Returns the explanatory failure message.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl Display for RenderError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for RenderError {}

impl From<std::io::Error> for RenderError {
    fn from(error: std::io::Error) -> Self {
        Self::new(RenderErrorKind::Io, error.to_string())
    }
}

impl From<serde_json::Error> for RenderError {
    fn from(error: serde_json::Error) -> Self {
        Self::new(RenderErrorKind::Serialization, error.to_string())
    }
}
