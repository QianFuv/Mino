//! Classified review, rework, resolution, and final-acceptance orchestration.

use std::path::{Path, PathBuf};

use crate::application::completion::{
    FreshnessScope, reconcile_stale_checks, validate_final_plan_scope, validate_review_evidence,
};
use crate::application::plan::{PlanMutationRequest, PlanOperationReport, PlanService};
use crate::domain::{
    DraftTaskInput, MaterialReviewDisposition, Plan, ReviewClassification, ReviewStatus, TaskId,
};
use crate::evidence::EvidenceStore;
use crate::validation::{validate_plan, validation_failure};
use crate::{ErrorCategory, MinoError};

/// Application boundary for the complete review and rework lifecycle.
#[derive(Clone, Debug)]
pub struct ReviewService {
    root: PathBuf,
    plans: PlanService,
    evidence: EvidenceStore,
}

impl ReviewService {
    /// Discovers an initialized project and creates its review service.
    ///
    /// # Errors
    ///
    /// Returns an environment error when no initialized project is discoverable.
    pub fn discover(start: &Path) -> Result<Self, MinoError> {
        let plans = PlanService::discover(start)?;
        let root = plans.root().to_path_buf();
        Ok(Self {
            evidence: EvidenceStore::new(&root),
            root,
            plans,
        })
    }

    /// Records one classified review request without silently expanding scope.
    ///
    /// # Errors
    ///
    /// Returns a typed error for stale revisions, illegal classifications or
    /// targets, storage failures, or projection drift.
    pub fn record(
        &self,
        request: PlanMutationRequest,
        classification: ReviewClassification,
        feedback: String,
        task_id: Option<TaskId>,
    ) -> Result<PlanOperationReport, MinoError> {
        let mut changed_fields = vec!["review_items".to_owned()];
        if classification == ReviewClassification::FollowUp {
            changed_fields.push("follow_ups".to_owned());
        }
        if classification == ReviewClassification::MaterialChange {
            changed_fields.extend([
                "status".to_owned(),
                "resume_status".to_owned(),
                "blocker".to_owned(),
            ]);
        }
        let stored = self.plans.load_stored(&request.plan_id)?;
        if is_replay_position(&stored, request.expected_revision)? {
            return self.plans.replay_semantic(request, changed_fields);
        }
        let actor = request.actor.clone();
        let at = request.updated_at.clone();
        self.plans.commit_semantic(
            request,
            changed_fields,
            |plan| {
                plan.next_review_item_id()
                    .map(Some)
                    .map_err(|error| map_domain_error(&error))
            },
            move |plan, _| {
                plan.record_review(
                    actor.clone(),
                    feedback.clone(),
                    classification,
                    task_id.clone(),
                    at.clone(),
                )
                .map(|_| ())
            },
        )
    }

    /// Starts an acceptance rerun or creates the reserved complete R task.
    ///
    /// # Errors
    ///
    /// Returns a typed error for stale revisions, malformed/incomplete task
    /// definitions, invalid plan policy, storage failures, or projection drift.
    pub fn rework(
        &self,
        request: PlanMutationRequest,
        review_id: String,
        task_input: Option<DraftTaskInput>,
    ) -> Result<PlanOperationReport, MinoError> {
        let stored = self.plans.load_stored(&request.plan_id)?;
        let classification = review_classification(&stored, &review_id)?;
        let changed_fields = match classification {
            ReviewClassification::AcceptanceDefect => vec![
                "review_items".to_owned(),
                "status".to_owned(),
                "tasks.status".to_owned(),
                "verification_plan".to_owned(),
            ],
            ReviewClassification::InScopeRework => vec![
                "review_items".to_owned(),
                "status".to_owned(),
                "tasks".to_owned(),
                "task_order".to_owned(),
                "approach.file_map".to_owned(),
                "verification_plan".to_owned(),
            ],
            ReviewClassification::MaterialChange
            | ReviewClassification::FollowUp
            | ReviewClassification::Accepted => vec!["review_items".to_owned()],
        };
        if is_replay_position(&stored, request.expected_revision)? {
            return self.plans.replay_semantic(request, changed_fields);
        }
        let current = self.plans.load_verified(&request.plan_id)?;
        let mut preview = current.clone();
        preview
            .begin_review_rework(&review_id, task_input.clone(), request.updated_at.clone())
            .map_err(|error| map_domain_error(&error))?;
        let validation = validate_plan(&self.root, &preview)?;
        if !validation.valid {
            return Err(validation_failure(&validation));
        }
        self.plans.commit_semantic(
            request,
            changed_fields,
            |_| Ok(None),
            move |plan, at| plan.begin_review_rework(&review_id, task_input.clone(), at),
        )
    }

