//! Evidence compatibility, File Map policy, and completion transitions.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::application::plan::{
    PlanMutationRequest, PlanOperationReport, PlanService, derived_request_id,
};
use crate::domain::{
    AcceptanceCriterion, CheckStatus, CommitStatus, CriterionId, CriterionStatus, Evidence,
    EvidenceId, EvidenceType, FileChange, GitFlowConsent, Plan, RequestId, ReviewClassification,
    ReviewStatus, Task, TaskId, Timestamp, VerificationCheck,
};
use crate::evidence::{EvidenceError, EvidenceErrorKind, EvidenceStore};
use crate::git::{ChangedFile, GitChangeError, inspect_changes, matches_file_map_path};
use crate::workspace::workspace_fingerprint_is_current;
use crate::{ErrorCategory, MinoError, NextAction};

#[derive(Clone, Copy)]
pub(crate) enum FreshnessScope<'a> {
    Task(&'a TaskId),
    All,
}

/// Application boundary for evidence binding and execution completion gates.
#[derive(Clone, Debug)]
pub struct CompletionService {
    root: PathBuf,
    plans: PlanService,
    evidence: EvidenceStore,
}

impl CompletionService {
    /// Discovers an initialized project and creates its completion service.
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

    /// Binds one compatible immutable evidence record to an active criterion.
    ///
    /// # Errors
    ///
    /// Returns a typed error for stale revisions, missing or superseded evidence,
    /// incompatible bindings, or an inactive criterion task.
    pub fn pass_criterion(
        &self,
        request: PlanMutationRequest,
        criterion_id: CriterionId,
        evidence_id: EvidenceId,
    ) -> Result<PlanOperationReport, MinoError> {
        let stored = self.plans.load_stored(&request.plan_id)?;
        let task_id = criterion_task(&stored, &criterion_id)?.id().clone();
        let changed_fields = vec![
            format!("tasks.{task_id}.acceptance_criteria.{criterion_id}.status"),
            format!("tasks.{task_id}.acceptance_criteria.{criterion_id}.evidence_refs"),
        ];
        if is_replay_position(&stored, request.expected_revision)? {
            return self.plans.replay_semantic(request, changed_fields);
        }
        let current = self.plans.load_verified(&request.plan_id)?;
        let evidence = self
            .evidence
            .show(&request.plan_id, &evidence_id)
            .map_err(|error| map_evidence_error(&error))?;
        let all_evidence = self
            .evidence
            .list(&request.plan_id)
            .map_err(|error| map_evidence_error(&error))?;
        if let Some((report, stale)) = reconcile_stale_checks(
            &self.plans,
            &self.root,
            &current,
            &all_evidence,
            FreshnessScope::Task(&task_id),
            &request.actor,
            &request.updated_at,
        )? {
            return Err(stale_error(&report, &stale));
        }
        validate_criterion_binding(
            &self.root,
            &current,
            &task_id,
            &criterion_id,
            &evidence,
            &all_evidence,
        )?;
        let is_accepted_exception = evidence.kind() == EvidenceType::AcceptedException;
        self.plans.commit_semantic(
            request,
            changed_fields,
            |_| Ok(None),
            move |plan, at| {
                plan.record_task_criterion_evidence(
                    &task_id,
                    &criterion_id,
                    evidence_id.clone(),
                    is_accepted_exception,
                    at,
                )
            },
        )
    }

