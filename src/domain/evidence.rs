//! Immutable evidence entity definitions and invariants.

use std::collections::BTreeSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{
    CheckId, CriterionId, DomainError, DomainErrorKind, EvidenceId, PlanId, TaskId, Timestamp,
};

/// Supported evidence kinds.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum EvidenceType {
    /// A captured planned-command result.
    Command,
    /// A copied file artifact.
    File,
    /// A captured Git diff.
    GitDiff,
    /// A recorded Git commit.
    Commit,
    /// A referenced URL.
    Url,
    /// A captured log.
    Log,
    /// A captured screenshot.
    Screenshot,
    /// A human observation with an actor.
    ManualObservation,
    /// An explicitly approved exception.
    AcceptedException,
}

/// A redaction rule applied before evidence persistence.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Redaction {
    rule_id: String,
    replacements: u32,
}

impl Redaction {
    pub(crate) fn new(rule_id: impl Into<String>, replacements: u32) -> Self {
        Self {
            rule_id: rule_id.into(),
            replacements,
        }
    }

    /// Returns the stable redaction-rule identifier.
    #[must_use]
    pub fn rule_id(&self) -> &str {
        &self.rule_id
    }

    /// Returns the number of replacements made by this rule.
    #[must_use]
    pub const fn replacements(&self) -> u32 {
        self.replacements
    }
}

pub(crate) struct EvidenceFields {
    pub id: EvidenceId,
    pub plan_id: PlanId,
    pub captured_revision: u64,
    pub task_id: Option<TaskId>,
    pub criterion_id: Option<CriterionId>,
    pub check_id: Option<CheckId>,
    pub kind: EvidenceType,
    pub command: Vec<String>,
    pub cwd: Option<String>,
    pub exit_code: Option<i32>,
    pub duration_milliseconds: Option<u64>,
    pub output_summary: Option<String>,
    pub output_digest: Option<String>,
    pub artifact_path: Option<String>,
    pub artifact_digest: Option<String>,
    pub actor: String,
    pub captured_at: Timestamp,
    pub redactions: Vec<Redaction>,
    pub supersedes: Option<EvidenceId>,
}

/// An immutable evidence record linked to a plan and optional task/check/criterion.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    id: EvidenceId,
    plan_id: PlanId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    captured_revision: Option<u64>,
    task_id: Option<TaskId>,
    criterion_id: Option<CriterionId>,
    check_id: Option<CheckId>,
    #[serde(rename = "type")]
    kind: EvidenceType,
    command: Vec<String>,
    cwd: Option<String>,
    exit_code: Option<i32>,
    duration_milliseconds: Option<u64>,
    output_summary: Option<String>,
    output_digest: Option<String>,
    artifact_path: Option<String>,
    artifact_digest: Option<String>,
    actor: String,
    captured_at: Timestamp,
    redactions: Vec<Redaction>,
    supersedes: Option<EvidenceId>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UncheckedEvidence {
    id: EvidenceId,
    plan_id: PlanId,
    #[serde(default)]
    captured_revision: Option<u64>,
    task_id: Option<TaskId>,
    criterion_id: Option<CriterionId>,
    check_id: Option<CheckId>,
    #[serde(rename = "type")]
    kind: EvidenceType,
    command: Vec<String>,
    cwd: Option<String>,
    exit_code: Option<i32>,
    duration_milliseconds: Option<u64>,
    output_summary: Option<String>,
    output_digest: Option<String>,
    artifact_path: Option<String>,
    artifact_digest: Option<String>,
    actor: String,
    captured_at: Timestamp,
    redactions: Vec<Redaction>,
    supersedes: Option<EvidenceId>,
}

impl TryFrom<UncheckedEvidence> for Evidence {
    type Error = DomainError;

    fn try_from(unchecked: UncheckedEvidence) -> Result<Self, Self::Error> {
        let evidence = Self {
            id: unchecked.id,
            plan_id: unchecked.plan_id,
            captured_revision: unchecked.captured_revision,
            task_id: unchecked.task_id,
            criterion_id: unchecked.criterion_id,
            check_id: unchecked.check_id,
            kind: unchecked.kind,
            command: unchecked.command,
            cwd: unchecked.cwd,
            exit_code: unchecked.exit_code,
            duration_milliseconds: unchecked.duration_milliseconds,
            output_summary: unchecked.output_summary,
            output_digest: unchecked.output_digest,
            artifact_path: unchecked.artifact_path,
            artifact_digest: unchecked.artifact_digest,
            actor: unchecked.actor,
            captured_at: unchecked.captured_at,
            redactions: unchecked.redactions,
            supersedes: unchecked.supersedes,
        };
        evidence.validate_invariants()?;
        Ok(evidence)
    }
}

