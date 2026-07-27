//! Plan aggregate and its supporting authored and execution entities.

use std::collections::{BTreeMap, BTreeSet};

use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};

use super::WORKSPACE_EXTENSION_KEY;
use super::execution::EXECUTION_EXTENSION_KEY;
use super::review::review_number;
use super::standards::{STANDARDS_CONFLICT_EXTENSION_KEY, required_language_package_for_path};
use super::{
    AcceptanceCriterion, Amendment, AmendmentClassification, AmendmentImpact, AmendmentOperation,
    AmendmentPatch, AmendmentStatus, CheckId, CheckStatus, CheckpointKind, CommitGate,
    CommitStatus, CriterionId, DeviationClassification, DomainError, DomainErrorKind,
    DraftContextInput, DraftCriterionInput, DraftDecisionInput, DraftEdgeCaseInput, DraftFileInput,
    DraftMetadataInput, DraftPlanInput, DraftScopeInput, DraftTaskInput, DraftTaskUpdateInput,
    DraftVerificationInput, EvidenceId, ExecutionState, FileMapEntry, GitFlowConsent, Lineage,
    MaterialReviewDisposition, PlanArchive, PlanDraftSeed, PlanId, PlanStatus, ProtocolVersion,
    ReviewClassification, ReviewItem, ReviewStatus, SchemaVersion, StandardConflict,
    StandardsConflictState, Task, TaskId, TaskStatus, Timestamp, VerificationCheck,
    WorkspaceFingerprint, WorkspaceProtocolState,
};

const PROJECT_SCAN_EXTENSION_KEY: &str = "project_scan";

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

/// Explicit acceptance of one exact truncated project scan.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectScanAcceptance {
    scan_digest: String,
    actor: String,
    reference: String,
    reason: String,
    accepted_at: Timestamp,
}

/// Persisted digest, resource metrics, and completeness state for project discovery.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectScanSummary {
    digest: String,
    files_scanned: u64,
    directories_excluded: u64,
    symlinks_skipped: u64,
    bytes_read: u64,
    truncated: bool,
    truncation_reasons: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    acceptance: Option<ProjectScanAcceptance>,
}

impl ProjectScanSummary {
    /// Creates one unaccepted summary for an exact deterministic project scan.
    ///
    /// # Errors
    ///
    /// Returns an invariant error for a malformed digest or inconsistent
    /// truncation fields.
    pub fn new(
        digest: String,
        files_scanned: u64,
        directories_excluded: u64,
        symlinks_skipped: u64,
        bytes_read: u64,
        truncated: bool,
        mut truncation_reasons: Vec<String>,
    ) -> Result<Self, DomainError> {
        truncation_reasons.sort();
        truncation_reasons.dedup();
        let summary = Self {
            digest,
            files_scanned,
            directories_excluded,
            symlinks_skipped,
            bytes_read,
            truncated,
            truncation_reasons,
            acceptance: None,
        };
        summary.validate()?;
        Ok(summary)
    }

    /// Returns the canonical digest of the complete scan result.
    #[must_use]
    pub fn digest(&self) -> &str {
        &self.digest
    }

    /// Returns the number of ordinary files considered by discovery.
    #[must_use]
    pub const fn files_scanned(&self) -> u64 {
        self.files_scanned
    }

    /// Returns the number of generated or cache directories excluded.
    #[must_use]
    pub const fn directories_excluded(&self) -> u64 {
        self.directories_excluded
    }

    /// Returns the number of symbolic links skipped.
    #[must_use]
    pub const fn symlinks_skipped(&self) -> u64 {
        self.symlinks_skipped
    }

    /// Returns the aggregate bytes read for scan evidence.
    #[must_use]
    pub const fn bytes_read(&self) -> u64 {
        self.bytes_read
    }

    /// Returns whether one or more scan resource budgets truncated discovery.
    #[must_use]
    pub const fn truncated(&self) -> bool {
        self.truncated
    }

    /// Returns stable sorted truncation reason codes.
    #[must_use]
    pub fn truncation_reasons(&self) -> &[String] {
        &self.truncation_reasons
    }

    /// Returns the acceptance bound to this exact digest when present.
    #[must_use]
    pub const fn acceptance(&self) -> Option<&ProjectScanAcceptance> {
        self.acceptance.as_ref()
    }

    /// Returns whether a truncated scan still needs explicit acceptance.
    #[must_use]
    pub const fn is_incomplete(&self) -> bool {
        self.truncated && self.acceptance.is_none()
    }

    fn accept(
        &mut self,
        actor: String,
        reference: String,
        reason: String,
        accepted_at: Timestamp,
    ) -> Result<(), DomainError> {
        if !self.truncated || self.acceptance.is_some() {
            return Err(DomainError::new(
                DomainErrorKind::InvalidTransition,
                "Only one unaccepted truncated project scan can be accepted",
            ));
        }
        self.acceptance = Some(ProjectScanAcceptance {
            scan_digest: self.digest.clone(),
            actor,
            reference,
            reason,
            accepted_at,
        });
        self.validate()
    }

    fn preserve_acceptance_from(&mut self, previous: &Self) {
        if self.digest == previous.digest {
            self.acceptance.clone_from(&previous.acceptance);
        }
    }

    fn validate(&self) -> Result<(), DomainError> {
        const REASONS: &[&str] = &[
            "depth_limit",
            "file_limit",
            "per_file_byte_limit",
            "total_byte_limit",
        ];
        let digest = self.digest.strip_prefix("sha256:");
        let valid_digest = digest.is_some_and(|value| {
            value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
        });
        let reasons_are_valid = self
            .truncation_reasons
            .iter()
            .all(|reason| REASONS.contains(&reason.as_str()))
            && self
                .truncation_reasons
                .windows(2)
                .all(|pair| pair[0] < pair[1]);
        let acceptance_is_valid = self.acceptance.as_ref().is_none_or(|acceptance| {
            self.truncated
                && acceptance.scan_digest == self.digest
                && !acceptance.actor.trim().is_empty()
                && !acceptance.reference.trim().is_empty()
                && !acceptance.reason.trim().is_empty()
        });
        if !valid_digest
            || !reasons_are_valid
            || self.truncated == self.truncation_reasons.is_empty()
            || !acceptance_is_valid
        {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                "Project scan summary is malformed or inconsistent",
            ));
        }
        Ok(())
    }
}

impl ProjectScanAcceptance {
    /// Returns the exact scan digest accepted by the user.
    #[must_use]
    pub fn scan_digest(&self) -> &str {
        &self.scan_digest
    }

    /// Returns the actor who accepted partial discovery.
    #[must_use]
    pub fn actor(&self) -> &str {
        &self.actor
    }

    /// Returns the auditable decision reference.
    #[must_use]
    pub fn reference(&self) -> &str {
        &self.reference
    }

    /// Returns why the partial scan was accepted.
    #[must_use]
    pub fn reason(&self) -> &str {
        &self.reason
    }

    /// Returns when partial discovery was accepted.
    #[must_use]
    pub const fn accepted_at(&self) -> &Timestamp {
        &self.accepted_at
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

/// User-visible result, residual risk, and non-blocking follow-up work.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FinalOutcome {
    summary: String,
    remaining_risk: String,
    follow_up_tasks: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    follow_up_sources: Vec<OutcomeFollowUpSource>,
}

/// One Final Outcome follow-up linked to the review item that created it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OutcomeFollowUpSource {
    review_id: String,
    task: String,
}

impl FinalOutcome {
    /// Returns the user-visible completion summary.
    #[must_use]
    pub fn summary(&self) -> &str {
        &self.summary
    }

    /// Returns the explicit residual risk statement.
    #[must_use]
    pub fn remaining_risk(&self) -> &str {
        &self.remaining_risk
    }

    /// Returns all explicit and review-sourced follow-up tasks.
    #[must_use]
    pub fn follow_up_tasks(&self) -> &[String] {
        &self.follow_up_tasks
    }

    /// Returns review identifiers retained for sourced follow-up tasks.
    #[must_use]
    pub fn follow_up_sources(&self) -> &[OutcomeFollowUpSource] {
        &self.follow_up_sources
    }

    /// Returns whether the required summary and residual risk are complete.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        !self.summary.trim().is_empty() && !self.remaining_risk.trim().is_empty()
    }

    fn set(
        &mut self,
        summary: String,
        remaining_risk: String,
        follow_up_tasks: Vec<String>,
    ) -> Result<(), DomainError> {
        if summary.trim().is_empty() || remaining_risk.trim().is_empty() {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                "Final Outcome requires a summary and explicit remaining risk",
            ));
        }
        if follow_up_tasks.iter().any(|task| task.trim().is_empty()) {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                "Final Outcome follow-up tasks cannot be empty",
            ));
        }
        let mut tasks = follow_up_tasks;
        for source in &self.follow_up_sources {
            if !tasks.contains(&source.task) {
                tasks.push(source.task.clone());
            }
        }
        self.summary = summary;
        self.remaining_risk = remaining_risk;
        self.follow_up_tasks = tasks;
        Ok(())
    }

    fn add_review_follow_up(&mut self, review_id: &str, task: &str) -> Result<(), DomainError> {
        if review_number(review_id).is_none() || task.trim().is_empty() {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                "A sourced Final Outcome follow-up requires a review ID and task",
            ));
        }
        if !self
            .follow_up_sources
            .iter()
            .any(|source| source.review_id == review_id)
        {
            self.follow_up_sources.push(OutcomeFollowUpSource {
                review_id: review_id.to_owned(),
                task: task.to_owned(),
            });
        }
        if !self.follow_up_tasks.iter().any(|existing| existing == task) {
            self.follow_up_tasks.push(task.to_owned());
        }
        Ok(())
    }

    fn invalidate(&mut self) {
        self.summary.clear();
        self.remaining_risk.clear();
    }

    fn validate(&self) -> Result<(), DomainError> {
        if self.summary.trim().is_empty() != self.remaining_risk.trim().is_empty()
            || self
                .follow_up_tasks
                .iter()
                .any(|task| task.trim().is_empty())
            || self.follow_up_sources.iter().any(|source| {
                review_number(&source.review_id).is_none()
                    || source.task.trim().is_empty()
                    || !self.follow_up_tasks.contains(&source.task)
            })
        {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                "Final Outcome fields are inconsistent",
            ));
        }
        let mut source_ids = BTreeSet::new();
        if self
            .follow_up_sources
            .iter()
            .any(|source| !source_ids.insert(&source.review_id))
        {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                "Final Outcome review sources must be unique",
            ));
        }
        Ok(())
    }
}

impl OutcomeFollowUpSource {
    /// Returns the review record that created this follow-up.
    #[must_use]
    pub fn review_id(&self) -> &str {
        &self.review_id
    }

    /// Returns the sourced follow-up task description.
    #[must_use]
    pub fn task(&self) -> &str {
        &self.task
    }
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    amendments: Vec<Amendment>,
    review_items: Vec<ReviewItem>,
    follow_ups: Vec<String>,
    lineage: Option<Lineage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    archive: Option<PlanArchive>,
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
    #[serde(default)]
    amendments: Vec<Amendment>,
    review_items: Vec<ReviewItem>,
    follow_ups: Vec<String>,
    lineage: Option<Lineage>,
    #[serde(default)]
    archive: Option<PlanArchive>,
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
            amendments: unchecked.amendments,
            review_items: unchecked.review_items,
            follow_ups: unchecked.follow_ups,
            lineage: unchecked.lineage,
            archive: unchecked.archive,
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
            amendments: Vec::new(),
            review_items: Vec::new(),
            follow_ups: Vec::new(),
            lineage: None,
            archive: None,
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

