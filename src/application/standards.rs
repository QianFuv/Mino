//! Revision-checked standards-conflict inspection, refresh, and resolution.

use std::path::Path;

use serde::Serialize;

use crate::application::plan::{PlanMutationRequest, PlanOperationReport, PlanService};
use crate::domain::{PlanId, PlanStatus};
use crate::standards::{
    AssessedStandardConflict, LocalStandardsSource, StandardsConflictAssessment,
    assess_standard_conflicts, detect_standard_conflicts,
};
use crate::{ErrorCategory, MinoError};

/// Current live and persisted standards-conflict state for one plan.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct StandardsConflictReport {
    /// Target plan identifier.
    pub plan_id: PlanId,
    /// Current plan lifecycle status.
    pub status: PlanStatus,
    /// Current optimistic-concurrency revision.
    pub revision: u64,
    /// Digest of the complete live conflict set.
    pub source_digest: String,
    /// Optional local declaration identity.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_source: Option<LocalStandardsSource>,
    /// Live conflicts, candidates, decisions, and status.
    pub conflicts: Vec<AssessedStandardConflict>,
    /// Persisted conflict IDs absent from current sources.
    pub stale_conflict_ids: Vec<String>,
    /// Whether every live conflict has a current explicit decision.
    pub resolved: bool,
}

/// Mutation result followed by the current standards-conflict assessment.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct StandardsConflictOperationReport {
    /// Revision and digest result from the normal plan transaction.
    #[serde(flatten)]
    pub operation: PlanOperationReport,
    /// Current post-mutation conflict report.
    pub standards_conflicts: StandardsConflictReport,
}

/// Application boundary for plan-scoped standards conflict decisions.
#[derive(Clone, Debug)]
pub struct StandardsConflictService {
    plans: PlanService,
}

impl StandardsConflictService {
    /// Discovers an initialized project and creates its standards service.
    ///
    /// # Errors
    ///
    /// Returns an environment error when no initialized project can be discovered.
    pub fn discover(start: &Path) -> Result<Self, MinoError> {
        Ok(Self {
            plans: PlanService::discover(start)?,
        })
    }

    /// Returns live candidates and their persisted decision relationship.
    ///
    /// # Errors
    ///
    /// Returns an error for missing/corrupt plan state, projection drift, or
    /// malformed/unavailable standards sources.
    pub fn inspect(&self, plan_id: &PlanId) -> Result<StandardsConflictReport, MinoError> {
        let plan = self.plans.load_verified(plan_id)?;
        report(self.plans.root(), &plan)
    }

    /// Refreshes persisted candidate snapshots from an exact live source digest.
    ///
    /// # Errors
    ///
    /// Returns an error for stale revisions, changed sources, illegal lifecycle
    /// state, a semantic no-op, or storage/projection failure.
    pub fn refresh(
        &self,
        request: PlanMutationRequest,
        expected_source_digest: &str,
    ) -> Result<StandardsConflictOperationReport, MinoError> {
        let current = self.plans.load_verified(&request.plan_id)?;
        let detected = detect_standard_conflicts(self.plans.root(), &current)?;
        require_source_digest(expected_source_digest, &detected.source_digest)?;
        let conflicts = detected.conflicts;
        let operation = self.plans.commit_semantic(
            request,
            vec![
                "extensions.standards_conflicts".to_owned(),
                "approvals".to_owned(),
                "git_readiness.approved_at".to_owned(),
                "git_readiness.git_flow_consent".to_owned(),
            ],
            |_| Ok(None),
            move |plan, at| plan.refresh_standards_conflicts(conflicts.clone(), at),
        )?;
        let standards_conflicts = self.inspect(&operation.plan_id)?;
        Ok(StandardsConflictOperationReport {
            operation,
            standards_conflicts,
        })
    }

