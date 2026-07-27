//! Revision-checked standards reconciliation, conflict inspection, and resolution.

use std::collections::BTreeSet;
use std::path::Path;

use serde::Serialize;

use crate::application::plan::{
    PlanMutationRequest, PlanOperationReport, PlanService, summarize_project_scan,
};
use crate::domain::{
    CheckId, PlanId, PlanStatus, ProjectScanSummary, StandardSelection, VerificationCheck,
};
use crate::standards::{
    AssessedStandardConflict, EmbeddedCatalog, LocalStandardsSource, StandardsConflictAssessment,
    SystemToolProbe, ToolProbe, apply_recommendation, assess_standard_conflicts,
    detect_standard_conflicts, recommend_for_paths, recommend_initial,
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

/// Atomic plan mutation result followed by its persisted scan and selections.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct StandardsPlanOperationReport {
    /// Revision and digest result from the normal plan transaction.
    #[serde(flatten)]
    pub operation: PlanOperationReport,
    /// Exact persisted scan summary used for the recommendation.
    pub project_scan: ProjectScanSummary,
    /// Current exact package selections in stable order.
    pub standards: Vec<String>,
    /// Current global verification identifiers in stable order.
    pub verification_checks: Vec<String>,
}

/// Application boundary for second-stage plan-scoped standards reconciliation.
#[derive(Clone, Debug)]
pub struct StandardsPlanService {
    plans: PlanService,
}

impl StandardsPlanService {
    /// Discovers an initialized project and creates its reconciliation service.
    ///
    /// # Errors
    ///
    /// Returns an environment error when no initialized project can be discovered.
    pub fn discover(start: &Path) -> Result<Self, MinoError> {
        Ok(Self {
            plans: PlanService::discover(start)?,
        })
    }

    /// Reconciles current File Map languages using the bounded system probe.
    ///
    /// # Errors
    ///
    /// Returns an error for stale revisions, malformed sources, illegal plan
    /// state, probe/application failures, or storage and projection failures.
    pub fn reconcile(
        &self,
        request: PlanMutationRequest,
    ) -> Result<StandardsPlanOperationReport, MinoError> {
        self.reconcile_with_probe(request, &SystemToolProbe)
    }

    /// Reconciles current File Map languages with a caller-provided tool probe.
    ///
    /// Exact request replay is resolved before any scan or external probe runs.
    ///
    /// # Errors
    ///
    /// Returns an error for stale revisions, malformed sources, illegal plan
    /// state, probe/application failures, or storage and projection failures.
    pub fn reconcile_with_probe<P: ToolProbe>(
        &self,
        request: PlanMutationRequest,
        probe: &P,
    ) -> Result<StandardsPlanOperationReport, MinoError> {
        let current = self.plans.load_verified(&request.plan_id)?;
        if current.revision() != request.expected_revision {
            let operation = self
                .plans
                .replay_semantic(request, standards_changed_fields())?;
            return self.operation_report(operation);
        }

        let scan = crate::project::scan(self.plans.root())?;
        let scan_summary = summarize_project_scan(&scan)?;
        let catalog = EmbeddedCatalog::load()?;
        let paths = current
            .approach()
            .file_map()
            .iter()
            .map(|entry| std::path::PathBuf::from(entry.path()))
            .collect::<Vec<_>>();
        let recommendation = if paths.is_empty() {
            recommend_initial(&catalog, &scan)?
        } else {
            recommend_for_paths(&catalog, &scan, &paths)?
        };
        let application =
            apply_recommendation(self.plans.root(), &catalog, &recommendation, probe)?;
        let standards = application
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
            .collect::<Vec<_>>();
        let checks = application
            .checks
            .into_iter()
            .map(|check| {
                Ok(VerificationCheck::new(
                    CheckId::parse(check.id).map_err(|error| domain_error(&error))?,
                    check.argv,
                    protocol_path(&check.cwd)?,
                    0,
                    check.required,
                ))
            })
            .collect::<Result<Vec<_>, MinoError>>()?;
        let catalog_check_ids = catalog
            .packages()
            .iter()
            .flat_map(crate::standards::StandardsPackage::checks)
            .map(|check| CheckId::parse(check.id.clone()).map_err(|error| domain_error(&error)))
            .collect::<Result<BTreeSet<_>, MinoError>>()?;
        let prior_conflicts = current
            .standards_conflict_state()
            .map_err(|error| domain_error(&error))?;
        let preview = current
            .preview_standards_reconciliation(
                standards.clone(),
                &catalog_check_ids,
                checks.clone(),
                scan_summary.clone(),
            )
            .map_err(|error| domain_error(&error))?;
        let detected = detect_standard_conflicts(self.plans.root(), &preview)?;
        let refreshed_conflicts = prior_conflicts
            .refreshed(detected.conflicts)
            .map_err(|error| domain_error(&error))?;
        let operation = self.plans.commit_semantic(
            request,
            standards_changed_fields(),
            |_| Ok(None),
            move |plan, at| {
                plan.reconcile_standards(
                    standards.clone(),
                    &catalog_check_ids,
                    checks.clone(),
                    scan_summary.clone(),
                    &refreshed_conflicts,
                    at,
                )
            },
        )?;
        self.operation_report(operation)
    }

    fn operation_report(
        &self,
        operation: PlanOperationReport,
    ) -> Result<StandardsPlanOperationReport, MinoError> {
        let plan = self.plans.load_verified(&operation.plan_id)?;
        let project_scan = plan
            .project_scan_summary()
            .map_err(|error| domain_error(&error))?
            .ok_or_else(|| {
                MinoError::new(
                    ErrorCategory::DriftDetected,
                    "Reconciled plan has no persisted project scan",
                )
            })?;
        let standards = plan
            .standards()
            .iter()
            .map(|standard| {
                format!(
                    "{}@{}#{}",
                    standard.package_id(),
                    standard.version(),
                    standard.digest()
                )
            })
            .collect();
        let verification_checks = plan
            .global_verification()
            .iter()
            .map(|check| check.id().to_string())
            .collect();
        Ok(StandardsPlanOperationReport {
            operation,
            project_scan,
            standards,
            verification_checks,
        })
    }
}

fn standards_changed_fields() -> Vec<String> {
    vec![
        "standards".to_owned(),
        "verification_plan".to_owned(),
        "extensions.project_scan".to_owned(),
        "extensions.standards_conflicts".to_owned(),
        "approvals".to_owned(),
        "git_readiness.approved_at".to_owned(),
        "git_readiness.git_flow_consent".to_owned(),
    ]
}

fn protocol_path(path: &Path) -> Result<String, MinoError> {
    path.to_str()
        .map(|value| value.replace('\\', "/"))
        .ok_or_else(|| {
            MinoError::new(
                ErrorCategory::IncompleteOrValidation,
                format!("Verification path {} is not UTF-8", path.display()),
            )
        })
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