    /// Creates an independent revision-one Draft from one immutable source snapshot.
    ///
    /// Execution state, approvals, evidence bindings, amendments, review records,
    /// archive state, and extensions are reset while authored fields remain exact.
    ///
    /// # Errors
    ///
    /// Returns an invariant error for an identical plan ID, incomplete fork
    /// metadata, malformed lineage, or source content that cannot be reset safely.
    #[allow(clippy::too_many_arguments)]
    pub fn fork_from_snapshot(
        source: &Self,
        id: PlanId,
        name: String,
        reason: String,
        source_state_hash: String,
        git_readiness: GitReadiness,
        branch: Option<String>,
        markdown_path: String,
        forked_at: Timestamp,
    ) -> Result<Self, DomainError> {
        if source.id == id || name.trim().is_empty() || markdown_path.trim().is_empty() {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                "Fork requires a new plan ID, name, and Markdown path",
            ));
        }
        let lineage = Lineage::new(
            source.id.clone(),
            source.revision,
            reason,
            source_state_hash,
            forked_at.clone(),
        )?;
        let mut fork = source.clone();
        fork.id = id;
        fork.revision = 1;
        fork.status = PlanStatus::Draft;
        fork.resume_status = None;
        fork.blocker = None;
        fork.metadata.name = name;
        fork.metadata.created_at = forked_at.clone();
        fork.metadata.updated_at = forked_at;
        fork.metadata.branch = branch;
        fork.metadata.markdown_path = Some(markdown_path);
        fork.git_readiness = git_readiness;
        for task in &mut fork.tasks {
            task.reset_for_fork();
        }
        for check in &mut fork.global_verification {
            check.reset_for_fork();
        }
        fork.approvals.clear();
        fork.amendments.clear();
        fork.review_items.clear();
        fork.follow_ups.clear();
        fork.lineage = Some(lineage);
        fork.archive = None;
        fork.final_outcome = FinalOutcome::default();
        fork.extensions.clear();
        fork.validate_invariants()?;
        Ok(fork)
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

    /// Returns the persisted project-scan summary when this plan was seeded by discovery.
    ///
    /// # Errors
    ///
    /// Returns an invariant error when the extension payload is malformed.
    pub fn project_scan_summary(&self) -> Result<Option<ProjectScanSummary>, DomainError> {
        self.extensions
            .get(PROJECT_SCAN_EXTENSION_KEY)
            .map(|value| {
                serde_json::from_value::<ProjectScanSummary>(value.clone()).map_err(|error| {
                    DomainError::new(
                        DomainErrorKind::InvariantViolation,
                        format!("Project scan extension is malformed: {error}"),
                    )
                })
            })
            .transpose()
    }

    /// Returns whether current project discovery was truncated and remains unaccepted.
    ///
    /// # Errors
    ///
    /// Returns an invariant error when the extension payload is malformed.
    pub fn scan_is_incomplete(&self) -> Result<bool, DomainError> {
        Ok(self
            .project_scan_summary()?
            .as_ref()
            .is_some_and(ProjectScanSummary::is_incomplete))
    }

    pub(crate) fn record_initial_project_scan(
        &mut self,
        summary: &ProjectScanSummary,
    ) -> Result<(), DomainError> {
        if self.status != PlanStatus::Draft
            || self.revision != 1
            || self.extensions.contains_key(PROJECT_SCAN_EXTENSION_KEY)
        {
            return Err(DomainError::new(
                DomainErrorKind::InvalidTransition,
                "Initial project scan can be recorded only once on a revision-one Draft",
            ));
        }
        summary.validate()?;
        self.store_project_scan_summary(summary)?;
        self.validate_invariants()
    }

    /// Accepts the current exact truncated scan so Draft authoring may continue.
    ///
    /// # Errors
    ///
    /// Returns an error unless the plan is Draft and its current scan is
    /// truncated, unaccepted, and supplied with complete audit fields.
    pub fn accept_project_scan(
        &mut self,
        actor: String,
        decision_reference: String,
        reason: String,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        self.require_status(PlanStatus::Draft, "accept partial project scan")?;
        let mut summary = self.project_scan_summary()?.ok_or_else(|| {
            DomainError::new(
                DomainErrorKind::InvalidTransition,
                "The plan has no persisted project scan to accept",
            )
        })?;
        summary.accept(actor, decision_reference, reason, updated_at.clone())?;
        let mut candidate = self.clone();
        candidate.store_project_scan_summary(&summary)?;
        candidate.record_revision(candidate.next_revision()?, updated_at);
        candidate.validate_invariants()?;
        *self = candidate;
        Ok(())
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

    /// Returns protected amendments in monotonic proposal order.
    #[must_use]
    pub fn amendments(&self) -> &[Amendment] {
        &self.amendments
    }

    /// Returns one protected amendment by its stable change identifier.
    #[must_use]
    pub fn amendment(&self, change_id: &str) -> Option<&Amendment> {
        self.amendments
            .iter()
            .find(|amendment| amendment.id() == change_id)
    }

    /// Returns the only unapplied amendment when one exists.
    #[must_use]
    pub fn pending_amendment(&self) -> Option<&Amendment> {
        self.amendments
            .iter()
            .find(|amendment| amendment.is_pending())
    }

    /// Returns whether an unapplied amendment owns the mutation boundary.
    #[must_use]
    pub fn has_pending_amendment(&self) -> bool {
        self.pending_amendment().is_some()
    }

    /// Returns whether applied amendment impact invalidated an evidence record.
    #[must_use]
    pub fn is_evidence_stale(&self, evidence_id: &EvidenceId) -> bool {
        self.amendments.iter().any(|amendment| {
            amendment.is_applied() && amendment.impact().stale_evidence().contains(evidence_id)
        })
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

    /// Returns the user-visible final execution result and residual risk.
    #[must_use]
    pub const fn final_outcome(&self) -> &FinalOutcome {
        &self.final_outcome
    }

    /// Returns immutable fork provenance when this plan is an alternative.
    #[must_use]
    pub const fn lineage(&self) -> Option<&Lineage> {
        self.lineage.as_ref()
    }

    /// Returns non-destructive archive metadata when the plan is deactivated.
    #[must_use]
    pub const fn archive_record(&self) -> Option<&PlanArchive> {
        self.archive.as_ref()
    }

    /// Returns whether this plan is semantically archived.
    #[must_use]
    pub const fn is_archived(&self) -> bool {
        self.archive.is_some()
    }

    /// Deactivates the plan without changing its lifecycle status or deleting history.
    ///
    /// # Errors
    ///
    /// Returns an error when the plan is already archived or the actor, reason,
    /// or approval reference is incomplete.
    pub fn archive(
        &mut self,
        reason: String,
        actor: String,
        approval_reference: String,
        archived_at: Timestamp,
    ) -> Result<(), DomainError> {
        if self.archive.is_some() {
            return Err(DomainError::new(
                DomainErrorKind::InvalidTransition,
                format!("Plan {} is already archived", self.id),
            ));
        }
        let archive = PlanArchive::new(reason, actor, approval_reference, archived_at.clone())?;
        let mut candidate = self.clone();
        candidate.archive = Some(archive);
        candidate.record_revision(candidate.next_revision()?, archived_at);
        candidate.validate_invariants()?;
        *self = candidate;
        Ok(())
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

    /// Returns whether a pending Material amendment owns the blocked state.
    #[must_use]
    pub fn is_blocked_for_material_amendment(&self) -> bool {
        self.status == PlanStatus::Blocked
            && self.pending_amendment().is_some_and(|amendment| {
                amendment.classification() == AmendmentClassification::Material
            })
            && matches!(
                self.resume_status,
                Some(PlanStatus::Ready | PlanStatus::InProgress | PlanStatus::Review)
            )
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
        let mut state = self
            .extensions
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
            )?;
        state.materialize_legacy_deviations()?;
        Ok(state)
    }

    /// Returns persisted plan and task workspace baselines.
    ///
    /// # Errors
    ///
    /// Returns an invariant error when the workspace extension is malformed.
    pub fn workspace_state(&self) -> Result<WorkspaceProtocolState, DomainError> {
        self.extensions
            .get(WORKSPACE_EXTENSION_KEY)
            .cloned()
            .map_or_else(
                || Ok(WorkspaceProtocolState::default()),
                |value| {
                    serde_json::from_value(value).map_err(|error| {
                        DomainError::new(
                            DomainErrorKind::InvariantViolation,
                            format!("Workspace extension is malformed: {error}"),
                        )
                    })
                },
            )
    }

    /// Returns typed standards-conflict snapshots stored in the extension namespace.
    ///
    /// # Errors
    ///
    /// Returns an invariant error when the standards-conflict extension is malformed.
    pub fn standards_conflict_state(&self) -> Result<StandardsConflictState, DomainError> {
        self.extensions
            .get(STANDARDS_CONFLICT_EXTENSION_KEY)
            .cloned()
            .map_or_else(
                || Ok(StandardsConflictState::default()),
                |value| {
                    serde_json::from_value(value).map_err(|error| {
                        DomainError::new(
                            DomainErrorKind::InvariantViolation,
                            format!("Standards conflict extension is malformed: {error}"),
                        )
                    })
                },
            )
    }

    /// Returns the next monotonic protected-change identifier without reserving it.
    ///
    /// # Errors
    ///
    /// Returns an invariant error when the change counter overflows.
    pub fn next_amendment_id(&self) -> Result<String, DomainError> {
        let number = self.amendments.len().checked_add(1).ok_or_else(|| {
            DomainError::new(
                DomainErrorKind::InvariantViolation,
                "Amendment count overflowed",
            )
        })?;
        Ok(format!("C{number}"))
    }

    pub(crate) fn amendment_minimum_classification(
        &self,
        patch: &AmendmentPatch,
    ) -> Result<AmendmentClassification, DomainError> {
        let operation_minimum = patch.minimum_classification()?;
        self.contextual_amendment_minimum(patch.operations(), operation_minimum)
    }

    /// Proposes one typed protected change against the exact current revision.
    ///
    /// # Errors
    ///
    /// Returns an error for an unsupported lifecycle state, a concurrent pending
    /// proposal, malformed operations, a lowered classification, or invalid targets.
    #[allow(clippy::too_many_arguments)]
    pub fn propose_amendment(
        &mut self,
        reason: String,
        patch: AmendmentPatch,
        requested_classification: Option<AmendmentClassification>,
        base_state_hash: String,
        proposer: String,
        updated_at: Timestamp,
    ) -> Result<String, DomainError> {
        if self.has_pending_amendment() {
            return Err(DomainError::new(
                DomainErrorKind::InvalidTransition,
                "Apply the current pending amendment before proposing another",
            ));
        }
        if self.running_check_count() != 0 {
            return Err(DomainError::new(
                DomainErrorKind::InvalidTransition,
                "An amendment cannot be proposed while a verification check is Running",
            ));
        }
        let is_material_review = self.is_blocked_for_material_review();
        if !matches!(self.status, PlanStatus::Ready | PlanStatus::InProgress) && !is_material_review
        {
            return Err(self.invalid_transition("propose a protected amendment"));
        }
        if is_material_review
            && !self.review_items.iter().any(|item| {
                item.classification() == ReviewClassification::MaterialChange
                    && item.status() == ReviewStatus::Blocked
                    && item.disposition() == Some(MaterialReviewDisposition::AcceptChange)
            })
        {
            return Err(DomainError::new(
                DomainErrorKind::ApprovalRequired,
                "Material review feedback requires an explicit accept-change disposition",
            ));
        }
        let minimum_classification = self.amendment_minimum_classification(&patch)?;
        let classification = requested_classification.unwrap_or(minimum_classification);
        if classification < minimum_classification
            || (is_material_review && classification != AmendmentClassification::Material)
        {
            return Err(DomainError::new(
                DomainErrorKind::ApprovalRequired,
                "The requested classification cannot lower the protected minimum",
            ));
        }
        let operations = patch.into_operations();
        let impact = self.amendment_impact(&operations, classification)?;
        let change_id = self.next_amendment_id()?;
        let amendment = Amendment::proposed(
            change_id.clone(),
            reason,
            minimum_classification,
            classification,
            operations,
            self.revision,
            base_state_hash,
            impact,
            proposer,
            updated_at.clone(),
        )?;
        let next_revision = self.next_revision()?;
        let mut candidate = self.clone();
        if classification == AmendmentClassification::Material && !is_material_review {
            let resume_status = candidate.status;
            if resume_status == PlanStatus::InProgress
                && let Some(task) = candidate
                    .tasks
                    .iter_mut()
                    .find(|task| task.status() == TaskStatus::InProgress)
            {
                task.block(format!(
                    "Material amendment {change_id} requires explicit approval"
                ))?;
            }
            candidate.status = PlanStatus::Blocked;
            candidate.resume_status = Some(resume_status);
            candidate.blocker = Some(format!(
                "Material amendment {change_id} requires explicit approval"
            ));
        }
        candidate.amendments.push(amendment);
        candidate.record_revision(next_revision, updated_at);
        candidate.validate_invariants()?;
        *self = candidate;
        Ok(change_id)
    }

    /// Records explicit approval for one pending Material amendment.
    ///
    /// # Errors
    ///
    /// Returns an error unless the named proposal is the current unapproved
    /// Material change and the actor plus approval reference are complete.
    pub fn approve_amendment(
        &mut self,
        change_id: &str,
        actor: String,
        approval_reference: String,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        let amendment_index = self.pending_amendment_index(change_id)?;
        let next_revision = self.next_revision()?;
        let mut candidate = self.clone();
        candidate.amendments[amendment_index].approve(
            actor,
            approval_reference,
            updated_at.clone(),
        )?;
        candidate.record_revision(next_revision, updated_at);
        candidate.validate_invariants()?;
        *self = candidate;
        Ok(())
    }

    /// Rejects one unapproved Material amendment without applying its operations.
    ///
    /// # Errors
    ///
    /// Returns an error unless the named proposal awaits Material approval and
    /// the actor, decision reference, and reason are complete.
    pub fn reject_amendment(
        &mut self,
        change_id: &str,
        actor: String,
        decision_reference: String,
        reason: String,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        self.dispose_amendment(change_id, updated_at, move |amendment, at| {
            amendment.reject(actor, decision_reference, reason, at)
        })
    }

    /// Withdraws one unapproved proposal as its original proposer.
    ///
    /// # Errors
    ///
    /// Returns an error unless the proposal is unapproved, the actor is its
    /// proposer, and the reason is complete.
    pub fn withdraw_amendment(
        &mut self,
        change_id: &str,
        actor: String,
        reason: String,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        self.dispose_amendment(change_id, updated_at, move |amendment, at| {
            amendment.withdraw(actor, reason, at)
        })
    }

    /// Cancels one approved Material amendment without applying its operations.
    ///
    /// # Errors
    ///
    /// Returns an error unless the named proposal is approved, the actor is its
    /// original approver, and the decision reference and reason are complete.
    pub fn cancel_amendment(
        &mut self,
        change_id: &str,
        actor: String,
        decision_reference: String,
        reason: String,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        self.dispose_amendment(change_id, updated_at, move |amendment, at| {
            amendment.cancel(actor, decision_reference, reason, at)
        })
    }

    /// Atomically applies one eligible typed amendment and its invalidations.
    ///
    /// # Errors
    ///
    /// Returns an error unless the named proposal is pending, its approval gate
    /// is satisfied, and every operation preserves plan invariants.
    pub fn apply_amendment(
        &mut self,
        change_id: &str,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        let amendment_index = self.pending_amendment_index(change_id)?;
        let classification = self.amendments[amendment_index].classification();
        let operations = self.amendments[amendment_index].operations().to_vec();
        let next_revision = self.next_revision()?;
        let mut candidate = self.clone();
        for operation in operations {
            candidate.apply_amendment_operation(operation)?;
        }
        match classification {
            AmendmentClassification::Minor => {
                if candidate.status == PlanStatus::Ready {
                    candidate.invalidate_plan_approval();
                }
            }
            AmendmentClassification::Material => {
                for task in &mut candidate.tasks {
                    task.reset_for_material_amendment();
                }
                for check in &mut candidate.global_verification {
                    check.reset_for_rework();
                }
                candidate.invalidate_plan_approval();
                candidate.final_outcome.invalidate();
                let mut execution = candidate.execution_state()?;
                execution.reset_for_material_amendment();
                candidate.store_execution_state(&execution)?;
                let mut workspace = candidate.workspace_state()?;
                workspace.reset_for_material_amendment();
                candidate.store_workspace_state(&workspace)?;
                for item in &mut candidate.review_items {
                    item.supersede_for_amendment(change_id)?;
                }
                candidate.status = PlanStatus::Ready;
                candidate.resume_status = None;
                candidate.blocker = None;
            }
        }
        candidate.amendments[amendment_index].mark_applied(updated_at.clone())?;
        candidate.record_revision(next_revision, updated_at);
        candidate.validate_invariants()?;
        *self = candidate;
        Ok(())
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
            let expected = task.next_criterion_id()?;
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

    /// Returns the next monotonic authored task identifier.
    ///
    /// # Errors
    ///
    /// Returns an invariant error when the numeric identifier would overflow.
    pub fn next_task_id(&self) -> Result<TaskId, DomainError> {
        let maximum = self
            .tasks
            .iter()
            .map(Task::id)
            .chain(
                self.amendments
                    .iter()
                    .flat_map(Amendment::operations)
                    .filter_map(|operation| match operation {
                        AmendmentOperation::AddTask { task } => task.id.as_ref(),
                        AmendmentOperation::RemoveTask { task_id } => Some(task_id),
                        _ => None,
                    }),
            )
            .filter_map(|task_id| task_id.as_str().strip_prefix('T'))
            .filter_map(|number| number.parse::<u64>().ok())
            .max()
            .unwrap_or(0);
        let number = maximum.checked_add(1).ok_or_else(|| {
            DomainError::new(
                DomainErrorKind::InvariantViolation,
                "Task identifier overflowed",
            )
        })?;
        TaskId::parse(format!("T{number}"))
    }

    /// Replaces supplied fields on one existing Draft task.
    ///
    /// # Errors
    ///
    /// Returns an error for missing tasks, empty updates, invalid dependencies,
    /// malformed gates, or non-Draft state.
    pub fn author_task_update(
        &mut self,
        task_id: &TaskId,
        update: DraftTaskUpdateInput,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        let task_id = task_id.clone();
        self.author_draft(updated_at, move |candidate| {
            candidate.task_mut(&task_id)?.update_draft(update)?;
            candidate.validate_authored_dependency_order()
        })
    }

    /// Removes one unreferenced Draft task and its file responsibilities.
    ///
    /// # Errors
    ///
    /// Returns an error when the task is missing, another task depends on it,
    /// or the plan is not Draft.
    pub fn author_task_remove(
        &mut self,
        task_id: &TaskId,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        let task_id = task_id.clone();
        self.author_draft(updated_at, move |candidate| {
            if let Some(dependent) = candidate
                .tasks
                .iter()
                .find(|task| task.dependencies().contains(&task_id))
            {
                return Err(DomainError::new(
                    DomainErrorKind::UnmetDependencies,
                    format!("Task {} depends on {task_id}", dependent.id()),
                ));
            }
            let index = candidate
                .tasks
                .iter()
                .position(|task| task.id() == &task_id)
                .ok_or_else(|| {
                    DomainError::new(
                        DomainErrorKind::TaskNotFound,
                        format!("Task {task_id} does not exist"),
                    )
                })?;
            candidate.tasks.remove(index);
            candidate.task_order.retain(|current| current != &task_id);
            candidate.rebuild_authored_file_map();
            if candidate.extensions.contains_key(WORKSPACE_EXTENSION_KEY) {
                let mut workspace = candidate.workspace_state()?;
                workspace.remove_task_baseline(&task_id);
                candidate.store_workspace_state(&workspace)?;
            }
            Ok(())
        })
    }

    /// Moves one Draft task to a one-based implementation position.
    ///
    /// # Errors
    ///
    /// Returns an error for a missing task, invalid position, dependency order
    /// violation, or non-Draft state.
    pub fn author_task_move(
        &mut self,
        task_id: &TaskId,
        position: usize,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        let task_id = task_id.clone();
        self.author_draft(updated_at, move |candidate| {
            let target = one_based_index(position, candidate.task_order.len(), "task")?;
            let source = candidate
                .task_order
                .iter()
                .position(|current| current == &task_id)
                .ok_or_else(|| {
                    DomainError::new(
                        DomainErrorKind::TaskNotFound,
                        format!("Task {task_id} does not exist"),
                    )
                })?;
            let moved = candidate.task_order.remove(source);
            candidate.task_order.insert(target, moved);
            candidate.validate_authored_dependency_order()?;
            candidate.rebuild_authored_file_map();
            Ok(())
        })
    }

    /// Replaces one one-based Draft task step.
    ///
    /// # Errors
    ///
    /// Returns an error for a non-Draft plan, missing task or position, or empty step.
    pub fn author_task_step_update(
        &mut self,
        task_id: &TaskId,
        position: usize,
        value: String,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        let task_id = task_id.clone();
        self.author_draft(updated_at, move |candidate| {
            candidate.task_mut(&task_id)?.update_step(position, value)
        })
    }

    /// Removes one one-based Draft task step.
    ///
    /// # Errors
    ///
    /// Returns an error for a non-Draft plan, missing task, or missing position.
    pub fn author_task_step_remove(
        &mut self,
        task_id: &TaskId,
        position: usize,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        let task_id = task_id.clone();
        self.author_draft(updated_at, move |candidate| {
            candidate.task_mut(&task_id)?.remove_step(position)
        })
    }

    /// Replaces one stable Draft criterion description.
    ///
    /// # Errors
    ///
    /// Returns an error for a non-Draft plan, missing task or criterion, or empty description.
    pub fn author_task_criterion_update(
        &mut self,
        task_id: &TaskId,
        criterion_id: &CriterionId,
        description: String,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        let task_id = task_id.clone();
        let criterion_id = criterion_id.clone();
        self.author_draft(updated_at, move |candidate| {
            candidate
                .task_mut(&task_id)?
                .update_criterion(&criterion_id, description)
        })
    }

    /// Removes one stable Draft criterion.
    ///
    /// # Errors
    ///
    /// Returns an error for a non-Draft plan, missing task, or missing criterion.
    pub fn author_task_criterion_remove(
        &mut self,
        task_id: &TaskId,
        criterion_id: &CriterionId,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        let task_id = task_id.clone();
        let criterion_id = criterion_id.clone();
        self.author_draft(updated_at, move |candidate| {
            candidate
                .task_mut(&task_id)?
                .remove_criterion(&criterion_id)
        })
    }

    /// Replaces one stable Draft task verification definition.
    ///
    /// # Errors
    ///
    /// Returns an error for a non-Draft plan, missing task or check, changed ID, or malformed check.
    pub fn author_task_verification_update(
        &mut self,
        task_id: &TaskId,
        check_id: &CheckId,
        verification: DraftVerificationInput,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        let task_id = task_id.clone();
        let check_id = check_id.clone();
        self.author_draft(updated_at, move |candidate| {
            candidate
                .task_mut(&task_id)?
                .update_verification(&check_id, verification)
        })
    }

    /// Removes one stable Draft task verification definition.
    ///
    /// # Errors
    ///
    /// Returns an error for a non-Draft plan, missing task, or missing check.
    pub fn author_task_verification_remove(
        &mut self,
        task_id: &TaskId,
        check_id: &CheckId,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        let task_id = task_id.clone();
        let check_id = check_id.clone();
        self.author_draft(updated_at, move |candidate| {
            candidate.task_mut(&task_id)?.remove_verification(&check_id)
        })
    }

    /// Replaces one one-based Draft file responsibility.
    ///
    /// # Errors
    ///
    /// Returns an error for a non-Draft plan, missing task or position, or malformed file entry.
    pub fn author_file_update(
        &mut self,
        task_id: &TaskId,
        position: usize,
        file: DraftFileInput,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        let task_id = task_id.clone();
        self.author_draft(updated_at, move |candidate| {
            candidate.task_mut(&task_id)?.update_file(position, file)?;
            candidate.rebuild_authored_file_map();
            Ok(())
        })
    }

    /// Removes one one-based Draft file responsibility.
    ///
    /// # Errors
    ///
    /// Returns an error for a non-Draft plan, missing task, or missing position.
    pub fn author_file_remove(
        &mut self,
        task_id: &TaskId,
        position: usize,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        let task_id = task_id.clone();
        self.author_draft(updated_at, move |candidate| {
            candidate.task_mut(&task_id)?.remove_file(position)?;
            candidate.rebuild_authored_file_map();
            Ok(())
        })
    }

    /// Replaces one stable global Draft verification definition.
    ///
    /// # Errors
    ///
    /// Returns an error for a non-Draft plan, missing check, changed ID, or malformed check.
    pub fn author_global_verification_update(
        &mut self,
        check_id: &CheckId,
        verification: DraftVerificationInput,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        let check_id = check_id.clone();
        self.author_draft(updated_at, move |candidate| {
            if verification.id != check_id {
                return Err(DomainError::new(
                    DomainErrorKind::InvariantViolation,
                    "Global verification update cannot change its stable identifier",
                ));
            }
            candidate
                .global_verification
                .iter_mut()
                .find(|check| check.id() == &check_id)
                .ok_or_else(|| {
                    DomainError::new(
                        DomainErrorKind::InvariantViolation,
                        format!("Global check {check_id} does not exist"),
                    )
                })?
                .replace_definition(
                    verification.command,
                    verification.cwd,
                    verification.expected_exit_code,
                    verification.required,
                )
        })
    }

    /// Removes one stable global Draft verification definition.
    ///
    /// # Errors
    ///
    /// Returns an error for a non-Draft plan or missing check.
    pub fn author_global_verification_remove(
        &mut self,
        check_id: &CheckId,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        let check_id = check_id.clone();
        self.author_draft(updated_at, move |candidate| {
            let index = candidate
                .global_verification
                .iter()
                .position(|check| check.id() == &check_id)
                .ok_or_else(|| {
                    DomainError::new(
                        DomainErrorKind::InvariantViolation,
                        format!("Global check {check_id} does not exist"),
                    )
                })?;
            candidate.global_verification.remove(index);
            Ok(())
        })
    }

    /// Replaces one one-based Draft decision.
    ///
    /// # Errors
    ///
    /// Returns an error for a non-Draft plan, missing position, or malformed decision.
    pub fn author_decision_update(
        &mut self,
        position: usize,
        decision: DraftDecisionInput,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        self.author_draft(updated_at, move |candidate| {
            let index = one_based_index(position, candidate.decisions.len(), "decision")?;
            candidate.decisions[index] = decision_from_input(decision);
            Ok(())
        })
    }

    /// Removes one one-based Draft decision.
    ///
    /// # Errors
    ///
    /// Returns an error for a non-Draft plan or missing position.
    pub fn author_decision_remove(
        &mut self,
        position: usize,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        self.author_draft(updated_at, move |candidate| {
            let index = one_based_index(position, candidate.decisions.len(), "decision")?;
            candidate.decisions.remove(index);
            Ok(())
        })
    }

    /// Replaces one one-based Draft edge case.
    ///
    /// # Errors
    ///
    /// Returns an error for a non-Draft plan, missing position, or malformed edge case.
    pub fn author_edge_case_update(
        &mut self,
        position: usize,
        edge_case: DraftEdgeCaseInput,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        self.author_draft(updated_at, move |candidate| {
            let index = one_based_index(position, candidate.edge_cases.len(), "edge case")?;
            candidate.edge_cases[index] = edge_case_from_input(edge_case);
            Ok(())
        })
    }

    /// Removes one one-based Draft edge case.
    ///
    /// # Errors
    ///
    /// Returns an error for a non-Draft plan or missing position.
    pub fn author_edge_case_remove(
        &mut self,
        position: usize,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        self.author_draft(updated_at, move |candidate| {
            let index = one_based_index(position, candidate.edge_cases.len(), "edge case")?;
            candidate.edge_cases.remove(index);
            Ok(())
        })
    }

    fn validate_authored_dependency_order(&self) -> Result<(), DomainError> {
        let positions = self
            .task_order
            .iter()
            .enumerate()
            .map(|(position, task_id)| (task_id, position))
            .collect::<BTreeMap<_, _>>();
        for task in &self.tasks {
            let task_position = positions.get(task.id()).ok_or_else(|| {
                DomainError::new(
                    DomainErrorKind::InvariantViolation,
                    format!("Task {} is missing from implementation order", task.id()),
                )
            })?;
            for dependency in task.dependencies() {
                let dependency_position = positions.get(dependency).ok_or_else(|| {
                    DomainError::new(
                        DomainErrorKind::UnmetDependencies,
                        format!("Task {} depends on missing task {dependency}", task.id()),
                    )
                })?;
                if dependency_position >= task_position {
                    return Err(DomainError::new(
                        DomainErrorKind::UnmetDependencies,
                        format!("Task {} dependency {dependency} must precede it", task.id()),
                    ));
                }
            }
        }
        Ok(())
    }

    fn rebuild_authored_file_map(&mut self) {
        self.approach.file_map = self
            .task_order
            .iter()
            .filter_map(|task_id| self.task(task_id))
            .flat_map(Task::file_map)
            .cloned()
            .collect();
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
        let expected_id = self.next_task_id()?;
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
        self.record_approval_internal(approval, None)
    }

    /// Records a plan approval and the exact approved workspace baseline.
    ///
    /// # Errors
    ///
    /// Returns an error unless the plan is Ready, the approval is explicit,
    /// and the baseline is a valid complete-project capture.
    pub fn record_approval_with_baseline(
        &mut self,
        approval: Approval,
        baseline: WorkspaceFingerprint,
    ) -> Result<(), DomainError> {
        self.record_approval_internal(approval, Some(baseline))
    }

    fn record_approval_internal(
        &mut self,
        approval: Approval,
        baseline: Option<WorkspaceFingerprint>,
    ) -> Result<(), DomainError> {
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
        if let Some(baseline) = baseline {
            let mut workspace = self.workspace_state()?;
            workspace.record_plan_baseline(baseline)?;
            self.store_workspace_state(&workspace)?;
        }
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
        self.start_task_internal(task_id, None, updated_at)
    }

    /// Starts the first eligible task and records its exact workspace baseline.
    ///
    /// # Errors
    ///
    /// Returns the normal task-start errors or an invariant error for a
    /// malformed complete-project baseline.
    pub fn start_task_with_baseline(
        &mut self,
        task_id: &TaskId,
        baseline: WorkspaceFingerprint,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        self.start_task_internal(task_id, Some(baseline), updated_at)
    }

    fn start_task_internal(
        &mut self,
        task_id: &TaskId,
        baseline: Option<WorkspaceFingerprint>,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        self.ensure_no_pending_amendment("start a task")?;
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

        self.validate_task_order_for_start(task_id)?;

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
        if let Some(baseline) = baseline {
            let mut workspace = self.workspace_state()?;
            if workspace.plan_baseline().is_none() {
                workspace.record_plan_baseline(baseline.clone())?;
            }
            workspace.record_task_baseline(task_id.clone(), baseline)?;
            self.store_workspace_state(&workspace)?;
        }
        self.status = PlanStatus::InProgress;
        self.record_revision(next_revision, updated_at);
        Ok(())
    }

    fn validate_task_order_for_start(&self, task_id: &TaskId) -> Result<(), DomainError> {
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
        if let Some(prior_id) = self.task_order[..task_position].iter().find(|prior_id| {
            self.task(prior_id).is_some_and(|prior| {
                prior
                    .commit_gate()
                    .is_some_and(|gate| gate.is_required() && !gate.is_satisfied())
            })
        }) {
            return Err(DomainError::new(
                DomainErrorKind::TaskOrderViolation,
                format!("Task {prior_id} must be committed before task {task_id} can start"),
            ));
        }
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
        self.store_execution_state(&execution)?;
        self.record_revision(next_revision, updated_at);
        self.validate_invariants()
    }

    /// Records one identified deviation for the active task.
    ///
    /// # Errors
    ///
    /// Returns an error unless the plan and task are In Progress and the
    /// deviation classification, summary, and actor are complete.
    pub fn record_deviation(
        &mut self,
        task_id: &TaskId,
        classification: DeviationClassification,
        summary: String,
        affected_paths: Vec<String>,
        actor: String,
        updated_at: Timestamp,
    ) -> Result<String, DomainError> {
        self.require_active_deviation_task(task_id, "record a deviation")?;
        let next_revision = self.next_revision()?;
        let mut candidate = self.clone();
        let mut execution = candidate.execution_state()?;
        let deviation_id = execution.record_deviation(
            task_id.clone(),
            classification,
            summary,
            affected_paths,
            actor,
            updated_at.clone(),
        )?;
        candidate.store_execution_state(&execution)?;
        candidate.record_revision(next_revision, updated_at);
        candidate.validate_invariants()?;
        *self = candidate;
        Ok(deviation_id)
    }

    /// Resolves one open deviation with immutable evidence references.
    ///
    /// # Errors
    ///
    /// Returns an error unless the deviation belongs to the active task and the
    /// actor, resolution, and sorted unique evidence references are complete.
    pub fn resolve_deviation(
        &mut self,
        deviation_id: &str,
        actor: String,
        resolution: String,
        evidence_refs: Vec<EvidenceId>,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        self.dispose_active_deviation(deviation_id, updated_at, move |execution, at| {
            execution.resolve_deviation(deviation_id, actor, resolution, evidence_refs, at)
        })
    }

    /// Rejects one open deviation through a protected decision reference.
    ///
    /// # Errors
    ///
    /// Returns an error unless the deviation belongs to the active task and the
    /// actor, decision reference, and reason are complete.
    pub fn reject_deviation(
        &mut self,
        deviation_id: &str,
        actor: String,
        decision_reference: String,
        reason: String,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        self.dispose_active_deviation(deviation_id, updated_at, move |execution, at| {
            execution.reject_deviation(deviation_id, actor, decision_reference, reason, at)
        })
    }

    /// Supersedes one open deviation with an applied protected amendment.
    ///
    /// # Errors
    ///
    /// Returns an error unless the plan is Ready or In Progress, the named
    /// amendment is Applied, and the actor and reason are complete.
    pub fn supersede_deviation(
        &mut self,
        deviation_id: &str,
        actor: String,
        amendment_id: String,
        reason: String,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        self.ensure_no_pending_amendment("supersede a deviation")?;
        if !matches!(self.status, PlanStatus::Ready | PlanStatus::InProgress)
            || self
                .amendment(&amendment_id)
                .is_none_or(|amendment| amendment.status() != AmendmentStatus::Applied)
        {
            return Err(self.invalid_transition("supersede a deviation"));
        }
        let next_revision = self.next_revision()?;
        let mut candidate = self.clone();
        let mut execution = candidate.execution_state()?;
        execution.supersede_deviation(
            deviation_id,
            actor,
            amendment_id,
            reason,
            updated_at.clone(),
        )?;
        candidate.store_execution_state(&execution)?;
        candidate.record_revision(next_revision, updated_at);
        candidate.validate_invariants()?;
        *self = candidate;
        Ok(())
    }

    /// Replaces detected standards conflicts while preserving only current decisions.
    ///
    /// A Ready-plan refresh invalidates its plan approval because the reviewed
    /// standards sources changed. Draft and Ready are the only legal states.
    ///
    /// # Errors
    ///
    /// Returns an error for an illegal state, malformed conflict snapshot, or
    /// a refresh that would make no semantic change.
    pub fn refresh_standards_conflicts(
        &mut self,
        conflicts: Vec<StandardConflict>,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        if !matches!(self.status, PlanStatus::Draft | PlanStatus::Ready) {
            return Err(DomainError::new(
                DomainErrorKind::InvalidTransition,
                format!(
                    "Plan {} cannot refresh standards conflicts from status {:?}",
                    self.id, self.status
                ),
            ));
        }
        let refreshed = self.standards_conflict_state()?.refreshed(conflicts)?;
        if refreshed == self.standards_conflict_state()? {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                "Standards conflict refresh made no semantic change",
            ));
        }
        let mut candidate = self.clone();
        candidate.store_standards_conflicts(&refreshed)?;
        if candidate.status == PlanStatus::Ready {
            candidate.invalidate_plan_approval();
        }
        candidate.record_revision(candidate.next_revision()?, updated_at);
        candidate.validate_invariants()?;
        *self = candidate;
        Ok(())
    }

    /// Records one explicit standards candidate decision with required rationale.
    ///
    /// # Errors
    ///
    /// Returns an error for an illegal lifecycle state, unknown conflict or
    /// candidate, duplicate resolution, empty rationale, or malformed state.
    pub fn resolve_standards_conflict(
        &mut self,
        conflict_id: &str,
        candidate_id: &str,
        rationale: String,
        decision_reference: String,
        actor: String,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        if !matches!(self.status, PlanStatus::Draft | PlanStatus::Ready) {
            return Err(DomainError::new(
                DomainErrorKind::InvalidTransition,
                format!(
                    "Plan {} cannot resolve standards conflicts from status {:?}",
                    self.id, self.status
                ),
            ));
        }
        let mut state = self.standards_conflict_state()?;
        state.resolve(
            conflict_id,
            candidate_id,
            rationale,
            decision_reference,
            actor,
            updated_at.clone(),
        )?;
        let mut candidate = self.clone();
        candidate.store_standards_conflicts(&state)?;
        if candidate.status == PlanStatus::Ready {
            candidate.invalidate_plan_approval();
        }
        candidate.record_revision(candidate.next_revision()?, updated_at);
        candidate.validate_invariants()?;
        *self = candidate;
        Ok(())
    }

    /// Reconciles embedded standards, their catalog-owned checks, project scan
    /// evidence, and detected conflict snapshots in one plan revision.
    ///
    /// Unchanged check definitions retain their current status and evidence.
    /// Changed or newly required definitions replace prior catalog checks with
    /// fresh Pending checks. Custom standards and global checks are preserved.
    ///
    /// # Errors
    ///
    /// Returns an error for an illegal lifecycle state, malformed or duplicate
    /// catalog inputs, a collision with task-owned checks, or a semantic no-op.
    pub fn reconcile_standards(
        &mut self,
        embedded_standards: Vec<StandardSelection>,
        catalog_check_ids: &BTreeSet<CheckId>,
        catalog_checks: Vec<VerificationCheck>,
        scan_summary: ProjectScanSummary,
        conflict_state: &StandardsConflictState,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        conflict_state.validate()?;
        let mut candidate = self.preview_standards_reconciliation(
            embedded_standards,
            catalog_check_ids,
            catalog_checks,
            scan_summary,
        )?;
        candidate.store_standards_conflicts(conflict_state)?;
        if candidate.standards == self.standards
            && candidate.global_verification == self.global_verification
            && candidate.project_scan_summary()? == self.project_scan_summary()?
            && candidate.standards_conflict_state()? == self.standards_conflict_state()?
        {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                "Standards reconciliation made no semantic change",
            ));
        }
        if candidate.status == PlanStatus::Ready {
            candidate.invalidate_plan_approval();
        }
        candidate.record_revision(candidate.next_revision()?, updated_at);
        candidate.validate_invariants()?;
        *self = candidate;
        Ok(())
    }

    pub(crate) fn preview_standards_reconciliation(
        &self,
        mut embedded_standards: Vec<StandardSelection>,
        catalog_check_ids: &BTreeSet<CheckId>,
        mut catalog_checks: Vec<VerificationCheck>,
        mut scan_summary: ProjectScanSummary,
    ) -> Result<Self, DomainError> {
        self.ensure_no_pending_amendment("reconcile standards")?;
        if !matches!(self.status, PlanStatus::Draft | PlanStatus::Ready) {
            return Err(self.invalid_transition("reconcile standards"));
        }
        scan_summary.validate()?;
        if let Some(previous) = self.project_scan_summary()? {
            scan_summary.preserve_acceptance_from(&previous);
        }
        if embedded_standards
            .iter()
            .any(|standard| standard.source() != "embedded")
            || embedded_standards
                .iter()
                .map(StandardSelection::package_id)
                .collect::<BTreeSet<_>>()
                .len()
                != embedded_standards.len()
        {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                "Reconciled embedded standards must have unique package identifiers",
            ));
        }
        if catalog_checks
            .iter()
            .any(|check| !catalog_check_ids.contains(check.id()))
            || catalog_checks
                .iter()
                .map(VerificationCheck::id)
                .collect::<BTreeSet<_>>()
                .len()
                != catalog_checks.len()
        {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                "Reconciled catalog checks must have unique catalog-owned identifiers",
            ));
        }
        if self.tasks.iter().any(|task| {
            task.verification_checks()
                .iter()
                .any(|check| catalog_check_ids.contains(check.id()))
        }) {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                "Catalog-owned verification checks must remain global",
            ));
        }

        let mut standards = self
            .standards
            .iter()
            .filter(|standard| standard.source() != "embedded")
            .cloned()
            .collect::<Vec<_>>();
        standards.append(&mut embedded_standards);
        standards.sort_by(|left, right| {
            left.package_id()
                .cmp(right.package_id())
                .then_with(|| left.source().cmp(right.source()))
        });
        if standards
            .iter()
            .map(StandardSelection::package_id)
            .collect::<BTreeSet<_>>()
            .len()
            != standards.len()
        {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                "Reconciled standards collide with a non-embedded package selection",
            ));
        }

        let current_catalog_checks = self
            .global_verification
            .iter()
            .filter(|check| catalog_check_ids.contains(check.id()))
            .map(|check| (check.id().clone(), check))
            .collect::<BTreeMap<_, _>>();
        let mut global_verification = self
            .global_verification
            .iter()
            .filter(|check| !catalog_check_ids.contains(check.id()))
            .cloned()
            .collect::<Vec<_>>();
        catalog_checks.sort_by(|left, right| left.id().cmp(right.id()));
        for check in catalog_checks {
            let retained = current_catalog_checks.get(check.id()).filter(|current| {
                current.command() == check.command()
                    && current.cwd() == check.cwd()
                    && current.expected_exit_code() == check.expected_exit_code()
                    && current.is_required() == check.is_required()
            });
            global_verification.push(retained.map_or(check, |current| (*current).clone()));
        }
        global_verification.sort_by(|left, right| left.id().cmp(right.id()));

        let mut candidate = self.clone();
        candidate.standards = standards;
        candidate.global_verification = global_verification;
        candidate.store_project_scan_summary(&scan_summary)?;
        Ok(candidate)
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

    /// Reopens one completed task after required final verification fails.
    ///
    /// # Errors
    ///
    /// Returns an error unless the plan is In Progress, every task is Done,
    /// at least one required global check is Failed, and the selected task is
    /// a completed task with a non-empty rework reason.
    pub fn rework_failed_global_verification(
        &mut self,
        task_id: &TaskId,
        reason: &str,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        self.ensure_no_pending_amendment("rework failed final verification")?;
        self.require_status(PlanStatus::InProgress, "rework failed final verification")?;
        if self.running_check_count() != 0 {
            return Err(DomainError::new(
                DomainErrorKind::InvalidTransition,
                "Final verification rework cannot start while a check is Running",
            ));
        }
        if self
            .tasks
            .iter()
            .any(|task| task.status() != TaskStatus::Done)
        {
            return Err(DomainError::new(
                DomainErrorKind::InvalidTransition,
                "Final verification rework requires every task to be Done",
            ));
        }
        if !self
            .global_verification
            .iter()
            .any(|check| check.is_required() && check.status() == CheckStatus::Failed)
        {
            return Err(DomainError::new(
                DomainErrorKind::InvalidTransition,
                "Final verification rework requires a failed required global check",
            ));
        }
        if self.task(task_id).is_none() {
            return Err(DomainError::new(
                DomainErrorKind::TaskNotFound,
                format!("Task {task_id} does not exist"),
            ));
        }
        let mut candidate = self.clone();
        candidate
            .task_mut(task_id)?
            .reopen_after_global_failure(reason)?;
        for check in &mut candidate.global_verification {
            check.reset_for_rework();
        }
        let mut workspace = candidate.workspace_state()?;
        workspace.remove_task_baseline(task_id);
        candidate.store_workspace_state(&workspace)?;
        candidate.record_revision(candidate.next_revision()?, updated_at);
        candidate.validate_invariants()?;
        *self = candidate;
        Ok(())
    }

    /// Marks passing verification checks stale after workspace content drift.
    ///
    /// # Errors
    ///
    /// Returns an error unless every identifier names a currently Passed check
    /// in an executable or review-state plan.
    pub fn mark_checks_stale(
        &mut self,
        check_ids: &[CheckId],
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        if check_ids.is_empty()
            || !matches!(
                self.status,
                PlanStatus::InProgress | PlanStatus::Blocked | PlanStatus::Review
            )
        {
            return Err(self.invalid_transition("mark verification checks stale"));
        }
        let mut candidate = self.clone();
        let mut reopened_task = false;
        for check_id in check_ids {
            if let Some(task_index) = candidate.tasks.iter().position(|task| {
                task.verification_checks()
                    .iter()
                    .any(|check| check.id() == check_id)
            }) {
                reopened_task |= candidate.tasks[task_index].mark_check_stale(check_id)?;
            } else {
                candidate
                    .global_verification
                    .iter_mut()
                    .find(|check| check.id() == check_id)
                    .ok_or_else(|| {
                        DomainError::new(
                            DomainErrorKind::InvariantViolation,
                            format!("Check {check_id} does not exist"),
                        )
                    })?
                    .mark_stale()?;
            }
        }
        if reopened_task {
            for check in &mut candidate.global_verification {
                check.reset_for_rework();
            }
        }
        if candidate.status != PlanStatus::InProgress {
            candidate.status = PlanStatus::InProgress;
            candidate.resume_status = None;
            candidate.blocker = None;
        }
        candidate.record_revision(candidate.next_revision()?, updated_at);
        candidate.validate_invariants()?;
        *self = candidate;
        Ok(())
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

    /// Records an explicitly approved exception for one required commit gate.
    ///
    /// # Errors
    ///
    /// Returns an error unless the plan is In Progress and the task is Done
    /// with a pending or blocked required commit gate.
    pub fn skip_task_commit(
        &mut self,
        task_id: &TaskId,
        evidence_id: EvidenceId,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        self.require_status(PlanStatus::InProgress, "skip a task commit")?;
        let next_revision = self.next_revision()?;
        self.task_mut(task_id)?.skip_commit(evidence_id)?;
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
        self.ensure_no_pending_amendment("block")?;
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

    /// Records the required user-visible outcome after final verification passes.
    ///
    /// # Errors
    ///
    /// Returns an error unless execution has complete task, commit, and global
    /// verification gates, or a legacy Review is being repaired.
    pub fn set_final_outcome(
        &mut self,
        summary: String,
        remaining_risk: String,
        follow_up_tasks: Vec<String>,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        if !(matches!(self.status, PlanStatus::InProgress | PlanStatus::Review)
            || self.status == PlanStatus::Blocked && self.resume_status == Some(PlanStatus::Review))
        {
            return Err(DomainError::new(
                DomainErrorKind::InvalidTransition,
                "Final Outcome can be set only after execution or during Review",
            ));
        }
        if self.tasks.iter().any(|task| {
            task.status() != TaskStatus::Done
                || task
                    .commit_gate()
                    .is_some_and(|gate| gate.is_required() && !gate.is_satisfied())
        }) || !self.global_verification_is_satisfied()
        {
            return Err(DomainError::new(
                DomainErrorKind::InvalidTransition,
                "Final Outcome requires complete tasks, commit gates, and global verification",
            ));
        }
        let mut candidate = self.clone();
        candidate
            .final_outcome
            .set(summary, remaining_risk, follow_up_tasks)?;
        candidate.record_revision(candidate.next_revision()?, updated_at);
        candidate.validate_invariants()?;
        *self = candidate;
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
                .is_some_and(|gate| gate.is_required() && !gate.is_satisfied())
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
        if !self.final_outcome.is_complete() {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                "Final Outcome requires a summary and explicit remaining risk before review",
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
        if classification == ReviewClassification::FollowUp {
            self.final_outcome
                .add_review_follow_up(&review_id, &feedback_for_state)?;
        }
        if classification == ReviewClassification::MaterialChange {
            self.resume_status = Some(PlanStatus::Review);
            self.status = PlanStatus::Blocked;
            self.blocker = Some(feedback_for_state);
        }
        self.record_revision(next_revision, updated_at);
        self.validate_invariants()?;
        Ok(review_id)
    }

    /// Records the explicit product decision for one blocked Material review item.
    ///
    /// # Errors
    ///
    /// Returns an error unless the plan is blocked by the named undecided Material
    /// review item and the actor, reference, and reason are complete.
    pub fn dispose_material_review(
        &mut self,
        review_id: &str,
        disposition: MaterialReviewDisposition,
        actor: String,
        reference: String,
        reason: String,
        updated_at: Timestamp,
    ) -> Result<(), DomainError> {
        if !self.is_blocked_for_material_review() {
            return Err(DomainError::new(
                DomainErrorKind::InvalidTransition,
                "The plan is not blocked by Material review feedback",
            ));
        }
        let item_index = self.review_item_index(review_id)?;
        let feedback = self.review_items[item_index].feedback().to_owned();
        let mut candidate = self.clone();
        candidate.review_items[item_index].dispose_material_change(
            disposition,
            actor,
            reference,
            reason,
            updated_at.clone(),
        )?;
        if disposition != MaterialReviewDisposition::AcceptChange {
            candidate.status = PlanStatus::Review;
            candidate.resume_status = None;
            candidate.blocker = None;
        }
        if disposition == MaterialReviewDisposition::DeferToFollowUp {
            candidate.follow_ups.push(feedback.clone());
            candidate
                .final_outcome
                .add_review_follow_up(review_id, &feedback)?;
        }
        candidate.record_revision(candidate.next_revision()?, updated_at);
        candidate.validate_invariants()?;
        *self = candidate;
        Ok(())
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
        self.final_outcome.invalidate();
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
                .is_some_and(|gate| gate.is_required() && !gate.is_satisfied())
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
        if !self.final_outcome.is_complete() {
            return Err(DomainError::new(
                DomainErrorKind::InvalidTransition,
                "Review acceptance requires a complete Final Outcome",
            ));
        }
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
                || task
                    .commit_gate()
                    .is_some_and(|gate| gate.is_required() && !gate.is_satisfied())
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
        if let Some(lineage) = &self.lineage {
            lineage.validate()?;
            if lineage.parent_plan_id() == &self.id {
                return Err(DomainError::new(
                    DomainErrorKind::InvariantViolation,
                    "Plan lineage cannot identify the plan as its own parent",
                ));
            }
        }
        if let Some(archive) = &self.archive {
            archive.validate()?;
        }
        self.validate_required_fields()?;
        self.validate_task_graph()?;
        self.validate_lifecycle()?;
        self.validate_approval_state()?;
        self.validate_global_verification()?;
        self.validate_amendment_state()?;
        self.final_outcome.validate()?;
        if let Some(summary) = self.project_scan_summary()? {
            summary.validate()?;
        }
        self.validate_review_state()?;
        self.validate_execution_state()?;
        let task_ids = self.tasks.iter().map(Task::id).collect::<BTreeSet<_>>();
        self.workspace_state()?.validate(&task_ids)?;
        self.standards_conflict_state()?.validate()?;
        Ok(())
    }

    fn store_standards_conflicts(
        &mut self,
        state: &StandardsConflictState,
    ) -> Result<(), DomainError> {
        if state.is_empty() {
            self.extensions.remove(STANDARDS_CONFLICT_EXTENSION_KEY);
            return Ok(());
        }
        let value = serde_json::to_value(state).map_err(|error| {
            DomainError::new(
                DomainErrorKind::InvariantViolation,
                format!("Failed to encode standards conflict extension: {error}"),
            )
        })?;
        self.extensions
            .insert(STANDARDS_CONFLICT_EXTENSION_KEY.to_owned(), value);
        Ok(())
    }

    fn store_project_scan_summary(
        &mut self,
        summary: &ProjectScanSummary,
    ) -> Result<(), DomainError> {
        summary.validate()?;
        let value = serde_json::to_value(summary).map_err(|error| {
            DomainError::new(
                DomainErrorKind::InvariantViolation,
                format!("Failed to encode project scan extension: {error}"),
            )
        })?;
        self.extensions
            .insert(PROJECT_SCAN_EXTENSION_KEY.to_owned(), value);
        Ok(())
    }

    fn store_workspace_state(&mut self, state: &WorkspaceProtocolState) -> Result<(), DomainError> {
        let value = serde_json::to_value(state).map_err(|error| {
            DomainError::new(
                DomainErrorKind::InvariantViolation,
                format!("Failed to encode workspace extension: {error}"),
            )
        })?;
        self.extensions
            .insert(WORKSPACE_EXTENSION_KEY.to_owned(), value);
        Ok(())
    }

    fn store_execution_state(&mut self, state: &ExecutionState) -> Result<(), DomainError> {
        if state.is_empty() {
            self.extensions.remove(EXECUTION_EXTENSION_KEY);
            return Ok(());
        }
        let value = serde_json::to_value(state).map_err(|error| {
            DomainError::new(
                DomainErrorKind::InvariantViolation,
                format!("Failed to encode execution extension: {error}"),
            )
        })?;
        self.extensions
            .insert(EXECUTION_EXTENSION_KEY.to_owned(), value);
        Ok(())
    }

    fn require_active_deviation_task(
        &self,
        task_id: &TaskId,
        action: &'static str,
    ) -> Result<(), DomainError> {
        self.require_status(PlanStatus::InProgress, action)?;
        if self
            .task(task_id)
            .is_some_and(|task| task.status() == TaskStatus::InProgress)
        {
            Ok(())
        } else {
            Err(DomainError::new(
                DomainErrorKind::InvalidTransition,
                format!("Task {task_id} is not the active In Progress task"),
            ))
        }
    }

    fn dispose_active_deviation<F>(
        &mut self,
        deviation_id: &str,
        updated_at: Timestamp,
        disposition: F,
    ) -> Result<(), DomainError>
    where
        F: FnOnce(&mut ExecutionState, Timestamp) -> Result<(), DomainError>,
    {
        let execution = self.execution_state()?;
        let task_id = execution
            .deviation(deviation_id)
            .ok_or_else(|| {
                DomainError::new(
                    DomainErrorKind::InvariantViolation,
                    format!("Deviation {deviation_id} does not exist"),
                )
            })?
            .task_id()
            .clone();
        self.require_active_deviation_task(&task_id, "dispose a deviation")?;
        let next_revision = self.next_revision()?;
        let mut candidate = self.clone();
        let mut execution = candidate.execution_state()?;
        disposition(&mut execution, updated_at.clone())?;
        candidate.store_execution_state(&execution)?;
        candidate.record_revision(next_revision, updated_at);
        candidate.validate_invariants()?;
        *self = candidate;
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
                        })
                        && !self.is_blocked_for_material_amendment())
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
        let task_check_ids = self
            .tasks
            .iter()
            .flat_map(Task::verification_checks)
            .map(VerificationCheck::id)
            .collect::<Vec<_>>();
        let unique_task_check_ids = task_check_ids.iter().copied().collect::<BTreeSet<_>>();
        if unique_task_check_ids.len() != task_check_ids.len()
            || global_check_ids
                .iter()
                .any(|check_id| unique_task_check_ids.contains(check_id))
        {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                "Task and global verification identifiers must be unique across the plan",
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
        for source in self.final_outcome.follow_up_sources() {
            let item = self.review_item(source.review_id()).ok_or_else(|| {
                DomainError::new(
                    DomainErrorKind::InvariantViolation,
                    format!(
                        "Final Outcome follow-up source {} does not exist",
                        source.review_id()
                    ),
                )
            })?;
            let is_sourced_follow_up = (item.classification() == ReviewClassification::FollowUp
                && item.status() == ReviewStatus::Deferred)
                || (item.classification() == ReviewClassification::MaterialChange
                    && item.status() == ReviewStatus::Deferred
                    && item.disposition() == Some(MaterialReviewDisposition::DeferToFollowUp));
            if !is_sourced_follow_up || item.feedback() != source.task() {
                return Err(DomainError::new(
                    DomainErrorKind::InvariantViolation,
                    format!(
                        "Final Outcome follow-up source {} does not match its review item",
                        source.review_id()
                    ),
                ));
            }
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
        if item.superseded_by_change().is_some() {
            if item.classification() == ReviewClassification::InScopeRework
                && let Some(task_id) = item.linked_task()
                && !reserved_tasks.insert(task_id.clone())
            {
                return Err(DomainError::new(
                    DomainErrorKind::InvariantViolation,
                    format!("Review task identifier {task_id} is reserved more than once"),
                ));
            }
            return Ok(());
        }
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

    fn amendment_impact(
        &self,
        operations: &[AmendmentOperation],
        classification: AmendmentClassification,
    ) -> Result<AmendmentImpact, DomainError> {
        let mut affected_fields = BTreeSet::new();
        let mut affected_tasks = BTreeSet::new();
        let mut affected_checks = BTreeSet::new();
        let mut stale_evidence = BTreeSet::new();
        let mut preview = self.clone();
        let mut next_task_id = self.next_task_id()?;
        for operation in operations {
            if let AmendmentOperation::AddTask { task } = operation {
                if task.id.as_ref() != Some(&next_task_id) {
                    return Err(DomainError::new(
                        DomainErrorKind::InvariantViolation,
                        format!("Expected amended task ID {next_task_id}"),
                    ));
                }
                next_task_id = increment_task_id(&next_task_id)?;
            }
            preview.apply_amendment_operation(operation.clone())?;
            match operation {
                AmendmentOperation::AddTaskFile { task_id, path, .. }
                | AmendmentOperation::ExpandTaskFile { task_id, path, .. } => {
                    let task = self.amendment_target_task(task_id, classification)?;
                    if task.file_map().iter().any(|entry| entry.path() == path) {
                        return Err(DomainError::new(
                            DomainErrorKind::InvariantViolation,
                            format!("Task {task_id} already owns file path {path}"),
                        ));
                    }
                    affected_tasks.insert(task_id.clone());
                    affected_fields.insert("approach.file_map".to_owned());
                    affected_fields.insert(format!("tasks.{task_id}.file_map"));
                    affected_fields.insert(format!("tasks.{task_id}.commit_gate.scope"));
                }
                AmendmentOperation::ReplaceTaskVerification {
                    task_id, check_id, ..
                } => {
                    let task = self.amendment_target_task(task_id, classification)?;
                    let check = task
                        .verification_checks()
                        .iter()
                        .find(|check| check.id() == check_id)
                        .ok_or_else(|| {
                            DomainError::new(
                                DomainErrorKind::InvariantViolation,
                                format!("Task {task_id} has no check {check_id}"),
                            )
                        })?;
                    affected_tasks.insert(task_id.clone());
                    affected_checks.insert(check_id.clone());
                    stale_evidence.extend(check.evidence_refs().iter().cloned());
                    affected_fields
                        .insert(format!("tasks.{task_id}.verification_checks.{check_id}"));
                }
                AmendmentOperation::AddImplementationNote { task_id, .. } => {
                    self.amendment_target_task(task_id, classification)?;
                    affected_tasks.insert(task_id.clone());
                    affected_fields.insert(format!("tasks.{task_id}.implementation_notes"));
                }
                AmendmentOperation::AddTask { task } => {
                    let task_id = task.id.as_ref().expect("validated AddTask ID exists");
                    affected_tasks.insert(task_id.clone());
                    affected_fields.extend([
                        "approach.file_map".to_owned(),
                        "task_order".to_owned(),
                        "tasks".to_owned(),
                        format!("tasks.{task_id}"),
                    ]);
                }
                AmendmentOperation::UpdateTaskDefinition { task_id, .. } => {
                    affected_tasks.insert(task_id.clone());
                    affected_fields.extend([
                        format!("tasks.{task_id}.title"),
                        format!("tasks.{task_id}.steps"),
                    ]);
                }
                AmendmentOperation::RemoveTask { task_id } => {
                    affected_tasks.insert(task_id.clone());
                    affected_fields.extend([
                        "approach.file_map".to_owned(),
                        "extensions.workspace.task_baselines".to_owned(),
                        "task_order".to_owned(),
                        "tasks".to_owned(),
                        format!("tasks.{task_id}"),
                    ]);
                }
                AmendmentOperation::ReplaceTaskDependencies {
                    task_id,
                    depends_on: _,
                } => {
                    affected_tasks.insert(task_id.clone());
                    affected_fields.insert(format!("tasks.{task_id}.depends_on"));
                }
                AmendmentOperation::AddCriterion { task_id, criterion } => {
                    affected_tasks.insert(task_id.clone());
                    let criterion_id = criterion
                        .id
                        .as_ref()
                        .expect("validated criterion ID exists");
                    affected_fields.insert(format!(
                        "tasks.{task_id}.acceptance_criteria.{criterion_id}"
                    ));
                }
                AmendmentOperation::UpdateCriterion {
                    task_id,
                    criterion_id,
                    ..
                }
                | AmendmentOperation::RemoveCriterion {
                    task_id,
                    criterion_id,
                } => {
                    affected_tasks.insert(task_id.clone());
                    affected_fields.insert(format!(
                        "tasks.{task_id}.acceptance_criteria.{criterion_id}"
                    ));
                }
                AmendmentOperation::AddTaskVerification {
                    task_id,
                    verification,
                } => {
                    affected_tasks.insert(task_id.clone());
                    affected_checks.insert(verification.id.clone());
                    affected_fields.insert(format!(
                        "tasks.{task_id}.verification_checks.{}",
                        verification.id
                    ));
                }
                AmendmentOperation::UpdateTaskVerification {
                    task_id, check_id, ..
                }
                | AmendmentOperation::RemoveTaskVerification { task_id, check_id } => {
                    affected_tasks.insert(task_id.clone());
                    affected_checks.insert(check_id.clone());
                    affected_fields
                        .insert(format!("tasks.{task_id}.verification_checks.{check_id}"));
                }
                AmendmentOperation::AddGlobalVerification { verification } => {
                    affected_checks.insert(verification.id.clone());
                    affected_fields.insert(format!("verification_plan.{}", verification.id));
                }
                AmendmentOperation::UpdateGlobalVerification { check_id, .. }
                | AmendmentOperation::RemoveGlobalVerification { check_id } => {
                    affected_checks.insert(check_id.clone());
                    affected_fields.insert(format!("verification_plan.{check_id}"));
                }
                AmendmentOperation::ReplaceCommitGate { task_id, .. }
                | AmendmentOperation::RemoveCommitGate { task_id } => {
                    affected_tasks.insert(task_id.clone());
                    affected_fields.insert(format!("tasks.{task_id}.commit_gate"));
                }
                AmendmentOperation::ReplaceSummary { .. } => {
                    affected_fields.insert("summary".to_owned());
                }
                AmendmentOperation::ReplaceScope { .. } => {
                    affected_fields.insert("scope".to_owned());
                }
                AmendmentOperation::ReplaceApproach { .. } => {
                    affected_fields.insert("approach.summary".to_owned());
                }
                AmendmentOperation::ReplaceInterfaces { .. } => {
                    affected_fields.insert("interfaces".to_owned());
                }
                AmendmentOperation::RecordProtectedDecision { .. } => {
                    affected_fields.insert("decisions".to_owned());
                }
                AmendmentOperation::ReplaceTaskOrder { .. } => {
                    affected_fields.insert("task_order".to_owned());
                }
            }
        }
        if classification == AmendmentClassification::Material {
            self.collect_material_amendment_impact(
                &mut affected_fields,
                &mut affected_tasks,
                &mut affected_checks,
                &mut stale_evidence,
            );
            preview.collect_material_amendment_impact(
                &mut affected_fields,
                &mut affected_tasks,
                &mut affected_checks,
                &mut stale_evidence,
            );
        }
        AmendmentImpact::new(
            affected_fields.into_iter().collect(),
            affected_tasks.into_iter().collect(),
            affected_checks.into_iter().collect(),
            stale_evidence.into_iter().collect(),
        )
    }

    fn contextual_amendment_minimum(
        &self,
        operations: &[AmendmentOperation],
        mut minimum: AmendmentClassification,
    ) -> Result<AmendmentClassification, DomainError> {
        for operation in operations {
            if let AmendmentOperation::AddTaskFile { path, .. } = operation
                && let Some(package_id) = required_language_package_for_path(path)
                && !self
                    .standards
                    .iter()
                    .any(|standard| standard.package_id() == package_id)
            {
                minimum = AmendmentClassification::Material;
            }
            if let AmendmentOperation::ReplaceTaskVerification {
                task_id,
                check_id,
                required,
                ..
            } = operation
            {
                let check = self
                    .task(task_id)
                    .and_then(|task| {
                        task.verification_checks()
                            .iter()
                            .find(|check| check.id() == check_id)
                    })
                    .ok_or_else(|| {
                        DomainError::new(
                            DomainErrorKind::InvariantViolation,
                            format!("Task {task_id} has no check {check_id}"),
                        )
                    })?;
                if check.is_required() != *required {
                    minimum = AmendmentClassification::Material;
                }
            }
        }
        Ok(minimum)
    }

    fn collect_material_amendment_impact(
        &self,
        affected_fields: &mut BTreeSet<String>,
        affected_tasks: &mut BTreeSet<TaskId>,
        affected_checks: &mut BTreeSet<CheckId>,
        stale_evidence: &mut BTreeSet<EvidenceId>,
    ) {
        affected_fields.extend([
            "approvals".to_owned(),
            "extensions.execution".to_owned(),
            "git_readiness.approved_at".to_owned(),
            "git_readiness.git_flow_consent".to_owned(),
            "review_items".to_owned(),
            "tasks.status".to_owned(),
            "verification_plan".to_owned(),
        ]);
        for task in &self.tasks {
            affected_tasks.insert(task.id().clone());
            stale_evidence.extend(task.evidence_refs().iter().cloned());
            for criterion in task.acceptance_criteria() {
                stale_evidence.extend(criterion.evidence_refs().iter().cloned());
            }
            for check in task.verification_checks() {
                affected_checks.insert(check.id().clone());
                stale_evidence.extend(check.evidence_refs().iter().cloned());
            }
            if let Some(gate) = task.commit_gate() {
                stale_evidence.extend(gate.evidence_refs().iter().cloned());
            }
        }
        for check in &self.global_verification {
            affected_checks.insert(check.id().clone());
            stale_evidence.extend(check.evidence_refs().iter().cloned());
        }
    }

    fn amendment_target_task(
        &self,
        task_id: &TaskId,
        classification: AmendmentClassification,
    ) -> Result<&Task, DomainError> {
        let task = self.task(task_id).ok_or_else(|| {
            DomainError::new(
                DomainErrorKind::TaskNotFound,
                format!("Task {task_id} does not exist"),
            )
        })?;
        if task.status() == TaskStatus::Draft
            || (classification == AmendmentClassification::Minor
                && !matches!(task.status(), TaskStatus::Ready | TaskStatus::InProgress))
        {
            return Err(DomainError::new(
                DomainErrorKind::InvalidTransition,
                format!("Task {task_id} is not eligible for this amendment classification"),
            ));
        }
        Ok(task)
    }

    fn apply_amendment_operation(
        &mut self,
        operation: AmendmentOperation,
    ) -> Result<(), DomainError> {
        match operation {
            AmendmentOperation::AddTaskFile {
                task_id,
                path,
                change,
                reason,
                ..
            }
            | AmendmentOperation::ExpandTaskFile {
                task_id,
                path,
                change,
                reason,
            } => {
                let entry = FileMapEntry::new(path, change, reason, task_id.clone());
                self.task_mut(&task_id)?.add_amended_file(entry)?;
                self.rebuild_authored_file_map();
            }
            AmendmentOperation::ReplaceTaskVerification {
                task_id,
                check_id,
                command,
                cwd,
                expected_exit_code,
                required,
            } => self.task_mut(&task_id)?.replace_amended_verification(
                &check_id,
                command,
                cwd,
                expected_exit_code,
                required,
            )?,
            AmendmentOperation::AddImplementationNote { task_id, note } => {
                self.task_mut(&task_id)?.add_implementation_note(note)?;
            }
            AmendmentOperation::AddTask { task } => {
                let task_id = task.id.clone().ok_or_else(|| {
                    DomainError::new(
                        DomainErrorKind::InvariantViolation,
                        "Amended task requires an explicit identifier",
                    )
                })?;
                if self.task(&task_id).is_some() {
                    return Err(DomainError::new(
                        DomainErrorKind::DuplicateTask,
                        format!("Task {task_id} already exists"),
                    ));
                }
                let mut task = Task::from_draft(&task_id, task)?;
                task.mark_ready()?;
                self.task_order.push(task_id);
                self.tasks.push(task);
                self.rebuild_authored_file_map();
            }
            AmendmentOperation::UpdateTaskDefinition {
                task_id,
                title,
                steps,
            } => self
                .task_mut(&task_id)?
                .replace_amended_definition(title, steps)?,
            AmendmentOperation::RemoveTask { task_id } => {
                let index = self
                    .tasks
                    .iter()
                    .position(|task| task.id() == &task_id)
                    .ok_or_else(|| {
                        DomainError::new(
                            DomainErrorKind::TaskNotFound,
                            format!("Task {task_id} does not exist"),
                        )
                    })?;
                self.tasks.remove(index);
                self.task_order.retain(|candidate| candidate != &task_id);
                self.rebuild_authored_file_map();
                if self.extensions.contains_key(WORKSPACE_EXTENSION_KEY) {
                    let mut workspace = self.workspace_state()?;
                    workspace.remove_task_baseline(&task_id);
                    self.store_workspace_state(&workspace)?;
                }
            }
            AmendmentOperation::ReplaceTaskDependencies {
                task_id,
                depends_on,
            } => self
                .task_mut(&task_id)?
                .replace_amended_dependencies(depends_on)?,
            AmendmentOperation::AddCriterion { task_id, criterion } => {
                let expected = self
                    .task(&task_id)
                    .ok_or_else(|| {
                        DomainError::new(
                            DomainErrorKind::TaskNotFound,
                            format!("Task {task_id} does not exist"),
                        )
                    })?
                    .next_criterion_id()?;
                if criterion.id.as_ref() != Some(&expected) {
                    return Err(DomainError::new(
                        DomainErrorKind::InvariantViolation,
                        format!("Expected criterion ID {expected}"),
                    ));
                }
                self.task_mut(&task_id)?
                    .add_amended_criterion(AcceptanceCriterion::new(
                        expected,
                        criterion.description,
                    ))?;
            }
            AmendmentOperation::UpdateCriterion {
                task_id,
                criterion_id,
                description,
            } => self
                .task_mut(&task_id)?
                .update_amended_criterion(&criterion_id, description)?,
            AmendmentOperation::RemoveCriterion {
                task_id,
                criterion_id,
            } => self
                .task_mut(&task_id)?
                .remove_amended_criterion(&criterion_id)?,
            AmendmentOperation::AddTaskVerification {
                task_id,
                verification,
            } => self
                .task_mut(&task_id)?
                .add_amended_verification(verification.into_check())?,
            AmendmentOperation::UpdateTaskVerification {
                task_id,
                check_id,
                verification,
            } => {
                if verification.id != check_id {
                    return Err(DomainError::new(
                        DomainErrorKind::InvariantViolation,
                        "Task verification update cannot change its stable identifier",
                    ));
                }
                self.task_mut(&task_id)?.replace_amended_verification(
                    &check_id,
                    verification.command,
                    verification.cwd,
                    verification.expected_exit_code,
                    verification.required,
                )?;
            }
            AmendmentOperation::RemoveTaskVerification { task_id, check_id } => self
                .task_mut(&task_id)?
                .remove_amended_verification(&check_id)?,
            AmendmentOperation::AddGlobalVerification { verification } => {
                self.add_global_verification_unversioned(verification.into_check())?;
            }
            AmendmentOperation::UpdateGlobalVerification {
                check_id,
                verification,
            } => {
                if verification.id != check_id {
                    return Err(DomainError::new(
                        DomainErrorKind::InvariantViolation,
                        "Global verification update cannot change its stable identifier",
                    ));
                }
                self.global_verification
                    .iter_mut()
                    .find(|check| check.id() == &check_id)
                    .ok_or_else(|| {
                        DomainError::new(
                            DomainErrorKind::InvariantViolation,
                            format!("Global check {check_id} does not exist"),
                        )
                    })?
                    .replace_definition(
                        verification.command,
                        verification.cwd,
                        verification.expected_exit_code,
                        verification.required,
                    )?;
            }
            AmendmentOperation::RemoveGlobalVerification { check_id } => {
                let index = self
                    .global_verification
                    .iter()
                    .position(|check| check.id() == &check_id)
                    .ok_or_else(|| {
                        DomainError::new(
                            DomainErrorKind::InvariantViolation,
                            format!("Global check {check_id} does not exist"),
                        )
                    })?;
                self.global_verification.remove(index);
            }
            AmendmentOperation::ReplaceCommitGate {
                task_id,
                commit_gate,
            } => self
                .task_mut(&task_id)?
                .replace_amended_commit_gate(Some(CommitGate::new(
                    commit_gate.required,
                    commit_gate.planned_message,
                    commit_gate.scope,
                )))?,
            AmendmentOperation::RemoveCommitGate { task_id } => {
                self.task_mut(&task_id)?.replace_amended_commit_gate(None)?;
            }
            AmendmentOperation::ReplaceSummary { summary } => self.summary = summary,
            AmendmentOperation::ReplaceScope {
                goal,
                deliverables,
                in_scope,
                out_of_scope,
            } => {
                self.scope = PlanScope {
                    goal,
                    deliverables,
                    in_scope,
                    out_of_scope,
                };
            }
            AmendmentOperation::ReplaceApproach { approach } => {
                self.approach.summary = approach;
            }
            AmendmentOperation::ReplaceInterfaces { interfaces } => {
                self.interfaces = interfaces;
            }
            AmendmentOperation::RecordProtectedDecision {
                category,
                item,
                decision,
                reason,
            } => self.decisions.push(Decision::new(
                item,
                category.as_str(),
                decision,
                reason,
                "Confirmed",
            )),
            AmendmentOperation::ReplaceTaskOrder { task_order } => {
                self.task_order = task_order;
                self.rebuild_authored_file_map();
            }
        }
        Ok(())
    }

    fn invalidate_plan_approval(&mut self) {
        self.approvals
            .retain(|approval| approval.kind() != ApprovalKind::Plan);
        self.git_readiness.git_flow_consent = GitFlowConsent::Pending;
        self.git_readiness.approved_at = None;
    }

    fn pending_amendment_index(&self, change_id: &str) -> Result<usize, DomainError> {
        self.amendments
            .iter()
            .position(|amendment| amendment.id() == change_id && amendment.is_pending())
            .ok_or_else(|| {
                DomainError::new(
                    DomainErrorKind::InvalidTransition,
                    format!("Amendment {change_id} is not the current pending change"),
                )
            })
    }

    fn dispose_amendment<F>(
        &mut self,
        change_id: &str,
        updated_at: Timestamp,
        disposition: F,
    ) -> Result<(), DomainError>
    where
        F: FnOnce(&mut Amendment, Timestamp) -> Result<(), DomainError>,
    {
        let amendment_index = self.pending_amendment_index(change_id)?;
        let amendment_blocker =
            format!("Material amendment {change_id} requires explicit approval");
        let owns_blocked_state = self.status == PlanStatus::Blocked
            && self.blocker.as_deref() == Some(amendment_blocker.as_str());
        let next_revision = self.next_revision()?;
        let mut candidate = self.clone();
        disposition(
            &mut candidate.amendments[amendment_index],
            updated_at.clone(),
        )?;
        if owns_blocked_state {
            let resume_status = candidate.resume_status.ok_or_else(|| {
                DomainError::new(
                    DomainErrorKind::InvariantViolation,
                    "Material amendment blocked state has no resume status",
                )
            })?;
            if resume_status == PlanStatus::InProgress {
                candidate
                    .tasks
                    .iter_mut()
                    .find(|task| task.status() == TaskStatus::Blocked)
                    .ok_or_else(|| {
                        DomainError::new(
                            DomainErrorKind::InvariantViolation,
                            "Material amendment blocked state has no blocked task",
                        )
                    })?
                    .resume()?;
            }
            candidate.status = resume_status;
            candidate.resume_status = None;
            candidate.blocker = None;
        }
        candidate.record_revision(next_revision, updated_at);
        candidate.validate_invariants()?;
        *self = candidate;
        Ok(())
    }

    fn validate_amendment_state(&self) -> Result<(), DomainError> {
        let mut pending_count = 0_usize;
        let mut previous_base_revision = 0_u64;
        for (index, amendment) in self.amendments.iter().enumerate() {
            amendment.validate()?;
            let expected_number = index.checked_add(1).ok_or_else(|| {
                DomainError::new(
                    DomainErrorKind::InvariantViolation,
                    "Amendment count overflowed",
                )
            })?;
            if amendment.id() != format!("C{expected_number}")
                || amendment.base_revision() <= previous_base_revision
            {
                return Err(DomainError::new(
                    DomainErrorKind::InvariantViolation,
                    "Amendment identifiers and base revisions must be monotonic",
                ));
            }
            previous_base_revision = amendment.base_revision();
            if amendment.is_pending() {
                pending_count = pending_count.checked_add(1).ok_or_else(|| {
                    DomainError::new(
                        DomainErrorKind::InvariantViolation,
                        "Pending amendment count overflowed",
                    )
                })?;
                if index + 1 != self.amendments.len() {
                    return Err(DomainError::new(
                        DomainErrorKind::InvariantViolation,
                        "Only the latest amendment may remain pending",
                    ));
                }
                let expected_revision = amendment
                    .base_revision()
                    .checked_add(match amendment.status() {
                        AmendmentStatus::Approved => 2,
                        AmendmentStatus::Proposed | AmendmentStatus::ApprovalRequired => 1,
                        AmendmentStatus::Rejected
                        | AmendmentStatus::Withdrawn
                        | AmendmentStatus::Cancelled
                        | AmendmentStatus::Applied => 0,
                    })
                    .ok_or_else(|| {
                        DomainError::new(
                            DomainErrorKind::InvariantViolation,
                            "Pending amendment revision overflowed",
                        )
                    })?;
                if self.revision != expected_revision {
                    return Err(DomainError::new(
                        DomainErrorKind::InvariantViolation,
                        "A pending amendment must be the only intervening semantic change",
                    ));
                }
            }
        }
        if pending_count > 1 {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                "A plan may contain at most one pending amendment",
            ));
        }
        if let Some(amendment) = self.pending_amendment() {
            match amendment.classification() {
                AmendmentClassification::Minor
                    if !matches!(self.status, PlanStatus::Ready | PlanStatus::InProgress) =>
                {
                    return Err(DomainError::new(
                        DomainErrorKind::InvariantViolation,
                        "A pending Minor amendment requires Ready or In Progress",
                    ));
                }
                AmendmentClassification::Material if !self.is_blocked_for_material_amendment() => {
                    return Err(DomainError::new(
                        DomainErrorKind::InvariantViolation,
                        "A pending Material amendment must own the Blocked state",
                    ));
                }
                AmendmentClassification::Minor | AmendmentClassification::Material => {}
            }
        }
        Ok(())
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
        self.ensure_no_pending_amendment(action)?;
        if self.status == required {
            Ok(())
        } else {
            Err(self.invalid_transition(action))
        }
    }

    fn ensure_no_pending_amendment(&self, action: &'static str) -> Result<(), DomainError> {
        if let Some(amendment) = self.pending_amendment() {
            Err(DomainError::new(
                if amendment.classification() == AmendmentClassification::Material {
                    DomainErrorKind::ApprovalRequired
                } else {
                    DomainErrorKind::InvalidTransition
                },
                format!(
                    "Plan {} cannot {action} while amendment {} awaits apply",
                    self.id,
                    amendment.id()
                ),
            ))
        } else {
            Ok(())
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

fn increment_task_id(task_id: &TaskId) -> Result<TaskId, DomainError> {
    let number = task_id
        .as_str()
        .strip_prefix('T')
        .and_then(|value| value.parse::<u64>().ok())
        .and_then(|value| value.checked_add(1))
        .ok_or_else(|| {
            DomainError::new(
                DomainErrorKind::InvariantViolation,
                "Task identifier overflowed",
            )
        })?;
    TaskId::parse(format!("T{number}"))
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

fn one_based_index(position: usize, length: usize, collection: &str) -> Result<usize, DomainError> {
    let index = position.checked_sub(1).ok_or_else(|| {
        DomainError::new(
            DomainErrorKind::InvariantViolation,
            format!("{collection} position must be one-based"),
        )
    })?;
    if index < length {
        Ok(index)
    } else {
        Err(DomainError::new(
            DomainErrorKind::InvariantViolation,
            format!("{collection} position {position} does not exist"),
        ))
    }
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
