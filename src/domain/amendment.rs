//! Typed protected-plan amendment records and classification policy.

use std::collections::BTreeSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::task::is_safe_repository_path;
use super::{
    CheckId, CriterionId, DomainError, DomainErrorKind, DraftCommitGateInput, DraftCriterionInput,
    DraftTaskInput, DraftVerificationInput, EvidenceId, FileChange, Task, TaskId, Timestamp,
};

/// Minimum protection class required by an amendment operation.
#[derive(
    Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize, JsonSchema,
)]
pub enum AmendmentClassification {
    /// Task-local allowlisted support that preserves behavior and scope.
    Minor,
    /// A protected change to behavior, scope, security, compatibility, or order.
    Material,
}

/// Lifecycle state of one immutable amendment proposal.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub enum AmendmentStatus {
    /// A Minor proposal is ready for deterministic application.
    Proposed,
    /// A Material proposal is blocked at its explicit approval boundary.
    #[serde(rename = "Approval Required")]
    ApprovalRequired,
    /// A Material proposal has an auditable approval declaration.
    Approved,
    /// An unapproved Material proposal was explicitly rejected.
    Rejected,
    /// The original proposer withdrew an unapplied proposal.
    Withdrawn,
    /// The original approver cancelled an approved Material proposal.
    Cancelled,
    /// The typed operations were applied and their invalidations recorded.
    Applied,
}

/// Allowlisted task-local file role for a Minor amendment.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub enum MinorFileKind {
    /// A test-only fixture.
    #[serde(rename = "Test Fixture")]
    TestFixture,
    /// A barrel export required by the existing task contract.
    #[serde(rename = "Barrel Export")]
    BarrelExport,
    /// A test or UI snapshot.
    Snapshot,
    /// A task-local support file that preserves approved behavior and scope.
    #[serde(rename = "Support File")]
    SupportFile,
}

/// Protected material decision category.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub enum ProtectedChangeCategory {
    /// User-observable behavior.
    #[serde(rename = "User-Visible Behavior")]
    UserVisibleBehavior,
    /// A public programming interface.
    #[serde(rename = "Public API")]
    PublicApi,
    /// Persistent or exchanged data shape.
    #[serde(rename = "Data or Schema")]
    DataOrSchema,
    /// A project or runtime dependency.
    Dependency,
    /// A compatibility commitment.
    Compatibility,
    /// A security or permission boundary.
    Security,
}

impl ProtectedChangeCategory {
    /// Returns the stable human-readable category label.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UserVisibleBehavior => "User-Visible Behavior",
            Self::PublicApi => "Public API",
            Self::DataOrSchema => "Data or Schema",
            Self::Dependency => "Dependency",
            Self::Compatibility => "Compatibility",
            Self::Security => "Security",
        }
    }
}

