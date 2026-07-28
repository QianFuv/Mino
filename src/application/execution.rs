//! Ordered plan execution, checkpointing, check running, and evidence attachment.

use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use serde::Serialize;

use crate::application::plan::{PlanMutationRequest, PlanOperationReport, PlanService};
use crate::domain::{
    CheckId, CheckRunContext, CheckRunLease, CheckRunLimits, CheckRunResult, CheckpointKind,
    Deviation, DeviationClassification, Evidence, EvidenceId, Plan, PlanId, RequestId, TaskId,
    Timestamp, VerificationCheck,
};
use crate::evidence::{CommandEvidenceRequest, EvidenceError, EvidenceErrorKind, EvidenceStore};
use crate::runner::{
    CheckRunJournal, ProcessRunner, Redactor, RunDisposition, RunEnvironment, RunnerError,
    RunnerErrorKind,
};
use crate::store::sha256_digest;
use crate::validation::{validate_plan, validation_failure};
use crate::workspace::{capture_task_workspace_baseline, capture_workspace_fingerprint};
use crate::{ErrorCategory, MinoError};

const DEFAULT_CHECK_TIMEOUT: Duration = Duration::from_mins(5);
const DEFAULT_OUTPUT_LIMIT_BYTES: u64 = 1024 * 1024;

/// Stable schema identifier for deviation list responses.
pub const DEVIATION_LIST_KIND: &str = "mino.deviation-list/v1";

/// Whether a check process executed or reused recoverable journal state.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckExecutionDisposition {
    /// This invocation started the external process.
    Executed,
    /// A prior immutable terminal result was reused.
    Replayed,
    /// An abandoned lease was closed as interrupted.
    RecoveredInterrupted,
}

impl From<RunDisposition> for CheckExecutionDisposition {
    fn from(value: RunDisposition) -> Self {
        match value {
            RunDisposition::Executed => Self::Executed,
            RunDisposition::Replayed => Self::Replayed,
            RunDisposition::RecoveredInterrupted => Self::RecoveredInterrupted,
        }
    }
}

/// Complete durable outcome of one planned verification invocation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CheckExecutionReport {
    plan: PlanOperationReport,
    evidence: Evidence,
    run: CheckRunResult,
    disposition: CheckExecutionDisposition,
}

impl CheckExecutionReport {
    /// Returns the plan revision after terminal evidence attachment.
    #[must_use]
    pub const fn plan(&self) -> &PlanOperationReport {
        &self.plan
    }

    /// Returns the immutable command evidence record.
    #[must_use]
    pub const fn evidence(&self) -> &Evidence {
        &self.evidence
    }

    /// Returns the immutable terminal process record.
    #[must_use]
    pub const fn run(&self) -> &CheckRunResult {
        &self.run
    }

    /// Returns the runner recovery disposition.
    #[must_use]
    pub const fn disposition(&self) -> CheckExecutionDisposition {
        self.disposition
    }

    /// Returns whether the planned command passed.
    #[must_use]
    pub const fn is_success(&self) -> bool {
        self.run.is_success()
    }
}

/// Current identified deviations returned without mutating the plan.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DeviationListReport {
    kind: &'static str,
    plan_id: PlanId,
    revision: u64,
    deviations: Vec<Deviation>,
}

/// Application boundary for revision-checked ordered execution.
#[derive(Clone, Debug)]
pub struct ExecutionService {
    root: PathBuf,
    plans: PlanService,
    evidence: EvidenceStore,
    runner: ProcessRunner,
    limits: CheckRunLimits,
    environment: RunEnvironment,
    redactor: Redactor,
}

impl ExecutionService {
    /// Discovers an initialized project with protocol-default execution bounds.
    ///
    /// # Errors
    ///
    /// Returns an environment error when no initialized project is discoverable.
    pub fn discover(start: &Path) -> Result<Self, MinoError> {
        let limits = CheckRunLimits::new(DEFAULT_CHECK_TIMEOUT, DEFAULT_OUTPUT_LIMIT_BYTES)
            .map_err(|error| map_domain_error(&error))?;
        Self::discover_with_limits(start, limits)
    }

