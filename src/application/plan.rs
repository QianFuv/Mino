//! Revision-checked plan authoring over the recoverable store and managed projection.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::domain::{
    CheckId, CriterionId, DraftContextInput, DraftCriterionInput, DraftDecisionInput,
    DraftEdgeCaseInput, DraftFileInput, DraftMetadataInput, DraftPlanInput, DraftScopeInput,
    DraftTaskInput, DraftTaskUpdateInput, DraftVerificationInput, Plan, PlanDraftSeed, PlanId,
    PlanStatus, ProjectScanSummary, RequestId, StandardSelection, TaskId, Timestamp,
    VerificationCheck,
};
use crate::managed_fs::{
    ManagedEntryKind, ManagedFsError, ManagedFsErrorKind, ManagedPath, ProjectFs,
};
use crate::project::{
    self, PlanSelectionRequest, PlanSelectionWriteReport, ProjectPlanSelection,
    ProjectPlanSelectionStore,
};
use crate::render::{
    ProjectionStatus, RenderError, RenderErrorKind, RenderedPlan, check_managed_projection,
    render_plan, write_managed_projection,
};
use crate::standards::{EmbeddedCatalog, SystemToolProbe, apply_recommendation, recommend_initial};
use crate::store::{MutationRequest, PlanStore, StoreError, StoreErrorKind, sha256_digest};
use crate::{ErrorCategory, MinoError, NextAction};

use super::AGENT_EXECUTOR_IDENTITY;
use super::git_readiness::{detect_initial_git_readiness, readiness_block_reason};

/// Maximum UTF-8 request or YAML input accepted by authoring adapters.
pub const MAX_AUTHORING_INPUT_BYTES: usize = 1024 * 1024;

/// Inputs required to initialize a new revision-one Draft.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreatePlanRequest {
    /// Human-readable requirement name.
    pub name: String,
    /// Planning trigger such as `durable`.
    pub trigger: String,
    /// Exact UTF-8 original request.
    pub original_request: String,
    /// Caller-supplied idempotency identifier.
    pub request_id: RequestId,
    /// Actor recorded in the audit event.
    pub actor: String,
    /// Canonical command vector including an input digest.
    pub command: Vec<String>,
    /// Timestamp captured once for deterministic creation.
    pub created_at: Timestamp,
}

/// Common metadata required by every revision-checked semantic plan mutation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlanMutationRequest {
    /// Target plan identifier.
    pub plan_id: PlanId,
    /// Required optimistic-concurrency revision.
    pub expected_revision: u64,
    /// Caller-supplied idempotency identifier.
    pub request_id: RequestId,
    /// Actor recorded in the audit event.
    pub actor: String,
    /// Canonical command vector including input digests where applicable.
    pub command: Vec<String>,
    /// Timestamp captured once for this semantic mutation.
    pub updated_at: Timestamp,
}

/// Backward-compatible name for authored Draft mutation metadata.
pub type DraftMutationRequest = PlanMutationRequest;

/// Canonical authored mutation variants accepted before finalization.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DraftMutation {
    /// Apply a strict batch input.
    Apply(DraftPlanInput),
    /// Replace supplied human metadata fields.
    Metadata(DraftMetadataInput),
    /// Replace the plan summary.
    Summary(String),
    /// Append one current-state reference.
    Context(DraftContextInput),
    /// Replace supplied scope fields.
    Scope(DraftScopeInput),
    /// Append one deliverable.
    AddDeliverable(String),
    /// Append one in-scope boundary.
    AddInScope(String),
    /// Append one out-of-scope boundary.
    AddOutOfScope(String),
    /// Append one decision, assumption, or question.
    Decision(DraftDecisionInput),
    /// Replace the implementation approach.
    Approach(String),
    /// Replace interfaces and data flow.
    Interfaces(String),
    /// Append one edge case.
    EdgeCase(DraftEdgeCaseInput),
    /// Append one deterministically identified task.
    Task(DraftTaskInput),
    /// Append one step to a task.
    TaskStep {
        /// Target task identifier.
        task_id: TaskId,
        /// Ordered implementation step.
        step: String,
    },
    /// Append one deterministically identified task criterion.
    TaskCriterion {
        /// Target task identifier.
        task_id: TaskId,
        /// Authored criterion.
        criterion: DraftCriterionInput,
    },
    /// Append one task-scoped verification command.
    TaskVerification {
        /// Target task identifier.
        task_id: TaskId,
        /// Authored verification command.
        verification: DraftVerificationInput,
    },
    /// Append one task-owned file responsibility.
    File {
        /// Target task identifier.
        task_id: TaskId,
        /// Authored file responsibility.
        file: DraftFileInput,
    },
    /// Append one global verification command.
    GlobalVerification(DraftVerificationInput),
    /// Replace supplied fields on one existing task.
    TaskUpdate {
        /// Target task identifier.
        task_id: TaskId,
        /// Partial task replacement input.
        update: DraftTaskUpdateInput,
    },
    /// Remove one unreferenced task.
    TaskRemove {
        /// Target task identifier.
        task_id: TaskId,
    },
    /// Move one task to a one-based implementation position.
    TaskMove {
        /// Target task identifier.
        task_id: TaskId,
        /// Destination one-based implementation position.
        position: usize,
    },
    /// Replace one one-based task step.
    TaskStepUpdate {
        /// Target task identifier.
        task_id: TaskId,
        /// One-based step position.
        position: usize,
        /// Replacement step text.
        value: String,
    },
    /// Remove one one-based task step.
    TaskStepRemove {
        /// Target task identifier.
        task_id: TaskId,
        /// One-based step position.
        position: usize,
    },
    /// Replace one stable task criterion.
    TaskCriterionUpdate {
        /// Target task identifier.
        task_id: TaskId,
        /// Stable criterion identifier.
        criterion_id: CriterionId,
        /// Replacement criterion description.
        description: String,
    },
    /// Remove one stable task criterion.
    TaskCriterionRemove {
        /// Target task identifier.
        task_id: TaskId,
        /// Stable criterion identifier.
        criterion_id: CriterionId,
    },
    /// Replace one stable task verification.
    TaskVerificationUpdate {
        /// Target task identifier.
        task_id: TaskId,
        /// Stable check identifier.
        check_id: CheckId,
        /// Replacement verification definition.
        verification: DraftVerificationInput,
    },
    /// Remove one stable task verification.
    TaskVerificationRemove {
        /// Target task identifier.
        task_id: TaskId,
        /// Stable check identifier.
        check_id: CheckId,
    },
    /// Replace one one-based task file responsibility.
    FileUpdate {
        /// Target task identifier.
        task_id: TaskId,
        /// One-based file responsibility position.
        position: usize,
        /// Replacement file responsibility.
        file: DraftFileInput,
    },
    /// Remove one one-based task file responsibility.
    FileRemove {
        /// Target task identifier.
        task_id: TaskId,
        /// One-based file responsibility position.
        position: usize,
    },
    /// Replace one stable global verification.
    GlobalVerificationUpdate {
        /// Stable check identifier.
        check_id: CheckId,
        /// Replacement verification definition.
        verification: DraftVerificationInput,
    },
    /// Remove one stable global verification.
    GlobalVerificationRemove {
        /// Stable check identifier.
        check_id: CheckId,
    },
    /// Replace one one-based decision.
    DecisionUpdate {
        /// One-based decision position.
        position: usize,
        /// Replacement decision.
        decision: DraftDecisionInput,
    },
    /// Remove one one-based decision.
    DecisionRemove {
        /// One-based decision position.
        position: usize,
    },
    /// Replace one one-based edge case.
    EdgeCaseUpdate {
        /// One-based edge-case position.
        position: usize,
        /// Replacement edge case.
        edge_case: DraftEdgeCaseInput,
    },
    /// Remove one one-based edge case.
    EdgeCaseRemove {
        /// One-based edge-case position.
        position: usize,
    },
}