/// One typed semantic operation accepted by the amendment protocol.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "operation", rename_all = "kebab-case", deny_unknown_fields)]
pub enum AmendmentOperation {
    /// Adds one explicitly allowlisted task-local file responsibility.
    AddTaskFile {
        /// Allowlisted file role.
        kind: MinorFileKind,
        /// Existing Ready or In Progress task.
        task_id: TaskId,
        /// Project-relative file path or narrow glob.
        path: String,
        /// Planned change kind.
        change: FileChange,
        /// Task-local reason for the file.
        reason: String,
    },
    /// Replaces one task verification definition while retaining its stable ID.
    ReplaceTaskVerification {
        /// Existing Ready or In Progress task.
        task_id: TaskId,
        /// Existing verification check identifier.
        check_id: CheckId,
        /// Exact replacement executable and arguments.
        command: Vec<String>,
        /// Project-relative working directory.
        cwd: String,
        /// Expected replacement exit code.
        expected_exit_code: i32,
        /// Whether the replacement check gates completion.
        required: bool,
    },
    /// Appends a non-behavioral implementation note to one task.
    AddImplementationNote {
        /// Existing Ready or In Progress task.
        task_id: TaskId,
        /// Non-empty implementation note.
        note: String,
    },
    /// Adds one complete task to the protected execution graph.
    AddTask {
        /// Complete authored task with an explicit monotonic identifier.
        task: DraftTaskInput,
    },
    /// Replaces one task's title and ordered implementation steps.
    UpdateTaskDefinition {
        /// Existing task to update.
        task_id: TaskId,
        /// Complete replacement title.
        title: String,
        /// Complete replacement ordered steps.
        steps: Vec<String>,
    },
    /// Removes one task and its current File Map responsibilities.
    RemoveTask {
        /// Existing task to remove.
        task_id: TaskId,
    },
    /// Replaces one task's complete dependency list.
    ReplaceTaskDependencies {
        /// Existing task to update.
        task_id: TaskId,
        /// Complete replacement dependency list.
        depends_on: Vec<TaskId>,
    },
    /// Adds one explicit acceptance criterion to a task.
    AddCriterion {
        /// Existing task to update.
        task_id: TaskId,
        /// Criterion with an explicit next identifier.
        criterion: DraftCriterionInput,
    },
    /// Replaces one acceptance criterion description.
    UpdateCriterion {
        /// Existing task to update.
        task_id: TaskId,
        /// Existing stable criterion identifier.
        criterion_id: CriterionId,
        /// Complete replacement description.
        description: String,
    },
    /// Removes one acceptance criterion.
    RemoveCriterion {
        /// Existing task to update.
        task_id: TaskId,
        /// Existing stable criterion identifier.
        criterion_id: CriterionId,
    },
    /// Adds one task-scoped verification check.
    AddTaskVerification {
        /// Existing task to update.
        task_id: TaskId,
        /// Complete pending verification definition.
        verification: DraftVerificationInput,
    },
    /// Replaces one task-scoped verification check.
    UpdateTaskVerification {
        /// Existing task to update.
        task_id: TaskId,
        /// Existing stable check identifier.
        check_id: CheckId,
        /// Complete replacement definition with the same identifier.
        verification: DraftVerificationInput,
    },
    /// Removes one task-scoped verification check.
    RemoveTaskVerification {
        /// Existing task to update.
        task_id: TaskId,
        /// Existing stable check identifier.
        check_id: CheckId,
    },
    /// Adds one global verification check.
    AddGlobalVerification {
        /// Complete pending verification definition.
        verification: DraftVerificationInput,
    },
    /// Replaces one global verification check.
    UpdateGlobalVerification {
        /// Existing stable check identifier.
        check_id: CheckId,
        /// Complete replacement definition with the same identifier.
        verification: DraftVerificationInput,
    },
    /// Removes one global verification check.
    RemoveGlobalVerification {
        /// Existing stable check identifier.
        check_id: CheckId,
    },
    /// Replaces one task's complete commit gate.
    ReplaceCommitGate {
        /// Existing task to update.
        task_id: TaskId,
        /// Complete replacement commit gate.
        commit_gate: DraftCommitGateInput,
    },
    /// Removes one task's commit gate.
    RemoveCommitGate {
        /// Existing task to update.
        task_id: TaskId,
    },
    /// Replaces the plan summary and therefore its approved outcome description.
    ReplaceSummary {
        /// Complete replacement summary.
        summary: String,
    },
    /// Replaces every scope boundary as one protected operation.
    ReplaceScope {
        /// Complete goal.
        goal: String,
        /// Complete deliverable list.
        deliverables: Vec<String>,
        /// Complete in-scope list.
        in_scope: Vec<String>,
        /// Complete out-of-scope list.
        out_of_scope: Vec<String>,
    },
    /// Replaces the implementation approach.
    ReplaceApproach {
        /// Complete replacement approach.
        approach: String,
    },
    /// Replaces the public interfaces and data-flow description.
    ReplaceInterfaces {
        /// Complete replacement interface description.
        interfaces: String,
    },
    /// Records a protected behavior/API/data/dependency/compatibility/security decision.
    RecordProtectedDecision {
        /// Protected decision category.
        category: ProtectedChangeCategory,
        /// Decision subject.
        item: String,
        /// Selected value.
        decision: String,
        /// Reason for the protected decision.
        reason: String,
    },
    /// Replaces the complete core task order.
    ReplaceTaskOrder {
        /// Complete unique task order.
        task_order: Vec<TaskId>,
    },
    /// Expands one task File Map outside the Minor allowlist.
    ExpandTaskFile {
        /// Existing task whose approved scope expands.
        task_id: TaskId,
        /// Project-relative path or narrow glob.
        path: String,
        /// Planned change kind.
        change: FileChange,
        /// Reason for the material scope expansion.
        reason: String,
    },
}

