//! Literal, regular-expression, and secret-shaped output redaction.

use std::collections::{BTreeMap, BTreeSet};

use regex::{NoExpand, Regex};

use crate::domain::AppliedRedaction;
use crate::store::sha256_digest;

use super::{RunnerError, RunnerErrorKind};

const SECRET_VALUE_RULE_ID: &str = "builtin-secret-key-value";
const SECRET_VALUE_PATTERN: &str =
    r"(?i)\b(?:api[_-]?key|token|secret|password|authorization)\s*[:=]\s*[^\s,;]+";
const REDACTION_MARKER: &str = "[REDACTED]";

/// One named output-redaction rule.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RedactionRule {
    /// Replaces every exact non-empty literal.
    Literal {
        /// Stable rule identifier recorded in evidence metadata.
        id: String,
        /// Sensitive literal that must never be persisted.
        value: String,
    },
    /// Replaces every match of a Rust regular-expression pattern.
    Regex {
        /// Stable rule identifier recorded in evidence metadata.
        id: String,
        /// Pattern compiled before any process starts.
        pattern: String,
    },
}

impl RedactionRule {
    /// Creates an exact-literal redaction rule.
    #[must_use]
    pub fn literal(id: impl Into<String>, value: impl Into<String>) -> Self {
        Self::Literal {
            id: id.into(),
            value: value.into(),
        }
    }

    /// Creates a regular-expression redaction rule.
    #[must_use]
    pub fn regex(id: impl Into<String>, pattern: impl Into<String>) -> Self {
        Self::Regex {
            id: id.into(),
            pattern: pattern.into(),
        }
    }
}

#[derive(Clone, Debug)]
enum CompiledMatcher {
    Literal(String),
    Regex(Regex),
}

#[derive(Clone, Debug)]
struct CompiledRule {
    id: String,
    matcher: CompiledMatcher,
}

/// A precompiled policy that removes sensitive text before hashing or persistence.
#[derive(Clone, Debug)]
pub struct Redactor {
    rules: Vec<CompiledRule>,
    policy_digest: String,
}

impl Redactor {
    /// Compiles an ordered policy and appends the built-in secret key/value rule.
    ///
    /// # Errors
    ///
    /// Returns an invalid-request error for duplicate or empty identifiers,
    /// empty literals, or invalid regular expressions.
    pub fn new(rules: Vec<RedactionRule>) -> Result<Self, RunnerError> {
        let mut identifiers = BTreeSet::new();
        let mut compiled = Vec::with_capacity(rules.len().saturating_add(1));
        let mut digest_material = String::new();
        for rule in rules {
            let (id, matcher, material) = match rule {
                RedactionRule::Literal { id, value } => {
                    if value.is_empty() {
                        return Err(invalid_rule("Literal redaction values cannot be empty"));
                    }
                    let value_digest = sha256_digest(value.as_bytes());
                    (
                        id,
                        CompiledMatcher::Literal(value),
                        format!("literal:{value_digest}"),
                    )
                }
                RedactionRule::Regex { id, pattern } => {
                    let expression = Regex::new(&pattern).map_err(|error| {
                        invalid_rule(format!("Invalid redaction regex: {error}"))
                    })?;
                    let pattern_digest = sha256_digest(pattern.as_bytes());
                    (
                        id,
                        CompiledMatcher::Regex(expression),
                        format!("regex:{pattern_digest}"),
                    )
                }
            };
            validate_rule_id(&id)?;
            if !identifiers.insert(id.clone()) {
                return Err(invalid_rule(format!(
                    "Duplicate redaction rule identifier {id}"
                )));
            }
            digest_material.push_str(&id);
            digest_material.push('\0');
            digest_material.push_str(&material);
            digest_material.push('\0');
            compiled.push(CompiledRule { id, matcher });
        }
        if !identifiers.insert(SECRET_VALUE_RULE_ID.to_owned()) {
            return Err(invalid_rule(format!(
                "Redaction rule identifier {SECRET_VALUE_RULE_ID} is reserved"
            )));
        }
        let secret_expression = Regex::new(SECRET_VALUE_PATTERN)
            .map_err(|error| invalid_rule(format!("Built-in redaction regex failed: {error}")))?;
        digest_material.push_str(SECRET_VALUE_RULE_ID);
        digest_material.push('\0');
        digest_material.push_str(&sha256_digest(SECRET_VALUE_PATTERN.as_bytes()));
        compiled.push(CompiledRule {
            id: SECRET_VALUE_RULE_ID.to_owned(),
            matcher: CompiledMatcher::Regex(secret_expression),
        });
        Ok(Self {
            rules: compiled,
            policy_digest: sha256_digest(digest_material.as_bytes()),
        })
    }

    /// Returns the non-secret digest stored in a check-run lease.
    #[must_use]
    pub fn policy_digest(&self) -> &str {
        &self.policy_digest
    }

    /// Redacts text and returns replacement counts for rules that matched.
    #[must_use]
    pub fn redact(&self, text: &str) -> (String, Vec<AppliedRedaction>) {
        self.redact_with_literals(text, &[])
    }

    pub(crate) fn redact_with_literals(
        &self,
        text: &str,
        runtime_literals: &[(String, String)],
    ) -> (String, Vec<AppliedRedaction>) {
        let mut redacted = text.to_owned();
        let mut counts = BTreeMap::<String, u32>::new();
        for (id, literal) in runtime_literals {
            if literal.is_empty() {
                continue;
            }
            let replacements = redacted.matches(literal).count();
            if replacements != 0 {
                redacted = redacted.replace(literal, REDACTION_MARKER);
            }
            record_count(&mut counts, id, replacements);
        }
        for rule in &self.rules {
            let replacements = match &rule.matcher {
                CompiledMatcher::Literal(value) => {
                    let count = redacted.matches(value).count();
                    if count != 0 {
                        redacted = redacted.replace(value, REDACTION_MARKER);
                    }
                    count
                }
                CompiledMatcher::Regex(expression) => {
                    let count = expression.find_iter(&redacted).count();
                    if count != 0 {
                        redacted = expression
                            .replace_all(&redacted, NoExpand(REDACTION_MARKER))
                            .into_owned();
                    }
                    count
                }
            };
            record_count(&mut counts, &rule.id, replacements);
        }
        let applied = counts
            .into_iter()
            .filter_map(|(id, replacements)| {
                (replacements != 0).then(|| AppliedRedaction::new(id, replacements))
            })
            .collect();
        (redacted, applied)
    }
}

impl Default for Redactor {
    fn default() -> Self {
        Self::new(Vec::new()).expect("the built-in redaction policy must compile")
    }
}

fn validate_rule_id(id: &str) -> Result<(), RunnerError> {
    if id.is_empty()
        || id.len() > 128
        || !id.is_ascii()
        || !id.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
        })
    {
        Err(invalid_rule(format!(
            "Invalid redaction rule identifier {id}"
        )))
    } else {
        Ok(())
    }
}

fn invalid_rule(message: impl Into<String>) -> RunnerError {
    RunnerError::new(RunnerErrorKind::InvalidRequest, message)
}

fn record_count(counts: &mut BTreeMap<String, u32>, id: &str, count: usize) {
    if count == 0 {
        return;
    }
    let count = u32::try_from(count).unwrap_or(u32::MAX);
    counts
        .entry(id.to_owned())
        .and_modify(|current| *current = current.saturating_add(count))
        .or_insert(count);
}