    /// Discovers an initialized project with caller-selected finite check bounds.
    ///
    /// # Errors
    ///
    /// Returns an environment error when no initialized project is discoverable.
    pub fn discover_with_limits(start: &Path, limits: CheckRunLimits) -> Result<Self, MinoError> {
        let plans = PlanService::discover(start)?;
        let root = plans.root().to_path_buf();
        Ok(Self {
            evidence: EvidenceStore::new(&root),
            root,
            plans,
            runner: ProcessRunner::default(),
            limits,
            environment: RunEnvironment::minimal(),
            redactor: Redactor::default(),
        })
    }

    /// Starts the first eligible task in declared order.
    ///
    /// # Errors
    ///
    /// Returns a typed error for stale revisions, missing approval, or an
    /// ineligible task.
    pub fn start_task(
        &self,
        request: PlanMutationRequest,
        task_id: TaskId,
    ) -> Result<PlanOperationReport, MinoError> {
        let stored = self.plans.load_stored(&request.plan_id)?;
        if stored.revision() == request.expected_revision {
            let validation = validate_plan(&self.root, &stored)?;
            if validation.findings.iter().any(|finding| {
                finding.blocking && finding.id.starts_with("POLICY-STANDARD-CONFLICT")
            }) {
                return Err(validation_failure(&validation));
            }
        }
        let baseline = if stored.revision() == request.expected_revision {
            Some(capture_task_workspace_baseline(
                &self.root, &stored, &task_id,
            )?)
        } else {
            None
        };
        let changed_fields = vec![
            "status".to_owned(),
            format!("tasks.{task_id}.status"),
            "extensions.workspace.task_baselines".to_owned(),
        ];
        self.plans.commit_semantic(
            request,
            changed_fields,
            |_| Ok(None),
            move |plan, at| match &baseline {
                Some(baseline) => plan.start_task_with_baseline(&task_id, baseline.clone(), at),
                None => plan.start_task(&task_id, at),
            },
        )
    }

    /// Records a typed checkpoint on the active task.
    ///
    /// # Errors
    ///
    /// Returns a typed error for stale revisions, invalid task state, or empty
    /// checkpoint content.
    pub fn checkpoint(
        &self,
        request: PlanMutationRequest,
        task_id: TaskId,
        kind: CheckpointKind,
        summary: String,
    ) -> Result<PlanOperationReport, MinoError> {
        let actor = request.actor.clone();
        self.plans.commit_semantic(
            request,
            vec!["extensions.execution".to_owned()],
            |_| Ok(None),
            move |plan, at| {
                plan.record_checkpoint(&task_id, kind, summary.clone(), actor.clone(), at)
            },
        )
    }

