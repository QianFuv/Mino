//! Historical plan forks, semantic comparison, and non-destructive archive operations.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::application::plan::{
    PlanMutationRequest, PlanOperationReport, PlanService, detect_git_readiness, map_render_error,
    map_store_error, operation_report, plan_id_for, projection_managed_path,
};
use crate::diff::{PlanDiff, diff_plans};
use crate::domain::{Lineage, Plan, PlanId, RequestId, Timestamp};
use crate::render::{render_plan, write_managed_projection};
use crate::store::{PlanStore, StoreErrorKind, canonical_json_bytes, sha256_digest};
use crate::{ErrorCategory, MinoError, NextAction};

/// Complete immutable request for creating one alternative plan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ForkPlanRequest {
    /// Source plan identifier.
    pub source_plan_id: PlanId,
    /// Exact retained source revision.
    pub from_revision: u64,
    /// Human-readable new plan name.
    pub name: String,
    /// Explicit reason for comparing this alternative.
    pub reason: String,
    /// Caller-supplied idempotency identifier.
    pub request_id: RequestId,
    /// Actor recorded in the new plan event.
    pub actor: String,
    /// Canonical command vector.
    pub command: Vec<String>,
    /// Timestamp captured once for the fork attempt.
    pub forked_at: Timestamp,
}

/// Result of creating or exactly replaying one plan fork.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ForkPlanReport {
    /// Normal revision and projection result for the new plan.
    #[serde(flatten)]
    pub operation: PlanOperationReport,
    /// Verified fork provenance stored in the new plan.
    pub lineage: Lineage,
}

/// Application boundary for plan alternatives and archive state.
#[derive(Clone, Debug)]
pub struct PlanVariantService {
    root: PathBuf,
    plans: PlanService,
    store: PlanStore,
}

impl PlanVariantService {
    /// Discovers one initialized project.
    ///
    /// # Errors
    ///
    /// Returns an environment error when no initialized project is discoverable.
    pub fn discover(start: &Path) -> Result<Self, MinoError> {
        let plans = PlanService::discover(start)?;
        let root = plans.root().to_path_buf();
        Ok(Self {
            store: PlanStore::new(&root),
            root,
            plans,
        })
    }

