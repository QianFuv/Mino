//! Strict single-document YAML deserialization for authored Mino inputs.

use serde::Deserialize;

use crate::application::git_readiness::PrePlanCleanupItemInput;
use crate::domain::{AmendmentPatch, DraftPlanInput, DraftTaskInput};
use crate::{ErrorCategory, MinoError};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CleanupProposalDocument {
    items: Vec<PrePlanCleanupItemInput>,
}

/// Deserializes one YAML document without lifecycle or execution-only fields.
///
/// # Errors
///
/// Returns an incomplete-input error for malformed YAML, multiple documents,
/// unknown fields, invalid identifiers, or mismatched field types.
pub fn parse_draft(source: &str) -> Result<DraftPlanInput, MinoError> {
    serde_saphyr::from_str(source).map_err(|error| {
        MinoError::new(
            ErrorCategory::IncompleteOrValidation,
            format!("Failed to parse strict Draft YAML: {error}"),
        )
    })
}

/// Deserializes one ordered pre-plan cleanup proposal document.
///
/// # Errors
///
/// Returns an incomplete-input error for malformed YAML, multiple documents,
/// unknown fields, or mismatched field types.
pub fn parse_cleanup_proposal(source: &str) -> Result<Vec<PrePlanCleanupItemInput>, MinoError> {
    serde_saphyr::from_str::<CleanupProposalDocument>(source)
        .map(|document| document.items)
        .map_err(|error| {
            MinoError::new(
                ErrorCategory::IncompleteOrValidation,
                format!("Failed to parse cleanup proposal YAML: {error}"),
            )
        })
}

/// Deserializes one typed protected-plan amendment patch.
///
/// # Errors
///
/// Returns an incomplete-input error for malformed YAML, multiple documents,
/// unknown fields, invalid identifiers, or mismatched field types.
pub fn parse_amendment_patch(source: &str) -> Result<AmendmentPatch, MinoError> {
    serde_saphyr::from_str(source).map_err(|error| {
        MinoError::new(
            ErrorCategory::IncompleteOrValidation,
            format!("Failed to parse strict amendment YAML: {error}"),
        )
    })
}

/// Deserializes one strict reserved review-rework task definition.
///
/// # Errors
///
/// Returns an incomplete-input error for malformed YAML, multiple documents,
/// unknown fields, invalid identifiers, or mismatched field types.
pub fn parse_review_rework_task(source: &str) -> Result<DraftTaskInput, MinoError> {
    serde_saphyr::from_str(source).map_err(|error| {
        MinoError::new(
            ErrorCategory::IncompleteOrValidation,
            format!("Failed to parse strict review-task YAML: {error}"),
        )
    })
}
