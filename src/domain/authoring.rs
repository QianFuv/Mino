//! Strict authored-plan inputs that exclude lifecycle and execution-only state.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{
    CheckId, CriterionId, FileChange, GitReadiness, StandardSelection, TaskId, VerificationCheck,
};

/// Automatically derived values used to construct a revision-one Draft.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlanDraftSeed {
    /// Stable date-prefixed plan identifier.
    pub id: super::PlanId,
    /// Human-readable requirement name.
    pub name: String,
    /// Planning trigger recorded as the initial plan type.
    pub trigger: String,
    /// Exact UTF-8 request supplied by the caller.
    pub original_request: String,
    /// Current branch when a Git repository is present.
    pub branch: Option<String>,
    /// Project-relative managed Markdown path.
    pub markdown_path: String,
    /// Read-only Git facts captured at creation.
    pub git_readiness: GitReadiness,
    /// Exact initially recommended standards packages.
    pub standards: Vec<StandardSelection>,
    /// Project-resolved verification checks seeded from standards.
    pub verification_plan: Vec<VerificationCheck>,
}

/// Mutable human metadata accepted while a plan is Draft.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DraftMetadataInput {
    /// Replacement requirement name.
    #[serde(default)]
    pub name: Option<String>,
    /// Replacement priority.
    #[serde(default)]
    pub priority: Option<String>,
    /// Replacement plan type.
    #[serde(default)]
    pub plan_type: Option<String>,
    /// Replacement area.
    #[serde(default)]
    pub area: Option<String>,
    /// Replacement owner.
    #[serde(default)]
    pub owner: Option<String>,
}

impl DraftMetadataInput {
    /// Returns whether no metadata field was supplied.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.name.is_none()
            && self.priority.is_none()
            && self.plan_type.is_none()
            && self.area.is_none()
            && self.owner.is_none()
    }
}

/// One current-state reference supplied during plan authoring.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DraftContextInput {
    /// Referenced file, command, issue, or observed source.
    pub reference: String,
    /// Repository fact derived from the reference.
    pub fact: String,
    /// Consequence of the fact for implementation.
    pub implication: String,
}

/// Replacement values for the authored plan scope.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DraftScopeInput {
    /// Replacement goal when supplied.
    #[serde(default)]
    pub goal: Option<String>,
    /// Complete replacement deliverable list when supplied.
    #[serde(default)]
    pub deliverables: Option<Vec<String>>,
    /// Complete replacement in-scope list when supplied.
    #[serde(default)]
    pub in_scope: Option<Vec<String>>,
    /// Complete replacement out-of-scope list when supplied.
    #[serde(default)]
    pub out_of_scope: Option<Vec<String>>,
}

impl DraftScopeInput {
    /// Returns whether no scope field was supplied.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.goal.is_none()
            && self.deliverables.is_none()
            && self.in_scope.is_none()
            && self.out_of_scope.is_none()
    }
}

/// One decision, assumption, or question supplied during authoring.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DraftDecisionInput {
    /// Decision subject.
    pub item: String,
    /// Decision, assumption, or question classification.
    #[serde(rename = "type")]
    pub kind: String,
    /// Selected value or current answer.
    #[serde(rename = "decision")]
    pub value: String,
    /// Reason for the value.
    pub reason: String,
    /// Resolution status.
    pub status: String,
}

/// One edge case and its expected observable behavior.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DraftEdgeCaseInput {
    /// Edge-case description.
    #[serde(rename = "case")]
    pub case_: String,
    /// Required observable behavior.
    pub expected_behavior: String,
    /// Criterion or check identifiers that cover the case.
    #[serde(default)]
    pub covered_by: Vec<String>,
}

/// One authored file responsibility inside a task.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DraftFileInput {
    /// Project-relative path or narrow glob.
    pub path: String,
    /// Planned change kind.
    pub change: FileChange,
    /// Responsibility assigned to the task.
    pub reason: String,
}

/// One authored acceptance criterion.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DraftCriterionInput {
    /// Optional identifier that must match the next deterministic identifier.
    #[serde(default)]
    pub id: Option<CriterionId>,
    /// Observable acceptance condition.
    pub description: String,
}

/// One authored deterministic verification command.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DraftVerificationInput {
    /// Stable check identifier.
    pub id: CheckId,
    /// Executable and exact argument vector.
    pub command: Vec<String>,
    /// Project-relative working directory.
    #[serde(default = "default_working_directory")]
    pub cwd: String,
    /// Expected process exit code.
    #[serde(default)]
    pub expected_exit_code: i32,
    /// Whether the check gates completion.
    #[serde(default = "default_required")]
    pub required: bool,
}