    /// Records one identified deviation on the active task.
    ///
    /// # Errors
    ///
    /// Returns a typed error for stale revisions, invalid task state, or
    /// incomplete deviation content.
    pub fn record_deviation(
        &self,
        request: PlanMutationRequest,
        task_id: TaskId,
        classification: DeviationClassification,
        summary: String,
        mut affected_paths: Vec<String>,
    ) -> Result<PlanOperationReport, MinoError> {
        affected_paths.sort();
        if affected_paths.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(MinoError::new(
                ErrorCategory::IncompleteOrValidation,
                "Deviation affected paths must be unique",
            ));
        }
        let actor = request.actor.clone();
        self.plans.commit_semantic(
            request,
            vec!["extensions.execution.deviations".to_owned()],
            |plan| {
                plan.execution_state()
                    .and_then(|state| state.next_deviation_id())
                    .map(Some)
                    .map_err(|error| map_domain_error(&error))
            },
            move |plan, at| {
                plan.record_deviation(
                    &task_id,
                    classification,
                    summary.clone(),
                    affected_paths.clone(),
                    actor.clone(),
                    at,
                )
                .map(|_| ())
            },
        )
    }

    /// Lists identified deviations, optionally filtered to one task.
    ///
    /// # Errors
    ///
    /// Returns a typed error for a missing plan or malformed execution state.
    pub fn list_deviations(
        &self,
        plan_id: &PlanId,
        task_id: Option<&TaskId>,
    ) -> Result<DeviationListReport, MinoError> {
        let plan = self.plans.load_verified(plan_id)?;
        let deviations = plan
            .execution_state()
            .map_err(|error| map_domain_error(&error))?
            .deviations()
            .iter()
            .filter(|deviation| task_id.is_none_or(|task_id| deviation.task_id() == task_id))
            .cloned()
            .collect();
        Ok(DeviationListReport {
            kind: DEVIATION_LIST_KIND,
            plan_id: plan.id().clone(),
            revision: plan.revision(),
            deviations,
        })
    }

    /// Resolves one open deviation with current immutable task evidence.
    ///
    /// # Errors
    ///
    /// Returns a typed error for stale revisions, missing/stale/foreign evidence,
    /// invalid task state, or an incompatible deviation lifecycle state.
    pub fn resolve_deviation(
        &self,
        request: PlanMutationRequest,
        deviation_id: String,
        resolution: String,
        mut evidence_refs: Vec<EvidenceId>,
    ) -> Result<PlanOperationReport, MinoError> {
        let changed_fields = vec!["extensions.execution.deviations".to_owned()];
        let stored = self.plans.load_stored(&request.plan_id)?;
        if is_replay_position(&stored, request.expected_revision)? {
            return self.plans.replay_semantic(request, changed_fields);
        }
        let current = self.plans.load_verified(&request.plan_id)?;
        evidence_refs.sort();
        if evidence_refs.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(MinoError::new(
                ErrorCategory::IncompleteOrValidation,
                "Deviation resolution evidence identifiers must be unique",
            ));
        }
        validate_deviation_evidence(&self.evidence, &current, &deviation_id, &evidence_refs)?;
        let actor = request.actor.clone();
        self.plans.commit_semantic(
            request,
            changed_fields,
            |_| Ok(None),
            move |plan, at| {
                plan.resolve_deviation(
                    &deviation_id,
                    actor.clone(),
                    resolution.clone(),
                    evidence_refs.clone(),
                    at,
                )
            },
        )
    }

    /// Rejects one open deviation with a protected decision reference.
    ///
    /// # Errors
    ///
    /// Returns a typed error for stale revisions, incomplete decision fields,
    /// invalid task state, or an incompatible deviation lifecycle state.
    pub fn reject_deviation(
        &self,
        request: PlanMutationRequest,
        deviation_id: String,
        decision_reference: String,
        reason: String,
    ) -> Result<PlanOperationReport, MinoError> {
        let actor = request.actor.clone();
        self.plans.commit_semantic(
            request,
            vec!["extensions.execution.deviations".to_owned()],
            |_| Ok(None),
            move |plan, at| {
                plan.reject_deviation(
                    &deviation_id,
                    actor.clone(),
                    decision_reference.clone(),
                    reason.clone(),
                    at,
                )
            },
        )
    }

    /// Supersedes one open deviation with an applied amendment.
    ///
    /// # Errors
    ///
    /// Returns a typed error for stale revisions, missing/unapplied amendments,
    /// incomplete fields, or an incompatible deviation lifecycle state.
    pub fn supersede_deviation(
        &self,
        request: PlanMutationRequest,
        deviation_id: String,
        amendment_id: String,
        reason: String,
    ) -> Result<PlanOperationReport, MinoError> {
        let actor = request.actor.clone();
        self.plans.commit_semantic(
            request,
            vec!["extensions.execution.deviations".to_owned()],
            |_| Ok(None),
            move |plan, at| {
                plan.supersede_deviation(
                    &deviation_id,
                    actor.clone(),
                    amendment_id.clone(),
                    reason.clone(),
                    at,
                )
            },
        )
    }

    /// Blocks the current Ready or In Progress plan with a resumable reason.
    ///
    /// # Errors
    ///
    /// Returns a typed error for stale revisions, illegal state, or an empty reason.
    pub fn block(
        &self,
        request: PlanMutationRequest,
        reason: String,
    ) -> Result<PlanOperationReport, MinoError> {
        self.plans.commit_semantic(
            request,
            vec![
                "status".to_owned(),
                "blocker".to_owned(),
                "resume_status".to_owned(),
                "tasks".to_owned(),
            ],
            |_| Ok(None),
            move |plan, at| plan.block(reason.clone(), at),
        )
    }

    /// Resumes a plan and its task from recorded blocked state.
    ///
    /// # Errors
    ///
    /// Returns a typed error for stale revisions or invalid resume state.
    pub fn resume(&self, request: PlanMutationRequest) -> Result<PlanOperationReport, MinoError> {
        self.plans.commit_semantic(
            request,
            vec![
                "status".to_owned(),
                "blocker".to_owned(),
                "resume_status".to_owned(),
                "tasks".to_owned(),
            ],
            |_| Ok(None),
            Plan::resume,
        )
    }

    /// Reopens one completed task after required global verification fails.
    ///
    /// # Errors
    ///
    /// Returns a typed error for an illegal lifecycle position, missing task,
    /// empty reason, revision/request conflict, storage failure, or drift.
    pub fn rework_failed_global_verification(
        &self,
        request: PlanMutationRequest,
        task_id: TaskId,
        reason: String,
    ) -> Result<PlanOperationReport, MinoError> {
        self.plans.commit_semantic(
            request,
            vec![
                "status".to_owned(),
                format!("tasks.{task_id}.status"),
                format!("tasks.{task_id}.acceptance_criteria"),
                format!("tasks.{task_id}.verification_checks"),
                format!("tasks.{task_id}.commit_gate"),
                "verification_plan".to_owned(),
                "extensions.workspace.task_baselines".to_owned(),
            ],
            |_| Ok(None),
            move |plan, at| plan.rework_failed_global_verification(&task_id, &reason, at),
        )
    }

    /// Runs one uniquely identified planned check and attaches immutable evidence.
    ///
    /// # Errors
    ///
    /// Returns a typed error for stale phases, unsafe invocations, runner or
    /// evidence-store failures, and incompatible plan state. A terminal command
    /// failure is returned in the successful report so callers can emit exit code 6.
    pub fn run_check(
        &self,
        request: &PlanMutationRequest,
        check_id: &CheckId,
    ) -> Result<CheckExecutionReport, MinoError> {
        let current = self.plans.load_stored(&request.plan_id)?;
        let selected = select_check(&current, check_id)?;
        validate_invocation(&self.root, &selected.check)?;
        let changed_prefix = selected.changed_prefix(check_id);
        let begin_request = phase_request(request, "begin", request.expected_revision)?;
        self.commit_or_replay(begin_request, vec![format!("{changed_prefix}.status")], {
            let check_id = check_id.clone();
            move |plan, at| plan.begin_check_run(&check_id, at)
        })?;

        let leased_revision = request.expected_revision.checked_add(1).ok_or_else(|| {
            MinoError::new(
                ErrorCategory::RevisionConflict,
                "Expected revision overflowed",
            )
        })?;
        let leased_plan = self
            .plans
            .load_snapshot(&request.plan_id, leased_revision)?;
        let leased_check = select_check(&leased_plan, check_id)?;
        if leased_check.check.status() != crate::domain::CheckStatus::Running {
            return Err(MinoError::new(
                ErrorCategory::DriftDetected,
                format!("Check {check_id} is not Running in leased revision {leased_revision}"),
            ));
        }
        let run_request_id = phase_request_id(&request.request_id, "run")?;
        let context = CheckRunContext::new(
            request.plan_id.clone(),
            leased_revision,
            leased_check.task_id.clone(),
            run_request_id,
            request.actor.clone(),
            leased_plan.metadata().updated_at().clone(),
        )
        .map_err(|error| map_domain_error(&error))?;
        let fingerprint =
            capture_workspace_fingerprint(&self.root, &leased_plan, leased_check.task_id.as_ref())?;
        let lease = CheckRunLease::new(
            context,
            &leased_check.check,
            self.limits,
            self.environment.variable_names(),
            self.environment.digest(),
            self.redactor.policy_digest(),
        )
        .and_then(|lease| lease.bind_workspace_fingerprint(fingerprint))
        .map_err(|error| map_domain_error(&error))?;
        let journal_directory = PathBuf::from(".mino")
            .join("plans")
            .join(request.plan_id.as_str())
            .join("runs");
        let journal = CheckRunJournal::new(&self.root, &journal_directory)
            .map_err(|error| map_runner_error(&error))?;
        let journaled = self
            .runner
            .run_journaled(
                &self.root,
                &journal,
                lease,
                &self.environment,
                &self.redactor,
            )
            .map_err(|error| map_runner_error(&error))?;
        let disposition = journaled.disposition().into();
        let run = journaled.into_result();
        if run.outcome() == crate::domain::CheckRunOutcome::CaptureBlocked {
            return self.fail_capture_blocked(request, leased_revision, &changed_prefix, check_id);
        }
        let evidence_command = phase_command(&request.command, "evidence");
        let evidence_request = CommandEvidenceRequest::new(run.clone(), evidence_command)
            .map_err(|error| map_evidence_error(&error))?;
        let evidence = self
            .evidence
            .record_command_result(&evidence_request)
            .map_err(|error| map_evidence_error(&error))?
            .evidence()
            .clone();
        let attach_request = phase_request(request, "attach", leased_revision)?;
        let passed = run.is_success();
        let plan = self.commit_or_replay(
            attach_request,
            vec![
                format!("{changed_prefix}.status"),
                format!("{changed_prefix}.evidence_refs"),
            ],
            {
                let check_id = check_id.clone();
                let evidence_id = evidence.id().clone();
                move |plan, at| plan.record_check_run(&check_id, evidence_id.clone(), passed, at)
            },
        )?;
        Ok(CheckExecutionReport {
            plan,
            evidence,
            run,
            disposition,
        })
    }

    fn fail_capture_blocked(
        &self,
        request: &PlanMutationRequest,
        leased_revision: u64,
        changed_prefix: &str,
        check_id: &CheckId,
    ) -> Result<CheckExecutionReport, MinoError> {
        let attach_request = phase_request(request, "attach", leased_revision)?;
        self.commit_or_replay(attach_request, vec![format!("{changed_prefix}.status")], {
            let check_id = check_id.clone();
            move |plan, at| plan.record_check_capture_blocked(&check_id, at)
        })?;
        Err(MinoError::new(
            ErrorCategory::PolicyViolation,
            format!("Check {check_id} output capture was blocked by the residual credential scan"),
        ))
    }

    fn commit_or_replay<F>(
        &self,
        request: PlanMutationRequest,
        changed_fields: Vec<String>,
        mutation: F,
    ) -> Result<PlanOperationReport, MinoError>
    where
        F: Fn(&mut Plan, Timestamp) -> Result<(), crate::domain::DomainError> + Clone,
    {
        let target_revision = request.expected_revision.checked_add(1).ok_or_else(|| {
            MinoError::new(
                ErrorCategory::RevisionConflict,
                "Expected revision overflowed",
            )
        })?;
        let current = self.plans.load_stored(&request.plan_id)?;
        if current.revision() > target_revision {
            self.plans.replay_semantic(request, changed_fields)
        } else {
            self.plans
                .commit_semantic(request, changed_fields, |_| Ok(None), mutation)
        }
    }
}

