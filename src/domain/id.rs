//! Strong identifier types used by plan protocol entities.

use std::borrow::Cow;
use std::fmt::{self, Display, Formatter};

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize};
use time::{Date, Month};

use super::{DomainError, DomainErrorKind};

const PLAN_ID_PATTERN: &str = concat!(
    r"^(?:",
    r"(?:[1-9][0-9]{3}|0[1-9][0-9]{2}|00[1-9][0-9]|000[1-9])-(?:",
    r"(?:01|03|05|07|08|10|12)-(?:0[1-9]|[12][0-9]|3[01])|",
    r"(?:04|06|09|11)-(?:0[1-9]|[12][0-9]|30)|02-(?:0[1-9]|1[0-9]|2[0-8]))|",
    r"(?:[0-9]{2}(?:0[48]|[2468][048]|[13579][26])|",
    r"(?:0[48]|[2468][048]|[13579][26])00)-02-29)",
    r"-[a-z0-9]+(?:-[a-z0-9]+)*$"
);
const TASK_ID_PATTERN: &str = r"^(?:T|R)[1-9][0-9]*$";
const CHECK_ID_PATTERN: &str = r"^[A-Z][A-Z0-9]*(?:-[A-Z][A-Z0-9]*)+$";
const CRITERION_ID_PATTERN: &str = r"^(?:T|R)[1-9][0-9]*-A[1-9][0-9]*$";
const EVIDENCE_ID_PATTERN: &str = r"^E0*[1-9][0-9]*$";
const REQUEST_ID_PATTERN: &str = r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$";

fn is_slug(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.is_ascii()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value
            .as_bytes()
            .last()
            .is_some_and(u8::is_ascii_alphanumeric)
        && !value.contains("--")
}

fn is_plan_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() < 12
        || bytes.get(4) != Some(&b'-')
        || bytes.get(7) != Some(&b'-')
        || bytes.get(10) != Some(&b'-')
        || !bytes[0..4].iter().all(u8::is_ascii_digit)
        || !bytes[5..7].iter().all(u8::is_ascii_digit)
        || !bytes[8..10].iter().all(u8::is_ascii_digit)
    {
        return false;
    }

    let year = i32::from((bytes[0] - b'0') * 10 + (bytes[1] - b'0')) * 100
        + i32::from((bytes[2] - b'0') * 10 + (bytes[3] - b'0'));
    let month = (bytes[5] - b'0') * 10 + (bytes[6] - b'0');
    let day = (bytes[8] - b'0') * 10 + (bytes[9] - b'0');
    year != 0
        && Month::try_from(month)
            .is_ok_and(|month| Date::from_calendar_date(year, month, day).is_ok())
        && is_slug(&value[11..])
}

fn is_positive_number(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes
        .first()
        .is_some_and(|byte| byte.is_ascii_digit() && *byte != b'0')
        && bytes[1..].iter().all(u8::is_ascii_digit)
}

fn is_prefixed_number(value: &str, prefix: u8) -> bool {
    value
        .as_bytes()
        .strip_prefix(&[prefix])
        .is_some_and(|number| {
            number
                .first()
                .is_some_and(|byte| byte.is_ascii_digit() && *byte != b'0')
                && number[1..].iter().all(u8::is_ascii_digit)
        })
}

fn is_task_id(value: &str) -> bool {
    is_prefixed_number(value, b'T') || is_prefixed_number(value, b'R')
}

fn is_check_id(value: &str) -> bool {
    let mut segments = value.split('-');
    let Some(first_segment) = segments.next() else {
        return false;
    };
    let mut segment_count = 1;

    if value.len() > 128 || !is_uppercase_segment(first_segment) {
        return false;
    }

    for segment in segments {
        segment_count += 1;
        if !is_uppercase_segment(segment) {
            return false;
        }
    }

    segment_count >= 2
}

fn is_uppercase_segment(value: &str) -> bool {
    let Some((first, remaining)) = value.as_bytes().split_first() else {
        return false;
    };
    first.is_ascii_uppercase()
        && remaining
            .iter()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
}