impl<'de> Deserialize<'de> for Evidence {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let unchecked = UncheckedEvidence::deserialize(deserializer)?;
        Self::try_from(unchecked).map_err(serde::de::Error::custom)
    }
}

impl Evidence {
    pub(crate) fn new(fields: EvidenceFields) -> Result<Self, DomainError> {
        let evidence = Self {
            id: fields.id,
            plan_id: fields.plan_id,
            captured_revision: Some(fields.captured_revision),
            task_id: fields.task_id,
            criterion_id: fields.criterion_id,
            check_id: fields.check_id,
            kind: fields.kind,
            command: fields.command,
            cwd: fields.cwd,
            exit_code: fields.exit_code,
            duration_milliseconds: fields.duration_milliseconds,
            output_summary: fields.output_summary,
            output_digest: fields.output_digest,
            artifact_path: fields.artifact_path,
            artifact_digest: fields.artifact_digest,
            actor: fields.actor,
            captured_at: fields.captured_at,
            redactions: fields.redactions,
            supersedes: fields.supersedes,
        };
        evidence.validate_invariants()?;
        Ok(evidence)
    }

    /// Returns the monotonic identifier within the owning plan.
    #[must_use]
    pub const fn id(&self) -> &EvidenceId {
        &self.id
    }

    /// Returns the owning plan identifier.
    #[must_use]
    pub const fn plan_id(&self) -> &PlanId {
        &self.plan_id
    }

    /// Returns the plan revision observed when evidence was captured.
    #[must_use]
    pub const fn captured_revision(&self) -> Option<u64> {
        self.captured_revision
    }

    /// Returns the optional task binding.
    #[must_use]
    pub const fn task_id(&self) -> Option<&TaskId> {
        self.task_id.as_ref()
    }

    /// Returns the optional acceptance-criterion binding.
    #[must_use]
    pub const fn criterion_id(&self) -> Option<&CriterionId> {
        self.criterion_id.as_ref()
    }

    /// Returns the optional verification-check binding.
    #[must_use]
    pub const fn check_id(&self) -> Option<&CheckId> {
        self.check_id.as_ref()
    }

    /// Returns the evidence kind.
    #[must_use]
    pub const fn kind(&self) -> EvidenceType {
        self.kind
    }

    /// Returns the exact planned command for command evidence.
    #[must_use]
    pub fn command(&self) -> &[String] {
        &self.command
    }

    /// Returns the project-relative command working directory.
    #[must_use]
    pub fn cwd(&self) -> Option<&str> {
        self.cwd.as_deref()
    }

    /// Returns the observed process exit code when present.
    #[must_use]
    pub const fn exit_code(&self) -> Option<i32> {
        self.exit_code
    }

    /// Returns the observed elapsed process time when present.
    #[must_use]
    pub const fn duration_milliseconds(&self) -> Option<u64> {
        self.duration_milliseconds
    }

    /// Returns the redacted output or human description.
    #[must_use]
    pub fn output_summary(&self) -> Option<&str> {
        self.output_summary.as_deref()
    }

    /// Returns the digest of redacted output when present.
    #[must_use]
    pub fn output_digest(&self) -> Option<&str> {
        self.output_digest.as_deref()
    }

    /// Returns the project-relative artifact path or external reference.
    #[must_use]
    pub fn artifact_path(&self) -> Option<&str> {
        self.artifact_path.as_deref()
    }

    /// Returns the digest of persisted artifact bytes when present.
    #[must_use]
    pub fn artifact_digest(&self) -> Option<&str> {
        self.artifact_digest.as_deref()
    }

    /// Returns the actor responsible for the observation.
    #[must_use]
    pub fn actor(&self) -> &str {
        &self.actor
    }

    /// Returns the capture timestamp.
    #[must_use]
    pub const fn captured_at(&self) -> &Timestamp {
        &self.captured_at
    }