impl AmendmentOperation {
    /// Returns the minimum classification required by this operation.
    #[must_use]
    pub const fn minimum_classification(&self) -> AmendmentClassification {
        match self {
            Self::AddTaskFile { .. }
            | Self::ReplaceTaskVerification { .. }
            | Self::AddImplementationNote { .. } => AmendmentClassification::Minor,
            Self::ReplaceSummary { .. }
            | Self::ReplaceScope { .. }
            | Self::ReplaceApproach { .. }
            | Self::ReplaceInterfaces { .. }
            | Self::RecordProtectedDecision { .. }
            | Self::ReplaceTaskOrder { .. }
            | Self::ExpandTaskFile { .. }
            | Self::AddTask { .. }
            | Self::UpdateTaskDefinition { .. }
            | Self::RemoveTask { .. }
            | Self::ReplaceTaskDependencies { .. }
            | Self::AddCriterion { .. }
            | Self::UpdateCriterion { .. }
            | Self::RemoveCriterion { .. }
            | Self::AddTaskVerification { .. }
            | Self::UpdateTaskVerification { .. }
            | Self::RemoveTaskVerification { .. }
            | Self::AddGlobalVerification { .. }
            | Self::UpdateGlobalVerification { .. }
            | Self::RemoveGlobalVerification { .. }
            | Self::ReplaceCommitGate { .. }
            | Self::RemoveCommitGate { .. } => AmendmentClassification::Material,
        }
    }

    pub(crate) fn validate(&self) -> Result<(), DomainError> {
        let valid = match self {
            Self::AddTaskFile {
                kind,
                path,
                change,
                reason,
                ..
            } => {
                is_safe_repository_path(path)
                    && !reason.trim().is_empty()
                    && minor_file_change_is_valid(*kind, *change)
            }
            Self::ReplaceTaskVerification { command, cwd, .. } => {
                !command.is_empty()
                    && command.iter().all(|part| !part.trim().is_empty())
                    && !cwd.trim().is_empty()
            }
            Self::AddImplementationNote { note, .. } => !note.trim().is_empty(),
            Self::AddTask { task } => complete_task_input(task),
            Self::UpdateTaskDefinition { title, steps, .. } => {
                !title.trim().is_empty() && steps.iter().all(|step| !step.trim().is_empty())
            }
            Self::RemoveTask { .. }
            | Self::RemoveCriterion { .. }
            | Self::RemoveTaskVerification { .. }
            | Self::RemoveGlobalVerification { .. }
            | Self::RemoveCommitGate { .. } => true,
            Self::ReplaceTaskDependencies { depends_on, .. } => {
                depends_on.iter().collect::<BTreeSet<_>>().len() == depends_on.len()
            }
            Self::AddCriterion { criterion, .. } => {
                criterion.id.is_some() && !criterion.description.trim().is_empty()
            }
            Self::UpdateCriterion { description, .. } => !description.trim().is_empty(),
            Self::AddTaskVerification { verification, .. }
            | Self::UpdateTaskVerification { verification, .. }
            | Self::AddGlobalVerification { verification }
            | Self::UpdateGlobalVerification { verification, .. } => {
                complete_verification_input(verification)
            }
            Self::ReplaceCommitGate { commit_gate, .. } => complete_commit_gate_input(commit_gate),
            Self::ReplaceSummary { summary } => !summary.trim().is_empty(),
            Self::ReplaceScope {
                goal,
                deliverables,
                in_scope,
                out_of_scope,
            } => {
                !goal.trim().is_empty()
                    && complete_text_list(deliverables)
                    && complete_text_list(in_scope)
                    && complete_text_list(out_of_scope)
            }
            Self::ReplaceApproach { approach } => !approach.trim().is_empty(),
            Self::ReplaceInterfaces { interfaces } => !interfaces.trim().is_empty(),
            Self::RecordProtectedDecision {
                item,
                decision,
                reason,
                ..
            } => {
                !item.trim().is_empty() && !decision.trim().is_empty() && !reason.trim().is_empty()
            }
            Self::ReplaceTaskOrder { task_order } => {
                !task_order.is_empty()
                    && task_order.iter().collect::<BTreeSet<_>>().len() == task_order.len()
            }
            Self::ExpandTaskFile { path, reason, .. } => {
                is_safe_repository_path(path) && !reason.trim().is_empty()
            }
        };
        if valid {
            Ok(())
        } else {
            Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                "Amendment operation is incomplete or violates its typed allowlist",
            ))
        }
    }
}