impl DraftMutation {
    fn apply(
        &self,
        plan: &mut Plan,
        updated_at: Timestamp,
    ) -> Result<(), crate::domain::DomainError> {
        match self {
            Self::Apply(input) => plan.apply_draft_input(input.clone(), updated_at),
            Self::Metadata(input) => plan.author_metadata(input.clone(), updated_at),
            Self::Summary(summary) => plan.author_summary(summary.clone(), updated_at),
            Self::Context(input) => plan.author_context(input.clone(), updated_at),
            Self::Scope(input) => plan.author_scope(input.clone(), updated_at),
            Self::AddDeliverable(value) => plan.author_deliverable(value.clone(), updated_at),
            Self::AddInScope(value) => plan.author_in_scope(value.clone(), updated_at),
            Self::AddOutOfScope(value) => plan.author_out_of_scope(value.clone(), updated_at),
            Self::Decision(input) => plan.author_decision(input.clone(), updated_at),
            Self::Approach(approach) => plan.author_approach(approach.clone(), updated_at),
            Self::Interfaces(interfaces) => plan.author_interfaces(interfaces.clone(), updated_at),
            Self::EdgeCase(input) => plan.author_edge_case(input.clone(), updated_at),
            Self::Task(input) => plan.author_task(input.clone(), updated_at).map(|_| ()),
            Self::TaskStep { task_id, step } => {
                plan.author_task_step(task_id, step.clone(), updated_at)
            }
            Self::TaskCriterion { task_id, criterion } => plan
                .author_task_criterion(task_id, criterion.clone(), updated_at)
                .map(|_| ()),
            Self::TaskVerification {
                task_id,
                verification,
            } => plan.author_task_verification(task_id, verification.clone(), updated_at),
            Self::File { task_id, file } => plan.author_file(task_id, file.clone(), updated_at),
            Self::GlobalVerification(verification) => {
                plan.author_global_verification(verification.clone(), updated_at)
            }
            Self::TaskUpdate { task_id, update } => {
                plan.author_task_update(task_id, update.clone(), updated_at)
            }
            Self::TaskRemove { task_id } => plan.author_task_remove(task_id, updated_at),
            Self::TaskMove { task_id, position } => {
                plan.author_task_move(task_id, *position, updated_at)
            }
            Self::TaskStepUpdate {
                task_id,
                position,
                value,
            } => plan.author_task_step_update(task_id, *position, value.clone(), updated_at),
            Self::TaskStepRemove { task_id, position } => {
                plan.author_task_step_remove(task_id, *position, updated_at)
            }
            Self::TaskCriterionUpdate {
                task_id,
                criterion_id,
                description,
            } => plan.author_task_criterion_update(
                task_id,
                criterion_id,
                description.clone(),
                updated_at,
            ),
            Self::TaskCriterionRemove {
                task_id,
                criterion_id,
            } => plan.author_task_criterion_remove(task_id, criterion_id, updated_at),
            Self::TaskVerificationUpdate {
                task_id,
                check_id,
                verification,
            } => plan.author_task_verification_update(
                task_id,
                check_id,
                verification.clone(),
                updated_at,
            ),
            Self::TaskVerificationRemove { task_id, check_id } => {
                plan.author_task_verification_remove(task_id, check_id, updated_at)
            }
            Self::FileUpdate {
                task_id,
                position,
                file,
            } => plan.author_file_update(task_id, *position, file.clone(), updated_at),
            Self::FileRemove { task_id, position } => {
                plan.author_file_remove(task_id, *position, updated_at)
            }
            Self::GlobalVerificationUpdate {
                check_id,
                verification,
            } => plan.author_global_verification_update(check_id, verification.clone(), updated_at),
            Self::GlobalVerificationRemove { check_id } => {
                plan.author_global_verification_remove(check_id, updated_at)
            }
            Self::DecisionUpdate { position, decision } => {
                plan.author_decision_update(*position, decision.clone(), updated_at)
            }
            Self::DecisionRemove { position } => plan.author_decision_remove(*position, updated_at),
            Self::EdgeCaseUpdate {
                position,
                edge_case,
            } => plan.author_edge_case_update(*position, edge_case.clone(), updated_at),
            Self::EdgeCaseRemove { position } => {
                plan.author_edge_case_remove(*position, updated_at)
            }
        }
    }

