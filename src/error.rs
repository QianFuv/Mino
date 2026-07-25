//! Error taxonomy shared by the Mino CLI and library.

use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::process::ExitCode;

use serde::Serialize;

/// Stable categories used to select CLI exit codes.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCategory {
    /// A plan or request is incomplete or fails deterministic validation.
    IncompleteOrValidation,
    /// The caller supplied an expected revision that is no longer current.
    RevisionConflict,
    /// The requested operation requires an explicit approval record.
    ApprovalRequired,
    /// A state transition or policy rule forbids the operation.
    PolicyViolation,
    /// A planned verification command completed unsuccessfully.
    CheckFailed,
    /// A required command, file, environment capability, or service is unavailable.
    EnvironmentUnavailable,
    /// Machine state and a managed projection no longer agree.
    DriftDetected,
}

impl ErrorCategory {
    /// Every stable category in ascending exit-code order.
    pub const ALL: [Self; 7] = [
        Self::IncompleteOrValidation,
        Self::RevisionConflict,
        Self::ApprovalRequired,
        Self::PolicyViolation,
        Self::CheckFailed,
        Self::EnvironmentUnavailable,
        Self::DriftDetected,
    ];

    /// Returns the stable symbolic code emitted in machine results.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::IncompleteOrValidation => "incomplete_or_validation",
            Self::RevisionConflict => "revision_conflict",
            Self::ApprovalRequired => "approval_required",
            Self::PolicyViolation => "policy_violation",
            Self::CheckFailed => "check_failed",
            Self::EnvironmentUnavailable => "environment_unavailable",
            Self::DriftDetected => "drift_detected",
        }
    }

    /// Returns the documented numeric process exit code.
    #[must_use]
    pub const fn exit_code_value(self) -> u8 {
        match self {
            Self::IncompleteOrValidation => 2,
            Self::RevisionConflict => 3,
            Self::ApprovalRequired => 4,
            Self::PolicyViolation => 5,
            Self::CheckFailed => 6,
            Self::EnvironmentUnavailable => 7,
            Self::DriftDetected => 8,
        }
    }

    /// Returns the documented process exit code.
    #[must_use]
    pub fn exit_code(self) -> ExitCode {
        ExitCode::from(self.exit_code_value())
    }
}

/// A typed Mino failure with a stable category and user-facing message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MinoError {
    category: ErrorCategory,
    message: String,
}

impl MinoError {
    /// Creates a typed failure.
    ///
    /// # Arguments
    ///
    /// * category - Stable category that determines the process exit code.
    /// * message - User-facing explanation of the failure.
    #[must_use]
    pub fn new(category: ErrorCategory, message: impl Into<String>) -> Self {
        Self {
            category,
            message: message.into(),
        }
    }

    /// Returns the stable failure category.
    #[must_use]
    pub const fn category(&self) -> ErrorCategory {
        self.category
    }

    /// Returns the documented process exit code.
    #[must_use]
    pub fn exit_code(&self) -> ExitCode {
        self.category.exit_code()
    }

    /// Returns the user-facing failure message.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl Display for MinoError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for MinoError {}
