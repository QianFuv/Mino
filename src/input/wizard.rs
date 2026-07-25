//! Line-oriented guided collection with preview and explicit confirmation.

use std::io::{BufRead, Write};

use crate::application::plan::MAX_AUTHORING_INPUT_BYTES;
use crate::{ErrorCategory, MinoError};

/// Minimal values collected before a human Draft is initialized.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WizardCreateInput {
    /// Human-readable requirement name.
    pub name: String,
    /// Planning trigger.
    pub trigger: String,
    /// Multi-line original request.
    pub original_request: String,
}

/// Runs the guided create wizard and returns confirmed input or cancellation.
///
/// The original request ends at a line containing only a period. No state is
/// written by this function; callers create the Draft only after `Some` is returned.
///
/// # Errors
///
/// Returns an environment or incomplete-input error when terminal I/O fails,
/// required fields are empty, or collected input exceeds the authoring limit.
pub fn collect(
    reader: &mut impl BufRead,
    writer: &mut impl Write,
) -> Result<Option<WizardCreateInput>, MinoError> {
    let name = prompt_line(reader, writer, "Requirement name: ")?;
    if name.trim().is_empty() {
        return Err(incomplete("Requirement name cannot be empty"));
    }
    let trigger = prompt_line(reader, writer, "Planning trigger [durable]: ")?;
    let trigger = if trigger.trim().is_empty() {
        "durable".to_owned()
    } else {
        trigger
    };
    writer
        .write_all(b"Original request; finish with a line containing only '.':\n")
        .and_then(|()| writer.flush())
        .map_err(|error| wizard_io_error(&error))?;
    let mut request_lines = Vec::new();
    let mut total_bytes = 0_usize;
    loop {
        let mut line = String::new();
        let count = reader
            .read_line(&mut line)
            .map_err(|error| wizard_io_error(&error))?;
        if count == 0 {
            return Err(incomplete(
                "Wizard input ended before the original request terminator",
            ));
        }
        trim_line_ending(&mut line);
        if line == "." {
            break;
        }
        total_bytes = total_bytes
            .checked_add(line.len() + 1)
            .ok_or_else(|| incomplete("Wizard original request length overflowed"))?;
        if total_bytes > MAX_AUTHORING_INPUT_BYTES {
            return Err(incomplete("Wizard original request is too large"));
        }
        request_lines.push(line);
    }
    let original_request = request_lines.join("\n");
    if original_request.trim().is_empty() {
        return Err(incomplete("Original request cannot be empty"));
    }
    writeln!(writer, "\nPreview")
        .and_then(|()| writeln!(writer, "Name: {name}"))
        .and_then(|()| writeln!(writer, "Trigger: {trigger}"))
        .and_then(|()| writeln!(writer, "Request:\n{original_request}"))
        .map_err(|error| wizard_io_error(&error))?;
    let confirmation = prompt_line(reader, writer, "Write this Draft? [y/N]: ")?;
    if matches!(
        confirmation.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ) {
        Ok(Some(WizardCreateInput {
            name,
            trigger,
            original_request,
        }))
    } else {
        Ok(None)
    }
}

fn prompt_line(
    reader: &mut impl BufRead,
    writer: &mut impl Write,
    prompt: &str,
) -> Result<String, MinoError> {
    writer
        .write_all(prompt.as_bytes())
        .and_then(|()| writer.flush())
        .map_err(|error| wizard_io_error(&error))?;
    let mut value = String::new();
    if reader
        .read_line(&mut value)
        .map_err(|error| wizard_io_error(&error))?
        == 0
    {
        return Err(incomplete("Wizard input ended before confirmation"));
    }
    trim_line_ending(&mut value);
    Ok(value)
}

fn trim_line_ending(value: &mut String) {
    if value.ends_with('\n') {
        value.pop();
        if value.ends_with('\r') {
            value.pop();
        }
    }
}

fn incomplete(message: impl Into<String>) -> MinoError {
    MinoError::new(ErrorCategory::IncompleteOrValidation, message)
}

fn wizard_io_error(error: &std::io::Error) -> MinoError {
    MinoError::new(
        ErrorCategory::EnvironmentUnavailable,
        format!("Guided authoring I/O failed: {error}"),
    )
}