    /// Creates or exactly replays an independent Draft from one audited snapshot.
    ///
    /// # Errors
    ///
    /// Returns a typed error for missing/corrupt snapshots, unsafe projections,
    /// colliding names, malformed requests, or storage publication failures.
    pub fn fork(&self, request: ForkPlanRequest) -> Result<ForkPlanReport, MinoError> {
        validate_fork_request(&request)?;
        let plan_id = plan_id_for(&request.name, &request.forked_at)?;
        let markdown_path = format!("docs/plan/{plan_id}.md");
        let projection = self.root.join(&markdown_path);
        let state_path = self.store.paths().current_plan(&plan_id);
        if projection.exists() && !state_path.exists() {
            return Err(fork_collision_error(&plan_id));
        }
        let proposed = if state_path.exists() {
            self.store
                .load_snapshot(&plan_id, 1)
                .map_err(|error| map_store_error(&error))?
        } else {
            self.store
                .audit(&request.source_plan_id)
                .map_err(|error| map_store_error(&error))?;
            let source = self
                .store
                .load_snapshot(&request.source_plan_id, request.from_revision)
                .map_err(|error| map_store_error(&error))?;
            let source_state_hash = sha256_digest(
                &canonical_json_bytes(&source).map_err(|error| map_store_error(&error))?,
            );
            let (git_readiness, branch) = detect_git_readiness(&self.root);
            Plan::fork_from_snapshot(
                &source,
                plan_id.clone(),
                request.name.clone(),
                request.reason.clone(),
                source_state_hash,
                git_readiness,
                branch,
                markdown_path,
                request.forked_at.clone(),
            )
            .map_err(|error| domain_error(&error))?
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
                    fork_collision_error(&plan_id)
                } else {
                    map_store_error(&error)
                }
            })?;
        let plan = self
            .store
            .load_plan(&plan_id)
            .map_err(|error| map_store_error(&error))?;
        let rendered = render_plan(&plan).map_err(|error| map_render_error(&error))?;
        let projection = projection_managed_path(&plan)?;
        write_managed_projection(self.plans.filesystem(), &projection, &rendered, None)
            .map_err(|error| map_render_error(&error))?;
        let lineage = plan.lineage().cloned().ok_or_else(|| {
            MinoError::new(
                ErrorCategory::DriftDetected,
                format!("Forked plan {plan_id} has no lineage"),
            )
        })?;
        Ok(ForkPlanReport {
            operation: operation_report(&plan, &rendered, receipt.is_replay(), None),
            lineage,
        })
    }

    /// Compares current or historical authored plan values without mutation.
    ///
    /// # Errors
    ///
    /// Returns an error for missing/corrupt plans, projection drift on current
    /// revisions, or unexpected serialization failure.
    pub fn diff(
        &self,
        left_plan_id: &PlanId,
        left_revision: Option<u64>,
        right_plan_id: &PlanId,
        right_revision: Option<u64>,
    ) -> Result<PlanDiff, MinoError> {
        let left = self.load_revision(left_plan_id, left_revision)?;
        let right = self.load_revision(right_plan_id, right_revision)?;
        diff_plans(&left, &right).map_err(|error| {
            MinoError::new(
                ErrorCategory::EnvironmentUnavailable,
                format!("Failed to serialize authored plan values: {error}"),
            )
        })
    }

    /// Records approval-bound semantic deactivation without deleting plan bytes.
    ///
    /// # Errors
    ///
    /// Returns an error for stale revisions, repeated archive, incomplete audit
    /// metadata, projection drift, or storage failure.
    pub fn archive(
        &self,
        request: PlanMutationRequest,
        reason: String,
        approval_reference: String,
    ) -> Result<PlanOperationReport, MinoError> {
        let actor = request.actor.clone();
        self.plans.commit_semantic(
            request,
            vec!["archive".to_owned()],
            |_| Ok(None),
            move |plan, at| {
                plan.archive(
                    reason.clone(),
                    actor.clone(),
                    approval_reference.clone(),
                    at,
                )
            },
        )
    }

    fn load_revision(&self, plan_id: &PlanId, revision: Option<u64>) -> Result<Plan, MinoError> {
        match revision {
            Some(revision) => {
                self.store
                    .audit(plan_id)
                    .map_err(|error| map_store_error(&error))?;
                self.store
                    .load_snapshot(plan_id, revision)
                    .map_err(|error| map_store_error(&error))
            }
            None => self.plans.load_verified(plan_id),
        }
    }
}

fn validate_fork_request(request: &ForkPlanRequest) -> Result<(), MinoError> {
    if request.from_revision == 0
        || request.name.trim().is_empty()
        || request.reason.trim().is_empty()
        || request.actor.trim().is_empty()
        || request.command.is_empty()
    {
        return Err(MinoError::new(
            ErrorCategory::IncompleteOrValidation,
            "Plan fork requires revision, name, reason, actor, and command",
        ));
    }
    Ok(())
}

fn fork_collision_error(plan_id: &PlanId) -> MinoError {
    MinoError::new(
        ErrorCategory::PolicyViolation,
        format!("Plan fork target {plan_id} already exists"),
    )
    .with_remediation(
        vec!["name".to_owned()],
        vec![NextAction {
            id: "plan.fork.choose-name".to_owned(),
            argv: vec![
                "mino".to_owned(),
                "plan".to_owned(),
                "fork".to_owned(),
                "--help".to_owned(),
            ],
        }],
    )
}

fn domain_error(error: &crate::domain::DomainError) -> MinoError {
    MinoError::new(ErrorCategory::IncompleteOrValidation, error.to_string())
}
