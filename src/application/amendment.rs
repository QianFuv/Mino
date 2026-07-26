//! Typed protected-plan amendment orchestration and validation gates.

use std::path::{Path, PathBuf};

use crate::application::plan::{PlanMutationRequest, PlanOperationReport, PlanService};
use crate::domain::{AmendmentClassification, AmendmentPatch, Plan};
use crate::store::{canonical_json_bytes, sha256_digest};
use crate::validation::{validate_plan, validation_failure};
use crate::{ErrorCategory, MinoError};

/// Application boundary for proposing, approving, and applying protected changes.
#[derive(Clone, Debug)]
pub struct AmendmentService {
    root: PathBuf,
    plans: PlanService,
}

impl AmendmentService {
    /// Discovers an initialized project and creates its amendment service.
    ///
    /// # Errors
    ///
    /// Returns an environment error when no initialized project is discoverable.
    pub fn discover(start: &Path) -> Result<Self, MinoError> {
        let plans = PlanService::discover(start)?;
        Ok(Self {
            root: plans.root().to_path_buf(),
            plans,
        })
    }

    /// Records a typed proposal and classifier-derived impact without applying it.
    ///
    /// # Errors
    ///
    /// Returns a typed error for stale revisions, malformed operations, lowered
    /// classifications, invalid targets, storage failures, or projection drift.
    pub fn propose(
        &self,
        request: PlanMutationRequest,
        reason: String,
        patch: AmendmentPatch,
        requested_classification: Option<AmendmentClassification>,
    ) -> Result<PlanOperationReport, MinoError> {
        let minimum = patch
            .minimum_classification()
            .map_err(|error| map_domain_error(&error))?;
        let classification = requested_classification.unwrap_or(minimum);
        let changed_fields = proposal_changed_fields(classification);
        let stored = self.plans.load_stored(&request.plan_id)?;
        if is_replay_position(&stored, request.expected_revision)? {
            return self.plans.replay_semantic(request, changed_fields);
        }
        let state_bytes = canonical_json_bytes(&stored).map_err(|error| {
            MinoError::new(
                ErrorCategory::DriftDetected,
                format!("Failed to hash amendment base state: {error}"),
            )
        })?;
        let base_state_hash = sha256_digest(&state_bytes);
        let proposer = request.actor.clone();
        self.plans.commit_semantic(
            request,
            changed_fields,
            |plan| {
                plan.next_amendment_id()
                    .map(Some)
                    .map_err(|error| map_domain_error(&error))
            },
            move |plan, at| {
                plan.propose_amendment(
                    reason.clone(),
                    patch.clone(),
                    requested_classification,
                    base_state_hash.clone(),
                    proposer.clone(),
                    at,
                )
                .map(|_| ())
            },
        )
    }

    /// Records an auditable explicit approval for a pending Material proposal.
    ///
    /// # Errors
    ///
    /// Returns a typed error for a missing/non-Material change, incomplete
    /// approval declaration, stale revision, storage failure, or projection drift.
    pub fn approve(
        &self,
        request: PlanMutationRequest,
        change_id: String,
        approval_reference: String,
    ) -> Result<PlanOperationReport, MinoError> {
        let changed_fields = vec!["amendments".to_owned()];
        let stored = self.plans.load_stored(&request.plan_id)?;
        if is_replay_position(&stored, request.expected_revision)? {
            return self.plans.replay_semantic(request, changed_fields);
        }
        let actor = request.actor.clone();
        self.plans.commit_semantic(
            request,
            changed_fields,
            |_| Ok(None),
            move |plan, at| {
                plan.approve_amendment(&change_id, actor.clone(), approval_reference.clone(), at)
            },
        )
    }

    /// Applies one eligible proposal and revalidates every resulting plan layer.
    ///
    /// # Errors
    ///
    /// Returns a typed error for an unsatisfied approval gate, invalid operation,
    /// stale revision, invalid Minor result, storage failure, or projection drift.
    pub fn apply(
        &self,
        request: PlanMutationRequest,
        change_id: String,
    ) -> Result<PlanOperationReport, MinoError> {
        let stored = self.plans.load_stored(&request.plan_id)?;
        let amendment = stored.amendment(&change_id).ok_or_else(|| {
            MinoError::new(
                ErrorCategory::IncompleteOrValidation,
                format!("Amendment {change_id} does not exist"),
            )
        })?;
        let classification = amendment.classification();
        let mut changed_fields = vec!["amendments".to_owned()];
        changed_fields.extend(amendment.impact().affected_fields().iter().cloned());
        changed_fields.sort();
        changed_fields.dedup();
        if is_replay_position(&stored, request.expected_revision)? {
            return self.plans.replay_semantic(request, changed_fields);
        }
        let current = self.plans.load_verified(&request.plan_id)?;
        let mut preview = current.clone();
        preview
            .apply_amendment(&change_id, request.updated_at.clone())
            .map_err(|error| map_domain_error(&error))?;
        let validation = validate_plan(&self.root, &preview)?;
        if classification == AmendmentClassification::Minor && !validation.valid {
            return Err(validation_failure(&validation));
        }
        self.plans.commit_semantic(
            request,
            changed_fields,
            |_| Ok(None),
            move |plan, at| plan.apply_amendment(&change_id, at),
        )
    }
}

fn proposal_changed_fields(classification: AmendmentClassification) -> Vec<String> {
    let mut fields = vec!["amendments".to_owned()];
    if classification == AmendmentClassification::Material {
        fields.extend([
            "blocker".to_owned(),
            "resume_status".to_owned(),
            "status".to_owned(),
            "tasks.status".to_owned(),
        ]);
    }
    fields
}

fn is_replay_position(plan: &Plan, expected_revision: u64) -> Result<bool, MinoError> {
    let target_revision = expected_revision.checked_add(1).ok_or_else(|| {
        MinoError::new(
            ErrorCategory::RevisionConflict,
            "Expected revision overflowed",
        )
    })?;
    if plan.revision() == expected_revision {
        Ok(false)
    } else if plan.revision() >= target_revision {
        Ok(true)
    } else {
        Err(MinoError::new(
            ErrorCategory::RevisionConflict,
            format!(
                "Plan {} is revision {}, not expected revision {expected_revision}",
                plan.id(),
                plan.revision()
            ),
        ))
    }
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