    /// Returns the applied redaction metadata.
    #[must_use]
    pub fn redactions(&self) -> &[Redaction] {
        &self.redactions
    }

    /// Returns the prior evidence corrected by this record.
    #[must_use]
    pub const fn supersedes(&self) -> Option<&EvidenceId> {
        self.supersedes.as_ref()
    }

    /// Validates immutable identity, binding, kind, and redaction invariants.
    ///
    /// # Errors
    ///
    /// Returns an invariant error for incomplete or contradictory evidence.
    pub fn validate_invariants(&self) -> Result<(), DomainError> {
        if self.captured_revision == Some(0) {
            return Err(invariant("Evidence captured revision must be positive"));
        }
        if self.actor.trim().is_empty() {
            return Err(invariant("Evidence requires a non-empty actor"));
        }
        if self
            .supersedes
            .as_ref()
            .is_some_and(|supersedes| supersedes == &self.id)
        {
            return Err(invariant("Evidence cannot supersede itself"));
        }
        if self.criterion_id.is_some() && self.task_id.is_none() {
            return Err(invariant(
                "Criterion-bound evidence also requires its task identifier",
            ));
        }
        if let (Some(task_id), Some(criterion_id)) = (&self.task_id, &self.criterion_id)
            && !criterion_id.as_str().starts_with(&format!("{task_id}-A"))
        {
            return Err(invariant(
                "Evidence criterion identifier does not belong to its task",
            ));
        }
        validate_redactions(&self.redactions)?;
        match self.kind {
            EvidenceType::Command => self.validate_command(),
            EvidenceType::File
            | EvidenceType::GitDiff
            | EvidenceType::Log
            | EvidenceType::Screenshot => self.validate_artifact(),
            EvidenceType::Commit | EvidenceType::Url => self.validate_reference(),
            EvidenceType::ManualObservation => self.validate_observation(false),
            EvidenceType::AcceptedException => self.validate_observation(true),
        }
    }

    fn validate_command(&self) -> Result<(), DomainError> {
        if self.command.is_empty()
            || self.command.iter().any(|part| part.trim().is_empty())
            || self.cwd.as_deref().is_none_or(str::is_empty)
            || self.duration_milliseconds.is_none()
            || self.output_summary.is_none()
            || self.output_digest.as_deref().is_none_or(str::is_empty)
        {
            Err(invariant(
                "Command evidence requires argv, cwd, duration, output summary, and digest",
            ))
        } else {
            Ok(())
        }
    }

    fn validate_artifact(&self) -> Result<(), DomainError> {
        if !self.command.is_empty()
            || self.artifact_path.as_deref().is_none_or(str::is_empty)
            || self.artifact_digest.as_deref().is_none_or(str::is_empty)
        {
            Err(invariant(
                "Artifact evidence requires a path and digest without command argv",
            ))
        } else {
            Ok(())
        }
    }

    fn validate_reference(&self) -> Result<(), DomainError> {
        if !self.command.is_empty()
            || self.artifact_path.as_deref().is_none_or(str::is_empty)
            || self.output_summary.as_deref().is_none_or(str::is_empty)
        {
            Err(invariant(
                "Reference evidence requires a reference and description without command argv",
            ))
        } else {
            Ok(())
        }
    }

    fn validate_observation(&self, requires_reference: bool) -> Result<(), DomainError> {
        if !self.command.is_empty()
            || self.output_summary.as_deref().is_none_or(str::is_empty)
            || requires_reference && self.artifact_path.as_deref().is_none_or(str::is_empty)
        {
            Err(invariant(
                "Observation evidence requires a description and any required approval reference",
            ))
        } else {
            Ok(())
        }
    }
}

fn validate_redactions(redactions: &[Redaction]) -> Result<(), DomainError> {
    let mut rule_ids = BTreeSet::new();
    if redactions.iter().any(|redaction| {
        redaction.rule_id.trim().is_empty()
            || redaction.replacements == 0
            || !rule_ids.insert(redaction.rule_id.as_str())
    }) {
        Err(invariant(
            "Evidence redactions require unique rule IDs and positive counts",
        ))
    } else {
        Ok(())
    }
}

fn invariant(message: impl Into<String>) -> DomainError {
    DomainError::new(DomainErrorKind::InvariantViolation, message)
}