    fn changed_fields(&self) -> Vec<String> {
        match self {
            Self::Apply(_) => vec!["authored_fields".to_owned()],
            Self::Metadata(_) => vec!["metadata".to_owned()],
            Self::Summary(_) => vec!["summary".to_owned()],
            Self::Context(_) => vec!["context".to_owned()],
            Self::Scope(_)
            | Self::AddDeliverable(_)
            | Self::AddInScope(_)
            | Self::AddOutOfScope(_) => vec!["scope".to_owned()],
            Self::Decision(_) | Self::DecisionUpdate { .. } | Self::DecisionRemove { .. } => {
                vec!["decisions".to_owned()]
            }
            Self::Approach(_) => vec!["approach.summary".to_owned()],
            Self::Interfaces(_) => vec!["interfaces".to_owned()],
            Self::EdgeCase(_) | Self::EdgeCaseUpdate { .. } | Self::EdgeCaseRemove { .. } => {
                vec!["edge_cases".to_owned()]
            }
            Self::Task(_) => vec!["tasks".to_owned(), "task_order".to_owned()],
            Self::TaskStep { task_id, .. }
            | Self::TaskStepUpdate { task_id, .. }
            | Self::TaskStepRemove { task_id, .. } => vec![format!("tasks.{task_id}.steps")],
            Self::TaskCriterion { task_id, .. }
            | Self::TaskCriterionUpdate { task_id, .. }
            | Self::TaskCriterionRemove { task_id, .. } => {
                vec![format!("tasks.{task_id}.acceptance_criteria")]
            }
            Self::TaskVerification { task_id, .. }
            | Self::TaskVerificationUpdate { task_id, .. }
            | Self::TaskVerificationRemove { task_id, .. } => {
                vec![format!("tasks.{task_id}.verification_checks")]
            }
            Self::File { task_id, .. }
            | Self::FileUpdate { task_id, .. }
            | Self::FileRemove { task_id, .. } => vec![
                "approach.file_map".to_owned(),
                format!("tasks.{task_id}.file_map"),
            ],
            Self::GlobalVerification(_)
            | Self::GlobalVerificationUpdate { .. }
            | Self::GlobalVerificationRemove { .. } => vec!["verification_plan".to_owned()],
            Self::TaskUpdate { task_id, .. } => vec![format!("tasks.{task_id}")],
            Self::TaskRemove { task_id } => vec![
                "tasks".to_owned(),
                "task_order".to_owned(),
                "approach.file_map".to_owned(),
                format!("tasks.{task_id}"),
            ],
            Self::TaskMove { .. } => {
                vec!["task_order".to_owned(), "approach.file_map".to_owned()]
            }
        }
    }

    fn affects_file_map(&self) -> bool {
        matches!(
            self,
            Self::Apply(_)
                | Self::Task(_)
                | Self::File { .. }
                | Self::TaskUpdate { .. }
                | Self::TaskRemove { .. }
                | Self::FileUpdate { .. }
                | Self::FileRemove { .. }
        )
    }

    fn assigned_id(&self, prior: &Plan) -> Result<Option<String>, MinoError> {
        match self {
            Self::Task(_) => Ok(Some(
                prior
                    .next_task_id()
                    .map_err(|error| map_domain_error(&error))?
                    .to_string(),
            )),
            Self::TaskCriterion { task_id, .. } => {
                let task = prior.task(task_id).ok_or_else(|| {
                    MinoError::new(
                        ErrorCategory::IncompleteOrValidation,
                        format!("Task {task_id} does not exist"),
                    )
                })?;
                Ok(Some(
                    task.next_criterion_id()
                        .map_err(|error| map_domain_error(&error))?
                        .to_string(),
                ))
            }
            _ => Ok(None),
        }
    }
}

/// Digest-bearing result returned after a create or authored mutation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PlanOperationReport {
    /// Stable plan identifier.
    pub plan_id: PlanId,
    /// Current lifecycle status.
    pub status: PlanStatus,
    /// Current optimistic-concurrency revision.
    pub revision: u64,
    /// Canonical source-state digest.
    pub state_hash: String,
    /// Managed Markdown digest.
    pub projection_digest: String,
    /// Whether the request replayed an already committed event.
    pub replayed: bool,
    /// Deterministically assigned task or criterion identifier when applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assigned_id: Option<String>,
}

/// Read-only guidance for the next plan-authoring action.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PlanNextReport {
    /// Stable plan identifier.
    pub plan_id: PlanId,
    /// Current lifecycle status.
    pub status: PlanStatus,
    /// Current optimistic-concurrency revision.
    pub revision: u64,
    /// Deterministically ordered missing authored fields.
    #[serde(skip)]
    pub missing: Vec<String>,
    /// Canonical next actions for the current revision.
    #[serde(skip)]
    pub next_actions: Vec<NextAction>,
}

/// Application boundary for project-local plan authoring.
#[derive(Clone, Debug)]
pub struct PlanService {
    root: PathBuf,
    filesystem: ProjectFs,
    store: PlanStore,
    selection: ProjectPlanSelectionStore,
}

impl PlanService {
    /// Discovers an initialized project and creates its plan service.
    ///
    /// # Errors
    ///
    /// Returns an environment-unavailable error when no initialized project root exists.
    pub fn discover(start: &Path) -> Result<Self, MinoError> {
        let root = project::discover(start)?;
        let filesystem = ProjectFs::open(root.path()).map_err(map_project_fs_error)?;
        Ok(Self {
            root: root.path().to_path_buf(),
            filesystem,
            store: PlanStore::new(root.path()),
            selection: ProjectPlanSelectionStore::new(root.path()),
        })
    }

