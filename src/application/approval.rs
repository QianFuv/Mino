//! Finalization, revision-bound review, read-side show, and approval orchestration.

use serde::Serialize;

use crate::application::plan::{PlanMutationRequest, PlanOperationReport, PlanService};
use crate::domain::{Approval, GitFlowConsent, Plan, PlanId, PlanStatus};
use crate::store::{canonical_json_bytes, sha256_digest};
use crate::validation::{ValidationReport, validate_plan, validation_failure};
use crate::{ErrorCategory, MinoError};

/// Versioned generated plan-review schema identifier.
pub const PLAN_REVIEW_KIND: &str = "mino.plan-review/v1";

/// Revision-bound summary presented immediately before explicit approval.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PlanReviewReport {
    /// Versioned review schema identifier.
    pub review_kind: &'static str,
    /// Reviewed plan identifier.
    pub plan_id: PlanId,
    /// Reviewed lifecycle state.
    pub status: PlanStatus,
    /// Exact reviewed optimistic-concurrency revision.
    pub revision: u64,
    /// Digest of the complete reviewed source-of-truth aggregate.
    pub state_hash: String,
    /// Complete source-of-truth plan covered by this review revision and hash.
    pub reviewed_plan: Plan,
    /// Whether this exact current revision has a plan approval declaration.
    pub approval_recorded: bool,
    /// Whether execution still requires an explicit plan approval declaration.
    pub approval_required: bool,
    /// Audit limitation shown to every reviewer.
    pub approval_notice: &'static str,
}

/// Application boundary for plan finalization, review, show, and approval.
#[derive(Clone, Debug)]
pub struct ApprovalService {
    plans: PlanService,
}

impl ApprovalService {
    /// Creates a lifecycle service over an already discovered plan service.
    #[must_use]
    pub const fn new(plans: PlanService) -> Self {
        Self { plans }
    }

    /// Loads the complete current plan after checking managed projection drift.
    ///
    /// # Errors
    ///
    /// Returns a typed error for missing/corrupt state or projection drift.
    pub fn show(&self, plan_id: &PlanId) -> Result<Plan, MinoError> {
        self.plans.load_verified(plan_id)
    }

    /// Generates a complete revision-bound review summary for a Ready plan.
    ///
    /// # Errors
    ///
    /// Returns a typed error unless the plan and projection are current and the
    /// plan is Ready for the review gate.
    pub fn review(&self, plan_id: &PlanId) -> Result<PlanReviewReport, MinoError> {
        let plan = self.plans.load_verified(plan_id)?;
        if plan.status() != PlanStatus::Ready {
            return Err(MinoError::new(
                ErrorCategory::PolicyViolation,
                format!("Plan {plan_id} must be Ready before review"),
            ));
        }
        review_report(&plan)
    }

    /// Validates and atomically finalizes one complete Draft revision.
    ///
    /// # Errors
    ///
    /// Returns structured validation findings, revision/request conflicts,
    /// illegal transitions, storage failures, or projection drift.
    pub fn finalize(&self, request: PlanMutationRequest) -> Result<PlanOperationReport, MinoError> {
        let current = self.plans.load_verified(&request.plan_id)?;
        if !is_replay_candidate(&current, &request)? {
            let validation = validate_plan(self.plans.root(), &current)?;
            require_valid(&validation)?;
        }
        self.plans.commit_semantic(
            request,
            vec!["status".to_owned(), "tasks.status".to_owned()],
            |_| Ok(None),
            Plan::finalize,
        )
    }

    /// Records an explicit approval and Git Flow consent without changing scope.
    ///
    /// # Errors
    ///
    /// Returns a typed error for an inconsistent consent decision, duplicate or
    /// illegal approval, revision/request conflict, storage failure, or drift.
    pub fn approve(
        &self,
        request: PlanMutationRequest,
        approval_reference: String,
        git_flow_consent: GitFlowConsent,
    ) -> Result<PlanOperationReport, MinoError> {
        let current = self.plans.load_verified(&request.plan_id)?;
        is_replay_candidate(&current, &request)?;
        if git_flow_consent == GitFlowConsent::Approved
            && !current.git_readiness().git_flow_enabled()
        {
            return Err(MinoError::new(
                ErrorCategory::PolicyViolation,
                "Git Flow consent cannot be Approved when Git Flow is disabled",
            ));
        }
        let approval = Approval::plan(
            request.actor.clone(),
            approval_reference,
            request.updated_at.clone(),
            git_flow_consent,
        );
        self.plans.commit_semantic(
            request,
            vec![
                "approvals".to_owned(),
                "git_readiness.git_flow_consent".to_owned(),
                "git_readiness.approved_at".to_owned(),
            ],
            |_| Ok(None),
            move |plan, _| plan.record_approval(approval.clone()),
        )
    }
}

fn is_replay_candidate(current: &Plan, request: &PlanMutationRequest) -> Result<bool, MinoError> {
    let target_revision = request.expected_revision.checked_add(1).ok_or_else(|| {
        MinoError::new(
            ErrorCategory::RevisionConflict,
            "Expected revision overflowed",
        )
    })?;
    if current.revision() == request.expected_revision {
        Ok(false)
    } else if current.revision() == target_revision {
        Ok(true)
    } else {
        Err(MinoError::new(
            ErrorCategory::RevisionConflict,
            format!(
                "Plan {} is revision {}, not expected revision {}",
                request.plan_id,
                current.revision(),
                request.expected_revision
            ),
        ))
    }
}

fn require_valid(report: &ValidationReport) -> Result<(), MinoError> {
    if report.valid {
        Ok(())
    } else {
        Err(validation_failure(report))
    }
}

fn review_report(plan: &Plan) -> Result<PlanReviewReport, MinoError> {
    let state_bytes = canonical_json_bytes(plan).map_err(|error| {
        MinoError::new(
            ErrorCategory::DriftDetected,
            format!("Failed to serialize reviewed plan state: {error}"),
        )
    })?;
    let approval_recorded = plan.has_plan_approval();
    Ok(PlanReviewReport {
        review_kind: PLAN_REVIEW_KIND,
        plan_id: plan.id().clone(),
        status: plan.status(),
        revision: plan.revision(),
        state_hash: sha256_digest(&state_bytes),
        reviewed_plan: plan.clone(),
        approval_recorded,
        approval_required: !approval_recorded,
        approval_notice: "This approval is an auditable declaration, not cryptographic authorization.",
    })
}