    /// Completes the active task only after every evidence and scope gate passes.
    ///
    /// # Errors
    ///
    /// Returns a typed error for stale revisions, incomplete evidence, unresolved
    /// deviations, out-of-scope files, or an ineligible commit gate.
    pub fn complete_task(
        &self,
        request: PlanMutationRequest,
        task_id: TaskId,
    ) -> Result<PlanOperationReport, MinoError> {
        let stored = self.plans.load_stored(&request.plan_id)?;
        let changed_fields = vec![format!("tasks.{task_id}.status")];
        if is_replay_position(&stored, request.expected_revision)? {
            return self.plans.replay_semantic(request, changed_fields);
        }
        let current = self.plans.load_verified(&request.plan_id)?;
        let task = current.task(&task_id).ok_or_else(|| {
            MinoError::new(
                ErrorCategory::IncompleteOrValidation,
                format!("Task {task_id} does not exist"),
            )
        })?;
        let evidence = self
            .evidence
            .list(&request.plan_id)
            .map_err(|error| map_evidence_error(&error))?;
        if let Some((report, stale)) = reconcile_stale_checks(
            &self.plans,
            &self.root,
            &current,
            &evidence,
            FreshnessScope::Task(&task_id),
            &request.actor,
            &request.updated_at,
        )? {
            return Err(stale_error(&report, &stale));
        }
        validate_task_evidence(&self.root, &current, task, &evidence)?;
        validate_task_deviations(&current, task)?;
        let changed_files = task_changed_files(&self.root, &current, task)?;
        validate_commit_precondition(&current, task, &changed_files)?;
        self.plans.commit_semantic(
            request,
            changed_fields,
            |_| Ok(None),
            move |plan, at| plan.complete_task(&task_id, at),
        )
    }

    /// Moves an executed plan to Review after global evidence gates pass.
    ///
    /// # Errors
    ///
    /// Returns a typed error for stale revisions, incomplete tasks, unresolved
    /// deviations, or missing/incompatible global verification evidence.
    pub fn finish(&self, request: PlanMutationRequest) -> Result<PlanOperationReport, MinoError> {
        let stored = self.plans.load_stored(&request.plan_id)?;
        let changed_fields = vec!["status".to_owned()];
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
            return Err(stale_error(&report, &stale));
        }
        validate_global_evidence(&self.root, &current, &evidence)?;
        validate_all_deviations(&current)?;
        self.plans.commit_semantic(
            request,
            changed_fields,
            |_| Ok(None),
            Plan::finish_execution,
        )
    }
}

fn criterion_task<'a>(plan: &'a Plan, criterion_id: &CriterionId) -> Result<&'a Task, MinoError> {
    let mut matches = plan.tasks().iter().filter(|task| {
        task.acceptance_criteria()
            .iter()
            .any(|criterion| criterion.id() == criterion_id)
    });
    let task = matches.next().ok_or_else(|| {
        MinoError::new(
            ErrorCategory::IncompleteOrValidation,
            format!("Criterion {criterion_id} does not exist"),
        )
    })?;
    if matches.next().is_some() {
        Err(MinoError::new(
            ErrorCategory::DriftDetected,
            format!("Criterion {criterion_id} is not globally unique"),
        ))
    } else {
        Ok(task)
    }
}

fn validate_criterion_binding(
    root: &Path,
    plan: &Plan,
    task_id: &TaskId,
    criterion_id: &CriterionId,
    evidence: &Evidence,
    all_evidence: &[Evidence],
) -> Result<(), MinoError> {
    validate_current_evidence(root, plan, evidence, all_evidence)?;
    if evidence.task_id() != Some(task_id) {
        return Err(incompatible(format!(
            "Evidence {} is not bound to task {task_id}",
            evidence.id()
        )));
    }
    if evidence.kind() == EvidenceType::Command {
        validate_command_criterion(root, plan, task_id, evidence)
    } else if evidence.criterion_id() == Some(criterion_id) {
        Ok(())
    } else {
        Err(incompatible(format!(
            "Evidence {} is not bound to criterion {criterion_id}",
            evidence.id()
        )))
    }
}