    /// Resolves one completed rework item after revalidating all live evidence.
    ///
    /// # Errors
    ///
    /// Returns a typed error for stale evidence, incomplete commits, unresolved
    /// deviations, stale revisions, storage failures, or projection drift.
    pub fn resolve(
        &self,
        request: PlanMutationRequest,
        review_id: String,
    ) -> Result<PlanOperationReport, MinoError> {
        let changed_fields = vec!["review_items".to_owned()];
        let stored = self.plans.load_stored(&request.plan_id)?;
        if is_replay_position(&stored, request.expected_revision)? {
            return self.plans.replay_semantic(request, changed_fields);
        }
        let current = self.plans.load_verified(&request.plan_id)?;
        let item = current.review_item(&review_id).ok_or_else(|| {
            MinoError::new(
                ErrorCategory::IncompleteOrValidation,
                format!("Review item {review_id} does not exist"),
            )
        })?;
        if item.status() != ReviewStatus::InProgress {
            return Err(MinoError::new(
                ErrorCategory::PolicyViolation,
                format!("Review item {review_id} is not In Progress"),
            ));
        }
        let evidence = self
            .evidence
            .list(&request.plan_id)
            .map_err(|error| map_evidence_error(&error))?;
        if let Some((report, stale)) = reconcile_stale_checks(
            &self.plans,
            &self.root,
            &current,
            &evidence,
            FreshnessScope::All,
            &request.actor,
            &request.updated_at,
        )? {
            return Err(stale_review_error(&report, &stale));
        }
        validate_review_evidence(&self.root, &current, &evidence)?;
        validate_final_plan_scope(&self.root, &current)?;
        self.plans.commit_semantic(
            request,
            changed_fields,
            |_| Ok(None),
            move |plan, at| plan.resolve_review(&review_id, at),
        )
    }

