//! Canonical JSON encoding and SHA-256 helpers.

use std::io::Write;

use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::StoreError;

/// Serializes a value as key-sorted compact JSON terminated by one LF byte.
///
/// # Errors
///
/// Returns an error when the value cannot be converted to JSON or written.
pub fn canonical_json_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, StoreError> {
    let value = serde_json::to_value(value)?;
    let mut output = Vec::new();
    write_value(&mut output, &value)?;
    output.push(b'\n');
    Ok(output)
}

/// Returns a lowercase prefixed SHA-256 digest for the supplied bytes.
#[must_use]
pub fn sha256_digest(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    format!("sha256:{digest:x}")
}

fn write_value(output: &mut Vec<u8>, value: &Value) -> Result<(), StoreError> {
    match value {
        Value::Object(object) => {
            output.push(b'{');
            let mut keys = object.keys().collect::<Vec<_>>();
            keys.sort_unstable();
            for (index, key) in keys.into_iter().enumerate() {
                if index != 0 {
                    output.push(b',');
                }
                serde_json::to_writer(&mut *output, key)?;
                output.push(b':');
                write_value(output, &object[key])?;
            }
            output.push(b'}');
        }
        Value::Array(array) => {
            output.push(b'[');
            for (index, item) in array.iter().enumerate() {
                if index != 0 {
                    output.push(b',');
                }
                write_value(output, item)?;
            }
            output.push(b']');
        }
        _ => serde_json::to_writer(&mut *output, value)?,
    }
    output.flush()?;
    Ok(())
}