fn validate_command_criterion(
    root: &Path,
    plan: &Plan,
    task_id: &TaskId,
    evidence: &Evidence,
) -> Result<(), MinoError> {
    let check_id = evidence.check_id().ok_or_else(|| {
        incompatible(format!(
            "Command evidence {} has no check binding",
            evidence.id()
        ))
    })?;
    let check = plan
        .task(task_id)
        .and_then(|task| {
            task.verification_checks()
                .iter()
                .find(|check| check.id() == check_id)
        })
        .ok_or_else(|| incompatible(format!("Check {check_id} does not belong to {task_id}")))?;
    validate_passing_check_evidence(root, plan, check, evidence)
}

pub(crate) fn validate_task_evidence(
    root: &Path,
    plan: &Plan,
    task: &Task,
    evidence: &[Evidence],
) -> Result<(), MinoError> {
    let superseded = superseded_ids(evidence);
    for criterion in task.acceptance_criteria() {
        validate_completed_criterion(root, plan, task.id(), criterion, evidence, &superseded)?;
    }
    for check in task
        .verification_checks()
        .iter()
        .filter(|check| check.is_required())
    {
        validate_completed_check(root, plan, Some(task.id()), check, evidence, &superseded)?;
    }
    Ok(())
}

fn validate_completed_criterion(
    root: &Path,
    plan: &Plan,
    task_id: &TaskId,
    criterion: &AcceptanceCriterion,
    evidence: &[Evidence],
    superseded: &BTreeSet<&EvidenceId>,
) -> Result<(), MinoError> {
    if !matches!(
        criterion.status(),
        CriterionStatus::Passed | CriterionStatus::AcceptedException
    ) {
        return Err(incomplete(format!(
            "Criterion {} has not passed",
            criterion.id()
        )));
    }
    let evidence_id = criterion
        .evidence_refs()
        .last()
        .ok_or_else(|| incomplete(format!("Criterion {} has no evidence", criterion.id())))?;
    if superseded.contains(evidence_id) {
        return Err(incomplete(format!(
            "Criterion {} references superseded evidence {evidence_id}",
            criterion.id()
        )));
    }
    let record = evidence_by_id(evidence, evidence_id)?;
    validate_criterion_binding(root, plan, task_id, criterion.id(), record, evidence)?;
    let is_exception = record.kind() == EvidenceType::AcceptedException;
    if (criterion.status() == CriterionStatus::AcceptedException) != is_exception {
        return Err(incompatible(format!(
            "Criterion {} status does not match evidence type",
            criterion.id()
        )));
    }
    Ok(())
}

fn validate_completed_check(
    root: &Path,
    plan: &Plan,
    task_id: Option<&TaskId>,
    check: &VerificationCheck,
    evidence: &[Evidence],
    superseded: &BTreeSet<&EvidenceId>,
) -> Result<(), MinoError> {
    if check.status() != CheckStatus::Passed {
        return Err(incomplete(format!("Check {} has not passed", check.id())));
    }
    let evidence_id = check
        .evidence_refs()
        .last()
        .ok_or_else(|| incomplete(format!("Check {} has no evidence", check.id())))?;
    if superseded.contains(evidence_id) {
        return Err(incomplete(format!(
            "Check {} references superseded evidence {evidence_id}",
            check.id()
        )));
    }
    let record = evidence_by_id(evidence, evidence_id)?;
    validate_current_evidence(root, plan, record, evidence)?;
    if record.kind() != EvidenceType::Command
        || record.task_id() != task_id
        || record.check_id() != Some(check.id())
    {
        return Err(incompatible(format!(
            "Evidence {evidence_id} is incompatible with check {}",
            check.id()
        )));
    }
    validate_passing_check_evidence(root, plan, check, record)
}

fn validate_passing_check_evidence(
    root: &Path,
    plan: &Plan,
    check: &VerificationCheck,
    evidence: &Evidence,
) -> Result<(), MinoError> {
    if check.status() == CheckStatus::Passed
        && check.evidence_refs().last() == Some(evidence.id())
        && evidence.exit_code() == Some(check.expected_exit_code())
        && evidence.workspace_fingerprint().is_some()
        && workspace_fingerprint_is_current(
            root,
            plan,
            evidence
                .workspace_fingerprint()
                .expect("checked workspace fingerprint exists"),
        )?
    {
        Ok(())
    } else {
        Err(incompatible(format!(
            "Evidence {} is not the latest passing result for check {}",
            evidence.id(),
            check.id()
        )))
    }
}

