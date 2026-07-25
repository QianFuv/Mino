//! Stable result envelopes for human and Agent consumers.

use clap::ValueEnum;
use serde::Serialize;

use crate::{ErrorCategory, MinoError};

/// Supported top-level output formats.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub enum OutputFormat {
    /// Concise text intended for a human terminal.
    #[default]
    Human,
    /// Versioned JSON intended for an Agent or another program.
    Json,
}

/// A canonical next command returned to an Agent.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NextAction {
    /// Stable action identifier.
    pub id: String,
    /// Complete canonical argument vector, including the executable name.
    pub argv: Vec<String>,
}

/// An empty flattened payload for results without command-specific fields.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
pub struct EmptyPayload {}

/// The versioned top-level result envelope returned by Mino commands.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MinoResult<T> {
    kind: &'static str,
    ok: bool,
    complete: bool,
    message: String,
    #[serde(flatten)]
    payload: T,
    missing: Vec<String>,
    next_actions: Vec<NextAction>,
}

impl<T> MinoResult<T> {
    /// Creates a successful result envelope.
    ///
    /// # Arguments
    ///
    /// * message - Concise human-readable result summary.
    /// * complete - Whether the requested workflow is complete.
    /// * payload - Command-specific fields flattened into the JSON envelope.
    #[must_use]
    pub fn success(message: impl Into<String>, complete: bool, payload: T) -> Self {
        Self {
            kind: "mino.result/v1",
            ok: true,
            complete,
            message: message.into(),
            payload,
            missing: Vec::new(),
            next_actions: Vec::new(),
        }
    }

    /// Replaces the missing-field list returned by an incomplete workflow.
    #[must_use]
    pub fn with_missing(mut self, missing: Vec<String>) -> Self {
        self.missing = missing;
        self
    }

    /// Replaces the canonical next-action list.
    #[must_use]
    pub fn with_next_actions(mut self, next_actions: Vec<NextAction>) -> Self {
        self.next_actions = next_actions;
        self
    }

    /// Returns whether the command succeeded.
    #[must_use]
    pub const fn is_ok(&self) -> bool {
        self.ok
    }

    /// Returns whether the requested workflow is complete.
    #[must_use]
    pub const fn is_complete(&self) -> bool {
        self.complete
    }

    /// Returns the human-readable result summary.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl<T> MinoResult<T>
where
    T: Serialize,
{
    /// Renders the result without writing diagnostics.
    ///
    /// # Errors
    ///
    /// Returns an environment-unavailable error if JSON serialization fails.
    pub fn render(&self, format: OutputFormat) -> Result<String, MinoError> {
        match format {
            OutputFormat::Human => Ok(format!("{}\n", self.message)),
            OutputFormat::Json => {
                let mut rendered = serde_json::to_string(self).map_err(|error| {
                    MinoError::new(
                        ErrorCategory::EnvironmentUnavailable,
                        format!("Failed to serialize the result: {error}"),
                    )
                })?;
                rendered.push('\n');
                Ok(rendered)
            }
        }
    }
}
