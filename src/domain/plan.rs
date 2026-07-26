//! Plan aggregate and its supporting authored and execution entities.

use std::collections::{BTreeMap, BTreeSet};

use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};

use super::execution::EXECUTION_EXTENSION_KEY;
use super::{
    CheckId, CheckStatus, CheckpointKind, CommitStatus, CriterionId, DomainError, DomainErrorKind,
    DraftContextInput, DraftCriterionInput, DraftDecisionInput, DraftEdgeCaseInput, DraftFileInput,
    DraftMetadataInput, DraftPlanInput, DraftScopeInput, DraftTaskInput, DraftVerificationInput,
    EvidenceId, ExecutionState, FileMapEntry, GitFlowConsent, PlanDraftSeed, PlanId, PlanStatus,
    ProtocolVersion, ReviewClassification, ReviewItem, ReviewStatus, SchemaVersion, Task, TaskId,
    TaskStatus, Timestamp, VerificationCheck,
};

/// Human and repository metadata associated with a plan.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanMetadata {
    name: String,
    priority: String,
    plan_type: String,
    area: String,
    owner: String,
    created_at: Timestamp,
    updated_at: Timestamp,
    branch: Option<String>,
    markdown_path: Option<String>,
}

impl PlanMetadata {
    /// Returns the human-readable requirement name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the declared priority.
    #[must_use]
    pub fn priority(&self) -> &str {
        &self.priority
    }

    /// Returns the declared plan type.
    #[must_use]
    pub fn plan_type(&self) -> &str {
        &self.plan_type
    }

    /// Returns the declared area.
    #[must_use]
    pub fn area(&self) -> &str {
        &self.area
    }

    /// Returns the declared owner.
    #[must_use]
    pub fn owner(&self) -> &str {
        &self.owner
    }

    /// Returns the creation timestamp.
    #[must_use]
    pub const fn created_at(&self) -> &Timestamp {
        &self.created_at
    }

    /// Returns the last authored or lifecycle update timestamp.
    #[must_use]
    pub const fn updated_at(&self) -> &Timestamp {
        &self.updated_at
    }

    /// Returns the captured Git branch when present.
    #[must_use]
    pub fn branch(&self) -> Option<&str> {
        self.branch.as_deref()
    }

    /// Returns the project-relative managed Markdown path when present.
    #[must_use]
    pub fn markdown_path(&self) -> Option<&str> {
        self.markdown_path.as_deref()
    }
}

/// A discovered fact and its implication for the plan.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContextReference {
    reference: String,
    fact: String,
    implication: String,
}

impl ContextReference {
    /// Creates one authored current-state reference.
    #[must_use]
    pub fn new(
        reference: impl Into<String>,
        fact: impl Into<String>,
        implication: impl Into<String>,
    ) -> Self {
        Self {
            reference: reference.into(),
            fact: fact.into(),
            implication: implication.into(),
        }
    }

    /// Returns the referenced source.
    #[must_use]
    pub fn reference(&self) -> &str {
        &self.reference
    }

    /// Returns the repository fact.
    #[must_use]
    pub fn fact(&self) -> &str {
        &self.fact
    }

    /// Returns the implementation implication.
    #[must_use]
    pub fn implication(&self) -> &str {
        &self.implication
    }
}

/// The user-visible objective and explicit scope boundaries.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanScope {
    goal: String,
    deliverables: Vec<String>,
    in_scope: Vec<String>,
    out_of_scope: Vec<String>,
}

impl PlanScope {
    /// Returns the plan goal.
    #[must_use]
    pub fn goal(&self) -> &str {
        &self.goal
    }

    /// Returns declared deliverables.
    #[must_use]
    pub fn deliverables(&self) -> &[String] {
        &self.deliverables
    }

    /// Returns declared in-scope boundaries.
    #[must_use]
    pub fn in_scope(&self) -> &[String] {
        &self.in_scope
    }

    /// Returns declared out-of-scope boundaries.
    #[must_use]
    pub fn out_of_scope(&self) -> &[String] {
        &self.out_of_scope
    }
}

/// A recorded decision, assumption, or question.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Decision {
    item: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(rename = "decision")]
    value: String,
    reason: String,
    status: String,
}

impl Decision {
    /// Creates one authored decision, assumption, or question.
    #[must_use]
    pub fn new(
        item: impl Into<String>,
        kind: impl Into<String>,
        value: impl Into<String>,
        reason: impl Into<String>,
        status: impl Into<String>,
    ) -> Self {
        Self {
            item: item.into(),
            kind: kind.into(),
            value: value.into(),
            reason: reason.into(),
            status: status.into(),
        }
    }

    /// Returns the decision subject.
    #[must_use]
    pub fn item(&self) -> &str {
        &self.item
    }

    /// Returns the decision classification.
    #[must_use]
    pub fn kind(&self) -> &str {
        &self.kind
    }

    /// Returns the selected value.
    #[must_use]
    pub fn value(&self) -> &str {
        &self.value
    }

    /// Returns the decision reason.
    #[must_use]
    pub fn reason(&self) -> &str {
        &self.reason
    }

    /// Returns the resolution status.
    #[must_use]
    pub fn status(&self) -> &str {
        &self.status
    }
}

/// The implementation approach and planned file responsibilities.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Approach {
    summary: String,
    file_map: Vec<FileMapEntry>,
}

impl Approach {
    /// Returns the implementation approach summary.
    #[must_use]
    pub fn summary(&self) -> &str {
        &self.summary
    }

    /// Returns the complete task-owned file map.
    #[must_use]
    pub fn file_map(&self) -> &[FileMapEntry] {
        &self.file_map
    }
}

/// An expected edge case and its observable result.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EdgeCase {
    case: String,
    expected_behavior: String,
    covered_by: Vec<String>,
}

impl EdgeCase {
    /// Creates one authored edge case.
    #[must_use]
    pub fn new(
        case_: impl Into<String>,
        expected_behavior: impl Into<String>,
        covered_by: Vec<String>,
    ) -> Self {
        Self {
            case: case_.into(),
            expected_behavior: expected_behavior.into(),
            covered_by,
        }
    }

    /// Returns the edge-case description.
    #[must_use]
    pub fn case(&self) -> &str {
        &self.case
    }

    /// Returns the required observable behavior.
    #[must_use]
    pub fn expected_behavior(&self) -> &str {
        &self.expected_behavior
    }

    /// Returns criterion and check identifiers covering the case.
    #[must_use]
    pub fn covered_by(&self) -> &[String] {
        &self.covered_by
    }
}

/// A versioned standards package selected for a plan.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StandardSelection {
    package_id: String,
    version: String,
    digest: String,
    source: String,
}

impl StandardSelection {
    /// Creates one exact standards package selection.
    #[must_use]
    pub fn new(
        package_id: impl Into<String>,
        version: impl Into<String>,
        digest: impl Into<String>,
        source: impl Into<String>,
    ) -> Self {
        Self {
            package_id: package_id.into(),
            version: version.into(),
            digest: digest.into(),
            source: source.into(),
        }
    }

    /// Returns the stable package identifier.
    #[must_use]
    pub fn package_id(&self) -> &str {
        &self.package_id
    }

    /// Returns the exact package version.
    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Returns the exact package digest.
    #[must_use]
    pub fn digest(&self) -> &str {
        &self.digest
    }

    /// Returns the package source description.
    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }
}

/// Git repository facts and plan-level consent captured at approval.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GitReadiness {
    repository: String,
    working_tree: String,
    branch: Option<String>,
    base_commit: Option<String>,
    base_status: String,
    git_flow_enabled: bool,
    git_flow_consent: GitFlowConsent,
    approved_at: Option<Timestamp>,
}

impl GitReadiness {
    /// Creates Git readiness facts without claiming user consent.
    #[must_use]
    pub fn detected(
        repository: impl Into<String>,
        working_tree: impl Into<String>,
        branch: Option<String>,
        base_commit: Option<String>,
        base_status: impl Into<String>,
        git_flow_enabled: bool,
    ) -> Self {
        Self {
            repository: repository.into(),
            working_tree: working_tree.into(),
            branch,
            base_commit,
            base_status: base_status.into(),
            git_flow_enabled,
            git_flow_consent: GitFlowConsent::Pending,
            approved_at: None,
        }
    }

    /// Returns the repository presence fact.
    #[must_use]
    pub fn repository(&self) -> &str {
        &self.repository
    }

    /// Returns the working-tree state fact.
    #[must_use]
    pub fn working_tree(&self) -> &str {
        &self.working_tree
    }

    /// Returns the captured branch when present.
    #[must_use]
    pub fn branch(&self) -> Option<&str> {
        self.branch.as_deref()
    }

    /// Returns the captured base commit when present.
    #[must_use]
    pub fn base_commit(&self) -> Option<&str> {
        self.base_commit.as_deref()
    }

    /// Returns the captured base-status description.
    #[must_use]
    pub fn base_status(&self) -> &str {
        &self.base_status
    }

    /// Returns whether clean-baseline Git Flow is eligible.
    #[must_use]
    pub const fn git_flow_enabled(&self) -> bool {
        self.git_flow_enabled
    }

    /// Returns the current Git Flow consent declaration.
    #[must_use]
    pub const fn git_flow_consent(&self) -> GitFlowConsent {
        self.git_flow_consent
    }
}

/// Kinds of explicit approval declarations recorded by Mino.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub enum ApprovalKind {
    /// Approval of a complete plan and its optional Git Flow consent.
    Plan,
    /// Approval of a protected plan amendment.
    Amendment,
    /// Approval of a Git branch mutation.
    Branch,
    /// Approval of an accepted verification or criterion exception.
    Exception,
}

/// An auditable approval declaration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Approval {
    kind: ApprovalKind,
    actor: String,
    reference: String,
    recorded_at: Timestamp,
    git_flow_consent: GitFlowConsent,
}

impl Approval {
    /// Creates a plan approval declaration.
    #[must_use]
    pub fn plan(
        actor: impl Into<String>,
        reference: impl Into<String>,
        recorded_at: Timestamp,
        git_flow_consent: GitFlowConsent,
    ) -> Self {
        Self {
            kind: ApprovalKind::Plan,
            actor: actor.into(),
            reference: reference.into(),
            recorded_at,
            git_flow_consent,
        }
    }

    /// Returns the approval declaration kind.
    #[must_use]
    pub const fn kind(&self) -> ApprovalKind {
        self.kind
    }
}