fn validate_current_evidence(
    root: &Path,
    plan: &Plan,
    evidence: &Evidence,
    all_evidence: &[Evidence],
) -> Result<(), MinoError> {
    if plan.is_evidence_stale(evidence.id()) {
        return Err(incomplete(format!(
            "Evidence {} was invalidated by an applied amendment",
            evidence.id()
        )));
    }
    if evidence.plan_id() != plan.id()
        || evidence
            .captured_revision()
            .is_none_or(|revision| revision > plan.revision())
    {
        return Err(incompatible(format!(
            "Evidence {} does not belong to the current plan history",
            evidence.id()
        )));
    }
    if evidence.kind() == EvidenceType::Command {
        let fingerprint = evidence.workspace_fingerprint().ok_or_else(|| {
            incomplete(format!(
                "Evidence {} predates workspace fingerprint binding",
                evidence.id()
            ))
        })?;
        if !workspace_fingerprint_is_current(root, plan, fingerprint)? {
            return Err(incomplete(format!(
                "Evidence {} is stale for the current workspace",
                evidence.id()
            )));
        }
    }
    if all_evidence
        .iter()
        .any(|record| record.supersedes() == Some(evidence.id()))
    {
        Err(incomplete(format!(
            "Evidence {} has been superseded",
            evidence.id()
        )))
    } else {
        Ok(())
    }
}

fn task_changed_files(
    root: &Path,
    plan: &Plan,
    task: &Task,
) -> Result<Vec<ChangedFile>, MinoError> {
    let inspection = inspect_changes(root).map_err(|error| map_git_error(&error))?;
    let files = inspection
        .files()
        .iter()
        .filter(|file| !is_mino_owned_path(plan, file.path()))
        .cloned()
        .collect::<Vec<_>>();
    let requires_inspection = task
        .file_map()
        .iter()
        .any(|entry| entry.change() != FileChange::NotApplicable);
    if !inspection.is_repository() && requires_inspection {
        return Err(MinoError::new(
            ErrorCategory::EnvironmentUnavailable,
            "Task File Map changes require an available Git work tree for completion",
        ));
    }
    let outside = files
        .iter()
        .filter(|file| {
            !task.file_map().iter().any(|entry| {
                matches_file_map_path(entry.path(), file.path())
                    && compatible_change(entry.change(), file)
            })
        })
        .map(|file| file.path().to_owned())
        .collect::<Vec<_>>();
    if outside.is_empty() {
        Ok(files)
    } else {
        Err(scope_error(plan, task.id(), &outside))
    }
}

fn validate_commit_precondition(
    plan: &Plan,
    task: &Task,
    changed_files: &[ChangedFile],
) -> Result<(), MinoError> {
    let is_acceptance_defect_rerun = plan.review_items().iter().any(|item| {
        item.classification() == ReviewClassification::AcceptanceDefect
            && item.status() == ReviewStatus::InProgress
            && item.linked_task() == Some(task.id())
    });
    if is_acceptance_defect_rerun {
        if changed_files.is_empty() {
            return Ok(());
        }
        return Err(MinoError::new(
            ErrorCategory::PolicyViolation,
            format!(
                "Acceptance-defect rework for task {} cannot include changed files; record In-Scope Rework instead",
                task.id()
            ),
        ));
    }
    let Some(gate) = task.commit_gate().filter(|gate| gate.is_required()) else {
        return Ok(());
    };
    if !plan.git_readiness().git_flow_enabled()
        || plan.git_readiness().git_flow_consent() != GitFlowConsent::Approved
        || gate.status() != CommitStatus::Pending
    {
        return Err(incomplete(format!(
            "Task {} required commit gate is not eligible",
            task.id()
        )));
    }
    if changed_files.is_empty() {
        return Err(incomplete(format!(
            "Task {} has no changed files for its required commit",
            task.id()
        )));
    }
    let outside_scope = changed_files
        .iter()
        .filter(|file| {
            !gate
                .scope()
                .iter()
                .any(|scope| matches_file_map_path(scope, file.path()))
        })
        .map(|file| file.path().to_owned())
        .collect::<Vec<_>>();
    if outside_scope.is_empty() {
        Ok(())
    } else {
        Err(scope_error(plan, task.id(), &outside_scope))
    }
}

