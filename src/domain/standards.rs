//! Typed standards-conflict snapshots and explicit resolution decisions.

use std::collections::BTreeSet;
use std::path::Path;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{DomainError, DomainErrorKind, Timestamp};

pub(crate) const STANDARDS_CONFLICT_EXTENSION_KEY: &str = "standards_conflicts";

pub(crate) fn required_language_package_for_path(path: &str) -> Option<&'static str> {
    let extension = Path::new(path).extension()?.to_str()?.to_ascii_lowercase();
    match extension.as_str() {
        "rs" => Some("rust"),
        "py" | "pyi" => Some("python"),
        "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "mts" | "cts" => {
            Some("typescript-javascript")
        }
        _ => None,
    }
}

/// Deterministic precedence class for one standards rule candidate.
#[derive(
    Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum StandardSourceKind {
    /// A requirement explicitly declared for the current plan request.
    UserRequirement,
    /// A repository-local hard rule or reviewed local standards declaration.
    RepositoryRule,
    /// Existing formatter, linter, build, or CI configuration.
    ProjectConfiguration,
    /// A selected Mino language standards package.
    LanguagePackage,
    /// The selected Mino Common package.
    CommonDefault,
}

impl StandardSourceKind {
    /// Returns the numeric precedence where a larger value has higher priority.
    #[must_use]
    pub const fn precedence(self) -> u8 {
        match self {
            Self::UserRequirement => 5,
            Self::RepositoryRule => 4,
            Self::ProjectConfiguration => 3,
            Self::LanguagePackage => 2,
            Self::CommonDefault => 1,
        }
    }
}

/// One exact value proposed for a standards rule by one immutable source.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StandardConflictCandidate {
    id: String,
    rule_id: String,
    value: String,
    source_kind: StandardSourceKind,
    precedence: u8,
    source: String,
    source_digest: String,
}

impl StandardConflictCandidate {
    /// Creates one validated standards rule candidate.
    ///
    /// # Errors
    ///
    /// Returns an invariant error when identifiers, value, source, or digest
    /// are incomplete or malformed.
    pub fn new(
        id: impl Into<String>,
        rule_id: impl Into<String>,
        value: impl Into<String>,
        source_kind: StandardSourceKind,
        source: impl Into<String>,
        source_digest: impl Into<String>,
    ) -> Result<Self, DomainError> {
        let candidate = Self {
            id: id.into(),
            rule_id: rule_id.into(),
            value: value.into(),
            source_kind,
            precedence: source_kind.precedence(),
            source: source.into(),
            source_digest: source_digest.into(),
        };
        candidate.validate()?;
        Ok(candidate)
    }

    /// Returns the stable candidate identifier.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns the stable standards rule identifier.
    #[must_use]
    pub fn rule_id(&self) -> &str {
        &self.rule_id
    }

    /// Returns the exact proposed rule value.
    #[must_use]
    pub fn value(&self) -> &str {
        &self.value
    }

    /// Returns the candidate source classification.
    #[must_use]
    pub const fn source_kind(&self) -> StandardSourceKind {
        self.source_kind
    }

    /// Returns the numeric precedence where larger values rank higher.
    #[must_use]
    pub const fn precedence(&self) -> u8 {
        self.precedence
    }

    /// Returns the exact source reference.
    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Returns the source byte or package digest.
    #[must_use]
    pub fn source_digest(&self) -> &str {
        &self.source_digest
    }

    fn validate(&self) -> Result<(), DomainError> {
        if !is_stable_id(&self.id)
            || !is_rule_id(&self.rule_id)
            || self.value.trim().is_empty()
            || self.source.trim().is_empty()
            || self.precedence != self.source_kind.precedence()
            || !is_sha256(&self.source_digest)
        {
            return Err(invariant(
                "A standards conflict candidate has malformed identity, value, source, or digest",
            ));
        }
        Ok(())
    }
}

/// One deterministic conflict containing every competing rule candidate.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StandardConflict {
    id: String,
    rule_id: String,
    fingerprint: String,
    default_candidate_id: Option<String>,
    candidates: Vec<StandardConflictCandidate>,
}