    /// Returns the discovered project root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Creates or idempotently replays a revision-one Draft and its projection.
    ///
    /// # Errors
    ///
    /// Returns a typed error for invalid input, ID collisions, storage failure,
    /// standards resolution failure, or unsafe/drifted projection paths.
    pub fn create(&self, request: CreatePlanRequest) -> Result<PlanOperationReport, MinoError> {
        validate_create_request(&request)?;
        let plan_id = plan_id_for(&request.name, &request.created_at)?;
        let projection_relative = format!("docs/plan/{plan_id}.md");
        let preflight_projection = self.root.join(&projection_relative);
        let state_path = self.store.paths().current_plan(&plan_id);
        if preflight_projection.exists() && !state_path.exists() {
            return Err(plan_collision_error(&plan_id));
        }
        if !state_path.exists() {
            let selection = self.plan_selection()?;
            if !selection.is_empty() {
                let candidates = selection
                    .candidates()
                    .into_iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(MinoError::new(
                    ErrorCategory::PolicyViolation,
                    format!("Project already has active plan candidates: {candidates}"),
                )
                .with_remediation(
                    vec!["active_plan".to_owned()],
                    vec![NextAction {
                        id: "agent.context".to_owned(),
                        argv: vec![
                            "mino".to_owned(),
                            "agent".to_owned(),
                            "context".to_owned(),
                            "--format".to_owned(),
                            "json".to_owned(),
                            "--no-input".to_owned(),
                        ],
                    }],
                ));
            }
        }
        let proposed = if state_path.exists() {
            self.store
                .load_snapshot(&plan_id, 1)
                .map_err(|error| map_store_error(&error))?
        } else {
            self.build_initial_plan(plan_id.clone(), projection_relative, &request)?
        };
        let receipt = self
            .store
            .create_plan(
                &proposed,
                request.request_id,
                request.actor,
                request.command,
            )
            .map_err(|error| {
                if error.kind() == StoreErrorKind::PlanAlreadyExists {
                    plan_collision_error(&plan_id)
                } else {
                    map_store_error(&error)
                }
            })?;
        let plan = self
            .store
            .load_plan(&plan_id)
            .map_err(|error| map_store_error(&error))?;
        let rendered = render_plan(&plan).map_err(|error| map_render_error(&error))?;
        let managed_projection = projection_managed_path(&plan)?;
        write_managed_projection(&self.filesystem, &managed_projection, &rendered, None)
            .map_err(|error| map_render_error(&error))?;
        let live_plan_ids = self.live_plan_ids()?;
        self.selection.register_created(&plan_id, &live_plan_ids)?;
        Ok(operation_report(
            &plan,
            &rendered,
            receipt.is_replay(),
            None,
        ))
    }

    /// Applies or idempotently replays one canonical Draft mutation.
    ///
    /// # Errors
    ///
    /// Returns a typed error for stale revisions, reused request IDs, illegal
    /// authoring, storage failure, or projection drift.
    pub fn mutate(
        &self,
        request: DraftMutationRequest,
        mutation: &DraftMutation,
    ) -> Result<PlanOperationReport, MinoError> {
        let should_reconcile_readiness = mutation.affects_file_map();
        let mut changed_fields = mutation.changed_fields();
        if should_reconcile_readiness {
            changed_fields.extend([
                "status".to_owned(),
                "resume_status".to_owned(),
                "blocker".to_owned(),
            ]);
        }
        let applied_mutation = mutation.clone();
        self.commit_semantic(
            request,
            changed_fields,
            |prior| mutation.assigned_id(prior),
            move |plan, updated_at| {
                applied_mutation.apply(plan, updated_at)?;
                if should_reconcile_readiness && let Some(state) = plan.git_readiness_state()? {
                    let block_reason = readiness_block_reason(plan, &state);
                    plan.reconcile_git_readiness_block(block_reason)?;
                }
                Ok(())
            },
        )
    }

    /// Records or idempotently replays the required Final Outcome mutation.
    ///
    /// # Errors
    ///
    /// Returns a typed error for stale revisions, incomplete execution gates,
    /// malformed outcome fields, storage failures, or projection drift.
    pub fn set_outcome(
        &self,
        request: PlanMutationRequest,
        summary: String,
        remaining_risk: String,
        follow_up_tasks: Vec<String>,
    ) -> Result<PlanOperationReport, MinoError> {
        self.commit_semantic(
            request,
            vec!["final_outcome".to_owned()],
            |_| Ok(None),
            move |plan, at| {
                plan.set_final_outcome(
                    summary.clone(),
                    remaining_risk.clone(),
                    follow_up_tasks.clone(),
                    at,
                )
            },
        )
    }

    /// Accepts or idempotently replays acceptance of one exact truncated scan.
    ///
    /// # Errors
    ///
    /// Returns a typed error for stale revisions, a complete or already
    /// accepted scan, incomplete audit fields, or persistence/projection failure.
    pub fn accept_scan(
        &self,
        request: PlanMutationRequest,
        decision_reference: String,
        reason: String,
    ) -> Result<PlanOperationReport, MinoError> {
        let actor = request.actor.clone();
        self.commit_semantic(
            request,
            vec!["extensions.project_scan.acceptance".to_owned()],
            |_| Ok(None),
            move |plan, at| {
                plan.accept_project_scan(
                    actor.clone(),
                    decision_reference.clone(),
                    reason.clone(),
                    at,
                )
            },
        )
    }

    /// Commits one retry-safe semantic transition and updates its projection.
    pub(crate) fn commit_semantic<F, A>(
        &self,
        request: PlanMutationRequest,
        changed_fields: Vec<String>,
        assigned_id: A,
        mutation: F,
    ) -> Result<PlanOperationReport, MinoError>
    where
        F: Fn(&mut Plan, Timestamp) -> Result<(), crate::domain::DomainError> + Clone,
        A: FnOnce(&Plan) -> Result<Option<String>, MinoError>,
    {
        let current = self
            .store
            .load_plan(&request.plan_id)
            .map_err(|error| map_store_error(&error))?;
        let target_revision = request.expected_revision.checked_add(1).ok_or_else(|| {
            MinoError::new(
                ErrorCategory::RevisionConflict,
                "Expected revision overflowed",
            )
        })?;
        let is_replay_candidate = current.revision() == target_revision;
        if current.is_archived() && !is_replay_candidate {
            return Err(MinoError::new(
                ErrorCategory::PolicyViolation,
                format!("Plan {} is archived and cannot be mutated", current.id()),
            ));
        }
        let prior = if current.revision() == request.expected_revision {
            current.clone()
        } else if is_replay_candidate {
            self.store
                .load_snapshot(&request.plan_id, request.expected_revision)
                .map_err(|error| map_store_error(&error))?
        } else {
            return Err(MinoError::new(
                ErrorCategory::RevisionConflict,
                format!(
                    "Plan {} is revision {}, not expected revision {}",
                    request.plan_id,
                    current.revision(),
                    request.expected_revision
                ),
            ));
        };
        if !is_replay_candidate {
            let mut preview = prior.clone();
            mutation(&mut preview, request.updated_at.clone())
                .map_err(|error| map_domain_error(&error))?;
        }
        let assigned_id = assigned_id(&prior)?;
        let (managed_projection, prior_for_write) =
            self.projection_before_commit(&prior, &current, is_replay_candidate)?;
        let store_request = MutationRequest::new(
            request.expected_revision,
            request.request_id,
            request.actor,
            request.command,
            changed_fields,
        )
        .map_err(|error| map_store_error(&error))?;
        let updated_at = request.updated_at;
        let applied_mutation = mutation.clone();
        let receipt = self
            .store
            .commit(&request.plan_id, store_request, move |plan| {
                applied_mutation(plan, updated_at)
            })
            .map_err(|error| map_store_error(&error))?;
        let plan = self
            .store
            .load_plan(&request.plan_id)
            .map_err(|error| map_store_error(&error))?;
        let rendered = render_plan(&plan).map_err(|error| map_render_error(&error))?;
        write_managed_projection(
            &self.filesystem,
            &managed_projection,
            &rendered,
            prior_for_write.as_ref(),
        )
        .map_err(|error| map_render_error(&error))?;
        Ok(operation_report(
            &plan,
            &rendered,
            receipt.is_replay(),
            assigned_id,
        ))
    }