pub(crate) fn reconcile_stale_checks(
    plans: &PlanService,
    root: &Path,
    plan: &Plan,
    evidence: &[Evidence],
    scope: FreshnessScope<'_>,
    actor: &str,
    updated_at: &Timestamp,
) -> Result<Option<(PlanOperationReport, Vec<crate::domain::CheckId>)>, MinoError> {
    let stale = stale_check_ids(root, plan, evidence, scope)?;
    if stale.is_empty() {
        return Ok(None);
    }
    let action = format!(
        "workspace.stale.{}",
        stale
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(".")
    );
    let request_id = RequestId::parse(derived_request_id(plan, &action))
        .expect("derived request identifiers are valid");
    let command = vec![
        "mino-internal".to_owned(),
        "workspace".to_owned(),
        "mark-stale".to_owned(),
        "--plan".to_owned(),
        plan.id().to_string(),
        "--checks".to_owned(),
        stale
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(","),
        "--expect-revision".to_owned(),
        plan.revision().to_string(),
        "--actor".to_owned(),
        actor.to_owned(),
    ];
    let changed_fields = stale
        .iter()
        .map(|check_id| format!("verification.{check_id}.status"))
        .chain(["status".to_owned(), "tasks.status".to_owned()])
        .collect::<Vec<_>>();
    let stale_for_mutation = stale.clone();
    let report = plans.commit_semantic(
        PlanMutationRequest {
            plan_id: plan.id().clone(),
            expected_revision: plan.revision(),
            request_id,
            actor: actor.to_owned(),
            command,
            updated_at: updated_at.clone(),
        },
        changed_fields,
        |_| Ok(None),
        move |candidate, at| candidate.mark_checks_stale(&stale_for_mutation, at),
    )?;
    Ok(Some((report, stale)))
}

fn stale_check_ids(
    root: &Path,
    plan: &Plan,
    evidence: &[Evidence],
    scope: FreshnessScope<'_>,
) -> Result<Vec<crate::domain::CheckId>, MinoError> {
    let checks = match scope {
        FreshnessScope::Task(task_id) => plan
            .task(task_id)
            .ok_or_else(|| incomplete(format!("Task {task_id} does not exist")))?
            .verification_checks()
            .iter()
            .collect::<Vec<_>>(),
        FreshnessScope::All => plan
            .tasks()
            .iter()
            .flat_map(Task::verification_checks)
            .chain(plan.global_verification())
            .collect::<Vec<_>>(),
    };
    let mut stale = Vec::new();
    for check in checks
        .into_iter()
        .filter(|check| check.status() == CheckStatus::Passed)
    {
        let Some(evidence_id) = check.evidence_refs().last() else {
            continue;
        };
        let record = evidence_by_id(evidence, evidence_id)?;
        let is_current = if record.kind() == EvidenceType::Command {
            match record.workspace_fingerprint() {
                Some(fingerprint) => workspace_fingerprint_is_current(root, plan, fingerprint)?,
                None => false,
            }
        } else {
            false
        };
        if !is_current {
            stale.push(check.id().clone());
        }
    }
    stale.sort();
    stale.dedup();
    Ok(stale)
}