/// Provenance for a plan created from another plan revision.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Lineage {
    parent_plan_id: PlanId,
    forked_from_revision: u64,
    fork_reason: String,
    source_state_hash: String,
    forked_at: Timestamp,
}

/// User-visible result, residual risk, and non-blocking follow-up work.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FinalOutcome {
    summary: String,
    remaining_risk: String,
    follow_up_tasks: Vec<String>,
}

/// The versioned source-of-truth aggregate for one Mino plan.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    id: PlanId,
    schema_version: SchemaVersion,
    protocol_version: ProtocolVersion,
    #[schemars(range(min = 1))]
    revision: u64,
    status: PlanStatus,
    resume_status: Option<PlanStatus>,
    blocker: Option<String>,
    metadata: PlanMetadata,
    original_request: String,
    #[serde(default)]
    summary: String,
    context: Vec<ContextReference>,
    scope: PlanScope,
    decisions: Vec<Decision>,
    approach: Approach,
    interfaces: String,
    edge_cases: Vec<EdgeCase>,
    standards: Vec<StandardSelection>,
    git_readiness: GitReadiness,
    tasks: Vec<Task>,
    task_order: Vec<TaskId>,
    #[serde(rename = "verification_plan")]
    global_verification: Vec<VerificationCheck>,
    approvals: Vec<Approval>,
    review_items: Vec<ReviewItem>,
    follow_ups: Vec<String>,
    lineage: Option<Lineage>,
    final_outcome: FinalOutcome,
    extensions: BTreeMap<String, serde_json::Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UncheckedPlan {
    id: PlanId,
    schema_version: SchemaVersion,
    protocol_version: ProtocolVersion,
    revision: u64,
    status: PlanStatus,
    resume_status: Option<PlanStatus>,
    blocker: Option<String>,
    metadata: PlanMetadata,
    original_request: String,
    #[serde(default)]
    summary: String,
    context: Vec<ContextReference>,
    scope: PlanScope,
    decisions: Vec<Decision>,
    approach: Approach,
    interfaces: String,
    edge_cases: Vec<EdgeCase>,
    standards: Vec<StandardSelection>,
    git_readiness: GitReadiness,
    tasks: Vec<Task>,
    task_order: Vec<TaskId>,
    #[serde(rename = "verification_plan")]
    global_verification: Vec<VerificationCheck>,
    approvals: Vec<Approval>,
    review_items: Vec<ReviewItem>,
    follow_ups: Vec<String>,
    lineage: Option<Lineage>,
    final_outcome: FinalOutcome,
    extensions: BTreeMap<String, serde_json::Value>,
}

impl TryFrom<UncheckedPlan> for Plan {
    type Error = DomainError;

    fn try_from(unchecked: UncheckedPlan) -> Result<Self, Self::Error> {
        let plan = Self {
            id: unchecked.id,
            schema_version: unchecked.schema_version,
            protocol_version: unchecked.protocol_version,
            revision: unchecked.revision,
            status: unchecked.status,
            resume_status: unchecked.resume_status,
            blocker: unchecked.blocker,
            metadata: unchecked.metadata,
            original_request: unchecked.original_request,
            summary: unchecked.summary,
            context: unchecked.context,
            scope: unchecked.scope,
            decisions: unchecked.decisions,
            approach: unchecked.approach,
            interfaces: unchecked.interfaces,
            edge_cases: unchecked.edge_cases,
            standards: unchecked.standards,
            git_readiness: unchecked.git_readiness,
            tasks: unchecked.tasks,
            task_order: unchecked.task_order,
            global_verification: unchecked.global_verification,
            approvals: unchecked.approvals,
            review_items: unchecked.review_items,
            follow_ups: unchecked.follow_ups,
            lineage: unchecked.lineage,
            final_outcome: unchecked.final_outcome,
            extensions: unchecked.extensions,
        };
        plan.validate_invariants()?;
        Ok(plan)
    }
}

impl<'de> Deserialize<'de> for Plan {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let unchecked = UncheckedPlan::deserialize(deserializer)?;
        Self::try_from(unchecked).map_err(serde::de::Error::custom)
    }
}

impl Plan {
    /// Creates a new Draft plan at revision one.
    #[must_use]
    pub fn new(id: PlanId, original_request: impl Into<String>, created_at: Timestamp) -> Self {
        Self {
            metadata: PlanMetadata {
                name: id.as_str().to_owned(),
                priority: "N/A".to_owned(),
                plan_type: "N/A".to_owned(),
                area: "N/A".to_owned(),
                owner: "N/A".to_owned(),
                created_at: created_at.clone(),
                updated_at: created_at,
                branch: None,
                markdown_path: None,
            },
            id,
            schema_version: SchemaVersion::current(),
            protocol_version: ProtocolVersion::current(),
            revision: 1,
            status: PlanStatus::Draft,
            resume_status: None,
            blocker: None,
            original_request: original_request.into(),
            summary: String::new(),
            context: Vec::new(),
            scope: PlanScope::default(),
            decisions: Vec::new(),
            approach: Approach::default(),
            interfaces: String::new(),
            edge_cases: Vec::new(),
            standards: Vec::new(),
            git_readiness: GitReadiness {
                repository: "Unknown".to_owned(),
                working_tree: "Unknown".to_owned(),
                branch: None,
                base_commit: None,
                base_status: "Unknown".to_owned(),
                git_flow_enabled: false,
                git_flow_consent: GitFlowConsent::Pending,
                approved_at: None,
            },
            tasks: Vec::new(),
            task_order: Vec::new(),
            global_verification: Vec::new(),
            approvals: Vec::new(),
            review_items: Vec::new(),
            follow_ups: Vec::new(),
            lineage: None,
            final_outcome: FinalOutcome::default(),
            extensions: BTreeMap::new(),
        }
    }

    /// Creates a revision-one Draft with automatic project and standards facts.
    #[must_use]
    pub fn from_draft_seed(seed: PlanDraftSeed, created_at: Timestamp) -> Self {
        let mut plan = Self::new(seed.id, seed.original_request, created_at);
        plan.metadata.name = seed.name;
        plan.metadata.plan_type = seed.trigger;
        plan.metadata.branch = seed.branch;
        plan.metadata.markdown_path = Some(seed.markdown_path);
        plan.git_readiness = seed.git_readiness;
        plan.standards = seed.standards;
        plan.standards
            .sort_by(|left, right| left.package_id.cmp(&right.package_id));
        plan.global_verification = seed.verification_plan;
        plan
    }

    /// Returns the stable plan identifier.
    #[must_use]
    pub const fn id(&self) -> &PlanId {
        &self.id
    }

    /// Returns the current serialized schema version.
    #[must_use]
    pub const fn schema_version(&self) -> SchemaVersion {
        self.schema_version
    }

    /// Returns the locked protocol version and revision.
    #[must_use]
    pub const fn protocol_version(&self) -> &ProtocolVersion {
        &self.protocol_version
    }

    /// Returns the optimistic-concurrency revision.
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// Returns the current plan status.
    #[must_use]
    pub const fn status(&self) -> PlanStatus {
        self.status
    }

    /// Returns human and repository metadata.
    #[must_use]
    pub const fn metadata(&self) -> &PlanMetadata {
        &self.metadata
    }

    /// Returns the exact original request.
    #[must_use]
    pub fn original_request(&self) -> &str {
        &self.original_request
    }

    /// Returns the authored plan summary.
    #[must_use]
    pub fn summary(&self) -> &str {
        &self.summary
    }

    /// Returns current-state references in authored order.
    #[must_use]
    pub fn context(&self) -> &[ContextReference] {
        &self.context
    }

    /// Returns the authored scope.
    #[must_use]
    pub const fn scope(&self) -> &PlanScope {
        &self.scope
    }

    /// Returns decisions, assumptions, and questions in authored order.
    #[must_use]
    pub fn decisions(&self) -> &[Decision] {
        &self.decisions
    }

    /// Returns the implementation approach and complete file map.
    #[must_use]
    pub const fn approach(&self) -> &Approach {
        &self.approach
    }

    /// Returns the authored interfaces and data-flow description.
    #[must_use]
    pub fn interfaces(&self) -> &str {
        &self.interfaces
    }

    /// Returns edge cases in authored order.
    #[must_use]
    pub fn edge_cases(&self) -> &[EdgeCase] {
        &self.edge_cases
    }

    /// Returns exact selected standards packages.
    #[must_use]
    pub fn standards(&self) -> &[StandardSelection] {
        &self.standards
    }

    /// Returns captured Git readiness and consent facts.
    #[must_use]
    pub const fn git_readiness(&self) -> &GitReadiness {
        &self.git_readiness
    }

    /// Returns tasks in stored order.
    #[must_use]
    pub fn tasks(&self) -> &[Task] {
        &self.tasks
    }

    /// Returns the explicit implementation task order.
    #[must_use]
    pub fn task_order(&self) -> &[TaskId] {
        &self.task_order
    }

    /// Returns recorded approvals.
    #[must_use]
    pub fn approvals(&self) -> &[Approval] {
        &self.approvals
    }

    /// Returns classified review records in monotonic identifier order.
    #[must_use]
    pub fn review_items(&self) -> &[ReviewItem] {
        &self.review_items
    }

    /// Returns one review record by its stable identifier.
    #[must_use]
    pub fn review_item(&self, review_id: &str) -> Option<&ReviewItem> {
        self.review_items.iter().find(|item| item.id() == review_id)
    }

    /// Returns deferred follow-up descriptions outside the implementation order.
    #[must_use]
    pub fn follow_ups(&self) -> &[String] {
        &self.follow_ups
    }

    /// Returns whether a material review request owns the plan's blocked state.
    #[must_use]
    pub fn is_blocked_for_material_review(&self) -> bool {
        self.status == PlanStatus::Blocked
            && self.resume_status == Some(PlanStatus::Review)
            && self.review_items.iter().any(|item| {
                item.classification() == ReviewClassification::MaterialChange
                    && item.status() == ReviewStatus::Blocked
            })
    }

    /// Returns whether the current revision has a recorded plan approval.
    #[must_use]
    pub fn has_plan_approval(&self) -> bool {
        self.approvals
            .iter()
            .any(|approval| approval.kind == ApprovalKind::Plan)
    }

    /// Returns the global verification checks in declared order.
    #[must_use]
    pub fn global_verification(&self) -> &[VerificationCheck] {
        &self.global_verification
    }