    fn projection_before_commit(
        &self,
        prior: &Plan,
        current: &Plan,
        is_replay_candidate: bool,
    ) -> Result<(ManagedPath, Option<RenderedPlan>), MinoError> {
        let prior_rendered = render_plan(prior).map_err(|error| map_render_error(&error))?;
        let managed_projection = projection_managed_path(prior)?;
        let projection_path = self.filesystem.display_path(&managed_projection);
        let prior_projection =
            check_managed_projection(&self.filesystem, &managed_projection, &prior_rendered)
                .map_err(|error| map_render_error(&error))?;
        let prior_for_write = match prior_projection.status() {
            ProjectionStatus::Current => Some(prior_rendered),
            ProjectionStatus::Missing if is_replay_candidate => None,
            ProjectionStatus::Drifted if is_replay_candidate => {
                let current_rendered =
                    render_plan(current).map_err(|error| map_render_error(&error))?;
                let current_projection = check_managed_projection(
                    &self.filesystem,
                    &managed_projection,
                    &current_rendered,
                )
                .map_err(|error| map_render_error(&error))?;
                if current_projection.status() == ProjectionStatus::Current {
                    Some(prior_rendered)
                } else {
                    return Err(projection_drift_error(&projection_path));
                }
            }
            ProjectionStatus::Missing | ProjectionStatus::Drifted => {
                return Err(projection_drift_error(&projection_path));
            }
        };
        Ok((managed_projection, prior_for_write))
    }

    /// Verifies an older semantic phase after later revisions have committed.
    pub(crate) fn replay_semantic(
        &self,
        request: PlanMutationRequest,
        changed_fields: Vec<String>,
    ) -> Result<PlanOperationReport, MinoError> {
        let store_request = MutationRequest::new(
            request.expected_revision,
            request.request_id,
            request.actor,
            request.command,
            changed_fields,
        )
        .map_err(|error| map_store_error(&error))?;
        self.store
            .replay(&request.plan_id, &store_request)
            .map_err(|error| map_store_error(&error))?;
        let plan = self
            .store
            .load_plan(&request.plan_id)
            .map_err(|error| map_store_error(&error))?;
        let rendered = render_plan(&plan).map_err(|error| map_render_error(&error))?;
        let managed_path = projection_managed_path(&plan)?;
        let path = self.filesystem.display_path(&managed_path);
        let projection = check_managed_projection(&self.filesystem, &managed_path, &rendered)
            .map_err(|error| map_render_error(&error))?;
        match projection.status() {
            ProjectionStatus::Current => {}
            ProjectionStatus::Missing => {
                write_managed_projection(&self.filesystem, &managed_path, &rendered, None)
                    .map_err(|error| map_render_error(&error))?;
            }
            ProjectionStatus::Drifted => return Err(projection_drift_error(&path)),
        }
        Ok(operation_report(&plan, &rendered, true, None))
    }

    /// Returns deterministic missing fields and the next canonical action.
    ///
    /// # Errors
    ///
    /// Returns a typed error for missing/corrupt state or projection drift.
    pub fn next(&self, plan_id: &PlanId) -> Result<PlanNextReport, MinoError> {
        let plan = self.load_verified(plan_id)?;
        let missing = draft_missing(&plan);
        let next_actions = draft_next_actions(&plan, &missing);
        Ok(PlanNextReport {
            plan_id: plan.id().clone(),
            status: plan.status(),
            revision: plan.revision(),
            missing,
            next_actions,
        })
    }

    /// Validates one current plan revision without mutating stored state.
    ///
    /// # Errors
    ///
    /// Returns a typed error for missing/corrupt state, projection drift, or
    /// unavailable repository facts required by policy validation.
    pub fn validate(
        &self,
        plan_id: &PlanId,
    ) -> Result<crate::validation::ValidationReport, MinoError> {
        let plan = self.load_verified(plan_id)?;
        crate::validation::validate_plan(&self.root, &plan)
    }

    /// Returns the current project-level selection and every live alternative.
    ///
    /// # Errors
    ///
    /// Returns a typed error for malformed private state, projection drift, or
    /// unsafe/unreadable selection state.
    pub fn plan_selection(&self) -> Result<ProjectPlanSelection, MinoError> {
        let live_plan_ids = self.live_plan_ids()?;
        self.selection.resolve(&live_plan_ids)
    }

    /// Locates the explicitly selected live plan, when one exists.
    ///
    /// Git worktree binding is intentionally not used as a plan-selection
    /// mechanism; it remains an independent Git identity fact.
    ///
    /// # Errors
    ///
    /// Returns a typed error for malformed private state, projection drift, or
    /// unsafe/unreadable selection state.
    pub fn active_plan(&self) -> Result<Option<Plan>, MinoError> {
        let plans = self.live_plans()?;
        let live_plan_ids = plans
            .iter()
            .map(|plan| plan.id().clone())
            .collect::<Vec<_>>();
        let selection = self.selection.resolve(&live_plan_ids)?;
        Ok(selection
            .selected_plan
            .and_then(|selected| plans.into_iter().find(|plan| plan.id() == &selected)))
    }