/// Strict YAML document containing only supported semantic operations.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AmendmentPatch {
    operations: Vec<AmendmentOperation>,
}

impl AmendmentPatch {
    /// Returns typed operations in document order.
    #[must_use]
    pub fn operations(&self) -> &[AmendmentOperation] {
        &self.operations
    }

    /// Consumes the patch and returns its typed operations.
    #[must_use]
    pub fn into_operations(self) -> Vec<AmendmentOperation> {
        self.operations
    }

    /// Returns the minimum classification required by every operation together.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty or malformed operation list.
    pub fn minimum_classification(&self) -> Result<AmendmentClassification, DomainError> {
        if self.operations.is_empty() {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                "Amendment patch requires at least one typed operation",
            ));
        }
        let mut classification = AmendmentClassification::Minor;
        for operation in &self.operations {
            operation.validate()?;
            classification = classification.max(operation.minimum_classification());
        }
        Ok(classification)
    }
}

/// Deterministically computed fields and evidence affected by a proposal.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AmendmentImpact {
    affected_fields: Vec<String>,
    affected_tasks: Vec<TaskId>,
    affected_checks: Vec<CheckId>,
    stale_evidence: Vec<EvidenceId>,
}

impl AmendmentImpact {
    pub(crate) fn new(
        affected_fields: Vec<String>,
        affected_tasks: Vec<TaskId>,
        affected_checks: Vec<CheckId>,
        stale_evidence: Vec<EvidenceId>,
    ) -> Result<Self, DomainError> {
        let impact = Self {
            affected_fields,
            affected_tasks,
            affected_checks,
            stale_evidence,
        };
        impact.validate()?;
        Ok(impact)
    }

    /// Returns exact semantic fields affected by the operations.
    #[must_use]
    pub fn affected_fields(&self) -> &[String] {
        &self.affected_fields
    }

    /// Returns tasks whose execution inputs are affected.
    #[must_use]
    pub fn affected_tasks(&self) -> &[TaskId] {
        &self.affected_tasks
    }

    /// Returns verification checks whose prior results are invalidated.
    #[must_use]
    pub fn affected_checks(&self) -> &[CheckId] {
        &self.affected_checks
    }

    /// Returns evidence made stale by application of the proposal.
    #[must_use]
    pub fn stale_evidence(&self) -> &[EvidenceId] {
        &self.stale_evidence
    }

    fn validate(&self) -> Result<(), DomainError> {
        if self.affected_fields.is_empty()
            || self
                .affected_fields
                .iter()
                .any(|field| field.trim().is_empty())
            || !strictly_sorted(&self.affected_fields)
            || !strictly_sorted(&self.affected_tasks)
            || !strictly_sorted(&self.affected_checks)
            || !strictly_sorted(&self.stale_evidence)
        {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                "Amendment impact must contain sorted unique affected fields and identifiers",
            ));
        }
        Ok(())
    }
}