impl DraftVerificationInput {
    /// Converts this authored value into a pending domain verification check.
    #[must_use]
    pub fn into_check(self) -> VerificationCheck {
        VerificationCheck::new(
            self.id,
            self.command,
            self.cwd,
            self.expected_exit_code,
            self.required,
        )
    }
}

/// One authored task-level Git commit gate.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DraftCommitGateInput {
    /// Whether task completion requires a commit.
    pub required: bool,
    /// Exact planned Conventional Commit message.
    pub planned_message: String,
    /// Exact project-relative paths or narrow globs allowed in the commit.
    #[serde(default)]
    pub scope: Vec<String>,
}

/// One task definition accepted before execution begins.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DraftTaskInput {
    /// Optional identifier that must match the next deterministic task ID.
    #[serde(default)]
    pub id: Option<TaskId>,
    /// Concise implementation outcome.
    pub title: String,
    /// Earlier task identifiers that must complete first.
    #[serde(default)]
    pub depends_on: Vec<TaskId>,
    /// Ordered implementation steps.
    #[serde(default)]
    pub steps: Vec<String>,
    /// File responsibilities owned by the task.
    #[serde(default)]
    pub files: Vec<DraftFileInput>,
    /// Observable acceptance criteria.
    #[serde(default)]
    pub acceptance_criteria: Vec<DraftCriterionInput>,
    /// Task-scoped verification commands.
    #[serde(default)]
    pub verification: Vec<DraftVerificationInput>,
    /// Optional task-level Git commit gate.
    #[serde(default)]
    pub commit_gate: Option<DraftCommitGateInput>,
}

/// Replacement fields for one existing Draft task.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DraftTaskUpdateInput {
    /// Replacement task title when supplied.
    #[serde(default)]
    pub title: Option<String>,
    /// Complete replacement dependency list when supplied.
    #[serde(default)]
    pub depends_on: Option<Vec<TaskId>>,
    /// Complete replacement commit gate when supplied.
    #[serde(default)]
    pub commit_gate: Option<DraftCommitGateInput>,
    /// Remove the existing optional commit gate.
    #[serde(default)]
    pub clear_commit_gate: bool,
}

impl DraftTaskUpdateInput {
    /// Returns whether the update contains no replacement operation.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.depends_on.is_none()
            && self.commit_gate.is_none()
            && !self.clear_commit_gate
    }
}

/// Strict batch input containing authored fields and no lifecycle state.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DraftPlanInput {
    /// Optional human metadata replacements.
    #[serde(default)]
    pub metadata: Option<DraftMetadataInput>,
    /// Optional plan summary replacement.
    #[serde(default)]
    pub summary: Option<String>,
    /// Current-state references appended in order.
    #[serde(default)]
    pub context: Vec<DraftContextInput>,
    /// Optional authored scope replacement.
    #[serde(default)]
    pub scope: Option<DraftScopeInput>,
    /// Decisions appended in order.
    #[serde(default)]
    pub decisions: Vec<DraftDecisionInput>,
    /// Optional implementation approach replacement.
    #[serde(default)]
    pub approach: Option<String>,
    /// Optional interfaces and data-flow replacement.
    #[serde(default)]
    pub interfaces: Option<String>,
    /// Edge cases appended in order.
    #[serde(default)]
    pub edge_cases: Vec<DraftEdgeCaseInput>,
    /// Tasks appended in implementation order.
    #[serde(default)]
    pub tasks: Vec<DraftTaskInput>,
    /// Global verification checks appended in order.
    #[serde(default)]
    pub verification_plan: Vec<DraftVerificationInput>,
}

impl DraftPlanInput {
    /// Returns whether the input would make no authored change.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.metadata
            .as_ref()
            .is_none_or(DraftMetadataInput::is_empty)
            && self.summary.is_none()
            && self.context.is_empty()
            && self.scope.as_ref().is_none_or(DraftScopeInput::is_empty)
            && self.decisions.is_empty()
            && self.approach.is_none()
            && self.interfaces.is_none()
            && self.edge_cases.is_empty()
            && self.tasks.is_empty()
            && self.verification_plan.is_empty()
    }
}

fn default_working_directory() -> String {
    ".".to_owned()
}

const fn default_required() -> bool {
    true
}