impl StandardConflict {
    /// Creates and orders one conflict snapshot.
    ///
    /// # Errors
    ///
    /// Returns an invariant error unless at least two distinct values, unique
    /// candidate IDs, one rule ID, and a valid fingerprint are supplied.
    pub fn new(
        id: impl Into<String>,
        rule_id: impl Into<String>,
        fingerprint: impl Into<String>,
        mut candidates: Vec<StandardConflictCandidate>,
    ) -> Result<Self, DomainError> {
        candidates.sort_by(candidate_order);
        let maximum_precedence = candidates
            .first()
            .map_or(0, StandardConflictCandidate::precedence);
        let top = candidates
            .iter()
            .filter(|candidate| candidate.precedence() == maximum_precedence)
            .collect::<Vec<_>>();
        let default_candidate_id = (top.len() == 1).then(|| top[0].id.clone());
        let conflict = Self {
            id: id.into(),
            rule_id: rule_id.into(),
            fingerprint: fingerprint.into(),
            default_candidate_id,
            candidates,
        };
        conflict.validate()?;
        Ok(conflict)
    }

    /// Returns the stable conflict identifier.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns the standards rule identifier shared by all candidates.
    #[must_use]
    pub fn rule_id(&self) -> &str {
        &self.rule_id
    }

    /// Returns the digest binding the exact candidate set and source digests.
    #[must_use]
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    /// Returns the unique highest-precedence candidate when one exists.
    #[must_use]
    pub fn default_candidate_id(&self) -> Option<&str> {
        self.default_candidate_id.as_deref()
    }

    /// Returns all candidates in descending precedence and stable source order.
    #[must_use]
    pub fn candidates(&self) -> &[StandardConflictCandidate] {
        &self.candidates
    }

    fn validate(&self) -> Result<(), DomainError> {
        let candidate_ids = self
            .candidates
            .iter()
            .map(StandardConflictCandidate::id)
            .collect::<BTreeSet<_>>();
        let values = self
            .candidates
            .iter()
            .map(StandardConflictCandidate::value)
            .collect::<BTreeSet<_>>();
        let expected_default_candidate_id = self.candidates.first().and_then(|first| {
            (self
                .candidates
                .iter()
                .filter(|candidate| candidate.precedence() == first.precedence())
                .count()
                == 1)
                .then(|| first.id())
        });
        if !is_stable_id(&self.id)
            || !is_rule_id(&self.rule_id)
            || !is_sha256(&self.fingerprint)
            || self.candidates.len() < 2
            || candidate_ids.len() != self.candidates.len()
            || values.len() < 2
            || self
                .candidates
                .iter()
                .any(|candidate| candidate.rule_id() != self.rule_id)
            || !self
                .candidates
                .windows(2)
                .all(|pair| candidate_order(&pair[0], &pair[1]).is_le())
            || self.default_candidate_id.as_deref() != expected_default_candidate_id
        {
            return Err(invariant(
                "A standards conflict must contain ordered unique competing candidates",
            ));
        }
        Ok(())
    }
}

/// One explicit user decision bound to an exact conflict fingerprint.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StandardConflictDecision {
    selected_candidate_id: String,
    rationale: String,
    reference: String,
    actor: String,
    decided_at: Timestamp,
    conflict_fingerprint: String,
}

impl StandardConflictDecision {
    /// Returns the explicitly selected candidate identifier.
    #[must_use]
    pub fn selected_candidate_id(&self) -> &str {
        &self.selected_candidate_id
    }

    /// Returns the required human rationale.
    #[must_use]
    pub fn rationale(&self) -> &str {
        &self.rationale
    }

    /// Returns the auditable external decision reference.
    #[must_use]
    pub fn reference(&self) -> &str {
        &self.reference
    }

    /// Returns the actor that recorded the decision.
    #[must_use]
    pub fn actor(&self) -> &str {
        &self.actor
    }

    /// Returns the decision timestamp.
    #[must_use]
    pub const fn decided_at(&self) -> &Timestamp {
        &self.decided_at
    }

    /// Returns the exact conflict fingerprint approved by the decision.
    #[must_use]
    pub fn conflict_fingerprint(&self) -> &str {
        &self.conflict_fingerprint
    }
}

/// Persisted conflict snapshot and its optional explicit decision.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StandardConflictRecord {
    conflict: StandardConflict,
    decision: Option<StandardConflictDecision>,
}

impl StandardConflictRecord {
    /// Returns the exact persisted conflict snapshot.
    #[must_use]
    pub const fn conflict(&self) -> &StandardConflict {
        &self.conflict
    }

    /// Returns the explicit decision when one has been recorded.
    #[must_use]
    pub const fn decision(&self) -> Option<&StandardConflictDecision> {
        self.decision.as_ref()
    }
}

/// Versioned current standards-conflict state stored in the plan extension.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StandardsConflictState {
    records: Vec<StandardConflictRecord>,
}

impl StandardsConflictState {
    /// Returns conflict records in stable conflict-ID order.
    #[must_use]
    pub fn records(&self) -> &[StandardConflictRecord] {
        &self.records
    }