fn is_criterion_id(value: &str) -> bool {
    value
        .rsplit_once("-A")
        .is_some_and(|(task_id, number)| is_task_id(task_id) && is_positive_number(number))
}

fn is_evidence_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.first() == Some(&b'E')
        && bytes.len() >= 5
        && bytes[1..].iter().all(u8::is_ascii_digit)
        && bytes[1..].iter().any(|byte| *byte != b'0')
}

fn is_request_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 36
        && [8, 13, 18, 23]
            .into_iter()
            .all(|index| bytes[index] == b'-')
        && bytes.iter().enumerate().all(|(index, byte)| {
            [8, 13, 18, 23].contains(&index)
                || byte.is_ascii_digit()
                || (b'a'..=b'f').contains(byte)
        })
}

fn identifier_schema(
    pattern: &'static str,
    minimum_length: u64,
    maximum_length: Option<u64>,
    format: Option<&'static str>,
) -> Schema {
    let mut schema = json_schema!({
        "type": "string",
        "pattern": pattern,
        "minLength": minimum_length
    });
    if let Some(maximum_length) = maximum_length {
        schema.insert("maxLength".to_owned(), maximum_length.into());
    }
    if let Some(format) = format {
        schema.insert("format".to_owned(), format.into());
    }
    schema
}

macro_rules! define_identifier {
    (
        $(#[$metadata:meta])*
        $name:ident,
        $kind:literal,
        $validator:ident,
        $pattern:ident,
        $minimum_length:literal,
        $maximum_length:expr,
        $format:expr
    ) => {
        $(#[$metadata])*
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Parses and validates an identifier.
            ///
            /// # Errors
            ///
            /// Returns an invalid-identifier error when the value violates its grammar.
            pub fn parse(value: impl Into<String>) -> Result<Self, DomainError> {
                let value = value.into();
                if $validator(&value) {
                    Ok(Self(value))
                } else {
                    Err(DomainError::new(
                        DomainErrorKind::InvalidIdentifier,
                        format!("Invalid {} identifier: {}", $kind, value),
                    ))
                }
            }

            /// Returns the serialized identifier value.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl Display for $name {
            fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::parse(value).map_err(serde::de::Error::custom)
            }
        }

        impl JsonSchema for $name {
            fn schema_name() -> Cow<'static, str> {
                stringify!($name).into()
            }

            fn schema_id() -> Cow<'static, str> {
                concat!(module_path!(), "::", stringify!($name)).into()
            }

            fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
                identifier_schema(
                    $pattern,
                    $minimum_length,
                    $maximum_length,
                    $format,
                )
            }
        }
    };
}

define_identifier!(
    /// Stable date-prefixed slug that identifies a plan.
    PlanId,
    "plan",
    is_plan_id,
    PLAN_ID_PATTERN,
    12,
    Some(139),
    None
);
define_identifier!(
    /// Stable implementation or review-rework task identifier.
    TaskId,
    "task",
    is_task_id,
    TASK_ID_PATTERN,
    2,
    None,
    None
);
define_identifier!(
    /// Stable verification-check identifier.
    CheckId,
    "check",
    is_check_id,
    CHECK_ID_PATTERN,
    3,
    Some(128),
    None
);
define_identifier!(
    /// Stable acceptance-criterion identifier.
    CriterionId,
    "criterion",
    is_criterion_id,
    CRITERION_ID_PATTERN,
    5,
    None,
    None
);
define_identifier!(
    /// Monotonic evidence identifier within a plan.
    EvidenceId,
    "evidence",
    is_evidence_id,
    EVIDENCE_ID_PATTERN,
    5,
    None,
    None
);
define_identifier!(
    /// Idempotency identifier supplied with a mutating request.
    RequestId,
    "request",
    is_request_id,
    REQUEST_ID_PATTERN,
    36,
    Some(36),
    Some("uuid")
);