    /// Returns typed execution checkpoints stored in the extension namespace.
    ///
    /// # Errors
    ///
    /// Returns an invariant error when the execution extension is malformed.
    pub fn execution_state(&self) -> Result<ExecutionState, DomainError> {
        self.extensions
            .get(EXECUTION_EXTENSION_KEY)
            .cloned()
            .map_or_else(
                || Ok(ExecutionState::default()),
                |value| {
                    serde_json::from_value(value).map_err(|error| {
                        DomainError::new(
                            DomainErrorKind::InvariantViolation,
                            format!("Execution extension is malformed: {error}"),
                        )
                    })
                },
            )
    }

    /// Returns a task by identifier.
    #[must_use]
    pub fn task(&self, task_id: &TaskId) -> Option<&Task> {
        self.tasks.iter().find(|task| task.id() == task_id)
    }

    /// Returns a generated JSON Schema for the complete plan aggregate.
    #[must_use]
    pub fn schema() -> schemars::Schema {
        schema_for!(Self)
    }

    /// Applies a strict batch of authored Draft fields in one revision.
    ///
    /// # Errors
    ///
    /// Returns an error when the plan is not Draft, the batch is empty, a
    /// deterministic identifier differs, or any resulting invariant is invalid.
    pub fn apply_draft_input(
        &mut self,
        input: DraftPlanInput,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        if input.is_empty() {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                "Draft apply requires at least one authored field",
            ));
        }
        self.author_draft(updated_at, move |candidate| {
            let DraftPlanInput {
                metadata,
                summary,
                context,
                scope,
                decisions,
                approach,
                interfaces,
                edge_cases,
                tasks,
                verification_plan,
            } = input;
            if let Some(metadata) = metadata {
                candidate.apply_metadata_unversioned(metadata)?;
            }
            if let Some(summary) = summary {
                candidate.summary = summary;
            }
            candidate
                .context
                .extend(context.into_iter().map(context_from_input));
            if let Some(scope) = scope {
                candidate.apply_scope_unversioned(scope)?;
            }
            candidate
                .decisions
                .extend(decisions.into_iter().map(decision_from_input));
            if let Some(approach) = approach {
                candidate.approach.summary = approach;
            }
            if let Some(interfaces) = interfaces {
                candidate.interfaces = interfaces;
            }
            candidate
                .edge_cases
                .extend(edge_cases.into_iter().map(edge_case_from_input));
            for task in tasks {
                candidate.append_task_unversioned(task)?;
            }
            for verification in verification_plan {
                candidate.add_global_verification_unversioned(verification.into_check())?;
            }
            Ok(())
        })
    }

    /// Replaces supplied Draft metadata fields in one revision.
    ///
    /// # Errors
    ///
    /// Returns an error when the plan is not Draft or no metadata field is supplied.
    pub fn author_metadata(
        &mut self,
        metadata: DraftMetadataInput,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        if metadata.is_empty() {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                "Metadata set requires at least one field",
            ));
        }
        self.author_draft(updated_at, move |candidate| {
            candidate.apply_metadata_unversioned(metadata)
        })
    }

    /// Replaces the Draft summary in one revision.
    ///
    /// # Errors
    ///
    /// Returns an error when the plan is not Draft.
    pub fn author_summary(
        &mut self,
        summary: impl Into<String>,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        let summary = summary.into();
        self.author_draft(updated_at, move |candidate| {
            candidate.summary = summary;
            Ok(())
        })
    }

    /// Appends one current-state reference in a new Draft revision.
    ///
    /// # Errors
    ///
    /// Returns an error when the plan is not Draft.
    pub fn author_context(
        &mut self,
        context: DraftContextInput,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        self.author_draft(updated_at, move |candidate| {
            candidate.context.push(context_from_input(context));
            Ok(())
        })
    }

    /// Replaces supplied scope fields in a new Draft revision.
    ///
    /// # Errors
    ///
    /// Returns an error when the plan is not Draft or no scope field is supplied.
    pub fn author_scope(
        &mut self,
        scope: DraftScopeInput,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        if scope.is_empty() {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                "Scope set requires at least one field",
            ));
        }
        self.author_draft(updated_at, move |candidate| {
            candidate.apply_scope_unversioned(scope)
        })
    }

    /// Appends one deliverable in a new Draft revision.
    ///
    /// # Errors
    ///
    /// Returns an error when the plan is not Draft or the value is empty.
    pub fn author_deliverable(
        &mut self,
        value: impl Into<String>,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        let value = non_empty_authored_value(value, "deliverable")?;
        self.author_draft(updated_at, move |candidate| {
            candidate.scope.deliverables.push(value);
            Ok(())
        })
    }

    /// Appends one in-scope boundary in a new Draft revision.
    ///
    /// # Errors
    ///
    /// Returns an error when the plan is not Draft or the value is empty.
    pub fn author_in_scope(
        &mut self,
        value: impl Into<String>,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        let value = non_empty_authored_value(value, "in-scope boundary")?;
        self.author_draft(updated_at, move |candidate| {
            candidate.scope.in_scope.push(value);
            Ok(())
        })
    }

    /// Appends one out-of-scope boundary in a new Draft revision.
    ///
    /// # Errors
    ///
    /// Returns an error when the plan is not Draft or the value is empty.
    pub fn author_out_of_scope(
        &mut self,
        value: impl Into<String>,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        let value = non_empty_authored_value(value, "out-of-scope boundary")?;
        self.author_draft(updated_at, move |candidate| {
            candidate.scope.out_of_scope.push(value);
            Ok(())
        })
    }

    /// Appends one decision, assumption, or question in a new Draft revision.
    ///
    /// # Errors
    ///
    /// Returns an error when the plan is not Draft.
    pub fn author_decision(
        &mut self,
        decision: DraftDecisionInput,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        self.author_draft(updated_at, move |candidate| {
            candidate.decisions.push(decision_from_input(decision));
            Ok(())
        })
    }

    /// Replaces the implementation approach in a new Draft revision.
    ///
    /// # Errors
    ///
    /// Returns an error when the plan is not Draft.
    pub fn author_approach(
        &mut self,
        approach: impl Into<String>,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        let approach = approach.into();
        self.author_draft(updated_at, move |candidate| {
            candidate.approach.summary = approach;
            Ok(())
        })
    }

    /// Replaces interfaces and data flow in a new Draft revision.
    ///
    /// # Errors
    ///
    /// Returns an error when the plan is not Draft.
    pub fn author_interfaces(
        &mut self,
        interfaces: impl Into<String>,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        let interfaces = interfaces.into();
        self.author_draft(updated_at, move |candidate| {
            candidate.interfaces = interfaces;
            Ok(())
        })
    }

    /// Appends one edge case in a new Draft revision.
    ///
    /// # Errors
    ///
    /// Returns an error when the plan is not Draft.
    pub fn author_edge_case(
        &mut self,
        edge_case: DraftEdgeCaseInput,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        self.author_draft(updated_at, move |candidate| {
            candidate.edge_cases.push(edge_case_from_input(edge_case));
            Ok(())
        })
    }

    /// Appends a deterministically identified task in a new Draft revision.
    ///
    /// # Errors
    ///
    /// Returns an error when the plan is not Draft, a supplied ID differs from
    /// the next task ID, or a dependency does not precede the task.
    pub fn author_task(
        &mut self,
        task: DraftTaskInput,
        updated_at: Timestamp,
    ) -> Result<TaskId, DomainError> {
        self.author_draft(updated_at, move |candidate| {
            candidate.append_task_unversioned(task)
        })
    }

    /// Appends one implementation step to a Draft task in a new plan revision.
    ///
    /// # Errors
    ///
    /// Returns an error for a missing task, a non-Draft plan/task, or an empty step.
    pub fn author_task_step(
        &mut self,
        task_id: &TaskId,
        step: impl Into<String>,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        let task_id = task_id.clone();
        let step = step.into();
        self.author_draft(updated_at, move |candidate| {
            candidate.task_mut(&task_id)?.add_step(step)
        })
    }

    /// Appends a deterministically identified acceptance criterion to a Draft task.
    ///
    /// # Errors
    ///
    /// Returns an error for a missing task, non-Draft state, empty description,
    /// or a supplied identifier that differs from the next criterion ID.
    pub fn author_task_criterion(
        &mut self,
        task_id: &TaskId,
        criterion: DraftCriterionInput,
        updated_at: Timestamp,
    ) -> Result<CriterionId, DomainError> {
        let task_id = task_id.clone();
        self.author_draft(updated_at, move |candidate| {
            let task = candidate.task_mut(&task_id)?;
            let number = task
                .acceptance_criteria()
                .len()
                .checked_add(1)
                .ok_or_else(|| {
                    DomainError::new(
                        DomainErrorKind::InvariantViolation,
                        "Acceptance criterion count overflowed",
                    )
                })?;
            let expected = CriterionId::parse(format!("{task_id}-A{number}"))?;
            if criterion.id.as_ref().is_some_and(|id| id != &expected) {
                return Err(DomainError::new(
                    DomainErrorKind::InvariantViolation,
                    format!("Expected criterion ID {expected}"),
                ));
            }
            task.add_acceptance_criterion(super::AcceptanceCriterion::new(
                expected.clone(),
                criterion.description,
            ))?;
            Ok(expected)
        })
    }

    /// Appends one verification command to a Draft task in a new plan revision.
    ///
    /// # Errors
    ///
    /// Returns an error for a missing task, non-Draft state, duplicate ID, or
    /// incomplete command definition.
    pub fn author_task_verification(
        &mut self,
        task_id: &TaskId,
        verification: DraftVerificationInput,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        let task_id = task_id.clone();
        self.author_draft(updated_at, move |candidate| {
            candidate
                .task_mut(&task_id)?
                .add_verification_check(verification.into_check())
        })
    }

    /// Appends one file responsibility to both the task and complete plan file map.
    ///
    /// # Errors
    ///
    /// Returns an error for a missing task, non-Draft state, empty fields, or duplicate path.
    pub fn author_file(
        &mut self,
        task_id: &TaskId,
        file: DraftFileInput,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        let task_id = task_id.clone();
        self.author_draft(updated_at, move |candidate| {
            candidate.add_file_unversioned(&task_id, file)
        })
    }

    /// Appends one global verification command in a new Draft revision.
    ///
    /// # Errors
    ///
    /// Returns an error for a non-Draft plan, duplicate ID, or incomplete command.
    pub fn author_global_verification(
        &mut self,
        verification: DraftVerificationInput,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        self.author_draft(updated_at, move |candidate| {
            candidate.add_global_verification_unversioned(verification.into_check())
        })
    }

    fn author_draft<T, F>(&mut self, updated_at: Timestamp, mutation: F) -> Result<T, DomainError>
    where
        F: FnOnce(&mut Self) -> Result<T, DomainError>,
    {
        self.require_status(PlanStatus::Draft, "edit authored fields")?;
        let next_revision = self.next_revision()?;
        let mut candidate = self.clone();
        let result = mutation(&mut candidate)?;
        candidate.record_revision(next_revision, updated_at);
        candidate.validate_invariants()?;
        *self = candidate;
        Ok(result)
    }

    fn apply_metadata_unversioned(
        &mut self,
        metadata: DraftMetadataInput,
    ) -> Result<(), DomainError> {
        if metadata.is_empty() {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                "Metadata set requires at least one field",
            ));
        }
        if let Some(name) = metadata.name {
            self.metadata.name = name;
        }
        if let Some(priority) = metadata.priority {
            self.metadata.priority = priority;
        }
        if let Some(plan_type) = metadata.plan_type {
            self.metadata.plan_type = plan_type;
        }
        if let Some(area) = metadata.area {
            self.metadata.area = area;
        }
        if let Some(owner) = metadata.owner {
            self.metadata.owner = owner;
        }
        Ok(())
    }

    fn apply_scope_unversioned(&mut self, scope: DraftScopeInput) -> Result<(), DomainError> {
        if scope.is_empty() {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                "Scope set requires at least one field",
            ));
        }
        if let Some(goal) = scope.goal {
            self.scope.goal = goal;
        }
        if let Some(deliverables) = scope.deliverables {
            self.scope.deliverables = deliverables;
        }
        if let Some(in_scope) = scope.in_scope {
            self.scope.in_scope = in_scope;
        }
        if let Some(out_of_scope) = scope.out_of_scope {
            self.scope.out_of_scope = out_of_scope;
        }
        Ok(())
    }

    fn append_task_unversioned(&mut self, task: DraftTaskInput) -> Result<TaskId, DomainError> {
        let number = self.tasks.len().checked_add(1).ok_or_else(|| {
            DomainError::new(DomainErrorKind::InvariantViolation, "Task count overflowed")
        })?;
        let expected_id = TaskId::parse(format!("T{number}"))?;
        let task = Task::from_draft(&expected_id, task)?;
        if self.tasks.iter().any(|current| current.id() == task.id()) {
            return Err(DomainError::new(
                DomainErrorKind::DuplicateTask,
                format!("Task {} already exists", task.id()),
            ));
        }
        self.approach
            .file_map
            .extend(task.file_map().iter().cloned());
        self.task_order.push(expected_id.clone());
        self.tasks.push(task);
        Ok(expected_id)
    }

    fn add_file_unversioned(
        &mut self,
        task_id: &TaskId,
        file: DraftFileInput,
    ) -> Result<(), DomainError> {
        let entry = FileMapEntry::new(file.path, file.change, file.reason, task_id.clone());
        self.task_mut(task_id)?.add_file_map_entry(entry.clone())?;
        self.approach.file_map.push(entry);
        Ok(())
    }

    fn add_global_verification_unversioned(
        &mut self,
        check: VerificationCheck,
    ) -> Result<(), DomainError> {
        if self
            .global_verification
            .iter()
            .any(|current| current.id() == check.id())
        {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                format!("Global check {} already exists", check.id()),
            ));
        }
        if check.command().is_empty()
            || check.command().iter().any(|part| part.trim().is_empty())
            || check.cwd().trim().is_empty()
        {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                format!("Global check {} is incomplete", check.id()),
            ));
        }
        self.global_verification.push(check);
        Ok(())
    }

    /// Adds a Draft task to the end of implementation order.
    ///
    /// # Errors
    ///
    /// Returns an error unless the plan is Draft and all identifiers and dependencies are valid.
    pub fn add_task(&mut self, task: Task, updated_at: Timestamp) -> Result<(), DomainError> {
        self.require_status(PlanStatus::Draft, "add a task")?;
        task.validate_invariants()?;
        if self.tasks.iter().any(|current| current.id() == task.id()) {
            return Err(DomainError::new(
                DomainErrorKind::DuplicateTask,
                format!("Task {} already exists", task.id()),
            ));
        }
        for dependency in task.dependencies() {
            if self.task(dependency).is_none() {
                return Err(DomainError::new(
                    DomainErrorKind::UnmetDependencies,
                    format!("Task {} depends on missing task {dependency}", task.id()),
                ));
            }
        }

        let next_revision = self.next_revision()?;
        self.task_order.push(task.id().clone());
        self.tasks.push(task);
        self.record_revision(next_revision, updated_at);
        Ok(())
    }

    /// Adds a unique global verification check while the plan is Draft.
    ///
    /// # Errors
    ///
    /// Returns an error unless the plan is Draft and the check identifier is unique.
    pub fn add_global_verification(
        &mut self,
        check: VerificationCheck,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        self.require_status(PlanStatus::Draft, "add global verification")?;
        if self
            .global_verification
            .iter()
            .any(|current| current.id() == check.id())
        {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                format!("Global check {} already exists", check.id()),
            ));
        }
        if check.command.is_empty()
            || check.command.iter().any(|part| part.trim().is_empty())
            || check.cwd.trim().is_empty()
        {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                format!("Global check {} is incomplete", check.id()),
            ));
        }
        let next_revision = self.next_revision()?;
        self.global_verification.push(check);
        self.record_revision(next_revision, updated_at);
        Ok(())
    }

    /// Marks a Draft task Ready during plan authoring.
    ///
    /// # Errors
    ///
    /// Returns an error for a missing task, wrong plan state, or illegal task transition.
    pub fn mark_task_ready(
        &mut self,
        task_id: &TaskId,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        self.require_status(PlanStatus::Draft, "mark a task ready")?;
        let next_revision = self.next_revision()?;
        self.task_mut(task_id)?.mark_ready()?;
        self.record_revision(next_revision, updated_at);
        Ok(())
    }

    /// Finalizes a complete Draft and moves it to Ready.
    ///
    /// # Errors
    ///
    /// Returns an error when the plan is not Draft, has no tasks, contains an
    /// incomplete task definition, or violates an invariant.
    pub fn finalize(&mut self, updated_at: Timestamp) -> Result<(), DomainError> {
        self.require_status(PlanStatus::Draft, "finalize")?;
        if self.tasks.is_empty() {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                "A plan must contain at least one task",
            ));
        }
        if self.global_verification.is_empty() {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                "A plan requires at least one global verification check",
            ));
        }
        let next_revision = self.next_revision()?;
        let mut ready = self.clone();
        for task in &mut ready.tasks {
            match task.status() {
                TaskStatus::Draft => task.mark_ready()?,
                TaskStatus::Ready => {}
                _ => {
                    return Err(DomainError::new(
                        DomainErrorKind::InvariantViolation,
                        "A Draft plan may finalize only Draft or Ready tasks",
                    ));
                }
            }
        }
        ready.status = PlanStatus::Ready;
        ready.record_revision(next_revision, updated_at);
        ready.validate_invariants()?;
        *self = ready;
        Ok(())
    }

    /// Records a plan approval without introducing an Approved status.
    ///
    /// # Errors
    ///
    /// Returns an error unless the plan is Ready and the approval kind is Plan.
    pub fn record_approval(&mut self, approval: Approval) -> Result<(), DomainError> {
        self.require_status(PlanStatus::Ready, "record approval")?;
        if approval.kind != ApprovalKind::Plan {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                "record_approval accepts only plan approvals",
            ));
        }
        if approval.actor.trim().is_empty() || approval.reference.trim().is_empty() {
            return Err(DomainError::new(
                DomainErrorKind::ApprovalRequired,
                "Plan approval requires a non-empty actor and reference",
            ));
        }
        if approval.git_flow_consent == GitFlowConsent::Pending {
            return Err(DomainError::new(
                DomainErrorKind::ApprovalRequired,
                "Plan approval requires an explicit Git Flow consent decision",
            ));
        }
        if self.has_plan_approval() {
            return Err(DomainError::new(
                DomainErrorKind::InvalidTransition,
                "The current plan revision already has a plan approval",
            ));
        }
        let next_revision = self.next_revision()?;
        let updated_at = approval.recorded_at.clone();
        self.git_readiness.git_flow_consent = approval.git_flow_consent;
        self.git_readiness.approved_at = Some(updated_at.clone());
        self.approvals.push(approval);
        self.record_revision(next_revision, updated_at);
        Ok(())
    }

    /// Starts the first eligible Ready task.
    ///
    /// # Errors
    ///
    /// Returns an error for missing approval, wrong order, incomplete dependencies,
    /// another active task, or an illegal status transition.
    pub fn start_task(
        &mut self,
        task_id: &TaskId,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        if !matches!(self.status, PlanStatus::Ready | PlanStatus::InProgress) {
            return Err(self.invalid_transition("start a task"));
        }
        if self.status == PlanStatus::Ready
            && !self
                .approvals
                .iter()
                .any(|approval| approval.kind == ApprovalKind::Plan)
        {
            return Err(DomainError::new(
                DomainErrorKind::ApprovalRequired,
                "Plan execution requires a plan approval",
            ));
        }
        if self
            .tasks
            .iter()
            .any(|task| matches!(task.status(), TaskStatus::InProgress | TaskStatus::Blocked))
        {
            return Err(DomainError::new(
                DomainErrorKind::ActiveTaskExists,
                "Another task already owns or blocks the execution slot",
            ));
        }

        let expected = self
            .task_order
            .iter()
            .find(|id| {
                self.task(id)
                    .is_some_and(|task| task.status() != TaskStatus::Done)
            })
            .ok_or_else(|| {
                DomainError::new(
                    DomainErrorKind::TaskOrderViolation,
                    "No incomplete task is available to start",
                )
            })?;
        if expected != task_id {
            return Err(DomainError::new(
                DomainErrorKind::TaskOrderViolation,
                format!("Task {task_id} is not the first eligible task; expected {expected}"),
            ));
        }

        let task_position = self
            .task_order
            .iter()
            .position(|candidate| candidate == task_id)
            .ok_or_else(|| {
                DomainError::new(
                    DomainErrorKind::TaskOrderViolation,
                    format!("Task {task_id} is missing from implementation order"),
                )
            })?;
        let uncommitted_prior = self.task_order[..task_position].iter().find(|prior_id| {
            self.task(prior_id).is_some_and(|prior| {
                prior.commit_gate().is_some_and(|gate| {
                    gate.is_required() && gate.status() != CommitStatus::Committed
                })
            })
        });
        if let Some(prior_id) = uncommitted_prior {
            return Err(DomainError::new(
                DomainErrorKind::TaskOrderViolation,
                format!("Task {prior_id} must be committed before task {task_id} can start"),
            ));
        }

        let task = self.task(task_id).ok_or_else(|| {
            DomainError::new(
                DomainErrorKind::TaskNotFound,
                format!("Task {task_id} does not exist"),
            )
        })?;
        let incomplete_dependencies = task
            .dependencies()
            .iter()
            .filter(|dependency| {
                self.task(dependency)
                    .is_none_or(|dependency_task| dependency_task.status() != TaskStatus::Done)
            })
            .cloned()
            .collect::<Vec<_>>();
        if !incomplete_dependencies.is_empty() {
            return Err(DomainError::new(
                DomainErrorKind::UnmetDependencies,
                format!(
                    "Task {task_id} has incomplete dependencies: {}",
                    incomplete_dependencies
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ));
        }

        let next_revision = self.next_revision()?;
        self.task_mut(task_id)?.start()?;
        self.status = PlanStatus::InProgress;
        self.record_revision(next_revision, updated_at);
        Ok(())
    }

    /// Records one typed checkpoint for the active task.
    ///
    /// # Errors
    ///
    /// Returns an error unless the plan and selected task are In Progress and
    /// the checkpoint fields are complete.
    pub fn record_checkpoint(
        &mut self,
        task_id: &TaskId,
        kind: CheckpointKind,
        summary: impl Into<String>,
        actor: impl Into<String>,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        self.require_status(PlanStatus::InProgress, "record a checkpoint")?;
        if self
            .task(task_id)
            .is_none_or(|task| task.status() != TaskStatus::InProgress)
        {
            return Err(DomainError::new(
                DomainErrorKind::InvalidTransition,
                format!("Task {task_id} is not the active In Progress task"),
            ));
        }
        let next_revision = self.next_revision()?;
        let mut execution = self.execution_state()?;
        execution.record_checkpoint(
            task_id.clone(),
            kind,
            summary.into(),
            actor.into(),
            updated_at.clone(),
        )?;
        let value = serde_json::to_value(execution).map_err(|error| {
            DomainError::new(
                DomainErrorKind::InvariantViolation,
                format!("Failed to encode execution extension: {error}"),
            )
        })?;
        self.extensions
            .insert(EXECUTION_EXTENSION_KEY.to_owned(), value);
        self.record_revision(next_revision, updated_at);
        self.validate_invariants()
    }

    /// Leases one task or global verification check for external execution.
    ///
    /// # Errors
    ///
    /// Returns an error unless the check is uniquely addressable and eligible
    /// at the current ordered execution position.
    pub fn begin_check_run(
        &mut self,
        check_id: &CheckId,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        self.require_status(PlanStatus::InProgress, "run a verification check")?;
        if self.running_check_count() != 0 {
            return Err(DomainError::new(
                DomainErrorKind::InvalidTransition,
                "Another verification check is already Running",
            ));
        }
        let next_revision = self.next_revision()?;
        if let Some(task_index) = self.tasks.iter().position(|task| {
            task.verification_checks()
                .iter()
                .any(|check| check.id() == check_id)
        }) {
            self.tasks[task_index].begin_check_run(check_id)?;
        } else {
            if self
                .tasks
                .iter()
                .any(|task| task.status() != TaskStatus::Done)
            {
                return Err(DomainError::new(
                    DomainErrorKind::TaskOrderViolation,
                    "Global verification may run only after every task is Done",
                ));
            }
            let check = self
                .global_verification
                .iter_mut()
                .find(|check| check.id() == check_id)
                .ok_or_else(|| {
                    DomainError::new(
                        DomainErrorKind::InvariantViolation,
                        format!("Check {check_id} does not exist"),
                    )
                })?;
            check.begin_run()?;
        }
        self.record_revision(next_revision, updated_at);
        self.validate_invariants()
    }

    /// Attaches immutable command evidence to a Running verification check.
    ///
    /// # Errors
    ///
    /// Returns an error unless the addressed check is Running at the current
    /// ordered execution position.
    pub fn record_check_run(
        &mut self,
        check_id: &CheckId,
        evidence_id: EvidenceId,
        passed: bool,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        self.require_status(PlanStatus::InProgress, "record a verification check run")?;
        let next_revision = self.next_revision()?;
        if let Some(task_index) = self.tasks.iter().position(|task| {
            task.verification_checks()
                .iter()
                .any(|check| check.id() == check_id)
        }) {
            self.tasks[task_index].record_check_run(check_id, evidence_id, passed)?;
        } else {
            let check = self
                .global_verification
                .iter_mut()
                .find(|check| check.id() == check_id)
                .ok_or_else(|| {
                    DomainError::new(
                        DomainErrorKind::InvariantViolation,
                        format!("Check {check_id} does not exist"),
                    )
                })?;
            check.record_run(evidence_id, passed)?;
        }
        self.record_revision(next_revision, updated_at);
        self.validate_invariants()
    }

    /// Completes the active task after its local evidence gates are satisfied.
    ///
    /// # Errors
    ///
    /// Returns an error unless the requested task is the active In Progress task.
    pub fn complete_task(
        &mut self,
        task_id: &TaskId,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        self.require_status(PlanStatus::InProgress, "complete a task")?;
        if self
            .task(task_id)
            .is_none_or(|task| task.status() != TaskStatus::InProgress)
        {
            return Err(DomainError::new(
                DomainErrorKind::InvalidTransition,
                format!("Task {task_id} is not the active In Progress task"),
            ));
        }
        let next_revision = self.next_revision()?;
        self.task_mut(task_id)?.complete()?;
        self.record_revision(next_revision, updated_at);
        Ok(())
    }

    /// Blocks the plan after a recoverable task-commit failure.
    ///
    /// # Errors
    ///
    /// Returns an error unless the plan is In Progress, the task is Done with
    /// a pending required commit gate, and the reason is non-empty.
    pub fn block_task_commit(
        &mut self,
        task_id: &TaskId,
        reason: impl Into<String>,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        self.require_status(PlanStatus::InProgress, "block a task commit")?;
        let reason = reason.into();
        if reason.trim().is_empty() {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                "A blocked task commit requires a reason",
            ));
        }
        let next_revision = self.next_revision()?;
        self.task_mut(task_id)?.block_commit()?;
        self.resume_status = Some(PlanStatus::InProgress);
        self.status = PlanStatus::Blocked;
        self.blocker = Some(reason);
        self.record_revision(next_revision, updated_at);
        self.validate_invariants()
    }

    /// Records one verified task commit and its immutable evidence.
    ///
    /// # Errors
    ///
    /// Returns an error unless the plan is In Progress and the task is Done
    /// with a pending or recoverably blocked required commit gate.
    pub fn record_task_commit(
        &mut self,
        task_id: &TaskId,
        commit: &str,
        files: Vec<String>,
        evidence_id: EvidenceId,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        self.require_status(PlanStatus::InProgress, "record a task commit")?;
        let next_revision = self.next_revision()?;
        self.task_mut(task_id)?
            .record_commit(commit, files, evidence_id)?;
        self.record_revision(next_revision, updated_at);
        self.validate_invariants()
    }

    /// Records passing evidence for a criterion on the active task.
    ///
    /// # Errors
    ///
    /// Returns an error unless the plan and task are In Progress and the criterion exists.
    pub fn record_task_criterion_pass(
        &mut self,
        task_id: &TaskId,
        criterion_id: &CriterionId,
        evidence_id: EvidenceId,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        self.require_status(PlanStatus::InProgress, "record criterion evidence")?;
        let next_revision = self.next_revision()?;
        self.task_mut(task_id)?
            .record_criterion_pass(criterion_id, evidence_id)?;
        self.record_revision(next_revision, updated_at);
        Ok(())
    }

    /// Binds compatible evidence to a criterion on the active task.
    ///
    /// # Errors
    ///
    /// Returns an error unless the plan and task are In Progress and the
    /// criterion exists.
    pub fn record_task_criterion_evidence(
        &mut self,
        task_id: &TaskId,
        criterion_id: &CriterionId,
        evidence_id: EvidenceId,
        is_accepted_exception: bool,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        self.require_status(PlanStatus::InProgress, "record criterion evidence")?;
        let next_revision = self.next_revision()?;
        self.task_mut(task_id)?.record_criterion_evidence(
            criterion_id,
            evidence_id,
            is_accepted_exception,
        )?;
        self.record_revision(next_revision, updated_at);
        Ok(())
    }

    /// Records passing evidence for a verification check on the active task.
    ///
    /// # Errors
    ///
    /// Returns an error unless the plan and task are In Progress and the check exists.
    pub fn record_task_check_pass(
        &mut self,
        task_id: &TaskId,
        check_id: &CheckId,
        evidence_id: EvidenceId,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        self.require_status(PlanStatus::InProgress, "record task check evidence")?;
        let next_revision = self.next_revision()?;
        self.task_mut(task_id)?
            .record_check_pass(check_id, evidence_id)?;
        self.record_revision(next_revision, updated_at);
        Ok(())
    }

    /// Records passing evidence for a global verification check.
    ///
    /// # Errors
    ///
    /// Returns an error unless the plan is In Progress and the check exists.
    pub fn record_global_check_pass(
        &mut self,
        check_id: &CheckId,
        evidence_id: EvidenceId,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        self.require_status(PlanStatus::InProgress, "record global check evidence")?;
        let next_revision = self.next_revision()?;
        let check = self
            .global_verification
            .iter_mut()
            .find(|check| check.id() == check_id)
            .ok_or_else(|| {
                DomainError::new(
                    DomainErrorKind::InvariantViolation,
                    format!("Global check {check_id} does not exist"),
                )
            })?;
        check.record_pass(evidence_id)?;
        self.record_revision(next_revision, updated_at);
        Ok(())
    }

    /// Blocks a Ready or In Progress plan and captures its resumable state.
    ///
    /// # Errors
    ///
    /// Returns an error when the plan cannot be blocked or the reason is empty.
    pub fn block(
        &mut self,
        reason: impl Into<String>,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        if !matches!(self.status, PlanStatus::Ready | PlanStatus::InProgress) {
            return Err(self.invalid_transition("block"));
        }
        if self.running_check_count() != 0 {
            return Err(DomainError::new(
                DomainErrorKind::InvalidTransition,
                "A plan cannot be blocked while a verification check is Running",
            ));
        }
        let reason = reason.into();
        if reason.trim().is_empty() {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                "A blocked plan requires a reason",
            ));
        }
        let next_revision = self.next_revision()?;
        if self.status == PlanStatus::InProgress
            && let Some(task) = self
                .tasks
                .iter_mut()
                .find(|task| task.status() == TaskStatus::InProgress)
        {
            task.block(reason.clone())?;
        }
        self.resume_status = Some(self.status);
        self.status = PlanStatus::Blocked;
        self.blocker = Some(reason);
        self.record_revision(next_revision, updated_at);
        Ok(())
    }

    /// Resumes a Blocked plan to its recorded prior state.
    ///
    /// # Errors
    ///
    /// Returns an error when the plan is not Blocked or lacks a legal resume state.
    pub fn resume(&mut self, updated_at: Timestamp) -> Result<(), DomainError> {
        self.require_status(PlanStatus::Blocked, "resume")?;
        let resume_status = self.resume_status.ok_or_else(|| {
            DomainError::new(
                DomainErrorKind::InvariantViolation,
                "Blocked plan has no resume status",
            )
        })?;
        if !matches!(resume_status, PlanStatus::Ready | PlanStatus::InProgress) {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                "Blocked plan has an invalid resume status",
            ));
        }
        let next_revision = self.next_revision()?;
        if resume_status == PlanStatus::InProgress
            && let Some(task) = self
                .tasks
                .iter_mut()
                .find(|task| task.status() == TaskStatus::Blocked)
        {
            task.resume()?;
        }
        self.status = resume_status;
        self.resume_status = None;
        self.blocker = None;
        self.record_revision(next_revision, updated_at);
        Ok(())
    }

    /// Moves an executed plan to Review when every task is Done and committed.
    ///
    /// # Errors
    ///
    /// Returns an error unless the plan is In Progress, all tasks are Done,
    /// every required commit gate is Committed, and global verification passes.
    pub fn finish_execution(&mut self, updated_at: Timestamp) -> Result<(), DomainError> {
        self.require_status(PlanStatus::InProgress, "finish execution")?;
        if self
            .tasks
            .iter()
            .any(|task| task.status() != TaskStatus::Done)
        {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                "Every task must be Done before plan review",
            ));
        }
        if let Some(task) = self.tasks.iter().find(|task| {
            task.commit_gate()
                .is_some_and(|gate| gate.is_required() && gate.status() != CommitStatus::Committed)
        }) {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                format!(
                    "Task {} required commit gate must be Committed before plan review",
                    task.id()
                ),
            ));
        }
        if !self.global_verification_is_satisfied() {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                "Every required global verification check must pass with evidence before review",
            ));
        }
        let next_revision = self.next_revision()?;
        self.status = PlanStatus::Review;
        self.record_revision(next_revision, updated_at);
        Ok(())
    }

    /// Returns the next monotonic review-item identifier without reserving it.
    ///
    /// # Errors
    ///
    /// Returns an invariant error when the identifier counter overflows.
    pub fn next_review_item_id(&self) -> Result<String, DomainError> {
        let number = self.review_items.len().checked_add(1).ok_or_else(|| {
            DomainError::new(
                DomainErrorKind::InvariantViolation,
                "Review item count overflowed",
            )
        })?;
        Ok(format!("REV-{number}"))
    }

    /// Records one classified review request and its protocol-selected action.
    ///
    /// # Errors
    ///
    /// Returns an error unless the plan is in Review and the classification has
    /// the required completed task target, or no target for a non-task request.
    pub fn record_review(
        &mut self,
        reviewer: String,
        feedback: String,
        classification: ReviewClassification,
        task_id: Option<TaskId>,
        updated_at: Timestamp,
    ) -> Result<String, DomainError> {
        self.require_status(PlanStatus::Review, "record review feedback")?;
        let review_id = self.next_review_item_id()?;
        let feedback_for_state = feedback.clone();
        let item = match classification {
            ReviewClassification::AcceptanceDefect => {
                let task_id = self.completed_review_target(task_id, classification)?;
                ReviewItem::acceptance_defect(
                    review_id.clone(),
                    reviewer,
                    feedback,
                    task_id,
                    updated_at.clone(),
                )?
            }
            ReviewClassification::InScopeRework => {
                let origin_task = self.completed_review_target(task_id, classification)?;
                let reserved_task = self.next_rework_task_id()?;
                ReviewItem::in_scope_rework(
                    review_id.clone(),
                    reviewer,
                    feedback,
                    origin_task,
                    reserved_task,
                    updated_at.clone(),
                )?
            }
            ReviewClassification::MaterialChange => {
                Self::reject_unexpected_review_target(task_id.as_ref(), classification)?;
                ReviewItem::material_change(
                    review_id.clone(),
                    reviewer,
                    feedback.clone(),
                    updated_at.clone(),
                )?
            }
            ReviewClassification::FollowUp => {
                Self::reject_unexpected_review_target(task_id.as_ref(), classification)?;
                ReviewItem::follow_up(
                    review_id.clone(),
                    reviewer,
                    feedback.clone(),
                    updated_at.clone(),
                )?
            }
            ReviewClassification::Accepted => {
                return Err(DomainError::new(
                    DomainErrorKind::InvalidTransition,
                    "Use final review acceptance instead of recording Accepted feedback",
                ));
            }
        };
        let next_revision = self.next_revision()?;
        if classification == ReviewClassification::FollowUp {
            self.follow_ups.push(feedback_for_state.clone());
        }
        self.review_items.push(item);
        if classification == ReviewClassification::MaterialChange {
            self.resume_status = Some(PlanStatus::Review);
            self.status = PlanStatus::Blocked;
            self.blocker = Some(feedback_for_state);
        }
        self.record_revision(next_revision, updated_at);
        self.validate_invariants()?;
        Ok(review_id)
    }

    /// Starts a recorded acceptance rerun or materializes a reserved rework task.
    ///
    /// # Errors
    ///
    /// Returns an error unless the plan is in Review, the review item is open,
    /// and its optional task definition is complete and classification-compatible.
    pub fn begin_review_rework(
        &mut self,
        review_id: &str,
        task_input: Option<DraftTaskInput>,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        self.require_status(PlanStatus::Review, "begin review rework")?;
        let item_index = self.review_item_index(review_id)?;
        if self.review_items[item_index].status() != ReviewStatus::Open {
            return Err(DomainError::new(
                DomainErrorKind::InvalidTransition,
                format!("Review item {review_id} is not Open"),
            ));
        }
        let classification = self.review_items[item_index].classification();
        let linked_task = self.review_items[item_index].linked_task().cloned();
        let origin_task = self.review_items[item_index].origin_task().cloned();
        let next_revision = self.next_revision()?;
        match classification {
            ReviewClassification::AcceptanceDefect => {
                if task_input.is_some() {
                    return Err(DomainError::new(
                        DomainErrorKind::InvariantViolation,
                        "Acceptance-defect rework cannot add or replace a task definition",
                    ));
                }
                let task_id = linked_task.ok_or_else(|| {
                    DomainError::new(
                        DomainErrorKind::InvariantViolation,
                        format!("Review item {review_id} has no task target"),
                    )
                })?;
                self.task_mut(&task_id)?.reopen_for_rework()?;
            }
            ReviewClassification::InScopeRework => {
                let task_id = linked_task.ok_or_else(|| {
                    DomainError::new(
                        DomainErrorKind::InvariantViolation,
                        format!("Review item {review_id} has no reserved task"),
                    )
                })?;
                let origin_task = origin_task.ok_or_else(|| {
                    DomainError::new(
                        DomainErrorKind::InvariantViolation,
                        format!("Review item {review_id} has no origin task"),
                    )
                })?;
                let input = task_input.ok_or_else(|| {
                    DomainError::new(
                        DomainErrorKind::InvariantViolation,
                        "In-scope rework requires one complete task definition",
                    )
                })?;
                self.append_review_task(task_id, &origin_task, input)?;
            }
            ReviewClassification::MaterialChange
            | ReviewClassification::FollowUp
            | ReviewClassification::Accepted => {
                return Err(DomainError::new(
                    DomainErrorKind::InvalidTransition,
                    format!("Review item {review_id} does not permit rework"),
                ));
            }
        }
        self.review_items[item_index].begin_rework()?;
        for check in &mut self.global_verification {
            check.reset_for_rework();
        }
        self.status = PlanStatus::InProgress;
        self.record_revision(next_revision, updated_at);
        self.validate_invariants()
    }

    /// Resolves a completed review rework item after execution returns to Review.
    ///
    /// # Errors
    ///
    /// Returns an error unless the linked task and its required commit are complete.
    pub fn resolve_review(
        &mut self,
        review_id: &str,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        self.require_status(PlanStatus::Review, "resolve review rework")?;
        let item_index = self.review_item_index(review_id)?;
        let task_id = self.review_items[item_index]
            .linked_task()
            .cloned()
            .ok_or_else(|| {
                DomainError::new(
                    DomainErrorKind::InvalidTransition,
                    format!("Review item {review_id} has no rework task"),
                )
            })?;
        let task = self.task(&task_id).ok_or_else(|| {
            DomainError::new(
                DomainErrorKind::TaskNotFound,
                format!("Review task {task_id} does not exist"),
            )
        })?;
        if task.status() != TaskStatus::Done
            || task
                .commit_gate()
                .is_some_and(|gate| gate.is_required() && gate.status() != CommitStatus::Committed)
        {
            return Err(DomainError::new(
                DomainErrorKind::InvalidTransition,
                format!("Review task {task_id} is not complete and committed"),
            ));
        }
        let next_revision = self.next_revision()?;
        self.review_items[item_index].resolve()?;
        self.record_revision(next_revision, updated_at);
        self.validate_invariants()
    }

    /// Records explicit final acceptance and moves a fully resolved Review to Done.
    ///
    /// # Errors
    ///
    /// Returns an error unless every feedback, task, commit, and global check gate
    /// is resolved and the reviewer plus approval reference are non-empty.
    pub fn accept_review(
        &mut self,
        reviewer: String,
        approval_reference: String,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        self.require_status(PlanStatus::Review, "accept review")?;
        if self.review_items.iter().any(|item| {
            matches!(
                item.status(),
                ReviewStatus::Open | ReviewStatus::InProgress | ReviewStatus::Blocked
            )
        }) {
            return Err(DomainError::new(
                DomainErrorKind::InvalidTransition,
                "Every blocking review item must be resolved before acceptance",
            ));
        }
        if self.tasks.iter().any(|task| {
            task.status() != TaskStatus::Done
                || task.commit_gate().is_some_and(|gate| {
                    gate.is_required() && gate.status() != CommitStatus::Committed
                })
        }) || !self.global_verification_is_satisfied()
        {
            return Err(DomainError::new(
                DomainErrorKind::InvalidTransition,
                "Review acceptance requires complete tasks, commits, and global checks",
            ));
        }
        let review_id = self.next_review_item_id()?;
        let acceptance =
            ReviewItem::accepted(review_id, reviewer, approval_reference, updated_at.clone())?;
        let next_revision = self.next_revision()?;
        self.review_items.push(acceptance);
        self.status = PlanStatus::Done;
        self.record_revision(next_revision, updated_at);
        self.validate_invariants()
    }

    /// Validates structural and lifecycle invariants after deserialization or mutation.
    ///
    /// # Errors
    ///
    /// Returns the first deterministic invariant violation.
    pub fn validate_invariants(&self) -> Result<(), DomainError> {
        self.validate_required_fields()?;
        self.validate_task_graph()?;
        self.validate_lifecycle()?;
        self.validate_approval_state()?;
        self.validate_global_verification()?;
        self.validate_review_state()?;
        self.validate_execution_state()?;
        Ok(())
    }

    fn validate_execution_state(&self) -> Result<(), DomainError> {
        let task_ids = self.tasks.iter().map(Task::id).collect::<BTreeSet<_>>();
        self.execution_state()?.validate(&task_ids)?;
        let running_count = self.running_check_count();
        if running_count > 1 {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                "At most one verification check may be Running",
            ));
        }
        if self
            .global_verification
            .iter()
            .any(|check| check.status() == CheckStatus::Running)
            && (self.status != PlanStatus::InProgress
                || self
                    .tasks
                    .iter()
                    .any(|task| task.status() != TaskStatus::Done))
        {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                "A global Running check requires an In Progress plan with every task Done",
            ));
        }
        Ok(())
    }

    fn running_check_count(&self) -> usize {
        self.tasks
            .iter()
            .flat_map(Task::verification_checks)
            .chain(self.global_verification.iter())
            .filter(|check| check.status() == CheckStatus::Running)
            .count()
    }

    fn validate_required_fields(&self) -> Result<(), DomainError> {
        if self.original_request.trim().is_empty() {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                "A plan requires the original request",
            ));
        }
        if self.revision == 0 {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                "Plan revision must be positive",
            ));
        }
        if self.status != PlanStatus::Draft && self.tasks.is_empty() {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                "A non-Draft plan requires at least one task",
            ));
        }
        Ok(())
    }

    fn validate_task_graph(&self) -> Result<(), DomainError> {
        let task_ids = self.tasks.iter().map(Task::id).collect::<BTreeSet<_>>();
        if task_ids.len() != self.tasks.len() {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                "Plan contains duplicate task identifiers",
            ));
        }
        let ordered_ids = self.task_order.iter().collect::<BTreeSet<_>>();
        if ordered_ids.len() != self.task_order.len()
            || ordered_ids != task_ids
            || self.task_order.len() != self.tasks.len()
        {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                "Task order must contain every task exactly once",
            ));
        }

        let positions = self
            .task_order
            .iter()
            .enumerate()
            .map(|(index, id)| (id, index))
            .collect::<BTreeMap<_, _>>();
        for task in &self.tasks {
            task.validate_invariants()?;
            if self.status != PlanStatus::Draft {
                let task_position = positions[task.id()];
                for dependency in task.dependencies() {
                    let dependency_position = positions.get(dependency).ok_or_else(|| {
                        DomainError::new(
                            DomainErrorKind::InvariantViolation,
                            format!("Task {} depends on missing task {dependency}", task.id()),
                        )
                    })?;
                    if dependency_position >= &task_position {
                        return Err(DomainError::new(
                            DomainErrorKind::InvariantViolation,
                            format!("Task {} dependency {dependency} must precede it", task.id()),
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    fn validate_lifecycle(&self) -> Result<(), DomainError> {
        let active_count = self
            .tasks
            .iter()
            .filter(|task| task.status() == TaskStatus::InProgress)
            .count();
        let blocked_count = self
            .tasks
            .iter()
            .filter(|task| task.status() == TaskStatus::Blocked)
            .count();
        if active_count > 1 || blocked_count > 1 || active_count + blocked_count > 1 {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                "At most one task may own or block the execution slot",
            ));
        }
        self.validate_task_statuses(blocked_count)?;
        self.validate_resume_state(blocked_count)
    }

    fn validate_task_statuses(&self, blocked_count: usize) -> Result<(), DomainError> {
        match self.status {
            PlanStatus::Draft
                if self.tasks.iter().any(|task| {
                    matches!(
                        task.status(),
                        TaskStatus::InProgress | TaskStatus::Blocked | TaskStatus::Done
                    )
                }) =>
            {
                return Err(DomainError::new(
                    DomainErrorKind::InvariantViolation,
                    "Draft plans may contain only Draft or Ready tasks",
                ));
            }
            PlanStatus::Ready
                if self
                    .tasks
                    .iter()
                    .any(|task| task.status() != TaskStatus::Ready) =>
            {
                return Err(DomainError::new(
                    DomainErrorKind::InvariantViolation,
                    "Ready plans require every task to be Ready",
                ));
            }
            PlanStatus::InProgress
                if blocked_count != 0
                    || self
                        .tasks
                        .iter()
                        .any(|task| task.status() == TaskStatus::Draft) =>
            {
                return Err(DomainError::new(
                    DomainErrorKind::InvariantViolation,
                    "In Progress plans cannot contain Draft or Blocked tasks",
                ));
            }
            PlanStatus::Review | PlanStatus::Done
                if self
                    .tasks
                    .iter()
                    .any(|task| task.status() != TaskStatus::Done) =>
            {
                return Err(DomainError::new(
                    DomainErrorKind::InvariantViolation,
                    "Review and Done plans require every task to be Done",
                ));
            }
            _ => {}
        }
        Ok(())
    }

    fn validate_resume_state(&self, blocked_count: usize) -> Result<(), DomainError> {
        if self.status == PlanStatus::Blocked && self.resume_status.is_none() {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                "Blocked plan requires a resume status",
            ));
        }
        if self.status != PlanStatus::Blocked && self.resume_status.is_some() {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                "Only a Blocked plan may retain a resume status",
            ));
        }
        if self.status == PlanStatus::Blocked {
            match self.resume_status {
                Some(PlanStatus::Ready)
                    if self
                        .tasks
                        .iter()
                        .any(|task| task.status() != TaskStatus::Ready) =>
                {
                    return Err(DomainError::new(
                        DomainErrorKind::InvariantViolation,
                        "A plan blocked from Ready requires every task to remain Ready",
                    ));
                }
                Some(PlanStatus::InProgress)
                    if (blocked_count == 0
                        && !self.tasks.iter().any(|task| {
                            task.status() == TaskStatus::Done
                                && task.commit_gate().is_some_and(|gate| {
                                    gate.is_required() && gate.status() == CommitStatus::Blocked
                                })
                        }))
                        || blocked_count > 1
                        || self
                            .tasks
                            .iter()
                            .any(|task| task.status() == TaskStatus::Draft) =>
                {
                    return Err(DomainError::new(
                        DomainErrorKind::InvariantViolation,
                        "A plan blocked from In Progress requires one blocked execution task",
                    ));
                }
                Some(PlanStatus::Review)
                    if self
                        .tasks
                        .iter()
                        .any(|task| task.status() != TaskStatus::Done)
                        || !self.is_blocked_for_material_review() =>
                {
                    return Err(DomainError::new(
                        DomainErrorKind::InvariantViolation,
                        "A plan blocked from Review requires completed tasks and material feedback",
                    ));
                }
                Some(PlanStatus::Draft | PlanStatus::Blocked | PlanStatus::Done) => {
                    return Err(DomainError::new(
                        DomainErrorKind::InvariantViolation,
                        "Blocked plan has an invalid resume status",
                    ));
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn validate_approval_state(&self) -> Result<(), DomainError> {
        let plan_approvals = self
            .approvals
            .iter()
            .filter(|approval| approval.kind == ApprovalKind::Plan)
            .collect::<Vec<_>>();
        if plan_approvals.len() > 1 {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                "A plan revision may contain at most one plan approval",
            ));
        }
        if let Some(approval) = plan_approvals.first() {
            if approval.git_flow_consent == GitFlowConsent::Pending
                || self.git_readiness.git_flow_consent != approval.git_flow_consent
                || self.git_readiness.approved_at.as_ref() != Some(&approval.recorded_at)
            {
                return Err(DomainError::new(
                    DomainErrorKind::InvariantViolation,
                    "Plan approval and Git Flow consent facts must identify the same declaration",
                ));
            }
        } else if self.git_readiness.git_flow_consent != GitFlowConsent::Pending
            || self.git_readiness.approved_at.is_some()
        {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                "Git Flow consent cannot exist without a plan approval",
            ));
        }
        if matches!(
            self.status,
            PlanStatus::InProgress | PlanStatus::Review | PlanStatus::Done
        ) && !self
            .approvals
            .iter()
            .any(|approval| approval.kind == ApprovalKind::Plan)
        {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                "Executing and completed plans require a plan approval",
            ));
        }
        if self.status == PlanStatus::Blocked
            && matches!(
                self.resume_status,
                Some(PlanStatus::InProgress | PlanStatus::Review)
            )
            && !self
                .approvals
                .iter()
                .any(|approval| approval.kind == ApprovalKind::Plan)
        {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                "A plan blocked from execution or Review requires a plan approval",
            ));
        }
        Ok(())
    }

    fn validate_global_verification(&self) -> Result<(), DomainError> {
        let global_check_ids = self
            .global_verification
            .iter()
            .map(VerificationCheck::id)
            .collect::<BTreeSet<_>>();
        if global_check_ids.len() != self.global_verification.len() {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                "Global verification contains duplicate check identifiers",
            ));
        }
        if self.status != PlanStatus::Draft && self.global_verification.is_empty() {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                "A non-Draft plan requires global verification",
            ));
        }
        if self.global_verification.iter().any(|check| {
            check.command.is_empty()
                || check.command.iter().any(|part| part.trim().is_empty())
                || check.cwd.trim().is_empty()
        }) {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                "Global verification contains an incomplete check",
            ));
        }
        if self
            .global_verification
            .iter()
            .filter(|check| check.status() == CheckStatus::Running)
            .count()
            > 1
        {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                "At most one global verification check may be Running",
            ));
        }
        if matches!(self.status, PlanStatus::Review | PlanStatus::Done)
            && !self.global_verification_is_satisfied()
        {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                "Review and Done plans require passing global verification evidence",
            ));
        }
        Ok(())
    }

    fn completed_review_target(
        &self,
        task_id: Option<TaskId>,
        classification: ReviewClassification,
    ) -> Result<TaskId, DomainError> {
        let task_id = task_id.ok_or_else(|| {
            DomainError::new(
                DomainErrorKind::InvariantViolation,
                format!("{classification:?} review feedback requires a task target"),
            )
        })?;
        let task = self.task(&task_id).ok_or_else(|| {
            DomainError::new(
                DomainErrorKind::TaskNotFound,
                format!("Task {task_id} does not exist"),
            )
        })?;
        if task.status() != TaskStatus::Done {
            return Err(DomainError::new(
                DomainErrorKind::InvalidTransition,
                format!("Review target {task_id} must be Done"),
            ));
        }
        Ok(task_id)
    }

    fn reject_unexpected_review_target(
        task_id: Option<&TaskId>,
        classification: ReviewClassification,
    ) -> Result<(), DomainError> {
        if task_id.is_some() {
            Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                format!("{classification:?} review feedback cannot target a task"),
            ))
        } else {
            Ok(())
        }
    }

    fn next_rework_task_id(&self) -> Result<TaskId, DomainError> {
        let highest_task = self
            .tasks
            .iter()
            .filter_map(|task| rework_task_number(task.id()))
            .chain(
                self.review_items
                    .iter()
                    .filter_map(|item| item.linked_task().and_then(rework_task_number)),
            )
            .max()
            .unwrap_or(0);
        let number = highest_task.checked_add(1).ok_or_else(|| {
            DomainError::new(
                DomainErrorKind::InvariantViolation,
                "Review rework task counter overflowed",
            )
        })?;
        TaskId::parse(format!("R{number}"))
    }

    fn review_item_index(&self, review_id: &str) -> Result<usize, DomainError> {
        self.review_items
            .iter()
            .position(|item| item.id() == review_id)
            .ok_or_else(|| {
                DomainError::new(
                    DomainErrorKind::InvariantViolation,
                    format!("Review item {review_id} does not exist"),
                )
            })
    }

    fn append_review_task(
        &mut self,
        task_id: TaskId,
        origin_task: &TaskId,
        input: DraftTaskInput,
    ) -> Result<(), DomainError> {
        if self.task(&task_id).is_some() {
            return Err(DomainError::new(
                DomainErrorKind::DuplicateTask,
                format!("Review task {task_id} already exists"),
            ));
        }
        let mut task = Task::from_draft(&task_id, input)?;
        if !task.dependencies().contains(origin_task) {
            return Err(DomainError::new(
                DomainErrorKind::UnmetDependencies,
                format!("Review task {task_id} must depend on origin task {origin_task}"),
            ));
        }
        for dependency in task.dependencies() {
            let dependency_task = self.task(dependency).ok_or_else(|| {
                DomainError::new(
                    DomainErrorKind::TaskNotFound,
                    format!("Review task {task_id} depends on missing task {dependency}"),
                )
            })?;
            if dependency_task.status() != TaskStatus::Done {
                return Err(DomainError::new(
                    DomainErrorKind::UnmetDependencies,
                    format!("Review task dependency {dependency} is not Done"),
                ));
            }
        }
        if self.git_readiness.git_flow_enabled
            && task.commit_gate().is_none_or(|gate| !gate.is_required())
        {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                format!("Git Flow review task {task_id} requires a commit gate"),
            ));
        }
        if task.verification_checks().iter().any(|check| {
            self.global_verification
                .iter()
                .any(|existing| existing.id() == check.id())
                || self.tasks.iter().any(|existing_task| {
                    existing_task
                        .verification_checks()
                        .iter()
                        .any(|existing| existing.id() == check.id())
                })
        }) {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                format!("Review task {task_id} contains a duplicate check identifier"),
            ));
        }
        task.mark_ready()?;
        self.approach
            .file_map
            .extend(task.file_map().iter().cloned());
        self.task_order.push(task_id);
        self.tasks.push(task);
        Ok(())
    }

    fn validate_review_state(&self) -> Result<(), DomainError> {
        let mut reserved_tasks = BTreeSet::new();
        for (index, item) in self.review_items.iter().enumerate() {
            item.validate()?;
            let expected_number = index.checked_add(1).ok_or_else(|| {
                DomainError::new(
                    DomainErrorKind::InvariantViolation,
                    "Review item count overflowed",
                )
            })?;
            if item.id() != format!("REV-{expected_number}") {
                return Err(DomainError::new(
                    DomainErrorKind::InvariantViolation,
                    "Review item identifiers must be monotonic and contiguous",
                ));
            }
            self.validate_review_relationship(item, &mut reserved_tasks)?;
        }
        if self.review_items.iter().any(|item| {
            item.classification() == ReviewClassification::MaterialChange
                && item.status() == ReviewStatus::Blocked
        }) && !self.is_blocked_for_material_review()
        {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                "Blocked material review feedback must own the plan Blocked state",
            ));
        }
        if self.status == PlanStatus::Done
            && self.review_items.iter().any(|item| {
                matches!(
                    item.status(),
                    ReviewStatus::Open | ReviewStatus::InProgress | ReviewStatus::Blocked
                )
            })
        {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                "Done plans cannot retain unresolved review feedback",
            ));
        }
        Ok(())
    }

    fn validate_review_relationship(
        &self,
        item: &ReviewItem,
        reserved_tasks: &mut BTreeSet<TaskId>,
    ) -> Result<(), DomainError> {
        match item.classification() {
            ReviewClassification::AcceptanceDefect => {
                let task_id = item.linked_task().ok_or_else(|| {
                    DomainError::new(
                        DomainErrorKind::InvariantViolation,
                        format!("Review item {} has no task", item.id()),
                    )
                })?;
                let task = self.task(task_id).ok_or_else(|| {
                    DomainError::new(
                        DomainErrorKind::TaskNotFound,
                        format!("Review item {} targets missing task {task_id}", item.id()),
                    )
                })?;
                if matches!(item.status(), ReviewStatus::Open | ReviewStatus::Resolved)
                    && task.status() != TaskStatus::Done
                {
                    return Err(DomainError::new(
                        DomainErrorKind::InvariantViolation,
                        format!(
                            "Review item {} requires task {task_id} to be Done",
                            item.id()
                        ),
                    ));
                }
            }
            ReviewClassification::InScopeRework => {
                let task_id = item.linked_task().ok_or_else(|| {
                    DomainError::new(
                        DomainErrorKind::InvariantViolation,
                        format!("Review item {} has no reserved task", item.id()),
                    )
                })?;
                if !reserved_tasks.insert(task_id.clone()) {
                    return Err(DomainError::new(
                        DomainErrorKind::InvariantViolation,
                        format!("Review task identifier {task_id} is reserved more than once"),
                    ));
                }
                let task_exists = self.task(task_id).is_some();
                if (item.status() == ReviewStatus::Open && task_exists)
                    || (matches!(
                        item.status(),
                        ReviewStatus::InProgress | ReviewStatus::Resolved
                    ) && !task_exists)
                {
                    return Err(DomainError::new(
                        DomainErrorKind::InvariantViolation,
                        format!("Review item {} and task {task_id} disagree", item.id()),
                    ));
                }
            }
            ReviewClassification::FollowUp => {
                if !self
                    .follow_ups
                    .iter()
                    .any(|follow_up| follow_up == item.feedback())
                {
                    return Err(DomainError::new(
                        DomainErrorKind::InvariantViolation,
                        format!("Review follow-up {} is missing", item.id()),
                    ));
                }
            }
            ReviewClassification::MaterialChange | ReviewClassification::Accepted => {}
        }
        Ok(())
    }

    fn global_verification_is_satisfied(&self) -> bool {
        !self.global_verification.is_empty()
            && self.global_verification.iter().all(|check| {
                !check.is_required()
                    || (check.status() == CheckStatus::Passed && !check.evidence_refs().is_empty())
            })
    }

    fn task_mut(&mut self, task_id: &TaskId) -> Result<&mut Task, DomainError> {
        self.tasks
            .iter_mut()
            .find(|task| task.id() == task_id)
            .ok_or_else(|| {
                DomainError::new(
                    DomainErrorKind::TaskNotFound,
                    format!("Task {task_id} does not exist"),
                )
            })
    }

    fn require_status(
        &self,
        required: PlanStatus,
        action: &'static str,
    ) -> Result<(), DomainError> {
        if self.status == required {
            Ok(())
        } else {
            Err(self.invalid_transition(action))
        }
    }

    fn invalid_transition(&self, action: &'static str) -> DomainError {
        DomainError::new(
            DomainErrorKind::InvalidTransition,
            format!("Plan {} cannot {action} from {:?}", self.id, self.status),
        )
    }

    fn next_revision(&self) -> Result<u64, DomainError> {
        self.revision.checked_add(1).ok_or_else(|| {
            DomainError::new(
                DomainErrorKind::InvariantViolation,
                "Plan revision overflowed",
            )
        })
    }

    fn record_revision(&mut self, revision: u64, updated_at: Timestamp) {
        self.revision = revision;
        self.metadata.updated_at = updated_at;
    }
}

fn rework_task_number(task_id: &TaskId) -> Option<u64> {
    task_id
        .as_str()
        .strip_prefix('R')
        .and_then(|number| number.parse().ok())
}

fn context_from_input(input: DraftContextInput) -> ContextReference {
    ContextReference::new(input.reference, input.fact, input.implication)
}

fn decision_from_input(input: DraftDecisionInput) -> Decision {
    Decision::new(
        input.item,
        input.kind,
        input.value,
        input.reason,
        input.status,
    )
}

fn edge_case_from_input(input: DraftEdgeCaseInput) -> EdgeCase {
    EdgeCase::new(input.case_, input.expected_behavior, input.covered_by)
}

fn non_empty_authored_value(value: impl Into<String>, field: &str) -> Result<String, DomainError> {
    let value = value.into();
    if value.trim().is_empty() {
        Err(DomainError::new(
            DomainErrorKind::InvariantViolation,
            format!("Authored {field} cannot be empty"),
        ))
    } else {
        Ok(value)
    }
}