#[derive(Clone)]
struct SelectedCheck {
    task_id: Option<TaskId>,
    check: VerificationCheck,
}

impl SelectedCheck {
    fn changed_prefix(&self, check_id: &CheckId) -> String {
        self.task_id.as_ref().map_or_else(
            || format!("verification_plan.{check_id}"),
            |task_id| format!("tasks.{task_id}.verification_checks.{check_id}"),
        )
    }
}

fn select_check(plan: &Plan, check_id: &CheckId) -> Result<SelectedCheck, MinoError> {
    let mut matches = plan
        .tasks()
        .iter()
        .flat_map(|task| {
            task.verification_checks()
                .iter()
                .filter(move |check| check.id() == check_id)
                .map(move |check| SelectedCheck {
                    task_id: Some(task.id().clone()),
                    check: check.clone(),
                })
        })
        .chain(
            plan.global_verification()
                .iter()
                .filter(|check| check.id() == check_id)
                .map(|check| SelectedCheck {
                    task_id: None,
                    check: check.clone(),
                }),
        );
    let selected = matches.next().ok_or_else(|| {
        MinoError::new(
            ErrorCategory::IncompleteOrValidation,
            format!("Check {check_id} does not exist"),
        )
    })?;
    if matches.next().is_some() {
        return Err(MinoError::new(
            ErrorCategory::DriftDetected,
            format!("Check {check_id} is not globally unique"),
        ));
    }
    Ok(selected)
}

