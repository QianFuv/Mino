//! Schema and protocol version values.

use std::borrow::Cow;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize};

use super::{DomainError, DomainErrorKind};

/// Current serialized plan schema version.
pub const CURRENT_SCHEMA_VERSION: u32 = 1;
/// Current imported plan protocol version.
pub const CURRENT_PROTOCOL_VERSION: &str = "2026-05-11";
/// Current imported plan protocol revision.
pub const CURRENT_PROTOCOL_REVISION: &str = "review-rework-git-flow-v1";

/// A validated serialized schema version.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct SchemaVersion(u32);

impl SchemaVersion {
    /// Returns the current supported schema version.
    #[must_use]
    pub const fn current() -> Self {
        Self(CURRENT_SCHEMA_VERSION)
    }

    /// Returns the numeric schema version.
    #[must_use]
    pub const fn value(self) -> u32 {
        self.0
    }
}

impl<'de> Deserialize<'de> for SchemaVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = u32::deserialize(deserializer)?;
        if value == CURRENT_SCHEMA_VERSION {
            Ok(Self(value))
        } else {
            Err(serde::de::Error::custom(DomainError::new(
                DomainErrorKind::UnsupportedSchemaVersion,
                format!("Unsupported schema version: {value}"),
            )))
        }
    }
}

impl JsonSchema for SchemaVersion {
    fn schema_name() -> Cow<'static, str> {
        "SchemaVersion".into()
    }

    fn schema_id() -> Cow<'static, str> {
        concat!(module_path!(), "::SchemaVersion").into()
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "integer",
            "const": CURRENT_SCHEMA_VERSION
        })
    }
}

/// A validated plan protocol version and revision pair.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProtocolVersion {
    version: String,
    revision: String,
}

impl ProtocolVersion {
    /// Returns the current imported protocol pair.
    #[must_use]
    pub fn current() -> Self {
        Self {
            version: CURRENT_PROTOCOL_VERSION.to_owned(),
            revision: CURRENT_PROTOCOL_REVISION.to_owned(),
        }
    }

    /// Returns the calendar protocol version.
    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Returns the protocol revision name.
    #[must_use]
    pub fn revision(&self) -> &str {
        &self.revision
    }
}

impl<'de> Deserialize<'de> for ProtocolVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawProtocolVersion {
            version: String,
            revision: String,
        }

        let raw = RawProtocolVersion::deserialize(deserializer)?;
        if raw.version == CURRENT_PROTOCOL_VERSION && raw.revision == CURRENT_PROTOCOL_REVISION {
            Ok(Self {
                version: raw.version,
                revision: raw.revision,
            })
        } else {
            Err(serde::de::Error::custom(DomainError::new(
                DomainErrorKind::UnsupportedProtocolVersion,
                format!(
                    "Unsupported protocol version/revision: {}/{}",
                    raw.version, raw.revision
                ),
            )))
        }
    }
}

impl JsonSchema for ProtocolVersion {
    fn schema_name() -> Cow<'static, str> {
        "ProtocolVersion".into()
    }

    fn schema_id() -> Cow<'static, str> {
        concat!(module_path!(), "::ProtocolVersion").into()
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "object",
            "properties": {
                "version": {
                    "type": "string",
                    "const": CURRENT_PROTOCOL_VERSION
                },
                "revision": {
                    "type": "string",
                    "const": CURRENT_PROTOCOL_REVISION
                }
            },
            "required": ["version", "revision"],
            "additionalProperties": false
        })
    }
}