    /// Records the explicit disposition of one blocked Material review request.
    ///
    /// # Errors
    ///
    /// Returns a typed error for stale revisions, a missing or already disposed
    /// review item, incomplete audit fields, storage failures, or projection drift.
    pub fn disposition(
        &self,
        request: PlanMutationRequest,
        review_id: String,
        disposition: MaterialReviewDisposition,
        decision_reference: String,
        reason: String,
    ) -> Result<PlanOperationReport, MinoError> {
        let mut changed_fields = vec!["review_items".to_owned()];
        if disposition != MaterialReviewDisposition::AcceptChange {
            changed_fields.extend([
                "status".to_owned(),
                "resume_status".to_owned(),
                "blocker".to_owned(),
            ]);
        }
        if disposition == MaterialReviewDisposition::DeferToFollowUp {
            changed_fields.extend(["follow_ups".to_owned(), "final_outcome".to_owned()]);
        }
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
                plan.dispose_material_review(
                    &review_id,
                    disposition,
                    actor.clone(),
                    decision_reference.clone(),
                    reason.clone(),
                    at,
                )
            },
        )
    }

    /// Revises an accepted Material decision after its linked amendment terminates.
    ///
    /// # Errors
    ///
    /// Returns a typed error for stale revisions, an unrelated or non-terminal
    /// amendment, an ineligible decision, incomplete audit fields, or persistence drift.
    pub fn revise_disposition(
        &self,
        request: PlanMutationRequest,
        review_id: String,
        disposition: MaterialReviewDisposition,
        decision_reference: String,
        reason: String,
    ) -> Result<PlanOperationReport, MinoError> {
        let mut changed_fields = vec![
            "review_items".to_owned(),
            "status".to_owned(),
            "resume_status".to_owned(),
            "blocker".to_owned(),
        ];
        if disposition == MaterialReviewDisposition::DeferToFollowUp {
            changed_fields.extend(["follow_ups".to_owned(), "final_outcome".to_owned()]);
        }
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
                plan.revise_material_review(
                    &review_id,
                    disposition,
                    actor.clone(),
                    decision_reference.clone(),
                    reason.clone(),
                    at,
                )
            },
        )
    }

    /// Records explicit final review acceptance and moves the plan to Done.
    ///
    /// # Errors
    ///
    /// Returns a typed error while feedback, evidence, task, commit, deviation,
    /// revision, storage, or projection gates remain incomplete.
    pub fn accept(
        &self,
        request: PlanMutationRequest,
        approval_reference: String,
    ) -> Result<PlanOperationReport, MinoError> {
        let changed_fields = vec!["review_items".to_owned(), "status".to_owned()];
        let stored = self.plans.load_stored(&request.plan_id)?;
        if is_replay_position(&stored, request.expected_revision)? {
            return self.plans.replay_semantic(request, changed_fields);
        }
        let current = self.plans.load_verified(&request.plan_id)?;
        let evidence = self
            .evidence
            .list(&request.plan_id)
            .map_err(|error| map_evidence_error(&error))?;
        if let Some((report, stale)) = reconcile_stale_checks(
            &self.plans,
            &self.root,
            &current,
            &evidence,
            FreshnessScope::All,
            &request.actor,
            &request.updated_at,
        )? {
            return Err(stale_review_error(&report, &stale));
        }
        validate_review_evidence(&self.root, &current, &evidence)?;
        validate_final_plan_scope(&self.root, &current)?;
        let actor = request.actor.clone();
        self.plans.commit_semantic(
            request,
            changed_fields,
            |plan| {
                plan.next_review_item_id()
                    .map(Some)
                    .map_err(|error| map_domain_error(&error))
            },
            move |plan, at| plan.accept_review(actor.clone(), approval_reference.clone(), at),
        )
    }
}

fn review_classification(plan: &Plan, review_id: &str) -> Result<ReviewClassification, MinoError> {
    plan.review_item(review_id)
        .map(crate::domain::ReviewItem::classification)
        .ok_or_else(|| {
            MinoError::new(
                ErrorCategory::IncompleteOrValidation,
                format!("Review item {review_id} does not exist"),
            )
        })
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

fn map_evidence_error(error: &crate::evidence::EvidenceError) -> MinoError {
    let category = match error.kind() {
        crate::evidence::EvidenceErrorKind::InvalidRequest
        | crate::evidence::EvidenceErrorKind::PlanNotFound
        | crate::evidence::EvidenceErrorKind::EvidenceNotFound => {
            ErrorCategory::IncompleteOrValidation
        }
        crate::evidence::EvidenceErrorKind::RevisionConflict
        | crate::evidence::EvidenceErrorKind::RequestConflict => ErrorCategory::RevisionConflict,
        crate::evidence::EvidenceErrorKind::Io
        | crate::evidence::EvidenceErrorKind::Serialization
        | crate::evidence::EvidenceErrorKind::LockTimeout => ErrorCategory::EnvironmentUnavailable,
        crate::evidence::EvidenceErrorKind::CorruptStore => ErrorCategory::DriftDetected,
    };
    MinoError::new(category, error.to_string())
}

fn stale_review_error(
    report: &crate::application::plan::PlanOperationReport,
    stale: &[crate::domain::CheckId],
) -> MinoError {
    MinoError::new(
        ErrorCategory::IncompleteOrValidation,
        format!(
            "Review evidence for checks {} became Stale at plan revision {}; rerun verification",
            stale
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", "),
            report.revision
        ),
    )
}
