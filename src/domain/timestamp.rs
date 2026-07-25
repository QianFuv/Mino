//! Validated UTC RFC3339 timestamps.

use std::borrow::Cow;
use std::fmt::{self, Display, Formatter};

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize};
use time::format_description::well_known::Rfc3339;
use time::{OffsetDateTime, UtcOffset};

use super::{DomainError, DomainErrorKind};

/// A normalized UTC RFC3339 timestamp.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct Timestamp(String);

impl Timestamp {
    /// Parses an RFC3339 value and normalizes it to UTC.
    ///
    /// # Errors
    ///
    /// Returns an invalid-timestamp error for malformed or unformattable values.
    pub fn parse(value: impl AsRef<str>) -> Result<Self, DomainError> {
        let source = value.as_ref();
        let parsed = OffsetDateTime::parse(source, &Rfc3339).map_err(|error| {
            DomainError::new(
                DomainErrorKind::InvalidTimestamp,
                format!("Invalid RFC3339 timestamp {source}: {error}"),
            )
        })?;
        let normalized = parsed
            .to_offset(UtcOffset::UTC)
            .format(&Rfc3339)
            .map_err(|error| {
                DomainError::new(
                    DomainErrorKind::InvalidTimestamp,
                    format!("Failed to normalize timestamp {source}: {error}"),
                )
            })?;
        Ok(Self(normalized))
    }

    /// Returns the current UTC time.
    ///
    /// # Panics
    ///
    /// Panics only if the time crate cannot format a UTC timestamp as RFC3339.
    #[must_use]
    pub fn now_utc() -> Self {
        let value = OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .expect("UTC timestamps must support RFC3339 formatting");
        Self(value)
    }

    /// Returns the normalized RFC3339 representation.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for Timestamp {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Timestamp {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for Timestamp {
    fn schema_name() -> Cow<'static, str> {
        "Timestamp".into()
    }

    fn schema_id() -> Cow<'static, str> {
        concat!(module_path!(), "::Timestamp").into()
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "format": "date-time"
        })
    }
}