    /// Records an explicit candidate choice and non-empty rationale.
    ///
    /// # Errors
    ///
    /// Returns an error for changed/unrefreshed sources, an unknown candidate,
    /// duplicate decision, stale revision, illegal state, or persistence failure.
    pub fn resolve(
        &self,
        request: PlanMutationRequest,
        expected_source_digest: &str,
        conflict_id: &str,
        candidate_id: &str,
        rationale: String,
        decision_reference: String,
    ) -> Result<StandardsConflictOperationReport, MinoError> {
        let current = self.plans.load_verified(&request.plan_id)?;
        let detected = detect_standard_conflicts(self.plans.root(), &current)?;
        require_source_digest(expected_source_digest, &detected.source_digest)?;
        let target_revision = request.expected_revision.checked_add(1).ok_or_else(|| {
            MinoError::new(
                ErrorCategory::RevisionConflict,
                "Expected standards conflict revision overflowed",
            )
        })?;
        if current.revision() != target_revision {
            let assessment = assess_standard_conflicts(
                &detected,
                &current
                    .standards_conflict_state()
                    .map_err(|error| domain_error(&error))?,
            );
            require_unresolved(&assessment, conflict_id, candidate_id)?;
        }
        let conflict_id = conflict_id.to_owned();
        let candidate_id = candidate_id.to_owned();
        let actor = request.actor.clone();
        let operation = self.plans.commit_semantic(
            request,
            vec![
                "extensions.standards_conflicts".to_owned(),
                "approvals".to_owned(),
                "git_readiness.approved_at".to_owned(),
                "git_readiness.git_flow_consent".to_owned(),
            ],
            |_| Ok(None),
            move |plan, at| {
                plan.resolve_standards_conflict(
                    &conflict_id,
                    &candidate_id,
                    rationale.clone(),
                    decision_reference.clone(),
                    actor.clone(),
                    at,
                )
            },
        )?;
        let standards_conflicts = self.inspect(&operation.plan_id)?;
        Ok(StandardsConflictOperationReport {
            operation,
            standards_conflicts,
        })
    }
}

fn report(root: &Path, plan: &crate::domain::Plan) -> Result<StandardsConflictReport, MinoError> {
    let detected = detect_standard_conflicts(root, plan)?;
    let assessment = assess_standard_conflicts(
        &detected,
        &plan
            .standards_conflict_state()
            .map_err(|error| domain_error(&error))?,
    );
    Ok(report_from_assessment(
        plan,
        detected.local_source,
        assessment,
    ))
}

fn report_from_assessment(
    plan: &crate::domain::Plan,
    local_source: Option<LocalStandardsSource>,
    assessment: StandardsConflictAssessment,
) -> StandardsConflictReport {
    let resolved = assessment.is_resolved();
    StandardsConflictReport {
        plan_id: plan.id().clone(),
        status: plan.status(),
        revision: plan.revision(),
        source_digest: assessment.source_digest,
        local_source,
        conflicts: assessment.conflicts,
        stale_conflict_ids: assessment.stale_conflict_ids,
        resolved,
    }
}

fn require_unresolved(
    assessment: &StandardsConflictAssessment,
    conflict_id: &str,
    candidate_id: &str,
) -> Result<(), MinoError> {
    let conflict = assessment
        .conflicts
        .iter()
        .find(|conflict| conflict.conflict.id() == conflict_id)
        .ok_or_else(|| {
            MinoError::new(
                ErrorCategory::PolicyViolation,
                format!("Standards conflict {conflict_id} is not live"),
            )
        })?;
    if conflict.status != crate::standards::StandardConflictStatus::Unresolved {
        return Err(MinoError::new(
            ErrorCategory::PolicyViolation,
            format!(
                "Standards conflict {conflict_id} must be refreshed and unresolved before decision"
            ),
        ));
    }
    if !conflict
        .conflict
        .candidates()
        .iter()
        .any(|candidate| candidate.id() == candidate_id)
    {
        return Err(MinoError::new(
            ErrorCategory::PolicyViolation,
            format!("Candidate {candidate_id} is not live for conflict {conflict_id}"),
        ));
    }
    Ok(())
}

fn require_source_digest(provided: &str, actual: &str) -> Result<(), MinoError> {
    if provided == actual {
        Ok(())
    } else {
        Err(MinoError::new(
            ErrorCategory::DriftDetected,
            "Standards conflict sources changed; inspect and refresh the current candidates",
        ))
    }
}

fn domain_error(error: &crate::domain::DomainError) -> MinoError {
    MinoError::new(ErrorCategory::PolicyViolation, error.to_string())
}