/// Auditable typed amendment proposal and its approval/application history.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Amendment {
    id: String,
    reason: String,
    minimum_classification: AmendmentClassification,
    classification: AmendmentClassification,
    status: AmendmentStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_review_id: Option<String>,
    operations: Vec<AmendmentOperation>,
    base_revision: u64,
    base_state_hash: String,
    impact: AmendmentImpact,
    proposer: String,
    proposed_at: Timestamp,
    approval_actor: Option<String>,
    approval_reference: Option<String>,
    approved_at: Option<Timestamp>,
    applied_at: Option<Timestamp>,
    #[serde(default)]
    disposition_actor: Option<String>,
    #[serde(default)]
    disposition_reference: Option<String>,
    #[serde(default)]
    disposition_reason: Option<String>,
    #[serde(default)]
    disposed_at: Option<Timestamp>,
}

impl Amendment {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn proposed(
        id: String,
        reason: String,
        minimum_classification: AmendmentClassification,
        classification: AmendmentClassification,
        source_review_id: Option<String>,
        operations: Vec<AmendmentOperation>,
        base_revision: u64,
        base_state_hash: String,
        impact: AmendmentImpact,
        proposer: String,
        proposed_at: Timestamp,
    ) -> Result<Self, DomainError> {
        let amendment = Self {
            id,
            reason,
            minimum_classification,
            classification,
            status: if classification == AmendmentClassification::Material {
                AmendmentStatus::ApprovalRequired
            } else {
                AmendmentStatus::Proposed
            },
            source_review_id,
            operations,
            base_revision,
            base_state_hash,
            impact,
            proposer,
            proposed_at,
            approval_actor: None,
            approval_reference: None,
            approved_at: None,
            applied_at: None,
            disposition_actor: None,
            disposition_reference: None,
            disposition_reason: None,
            disposed_at: None,
        };
        amendment.validate()?;
        Ok(amendment)
    }

    /// Returns the monotonic change identifier.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns the exact reason supplied for the proposal.
    #[must_use]
    pub fn reason(&self) -> &str {
        &self.reason
    }

    /// Returns the classifier-derived minimum protection class.
    #[must_use]
    pub const fn minimum_classification(&self) -> AmendmentClassification {
        self.minimum_classification
    }

    /// Returns the selected protection class, which may only raise the minimum.
    #[must_use]
    pub const fn classification(&self) -> AmendmentClassification {
        self.classification
    }

    /// Returns the current amendment lifecycle state.
    #[must_use]
    pub const fn status(&self) -> AmendmentStatus {
        self.status
    }

    /// Returns the Material review request that owns this amendment.
    #[must_use]
    pub fn source_review_id(&self) -> Option<&str> {
        self.source_review_id.as_deref()
    }

    /// Returns immutable typed operations in proposal order.
    #[must_use]
    pub fn operations(&self) -> &[AmendmentOperation] {
        &self.operations
    }

    /// Returns the exact source plan revision preserved by the store snapshot.
    #[must_use]
    pub const fn base_revision(&self) -> u64 {
        self.base_revision
    }

    /// Returns the canonical source-state digest at proposal time.
    #[must_use]
    pub fn base_state_hash(&self) -> &str {
        &self.base_state_hash
    }

    /// Returns the classifier-derived impact and stale-evidence set.
    #[must_use]
    pub const fn impact(&self) -> &AmendmentImpact {
        &self.impact
    }

    /// Returns the approval reference for an approved Material change.
    #[must_use]
    pub fn approval_reference(&self) -> Option<&str> {
        self.approval_reference.as_deref()
    }

    /// Returns the actor who proposed the change.
    #[must_use]
    pub fn proposer(&self) -> &str {
        &self.proposer
    }

    /// Returns when the proposal was recorded.
    #[must_use]
    pub const fn proposed_at(&self) -> &Timestamp {
        &self.proposed_at
    }