    /// Returns whether no conflict snapshot is persisted.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub(crate) fn refreshed(
        &self,
        mut conflicts: Vec<StandardConflict>,
    ) -> Result<Self, DomainError> {
        conflicts.sort_by(|left, right| left.id.cmp(&right.id));
        let records = conflicts
            .into_iter()
            .map(|conflict| {
                let decision = self.records.iter().find_map(|record| {
                    (record.conflict.id == conflict.id
                        && record.conflict.fingerprint == conflict.fingerprint)
                        .then(|| record.decision.clone())
                        .flatten()
                });
                StandardConflictRecord { conflict, decision }
            })
            .collect();
        let state = Self { records };
        state.validate()?;
        Ok(state)
    }

    pub(crate) fn resolve(
        &mut self,
        conflict_id: &str,
        candidate_id: &str,
        rationale: String,
        reference: String,
        actor: String,
        decided_at: Timestamp,
    ) -> Result<(), DomainError> {
        let record = self
            .records
            .iter_mut()
            .find(|record| record.conflict.id == conflict_id)
            .ok_or_else(|| invariant(format!("Standards conflict {conflict_id} is not tracked")))?;
        if rationale.trim().is_empty() || reference.trim().is_empty() || actor.trim().is_empty() {
            return Err(invariant(
                "A standards conflict decision requires rationale, reference, and actor",
            ));
        }
        if !record
            .conflict
            .candidates
            .iter()
            .any(|candidate| candidate.id == candidate_id)
        {
            return Err(invariant(format!(
                "Candidate {candidate_id} does not belong to conflict {conflict_id}"
            )));
        }
        if record.decision.is_some() {
            return Err(invariant(format!(
                "Standards conflict {conflict_id} is already resolved"
            )));
        }
        record.decision = Some(StandardConflictDecision {
            selected_candidate_id: candidate_id.to_owned(),
            rationale,
            reference,
            actor,
            decided_at,
            conflict_fingerprint: record.conflict.fingerprint.clone(),
        });
        self.validate()
    }

    pub(crate) fn validate(&self) -> Result<(), DomainError> {
        if !self
            .records
            .windows(2)
            .all(|pair| pair[0].conflict.id < pair[1].conflict.id)
        {
            return Err(invariant(
                "Standards conflict records must have unique sorted identifiers",
            ));
        }
        for record in &self.records {
            record.conflict.validate()?;
            if let Some(decision) = &record.decision
                && (decision.rationale.trim().is_empty()
                    || decision.reference.trim().is_empty()
                    || decision.actor.trim().is_empty()
                    || decision.conflict_fingerprint != record.conflict.fingerprint
                    || !record
                        .conflict
                        .candidates
                        .iter()
                        .any(|candidate| candidate.id == decision.selected_candidate_id))
            {
                return Err(invariant(
                    "A standards decision must select a current candidate with rationale",
                ));
            }
        }
        Ok(())
    }
}

fn candidate_order(
    left: &StandardConflictCandidate,
    right: &StandardConflictCandidate,
) -> std::cmp::Ordering {
    right
        .precedence
        .cmp(&left.precedence)
        .then_with(|| left.source.cmp(&right.source))
        .then_with(|| left.id.cmp(&right.id))
}

fn is_stable_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'-')
}

fn is_rule_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.is_ascii()
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'-' | b'_')
        })
}

fn is_sha256(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

fn invariant(message: impl Into<String>) -> DomainError {
    DomainError::new(DomainErrorKind::InvariantViolation, message)
}

#[cfg(test)]
mod tests {
    use super::required_language_package_for_path;

    #[test]
    fn file_extensions_map_to_deterministic_language_packages() {
        let cases = [
            ("src/lib.rs", Some("rust")),
            ("tests/**/*.PY", Some("python")),
            ("types/schema.pyi", Some("python")),
            ("src/view.ts", Some("typescript-javascript")),
            ("src/view.tsx", Some("typescript-javascript")),
            ("src/tool.js", Some("typescript-javascript")),
            ("src/tool.jsx", Some("typescript-javascript")),
            ("src/tool.mjs", Some("typescript-javascript")),
            ("src/tool.cjs", Some("typescript-javascript")),
            ("src/tool.mts", Some("typescript-javascript")),
            ("src/tool.cts", Some("typescript-javascript")),
            ("snapshots/output.snap", None),
            ("assets/generated", None),
        ];
        for (path, expected) in cases {
            assert_eq!(required_language_package_for_path(path), expected);
        }
    }
}
