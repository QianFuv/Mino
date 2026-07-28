//! Literal, regular-expression, and secret-shaped output redaction.

use std::collections::{BTreeMap, BTreeSet};

use regex::{Captures, Regex};

use crate::domain::AppliedRedaction;
use crate::store::sha256_digest;

use super::{RunnerError, RunnerErrorKind};

const REDACTION_MARKER: &str = "[REDACTED]";
const RESIDUAL_SECRET_RULE_ID: &str = "builtin-residual-secret-scan";
const BUILTIN_RULES: &[(&str, &str, &str)] = &[
    (
        "builtin-authorization-scheme",
        r"(?im)\b(authorization[ \t]*:[ \t]*(?:bearer|basic|digest)[ \t]+)[^\r\n]+",
        "${1}[REDACTED]",
    ),
    (
        "builtin-authorization-header",
        r"(?im)\b(authorization[ \t]*:[ \t]*)[^\r\n]+",
        "${1}[REDACTED]",
    ),
    (
        "builtin-auth-credential",
        r"(?i)\b((?:bearer|basic|digest)[ \t]+)[A-Za-z0-9._~+/=-]{8,}",
        "${1}[REDACTED]",
    ),
    (
        "builtin-json-double-quoted-secret",
        r#"(?i)("(?:api[_-]?key|access[_-]?token|refresh[_-]?token|client[_-]?secret|token|secret|password|authorization)"[ \t]*:[ \t]*")[^"\r\n]*(")"#,
        "${1}[REDACTED]${2}",
    ),
    (
        "builtin-json-single-quoted-secret",
        r"(?i)('(?:api[_-]?key|access[_-]?token|refresh[_-]?token|client[_-]?secret|token|secret|password|authorization)'[ \t]*:[ \t]*')[^'\r\n]*(')",
        "${1}[REDACTED]${2}",
    ),
    (
        "builtin-url-query-secret",
        r"(?i)([?&](?:api[_-]?key|access[_-]?token|refresh[_-]?token|client[_-]?secret|token|secret|password|authorization)=)[^&#\s]+",
        "${1}[REDACTED]",
    ),
    (
        "builtin-shell-secret-assignment",
        r#"(?im)\b((?:api[_-]?key|access[_-]?token|refresh[_-]?token|client[_-]?secret|token|secret|password|authorization)[ \t]*=[ \t]*)(?:"[^"\r\n]*"|'[^'\r\n]*'|[^\s,;]+)"#,
        "${1}[REDACTED]",
    ),
    (
        "builtin-secret-key-value",
        r"(?im)\b((?:api[_-]?key|access[_-]?token|refresh[_-]?token|client[_-]?secret|token|secret|password)[ \t]*:[ \t]*)[^\s,;\r\n]+",
        "${1}[REDACTED]",
    ),
    (
        "builtin-known-token-prefix",
        r"(?i)\b(?:github_pat_[A-Za-z0-9_]{20,}|gh[pousr]_[A-Za-z0-9]{20,}|sk-[A-Za-z0-9_-]{16,}|AKIA[0-9A-Z]{16})\b",
        REDACTION_MARKER,
    ),
    (
        "builtin-jwt",
        r"\b[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\b",
        REDACTION_MARKER,
    ),
    (
        "builtin-private-key",
        r"(?s)-----BEGIN (?:[A-Z0-9 ]+ )?PRIVATE KEY-----.*?-----END (?:[A-Z0-9 ]+ )?PRIVATE KEY-----",
        REDACTION_MARKER,
    ),
];
const RESIDUAL_SECRET_PATTERNS: &[&str] = &[
    r"(?im)\bauthorization[ \t]*:[ \t]*[^\r\n]+",
    r"(?im)\bauthorization[ \t]*=[ \t]*[^\r\n]+",
    r#"(?i)["'](?:api[_-]?key|access[_-]?token|refresh[_-]?token|client[_-]?secret|token|secret|password|authorization)["'][ \t]*:[ \t]*["'][^"'\r\n]+["']"#,
    r"(?im)\b(?:api[_-]?key|access[_-]?token|refresh[_-]?token|client[_-]?secret|token|secret|password)[ \t]*[:=][ \t]*[^\s,;\r\n]+",
    r"(?i)[?&](?:api[_-]?key|access[_-]?token|refresh[_-]?token|client[_-]?secret|token|secret|password|authorization)=[^&#\s]+",
    r"(?i)\b(?:api[_-]?key|access[_-]?token|refresh[_-]?token|client[_-]?secret|token|secret|password)[ \t]+[A-Za-z0-9._~+/=-]{12,}\b",
    r"(?i)\b(?:github_pat_[A-Za-z0-9_]{20,}|gh[pousr]_[A-Za-z0-9]{20,}|sk-[A-Za-z0-9_-]{16,}|AKIA[0-9A-Z]{16})\b",
    r"\b[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\b",
    r"-----BEGIN (?:[A-Z0-9 ]+ )?PRIVATE KEY-----",
];

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
    Regex {
        expression: Regex,
        replacement: String,
    },
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
    residual_scanners: Vec<Regex>,
    policy_digest: String,
}