    /// Returns the actor who approved a Material proposal.
    #[must_use]
    pub fn approval_actor(&self) -> Option<&str> {
        self.approval_actor.as_deref()
    }

    /// Returns when a Material proposal was approved.
    #[must_use]
    pub const fn approved_at(&self) -> Option<&Timestamp> {
        self.approved_at.as_ref()
    }

    /// Returns when the operations were atomically applied.
    #[must_use]
    pub const fn applied_at(&self) -> Option<&Timestamp> {
        self.applied_at.as_ref()
    }

    /// Returns the actor who recorded a terminal disposition without applying the patch.
    #[must_use]
    pub fn disposition_actor(&self) -> Option<&str> {
        self.disposition_actor.as_deref()
    }

    /// Returns the approval or decision reference for a protected terminal disposition.
    #[must_use]
    pub fn disposition_reference(&self) -> Option<&str> {
        self.disposition_reference.as_deref()
    }

    /// Returns why the proposal was terminated without applying it.
    #[must_use]
    pub fn disposition_reason(&self) -> Option<&str> {
        self.disposition_reason.as_deref()
    }

    /// Returns when the terminal disposition was recorded.
    #[must_use]
    pub const fn disposed_at(&self) -> Option<&Timestamp> {
        self.disposed_at.as_ref()
    }

    /// Returns whether the proposal still blocks unrelated plan mutations.
    #[must_use]
    pub fn is_pending(&self) -> bool {
        matches!(
            self.status,
            AmendmentStatus::Proposed
                | AmendmentStatus::ApprovalRequired
                | AmendmentStatus::Approved
        )
    }

    /// Returns whether the typed operations were applied to the plan.
    #[must_use]
    pub fn is_applied(&self) -> bool {
        self.status == AmendmentStatus::Applied
    }

    /// Returns whether the proposal ended without applying its operations.
    #[must_use]
    pub const fn is_terminated_without_apply(&self) -> bool {
        matches!(
            self.status,
            AmendmentStatus::Rejected | AmendmentStatus::Withdrawn | AmendmentStatus::Cancelled
        )
    }

    pub(crate) fn approve(
        &mut self,
        actor: String,
        reference: String,
        approved_at: Timestamp,
    ) -> Result<(), DomainError> {
        if self.classification != AmendmentClassification::Material
            || self.status != AmendmentStatus::ApprovalRequired
            || actor.trim().is_empty()
            || reference.trim().is_empty()
        {
            return Err(DomainError::new(
                DomainErrorKind::ApprovalRequired,
                format!(
                    "Amendment {} requires one explicit Material approval",
                    self.id
                ),
            ));
        }
        self.status = AmendmentStatus::Approved;
        self.approval_actor = Some(actor);
        self.approval_reference = Some(reference);
        self.approved_at = Some(approved_at);
        self.validate()
    }

    pub(crate) fn mark_applied(&mut self, applied_at: Timestamp) -> Result<(), DomainError> {
        let is_eligible = match self.classification {
            AmendmentClassification::Minor => self.status == AmendmentStatus::Proposed,
            AmendmentClassification::Material => self.status == AmendmentStatus::Approved,
        };
        if !is_eligible {
            return Err(DomainError::new(
                DomainErrorKind::InvalidTransition,
                format!("Amendment {} is not eligible to apply", self.id),
            ));
        }
        self.status = AmendmentStatus::Applied;
        self.applied_at = Some(applied_at);
        self.validate()
    }

    pub(crate) fn reject(
        &mut self,
        actor: String,
        reference: String,
        reason: String,
        disposed_at: Timestamp,
    ) -> Result<(), DomainError> {
        if self.classification != AmendmentClassification::Material
            || self.status != AmendmentStatus::ApprovalRequired
            || actor.trim().is_empty()
            || reference.trim().is_empty()
            || reason.trim().is_empty()
        {
            return Err(DomainError::new(
                DomainErrorKind::InvalidTransition,
                format!("Amendment {} is not eligible for rejection", self.id),
            ));
        }
        self.status = AmendmentStatus::Rejected;
        self.record_disposition(actor, Some(reference), reason, disposed_at);
        self.validate()
    }