    /// Changes the selected plan using project-selection optimistic concurrency.
    ///
    /// # Errors
    ///
    /// Returns a typed error for a stale/reused request, unknown or completed
    /// candidate, incomplete approval audit, or persistence failure.
    pub fn select_plan(
        &self,
        request: PlanSelectionRequest,
    ) -> Result<PlanSelectionWriteReport, MinoError> {
        let live_plan_ids = self.live_plan_ids()?;
        self.selection.select(request, &live_plan_ids)
    }

    pub(crate) fn register_fork_selection(
        &self,
        source_plan_id: &PlanId,
    ) -> Result<ProjectPlanSelection, MinoError> {
        let live_plan_ids = self.live_plan_ids()?;
        self.selection.register_fork(source_plan_id, &live_plan_ids)
    }

    pub(crate) fn remove_archived_selection(&self) -> Result<ProjectPlanSelection, MinoError> {
        let live_plan_ids = self.live_plan_ids()?;
        self.selection.remove_archived(&live_plan_ids)
    }

    fn live_plan_ids(&self) -> Result<Vec<PlanId>, MinoError> {
        self.live_plans()
            .map(|plans| plans.into_iter().map(|plan| plan.id().clone()).collect())
    }

    fn live_plans(&self) -> Result<Vec<Plan>, MinoError> {
        let plans_directory = self.store.paths().plans_managed();
        let entries = self
            .filesystem
            .read_directory(&plans_directory)
            .map_err(map_project_fs_error)?;
        let mut plan_ids = Vec::new();
        for entry in entries {
            let name = entry.name.into_string().map_err(|_| {
                MinoError::new(
                    ErrorCategory::DriftDetected,
                    "Plan-state directory contains a non-UTF-8 entry",
                )
            })?;
            if entry.kind != ManagedEntryKind::Directory {
                return Err(MinoError::new(
                    ErrorCategory::DriftDetected,
                    format!("Unexpected file in private plan state: {name}"),
                ));
            }
            let plan_id = PlanId::parse(&name).map_err(|error| {
                MinoError::new(
                    ErrorCategory::DriftDetected,
                    format!("Invalid private plan directory {name}: {error}"),
                )
            })?;
            plan_ids.push(plan_id);
        }
        plan_ids.sort();
        let mut active = Vec::new();
        for plan_id in plan_ids {
            let plan = self.load_verified(&plan_id)?;
            if plan.status() != PlanStatus::Done && !plan.is_archived() {
                active.push(plan);
            }
        }
        Ok(active)
    }

    /// Loads a plan only when its managed Markdown projection is current.
    ///
    /// # Errors
    ///
    /// Returns a typed error for missing/corrupt state or projection drift.
    pub fn load_verified(&self, plan_id: &PlanId) -> Result<Plan, MinoError> {
        let plan = self
            .store
            .load_plan(plan_id)
            .map_err(|error| map_store_error(&error))?;
        let rendered = render_plan(&plan).map_err(|error| map_render_error(&error))?;
        let managed_path = projection_managed_path(&plan)?;
        let path = self.filesystem.display_path(&managed_path);
        let check = check_managed_projection(&self.filesystem, &managed_path, &rendered)
            .map_err(|error| map_render_error(&error))?;
        if check.status() != ProjectionStatus::Current {
            return Err(projection_drift_error(&path));
        }
        Ok(plan)
    }

    /// Loads current machine state without asserting projection availability.
    pub(crate) fn load_stored(&self, plan_id: &PlanId) -> Result<Plan, MinoError> {
        self.store
            .load_plan(plan_id)
            .map_err(|error| map_store_error(&error))
    }

    /// Loads one immutable historical snapshot for retry reconciliation.
    pub(crate) fn load_snapshot(&self, plan_id: &PlanId, revision: u64) -> Result<Plan, MinoError> {
        self.store
            .load_snapshot(plan_id, revision)
            .map_err(|error| map_store_error(&error))
    }

    pub(crate) fn filesystem(&self) -> &ProjectFs {
        &self.filesystem
    }

    fn build_initial_plan(
        &self,
        plan_id: PlanId,
        markdown_path: String,
        request: &CreatePlanRequest,
    ) -> Result<Plan, MinoError> {
        let scan = project::scan(&self.root)?;
        let catalog = EmbeddedCatalog::load()?;
        let recommendation = recommend_initial(&catalog, &scan)?;
        let applied =
            apply_recommendation(&self.root, &catalog, &recommendation, &SystemToolProbe)?;
        let standards = applied
            .standards
            .into_iter()
            .map(|standard| {
                StandardSelection::new(
                    standard.package_id,
                    standard.version,
                    standard.digest,
                    "embedded",
                )
            })
            .collect();
        let verification_plan = applied
            .checks
            .into_iter()
            .map(|check| {
                let id = crate::domain::CheckId::parse(check.id)
                    .map_err(|error| domain_input_error(&error))?;
                Ok(VerificationCheck::new(
                    id,
                    check.argv,
                    path_to_protocol_string(&check.cwd)?,
                    0,
                    check.required,
                ))
            })
            .collect::<Result<Vec<_>, MinoError>>()?;
        let (git_readiness, branch, git_readiness_state) =
            detect_initial_git_readiness(&self.root, request.created_at.clone())?;
        let seed = PlanDraftSeed {
            id: plan_id,
            name: request.name.clone(),
            trigger: request.trigger.clone(),
            original_request: request.original_request.clone(),
            branch,
            markdown_path,
            git_readiness,
            standards,
            verification_plan,
        };
        let mut plan = Plan::from_draft_seed(seed, request.created_at.clone());
        let scan_summary = summarize_project_scan(&scan)?;
        plan.record_initial_project_scan(&scan_summary)
            .map_err(|error| domain_input_error(&error))?;
        plan.record_initial_git_readiness(&git_readiness_state)
            .map_err(|error| domain_input_error(&error))?;
        let block_reason = readiness_block_reason(&plan, &git_readiness_state);
        plan.reconcile_git_readiness_block(block_reason)
            .map_err(|error| domain_input_error(&error))?;
        plan.validate_invariants()
            .map_err(|error| domain_input_error(&error))?;
        Ok(plan)
    }
}