pub(crate) struct RedactedText {
    text: String,
    applied: Vec<AppliedRedaction>,
    capture_blocked: bool,
}

impl RedactedText {
    pub(crate) fn into_parts(self) -> (String, Vec<AppliedRedaction>, bool) {
        (self.text, self.applied, self.capture_blocked)
    }
}

impl Redactor {
    /// Compiles an ordered policy and appends the built-in credential rules.
    ///
    /// # Errors
    ///
    /// Returns an invalid-request error for duplicate or empty identifiers,
    /// empty literals, or invalid regular expressions.
    pub fn new(rules: Vec<RedactionRule>) -> Result<Self, RunnerError> {
        let mut identifiers = BTreeSet::new();
        let mut compiled = Vec::with_capacity(rules.len().saturating_add(BUILTIN_RULES.len()));
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
                        CompiledMatcher::Regex {
                            expression,
                            replacement: REDACTION_MARKER.to_owned(),
                        },
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
        for (id, pattern, replacement) in BUILTIN_RULES {
            if !identifiers.insert((*id).to_owned()) {
                return Err(invalid_rule(format!(
                    "Redaction rule identifier {id} is reserved"
                )));
            }
            let expression = Regex::new(pattern).map_err(|error| {
                invalid_rule(format!("Built-in redaction regex {id} failed: {error}"))
            })?;
            digest_material.push_str(id);
            digest_material.push('\0');
            digest_material.push_str(&sha256_digest(pattern.as_bytes()));
            digest_material.push('\0');
            digest_material.push_str(&sha256_digest(replacement.as_bytes()));
            digest_material.push('\0');
            compiled.push(CompiledRule {
                id: (*id).to_owned(),
                matcher: CompiledMatcher::Regex {
                    expression,
                    replacement: (*replacement).to_owned(),
                },
            });
        }
        if !identifiers.insert(RESIDUAL_SECRET_RULE_ID.to_owned()) {
            return Err(invalid_rule(format!(
                "Redaction rule identifier {RESIDUAL_SECRET_RULE_ID} is reserved"
            )));
        }
        let residual_scanners = RESIDUAL_SECRET_PATTERNS
            .iter()
            .map(|pattern| {
                digest_material.push_str(RESIDUAL_SECRET_RULE_ID);
                digest_material.push('\0');
                digest_material.push_str(&sha256_digest(pattern.as_bytes()));
                digest_material.push('\0');
                Regex::new(pattern).map_err(|error| {
                    invalid_rule(format!("Residual secret scan regex failed: {error}"))
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            rules: compiled,
            residual_scanners,
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
        let (text, applied, _) = self.redact_checked(text, &[]).into_parts();
        (text, applied)
    }

    pub(crate) fn redact_checked(
        &self,
        text: &str,
        runtime_literals: &[(String, String)],
    ) -> RedactedText {
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
                CompiledMatcher::Regex {
                    expression,
                    replacement,
                } => {
                    let (next, count) = replace_matches(expression, replacement, &redacted);
                    redacted = next;
                    count
                }
            };
            record_count(&mut counts, &rule.id, replacements);
        }
        let residual_count = self
            .residual_scanners
            .iter()
            .map(|scanner| {
                scanner
                    .find_iter(&redacted)
                    .filter(|matched| !matched.as_str().contains(REDACTION_MARKER))
                    .count()
            })
            .sum::<usize>();
        let capture_blocked = residual_count != 0;
        if capture_blocked {
            redacted.clear();
            record_count(&mut counts, RESIDUAL_SECRET_RULE_ID, residual_count);
        }
        let applied = counts
            .into_iter()
            .filter_map(|(id, replacements)| {
                (replacements != 0).then(|| AppliedRedaction::new(id, replacements))
            })
            .collect();
        RedactedText {
            text: redacted,
            applied,
            capture_blocked,
        }
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

fn replace_matches(expression: &Regex, replacement: &str, text: &str) -> (String, usize) {
    let mut replacements = 0_usize;
    let replaced = expression
        .replace_all(text, |captures: &Captures<'_>| {
            let matched = captures
                .get(0)
                .expect("a regex capture always contains the complete match")
                .as_str();
            if matched.contains(REDACTION_MARKER) {
                matched.to_owned()
            } else {
                replacements = replacements.saturating_add(1);
                let mut expanded = String::new();
                captures.expand(replacement, &mut expanded);
                expanded
            }
        })
        .into_owned();
    (replaced, replacements)
}