fn phase_request(
    request: &PlanMutationRequest,
    phase: &str,
    expected_revision: u64,
) -> Result<PlanMutationRequest, MinoError> {
    Ok(PlanMutationRequest {
        plan_id: request.plan_id.clone(),
        expected_revision,
        request_id: phase_request_id(&request.request_id, phase)?,
        actor: request.actor.clone(),
        command: phase_command(&request.command, phase),
        updated_at: request.updated_at.clone(),
    })
}

fn phase_request_id(request_id: &RequestId, phase: &str) -> Result<RequestId, MinoError> {
    let digest = sha256_digest(format!("{request_id}:{phase}").as_bytes());
    let value = &digest["sha256:".len().."sha256:".len() + 32];
    RequestId::parse(format!(
        "{}-{}-{}-{}-{}",
        &value[0..8],
        &value[8..12],
        &value[12..16],
        &value[16..20],
        &value[20..32]
    ))
    .map_err(|error| map_domain_error(&error))
}

fn phase_command(command: &[String], phase: &str) -> Vec<String> {
    let mut command = command.to_vec();
    command.extend(["--mino-phase".to_owned(), phase.to_owned()]);
    command
}

fn validate_invocation(root: &Path, check: &VerificationCheck) -> Result<(), MinoError> {
    let program = check.command().first().ok_or_else(|| {
        MinoError::new(
            ErrorCategory::IncompleteOrValidation,
            "Verification check has no executable",
        )
    })?;
    let normalized = Path::new(program)
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or(program)
        .to_ascii_lowercase();
    if matches!(
        normalized.as_str(),
        "ash"
            | "bash"
            | "cmd"
            | "csh"
            | "dash"
            | "fish"
            | "ksh"
            | "powershell"
            | "pwsh"
            | "sh"
            | "tcsh"
            | "zsh"
    ) {
        return Err(MinoError::new(
            ErrorCategory::PolicyViolation,
            format!("Shell executable {program} is not permitted for planned checks"),
        ));
    }
    let relative = Path::new(check.cwd());
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::CurDir | Component::Normal(_)))
    {
        return Err(MinoError::new(
            ErrorCategory::IncompleteOrValidation,
            "Check working directory must be project-relative without parent traversal",
        ));
    }
    let canonical_root = root.canonicalize().map_err(|error| {
        MinoError::new(
            ErrorCategory::EnvironmentUnavailable,
            format!("Failed to resolve project root {}: {error}", root.display()),
        )
    })?;
    let working_directory = canonical_root
        .join(relative)
        .canonicalize()
        .map_err(|error| {
            MinoError::new(
                ErrorCategory::IncompleteOrValidation,
                format!("Failed to resolve check working directory: {error}"),
            )
        })?;
    if !working_directory.starts_with(&canonical_root) || !working_directory.is_dir() {
        return Err(MinoError::new(
            ErrorCategory::IncompleteOrValidation,
            "Check working directory resolves outside the project or is not a directory",
        ));
    }
    Ok(())
}