    pub(crate) fn withdraw(
        &mut self,
        actor: String,
        reason: String,
        disposed_at: Timestamp,
    ) -> Result<(), DomainError> {
        if !matches!(
            self.status,
            AmendmentStatus::Proposed | AmendmentStatus::ApprovalRequired
        ) || actor != self.proposer
            || reason.trim().is_empty()
        {
            return Err(DomainError::new(
                DomainErrorKind::InvalidTransition,
                format!("Amendment {} is not eligible for withdrawal", self.id),
            ));
        }
        self.status = AmendmentStatus::Withdrawn;
        self.record_disposition(actor, None, reason, disposed_at);
        self.validate()
    }

    pub(crate) fn cancel(
        &mut self,
        actor: String,
        reference: String,
        reason: String,
        disposed_at: Timestamp,
    ) -> Result<(), DomainError> {
        if self.classification != AmendmentClassification::Material
            || self.status != AmendmentStatus::Approved
            || self.approval_actor.as_deref() != Some(actor.as_str())
            || reference.trim().is_empty()
            || reason.trim().is_empty()
        {
            return Err(DomainError::new(
                DomainErrorKind::InvalidTransition,
                format!("Amendment {} is not eligible for cancellation", self.id),
            ));
        }
        self.status = AmendmentStatus::Cancelled;
        self.record_disposition(actor, Some(reference), reason, disposed_at);
        self.validate()
    }

    fn record_disposition(
        &mut self,
        actor: String,
        reference: Option<String>,
        reason: String,
        disposed_at: Timestamp,
    ) {
        self.disposition_actor = Some(actor);
        self.disposition_reference = reference;
        self.disposition_reason = Some(reason);
        self.disposed_at = Some(disposed_at);
    }

    pub(crate) fn validate(&self) -> Result<(), DomainError> {
        if change_number(&self.id).is_none()
            || self.reason.trim().is_empty()
            || self.proposer.trim().is_empty()
            || self.base_revision == 0
            || !is_sha256(&self.base_state_hash)
            || self.operations.is_empty()
            || self.classification < self.minimum_classification
            || self.source_review_id.as_deref().is_some_and(|review_id| {
                self.classification != AmendmentClassification::Material || !is_review_id(review_id)
            })
        {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                "Amendment identity, source, or classification is malformed",
            ));
        }
        let computed_minimum = self.operations.iter().try_fold(
            AmendmentClassification::Minor,
            |current, operation| {
                operation.validate()?;
                Ok::<_, DomainError>(current.max(operation.minimum_classification()))
            },
        )?;
        if computed_minimum > self.minimum_classification {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                "Amendment minimum classification is below its typed operations",
            ));
        }
        self.impact.validate()?;
        let approval_complete = self
            .approval_actor
            .as_deref()
            .is_some_and(|actor| !actor.trim().is_empty())
            && self
                .approval_reference
                .as_deref()
                .is_some_and(|reference| !reference.trim().is_empty())
            && self.approved_at.is_some();
        let state_valid = self.lifecycle_is_valid(approval_complete);
        if state_valid {
            Ok(())
        } else {
            Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                format!("Amendment {} has inconsistent lifecycle fields", self.id),
            ))
        }
    }

    fn lifecycle_is_valid(&self, approval_complete: bool) -> bool {
        let disposition_actor = self
            .disposition_actor
            .as_deref()
            .filter(|actor| !actor.trim().is_empty());
        let disposition_reference = self
            .disposition_reference
            .as_deref()
            .filter(|reference| !reference.trim().is_empty());
        let disposition_reason = self
            .disposition_reason
            .as_deref()
            .filter(|reason| !reason.trim().is_empty());
        let has_no_disposition = self.disposition_actor.is_none()
            && self.disposition_reference.is_none()
            && self.disposition_reason.is_none()
            && self.disposed_at.is_none();
        match self.status {
            AmendmentStatus::Proposed => {
                self.classification == AmendmentClassification::Minor
                    && !approval_complete
                    && self.applied_at.is_none()
                    && has_no_disposition
            }
            AmendmentStatus::ApprovalRequired => {
                self.classification == AmendmentClassification::Material
                    && !approval_complete
                    && self.applied_at.is_none()
                    && has_no_disposition
            }
            AmendmentStatus::Approved => {
                self.classification == AmendmentClassification::Material
                    && approval_complete
                    && self.applied_at.is_none()
                    && has_no_disposition
            }
            AmendmentStatus::Rejected => {
                self.classification == AmendmentClassification::Material
                    && !approval_complete
                    && self.applied_at.is_none()
                    && disposition_actor.is_some()
                    && disposition_reference.is_some()
                    && disposition_reason.is_some()
                    && self.disposed_at.is_some()
            }
            AmendmentStatus::Withdrawn => {
                !approval_complete
                    && self.applied_at.is_none()
                    && disposition_actor == Some(self.proposer.as_str())
                    && self.disposition_reference.is_none()
                    && disposition_reason.is_some()
                    && self.disposed_at.is_some()
            }
            AmendmentStatus::Cancelled => {
                self.classification == AmendmentClassification::Material
                    && approval_complete
                    && self.applied_at.is_none()
                    && disposition_actor == self.approval_actor.as_deref()
                    && disposition_reference.is_some()
                    && disposition_reason.is_some()
                    && self.disposed_at.is_some()
            }
            AmendmentStatus::Applied => {
                self.applied_at.is_some()
                    && (self.classification == AmendmentClassification::Minor || approval_complete)
                    && has_no_disposition
            }
        }
    }
}