/// Returns deterministic basic Draft completeness paths.
#[must_use]
pub fn draft_missing(plan: &Plan) -> Vec<String> {
    if plan.status() != PlanStatus::Draft {
        return Vec::new();
    }
    let mut missing = Vec::new();
    if plan.scan_is_incomplete() == Ok(true) {
        missing.push("scan.acceptance".to_owned());
    }
    if plan.summary().trim().is_empty() {
        missing.push("summary".to_owned());
    }
    if plan.scope().goal().trim().is_empty() {
        missing.push("scope.goal".to_owned());
    }
    if plan.scope().deliverables().is_empty() {
        missing.push("scope.deliverables".to_owned());
    }
    if plan.scope().in_scope().is_empty() {
        missing.push("scope.in_scope".to_owned());
    }
    if plan.scope().out_of_scope().is_empty() {
        missing.push("scope.out_of_scope".to_owned());
    }
    if plan.approach().summary().trim().is_empty() {
        missing.push("approach".to_owned());
    }
    if plan.tasks().is_empty() {
        missing.push("tasks".to_owned());
    }
    for task in plan.tasks() {
        if task.steps().is_empty() {
            missing.push(format!("tasks.{}.steps", task.id()));
        }
        if task.file_map().is_empty() {
            missing.push(format!("tasks.{}.file_map", task.id()));
        }
        if task.acceptance_criteria().is_empty() {
            missing.push(format!("tasks.{}.acceptance_criteria", task.id()));
        }
        if task.verification_checks().is_empty() {
            missing.push(format!("tasks.{}.verification", task.id()));
        }
        if plan.git_readiness().git_flow_enabled() && task.commit_gate().is_none() {
            missing.push(format!("tasks.{}.commit_gate", task.id()));
        }
    }
    if plan.global_verification().is_empty() {
        missing.push("verification_plan".to_owned());
    }
    missing
}

pub(crate) fn operation_report(
    plan: &Plan,
    rendered: &RenderedPlan,
    replayed: bool,
    assigned_id: Option<String>,
) -> PlanOperationReport {
    PlanOperationReport {
        plan_id: plan.id().clone(),
        status: plan.status(),
        revision: plan.revision(),
        state_hash: rendered.state_hash().to_owned(),
        projection_digest: rendered.projection_digest().to_owned(),
        replayed,
        assigned_id,
    }
}

fn validate_create_request(request: &CreatePlanRequest) -> Result<(), MinoError> {
    if request.name.trim().is_empty()
        || request.trigger.trim().is_empty()
        || request.original_request.trim().is_empty()
        || request.actor.trim().is_empty()
        || request.command.is_empty()
    {
        return Err(MinoError::new(
            ErrorCategory::IncompleteOrValidation,
            "Plan create requires non-empty name, trigger, request, actor, and command",
        ));
    }
    if request.original_request.len() > MAX_AUTHORING_INPUT_BYTES {
        return Err(MinoError::new(
            ErrorCategory::IncompleteOrValidation,
            format!(
                "Original request exceeds the {MAX_AUTHORING_INPUT_BYTES}-byte authoring limit"
            ),
        ));
    }
    Ok(())
}

pub(crate) fn plan_id_for(name: &str, created_at: &Timestamp) -> Result<PlanId, MinoError> {
    let slug = ascii_slug(name).unwrap_or_else(|| hashed_name_slug(name));
    let date = created_at.as_str().get(..10).ok_or_else(|| {
        MinoError::new(
            ErrorCategory::IncompleteOrValidation,
            "Creation timestamp has no calendar date",
        )
    })?;
    PlanId::parse(format!("{date}-{slug}")).map_err(|error| domain_input_error(&error))
}

fn ascii_slug(name: &str) -> Option<String> {
    let mut slug = String::new();
    let mut needs_separator = false;
    for character in name.chars() {
        if character.is_ascii_alphanumeric() {
            if needs_separator && !slug.is_empty() {
                slug.push('-');
            }
            slug.push(character.to_ascii_lowercase());
            needs_separator = false;
        } else if !slug.is_empty() {
            needs_separator = true;
        }
        if slug.len() >= 96 {
            break;
        }
    }
    (!slug.is_empty()).then_some(slug)
}

fn hashed_name_slug(name: &str) -> String {
    let digest = sha256_digest(name.as_bytes());
    format!("plan-{}", &digest["sha256:".len().."sha256:".len() + 8])
}

pub(crate) fn projection_managed_path(plan: &Plan) -> Result<ManagedPath, MinoError> {
    let relative = plan.metadata().markdown_path().ok_or_else(|| {
        MinoError::new(
            ErrorCategory::DriftDetected,
            format!("Plan {} has no managed Markdown path", plan.id()),
        )
    })?;
    let path = Path::new(relative);
    if path.extension().is_none_or(|extension| extension != "md") {
        return Err(MinoError::new(
            ErrorCategory::DriftDetected,
            format!("Plan {} has unsafe Markdown path {relative}", plan.id()),
        ));
    }
    ManagedPath::new(path).map_err(map_project_fs_error)
}

fn map_project_fs_error(error: ManagedFsError) -> MinoError {
    let category = match error.kind() {
        ManagedFsErrorKind::InvalidPath | ManagedFsErrorKind::UnsafeComponent => {
            ErrorCategory::DriftDetected
        }
        ManagedFsErrorKind::Io => ErrorCategory::EnvironmentUnavailable,
    };
    MinoError::new(category, error.into_message())
}