fn validate_deviation_evidence(
    store: &EvidenceStore,
    plan: &Plan,
    deviation_id: &str,
    evidence_refs: &[EvidenceId],
) -> Result<(), MinoError> {
    if evidence_refs.is_empty() {
        return Err(MinoError::new(
            ErrorCategory::IncompleteOrValidation,
            "Deviation resolution requires at least one evidence reference",
        ));
    }
    let execution = plan
        .execution_state()
        .map_err(|error| map_domain_error(&error))?;
    let deviation = execution.deviation(deviation_id).ok_or_else(|| {
        MinoError::new(
            ErrorCategory::IncompleteOrValidation,
            format!("Deviation {deviation_id} does not exist"),
        )
    })?;
    let records = store
        .list(plan.id())
        .map_err(|error| map_evidence_error(&error))?;
    for evidence_id in evidence_refs {
        let evidence = records
            .iter()
            .find(|evidence| evidence.id() == evidence_id)
            .ok_or_else(|| {
                MinoError::new(
                    ErrorCategory::IncompleteOrValidation,
                    format!(
                        "Evidence {evidence_id} does not exist in plan {}",
                        plan.id()
                    ),
                )
            })?;
        let is_superseded = records
            .iter()
            .any(|record| record.supersedes() == Some(evidence_id));
        if evidence.task_id() != Some(deviation.task_id())
            || evidence
                .captured_revision()
                .is_none_or(|revision| revision > plan.revision())
            || plan.is_evidence_stale(evidence_id)
            || is_superseded
        {
            return Err(MinoError::new(
                ErrorCategory::IncompleteOrValidation,
                format!(
                    "Evidence {evidence_id} is not current task evidence for deviation {deviation_id}"
                ),
            ));
        }
    }
    Ok(())
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
    use crate::domain::DomainErrorKind;
    let category = match error.kind() {
        DomainErrorKind::ApprovalRequired => ErrorCategory::ApprovalRequired,
        DomainErrorKind::InvalidTransition
        | DomainErrorKind::TaskOrderViolation
        | DomainErrorKind::ActiveTaskExists => ErrorCategory::PolicyViolation,
        DomainErrorKind::InvalidIdentifier
        | DomainErrorKind::InvalidTimestamp
        | DomainErrorKind::UnsupportedSchemaVersion
        | DomainErrorKind::UnsupportedProtocolVersion
        | DomainErrorKind::DuplicateTask
        | DomainErrorKind::TaskNotFound
        | DomainErrorKind::UnmetDependencies
        | DomainErrorKind::InvariantViolation => ErrorCategory::IncompleteOrValidation,
    };
    MinoError::new(category, error.to_string())
}

fn map_runner_error(error: &RunnerError) -> MinoError {
    let category = match error.kind() {
        RunnerErrorKind::InvalidRequest => ErrorCategory::IncompleteOrValidation,
        RunnerErrorKind::AlreadyRunning | RunnerErrorKind::JournalConflict => {
            ErrorCategory::RevisionConflict
        }
        RunnerErrorKind::CorruptJournal => ErrorCategory::DriftDetected,
        RunnerErrorKind::Io | RunnerErrorKind::Serialization | RunnerErrorKind::CaptureFailed => {
            ErrorCategory::EnvironmentUnavailable
        }
    };
    MinoError::new(category, error.to_string())
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