fn is_review_id(value: &str) -> bool {
    value
        .strip_prefix("REV-")
        .filter(|number| !number.starts_with('0'))
        .and_then(|number| number.parse::<u64>().ok())
        .is_some_and(|number| number > 0)
}

pub(crate) fn change_number(id: &str) -> Option<u64> {
    id.strip_prefix('C')
        .filter(|number| !number.starts_with('0'))
        .and_then(|number| number.parse().ok())
        .filter(|number| *number > 0)
}

fn minor_file_change_is_valid(kind: MinorFileKind, change: FileChange) -> bool {
    match kind {
        MinorFileKind::TestFixture | MinorFileKind::SupportFile => {
            matches!(change, FileChange::Create | FileChange::Test)
        }
        MinorFileKind::BarrelExport => {
            matches!(change, FileChange::Create | FileChange::Modify)
        }
        MinorFileKind::Snapshot => matches!(
            change,
            FileChange::Create | FileChange::Modify | FileChange::Test
        ),
    }
}

fn complete_text_list(values: &[String]) -> bool {
    !values.is_empty() && values.iter().all(|value| !value.trim().is_empty())
}

fn complete_task_input(input: &DraftTaskInput) -> bool {
    let Some(task_id) = input.id.as_ref() else {
        return false;
    };
    if input
        .files
        .iter()
        .any(|file| !is_safe_repository_path(&file.path))
    {
        return false;
    }
    Task::from_draft(task_id, input.clone())
        .and_then(|mut task| task.mark_ready())
        .is_ok()
}

fn complete_verification_input(input: &DraftVerificationInput) -> bool {
    !input.command.is_empty()
        && input.command.iter().all(|part| !part.trim().is_empty())
        && !input.cwd.trim().is_empty()
}

fn complete_commit_gate_input(input: &DraftCommitGateInput) -> bool {
    !input.planned_message.trim().is_empty()
        && !input.planned_message.contains(['\r', '\n'])
        && !input.scope.is_empty()
        && input.scope.iter().all(|path| is_safe_repository_path(path))
}

fn strictly_sorted<T: Ord>(values: &[T]) -> bool {
    values.windows(2).all(|pair| pair[0] < pair[1])
}

fn is_sha256(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}
