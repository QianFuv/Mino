//! Immutable evidence entity definitions.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{CheckId, CriterionId, EvidenceId, PlanId, TaskId, Timestamp};

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
    pub(crate) rule_id: String,
    pub(crate) replacements: u32,
}

/// An immutable evidence record linked to a plan and optional task/check/criterion.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    pub(crate) id: EvidenceId,
    pub(crate) plan_id: PlanId,
    pub(crate) task_id: Option<TaskId>,
    pub(crate) criterion_id: Option<CriterionId>,
    pub(crate) check_id: Option<CheckId>,
    #[serde(rename = "type")]
    pub(crate) kind: EvidenceType,
    pub(crate) command: Vec<String>,
    pub(crate) cwd: Option<String>,
    pub(crate) exit_code: Option<i32>,
    pub(crate) duration_milliseconds: Option<u64>,
    pub(crate) output_summary: Option<String>,
    pub(crate) output_digest: Option<String>,
    pub(crate) artifact_path: Option<String>,
    pub(crate) artifact_digest: Option<String>,
    pub(crate) actor: String,
    pub(crate) captured_at: Timestamp,
    pub(crate) redactions: Vec<Redaction>,
    pub(crate) supersedes: Option<EvidenceId>,
}