pub(crate) fn draft_next_actions(plan: &Plan, missing: &[String]) -> Vec<NextAction> {
    if plan.status() != PlanStatus::Draft {
        return Vec::new();
    }
    if missing
        .first()
        .is_some_and(|field| field == "scan.acceptance")
    {
        return Vec::new();
    }
    let (id, argv) = if missing.first().is_some_and(|field| field == "summary") {
        (
            "plan.summary.set",
            vec![
                "mino".to_owned(),
                "plan".to_owned(),
                "summary".to_owned(),
                "set".to_owned(),
                "--plan".to_owned(),
                plan.id().to_string(),
                "--stdin".to_owned(),
                "--expect-revision".to_owned(),
                plan.revision().to_string(),
                "--request-id".to_owned(),
                derived_request_id(plan, "plan.summary.set"),
                "--actor".to_owned(),
                AGENT_EXECUTOR_IDENTITY.to_owned(),
                "--format".to_owned(),
                "json".to_owned(),
                "--no-input".to_owned(),
            ],
        )
    } else if missing.is_empty() {
        (
            "plan.validate",
            vec![
                "mino".to_owned(),
                "plan".to_owned(),
                "validate".to_owned(),
                "--plan".to_owned(),
                plan.id().to_string(),
                "--format".to_owned(),
                "json".to_owned(),
                "--no-input".to_owned(),
            ],
        )
    } else {
        (
            "plan.apply",
            vec![
                "mino".to_owned(),
                "plan".to_owned(),
                "apply".to_owned(),
                "--plan".to_owned(),
                plan.id().to_string(),
                "--file".to_owned(),
                "draft.yaml".to_owned(),
                "--expect-revision".to_owned(),
                plan.revision().to_string(),
                "--request-id".to_owned(),
                derived_request_id(plan, "plan.apply"),
                "--actor".to_owned(),
                AGENT_EXECUTOR_IDENTITY.to_owned(),
                "--format".to_owned(),
                "json".to_owned(),
                "--no-input".to_owned(),
            ],
        )
    };
    vec![NextAction {
        id: id.to_owned(),
        argv,
    }]
}

pub(crate) fn summarize_project_scan(
    scan: &crate::project::ProjectScan,
) -> Result<ProjectScanSummary, MinoError> {
    ProjectScanSummary::new(
        scan.digest()?,
        scan.files_scanned,
        scan.directories_excluded,
        scan.symlinks_skipped,
        scan.bytes_read,
        scan.truncated,
        scan.truncation_reasons.clone(),
    )
    .map_err(|error| domain_input_error(&error))
}

pub(crate) fn derived_request_id(plan: &Plan, action: &str) -> String {
    let digest = sha256_digest(format!("{}:{}:{action}", plan.id(), plan.revision()).as_bytes());
    let value = &digest["sha256:".len().."sha256:".len() + 32];
    format!(
        "{}-{}-{}-{}-{}",
        &value[0..8],
        &value[8..12],
        &value[12..16],
        &value[16..20],
        &value[20..32]
    )
}

fn path_to_protocol_string(path: &Path) -> Result<String, MinoError> {
    let value = path.to_str().ok_or_else(|| {
        MinoError::new(
            ErrorCategory::IncompleteOrValidation,
            format!("Verification path {} is not UTF-8", path.display()),
        )
    })?;
    Ok(value.replace('\\', "/"))
}

fn plan_collision_error(plan_id: &PlanId) -> MinoError {
    MinoError::new(
        ErrorCategory::IncompleteOrValidation,
        format!("Managed Markdown already exists for plan ID {plan_id}"),
    )
    .with_remediation(
        vec!["plan_id".to_owned()],
        vec![NextAction {
            id: "plan.create.choose-name".to_owned(),
            argv: vec![
                "mino".to_owned(),
                "plan".to_owned(),
                "create".to_owned(),
                "--help".to_owned(),
            ],
        }],
    )
}

fn projection_drift_error(path: &Path) -> MinoError {
    MinoError::new(
        ErrorCategory::DriftDetected,
        format!(
            "Managed Markdown projection {} is missing or drifted",
            path.display()
        ),
    )
}

fn domain_input_error(error: &crate::domain::DomainError) -> MinoError {
    MinoError::new(ErrorCategory::IncompleteOrValidation, error.to_string())
}

fn map_domain_error(error: &crate::domain::DomainError) -> MinoError {
    let category = match error.kind() {
        crate::domain::DomainErrorKind::ApprovalRequired => ErrorCategory::ApprovalRequired,
        crate::domain::DomainErrorKind::InvalidTransition
        | crate::domain::DomainErrorKind::TaskOrderViolation
        | crate::domain::DomainErrorKind::ActiveTaskExists => ErrorCategory::PolicyViolation,
        crate::domain::DomainErrorKind::InvalidIdentifier
        | crate::domain::DomainErrorKind::InvalidTimestamp
        | crate::domain::DomainErrorKind::UnsupportedSchemaVersion
        | crate::domain::DomainErrorKind::UnsupportedProtocolVersion
        | crate::domain::DomainErrorKind::DuplicateTask
        | crate::domain::DomainErrorKind::TaskNotFound
        | crate::domain::DomainErrorKind::UnmetDependencies
        | crate::domain::DomainErrorKind::InvariantViolation => {
            ErrorCategory::IncompleteOrValidation
        }
    };
    MinoError::new(category, error.to_string())
}

pub(crate) fn map_store_error(error: &StoreError) -> MinoError {
    let category = match error.kind() {
        StoreErrorKind::StaleRevision | StoreErrorKind::RequestConflict => {
            ErrorCategory::RevisionConflict
        }
        StoreErrorKind::PlanAlreadyExists | StoreErrorKind::PlanNotFound => {
            ErrorCategory::IncompleteOrValidation
        }
        StoreErrorKind::InvalidMutation => ErrorCategory::PolicyViolation,
        StoreErrorKind::CorruptState => ErrorCategory::DriftDetected,
        StoreErrorKind::Io
        | StoreErrorKind::Serialization
        | StoreErrorKind::LockTimeout
        | StoreErrorKind::InjectedFailure => ErrorCategory::EnvironmentUnavailable,
    };
    MinoError::new(category, error.to_string())
}

pub(crate) fn map_render_error(error: &RenderError) -> MinoError {
    let category = match error.kind() {
        RenderErrorKind::Drift => ErrorCategory::DriftDetected,
        RenderErrorKind::Io | RenderErrorKind::Serialization => {
            ErrorCategory::EnvironmentUnavailable
        }
    };
    MinoError::new(category, error.to_string())
}
