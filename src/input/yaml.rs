//! Strict single-document YAML deserialization for authored Draft fields.

use crate::domain::DraftPlanInput;
use crate::{ErrorCategory, MinoError};

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