fn stale_error(report: &PlanOperationReport, stale: &[crate::domain::CheckId]) -> MinoError {
    let checks = stale.iter().map(ToString::to_string).collect::<Vec<_>>();
    MinoError::new(
        ErrorCategory::IncompleteOrValidation,
        format!(
            "Workspace content changed after checks {}; they are Stale at plan revision {}",
            checks.join(", "),
            report.revision
        ),
    )
    .with_remediation(
        checks,
        vec![NextAction {
            id: "agent.next".to_owned(),
            argv: vec![
                "mino".to_owned(),
                "agent".to_owned(),
                "next".to_owned(),
                "--format".to_owned(),
                "json".to_owned(),
                "--no-input".to_owned(),
            ],
        }],
    )
}

pub(crate) fn validate_review_evidence(
    root: &Path,
    plan: &Plan,
    evidence: &[Evidence],
) -> Result<(), MinoError> {
    let superseded = superseded_ids(evidence);
    for task in plan.tasks() {
        validate_task_evidence(root, plan, task, evidence)?;
        if let Some(gate) = task
            .commit_gate()
            .filter(|gate| gate.is_required() && gate.status() == CommitStatus::Committed)
        {
            let evidence_id = gate.evidence_refs().first().ok_or_else(|| {
                incomplete(format!("Task {} commit evidence is missing", task.id()))
            })?;
            let record = evidence_by_id(evidence, evidence_id)?;
            if superseded.contains(record.id())
                || plan.is_evidence_stale(record.id())
                || record.kind() != EvidenceType::Commit
                || record.plan_id() != plan.id()
                || record.task_id() != Some(task.id())
                || record.artifact_path() != gate.actual_commit()
            {
                return Err(incompatible(format!(
                    "Task {} commit evidence {} is stale or incompatible",
                    task.id(),
                    record.id()
                )));
            }
        }
    }
    validate_global_evidence(root, plan, evidence)?;
    validate_all_deviations(plan)
}

fn compatible_change(change: FileChange, file: &ChangedFile) -> bool {
    match change {
        FileChange::Create => file.is_added(),
        FileChange::Modify => !file.is_added() && !file.is_deleted(),
        FileChange::Delete => file.is_deleted(),
        FileChange::Test => true,
        FileChange::NotApplicable => false,
    }
}

pub(crate) fn validate_task_deviations(plan: &Plan, task: &Task) -> Result<(), MinoError> {
    if plan
        .execution_state()
        .map_err(|error| map_domain_error(&error))?
        .checkpoints()
        .iter()
        .any(|checkpoint| {
            checkpoint.task_id() == task.id()
                && checkpoint.kind() == crate::domain::CheckpointKind::Deviation
        })
    {
        Err(incomplete(format!(
            "Task {} has an unresolved deviation",
            task.id()
        )))
    } else {
        Ok(())
    }
}

fn validate_all_deviations(plan: &Plan) -> Result<(), MinoError> {
    if plan
        .execution_state()
        .map_err(|error| map_domain_error(&error))?
        .checkpoints()
        .iter()
        .any(|checkpoint| checkpoint.kind() == crate::domain::CheckpointKind::Deviation)
    {
        Err(incomplete("Plan has unresolved execution deviations"))
    } else {
        Ok(())
    }
}

fn validate_global_evidence(
    root: &Path,
    plan: &Plan,
    evidence: &[Evidence],
) -> Result<(), MinoError> {
    if plan
        .tasks()
        .iter()
        .any(|task| task.status() != crate::domain::TaskStatus::Done)
    {
        return Err(incomplete("Every task must be Done before finish"));
    }
    if let Some(task) = plan.tasks().iter().find(|task| {
        task.commit_gate()
            .is_some_and(|gate| gate.is_required() && gate.status() != CommitStatus::Committed)
    }) {
        return Err(incomplete(format!(
            "Task {} required commit gate is not Committed",
            task.id()
        )));
    }
    let superseded = superseded_ids(evidence);
    for check in plan
        .global_verification()
        .iter()
        .filter(|check| check.is_required())
    {
        validate_completed_check(root, plan, None, check, evidence, &superseded)?;
    }
    Ok(())
}

