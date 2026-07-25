//! Bounded UTF-8 input adapters for strict batch and guided plan authoring.

use std::fs;
use std::io::Read;
use std::path::Path;

use crate::application::plan::MAX_AUTHORING_INPUT_BYTES;
use crate::{ErrorCategory, MinoError};

/// Line-oriented guided plan creation.
pub mod wizard;
/// Strict authored-plan YAML parsing.
pub mod yaml;

/// Reads one bounded UTF-8 file without accepting partial or lossy content.
///
/// # Errors
///
/// Returns an incomplete-input or environment error when the file is missing,
/// oversized, unreadable, or not valid UTF-8.
pub fn read_utf8_file(path: &Path) -> Result<String, MinoError> {
    let metadata = fs::metadata(path).map_err(|error| {
        MinoError::new(
            ErrorCategory::EnvironmentUnavailable,
            format!("Failed to inspect input file {}: {error}", path.display()),
        )
    })?;
    if metadata.len() > MAX_AUTHORING_INPUT_BYTES as u64 {
        return Err(input_too_large_error());
    }
    let bytes = fs::read(path).map_err(|error| {
        MinoError::new(
            ErrorCategory::EnvironmentUnavailable,
            format!("Failed to read input file {}: {error}", path.display()),
        )
    })?;
    decode_utf8(bytes)
}

/// Reads one bounded UTF-8 stream to completion.
///
/// # Errors
///
/// Returns an incomplete-input or environment error when reading fails, the
/// stream is oversized, or its bytes are not valid UTF-8.
pub fn read_utf8_stream(reader: &mut impl Read) -> Result<String, MinoError> {
    let maximum = u64::try_from(MAX_AUTHORING_INPUT_BYTES)
        .map_err(|_| input_too_large_error())?
        .saturating_add(1);
    let mut bytes = Vec::new();
    reader
        .take(maximum)
        .read_to_end(&mut bytes)
        .map_err(|error| {
            MinoError::new(
                ErrorCategory::EnvironmentUnavailable,
                format!("Failed to read standard input: {error}"),
            )
        })?;
    if bytes.len() > MAX_AUTHORING_INPUT_BYTES {
        return Err(input_too_large_error());
    }
    decode_utf8(bytes)
}

fn decode_utf8(bytes: Vec<u8>) -> Result<String, MinoError> {
    String::from_utf8(bytes).map_err(|error| {
        MinoError::new(
            ErrorCategory::IncompleteOrValidation,
            format!("Authoring input is not valid UTF-8: {error}"),
        )
    })
}

fn input_too_large_error() -> MinoError {
    MinoError::new(
        ErrorCategory::IncompleteOrValidation,
        format!("Authoring input exceeds the {MAX_AUTHORING_INPUT_BYTES}-byte limit"),
    )
}
