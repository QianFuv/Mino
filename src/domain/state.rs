//! Enumerated lifecycle and review states.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Top-level plan lifecycle states.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub enum PlanStatus {
    /// The plan is still being authored.
    Draft,
    /// The plan is complete enough for review or execution.
    Ready,
    /// A task or final verification is executing.
    #[serde(rename = "In Progress")]
    InProgress,
    /// Execution cannot continue until a recorded condition is resolved.
    Blocked,
    /// Implementation is complete and awaits review or acceptance.
    Review,
    /// The plan is accepted and complete.
    Done,
}

/// Per-task lifecycle states.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub enum TaskStatus {
    /// The task is still being authored.
    Draft,
    /// The task is executable when its dependencies permit it.
    Ready,
    /// The task currently owns the execution slot.
    #[serde(rename = "In Progress")]
    InProgress,
    /// The task cannot continue until a recorded condition is resolved.
    Blocked,
    /// The task passed its acceptance and verification gates.
    Done,
}

/// Verification-check execution states.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub enum CheckStatus {
    /// The check has not run.
    Pending,
    /// A check run is currently leased.
    Running,
    /// The latest required run passed.
    Passed,
    /// Previously passing evidence no longer matches current workspace content.
    Stale,
    /// The latest required run failed.
    Failed,
    /// The check cannot currently run.
    Blocked,
}

/// Acceptance-criterion evaluation states.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub enum CriterionStatus {
    /// The criterion has not been evaluated.
    Pending,
    /// Bound evidence proves the criterion.
    Passed,
    /// Bound evidence contradicts the criterion.
    Failed,
    /// An explicitly approved exception satisfies the criterion.
    #[serde(rename = "Accepted Exception")]
    AcceptedException,
}

/// Plan-level Git Flow consent states.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub enum GitFlowConsent {
    /// Consent has not been decided.
    Pending,
    /// The approved plan authorizes its declared task commits.
    Approved,
    /// Plan-level Git Flow was explicitly disabled.
    Disabled,
}

/// Per-task Git commit-gate states.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub enum CommitStatus {
    /// The task commit gate has not run.
    Pending,
    /// The task does not require a commit.
    #[serde(rename = "Not Required")]
    NotRequired,
    /// The commit was intentionally skipped with an accepted reason.
    Skipped,
    /// The commit could not be created safely.
    Blocked,
    /// The task commit was created and recorded.
    Committed,
}

/// Review feedback classifications.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub enum ReviewClassification {
    /// Existing acceptance or evidence is defective.
    #[serde(rename = "Acceptance Defect")]
    AcceptanceDefect,
    /// The correction stays inside approved scope.
    #[serde(rename = "In-Scope Rework")]
    InScopeRework,
    /// The request materially changes the plan.
    #[serde(rename = "Material Change")]
    MaterialChange,
    /// The request belongs to a separate future objective.
    #[serde(rename = "Follow-Up")]
    FollowUp,
    /// The reviewer accepts the current result.
    Accepted,
}

/// Review item resolution states.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub enum ReviewStatus {
    /// The review item has not been addressed.
    Open,
    /// Rework for the item is underway.
    #[serde(rename = "In Progress")]
    InProgress,
    /// The review item is resolved.
    Resolved,
    /// The review item awaits approval or another dependency.
    Blocked,
    /// The item is recorded as non-blocking follow-up work.
    Deferred,
}