fn evidence_by_id<'a>(
    evidence: &'a [Evidence],
    evidence_id: &EvidenceId,
) -> Result<&'a Evidence, MinoError> {
    evidence
        .iter()
        .find(|record| record.id() == evidence_id)
        .ok_or_else(|| incomplete(format!("Evidence {evidence_id} is missing")))
}

fn superseded_ids(evidence: &[Evidence]) -> BTreeSet<&EvidenceId> {
    evidence.iter().filter_map(Evidence::supersedes).collect()
}

fn is_mino_owned_path(plan: &Plan, path: &str) -> bool {
    path == ".mino" || path.starts_with(".mino/") || plan.metadata().markdown_path() == Some(path)
}

fn is_replay_position(plan: &Plan, expected_revision: u64) -> Result<bool, MinoError> {
    let target = expected_revision.checked_add(1).ok_or_else(|| {
        MinoError::new(
            ErrorCategory::RevisionConflict,
            "Expected revision overflowed",
        )
    })?;
    if plan.revision() == expected_revision {
        Ok(false)
    } else if plan.revision() >= target {
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

fn scope_error(plan: &Plan, task_id: &TaskId, outside: &[String]) -> MinoError {
    let summary = format!(
        "Changed paths outside task {task_id} File Map: {}",
        outside.join(", ")
    );
    let request_id = RequestId::parse(derived_request_id(plan, "exec.checkpoint.deviation"))
        .expect("derived request identifiers are valid");
    MinoError::new(ErrorCategory::PolicyViolation, summary.clone()).with_remediation(
        outside.to_vec(),
        vec![NextAction {
            id: "exec.checkpoint".to_owned(),
            argv: vec![
                "mino".to_owned(),
                "exec".to_owned(),
                "checkpoint".to_owned(),
                "--plan".to_owned(),
                plan.id().to_string(),
                "--task".to_owned(),
                task_id.to_string(),
                "--kind".to_owned(),
                "deviation".to_owned(),
                "--summary".to_owned(),
                summary,
                "--expect-revision".to_owned(),
                plan.revision().to_string(),
                "--request-id".to_owned(),
                request_id.to_string(),
                "--format".to_owned(),
                "json".to_owned(),
                "--no-input".to_owned(),
            ],
        }],
    )
}

fn incomplete(message: impl Into<String>) -> MinoError {
    MinoError::new(ErrorCategory::IncompleteOrValidation, message)
}

fn incompatible(message: impl Into<String>) -> MinoError {
    MinoError::new(ErrorCategory::PolicyViolation, message)
}

fn map_domain_error(error: &crate::domain::DomainError) -> MinoError {
    MinoError::new(ErrorCategory::DriftDetected, error.to_string())
}

fn map_git_error(error: &GitChangeError) -> MinoError {
    MinoError::new(ErrorCategory::EnvironmentUnavailable, error.message())
}

fn map_evidence_error(error: &EvidenceError) -> MinoError {
    let category = match error.kind() {
        EvidenceErrorKind::InvalidRequest
        | EvidenceErrorKind::PlanNotFound
        | EvidenceErrorKind::EvidenceNotFound => ErrorCategory::IncompleteOrValidation,
        EvidenceErrorKind::RevisionConflict | EvidenceErrorKind::RequestConflict => {
            ErrorCategory::RevisionConflict
        }
        EvidenceErrorKind::CorruptStore => ErrorCategory::DriftDetected,
        EvidenceErrorKind::Io
        | EvidenceErrorKind::Serialization
        | EvidenceErrorKind::LockTimeout => ErrorCategory::EnvironmentUnavailable,
    };
    MinoError::new(category, error.message())
}
