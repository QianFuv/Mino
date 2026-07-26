//! Ordered execution CLI adapter.

use std::path::{Path, PathBuf};

use clap::{Args, Subcommand, ValueEnum};
use serde::Serialize;

use crate::application::agent::AgentService;
use crate::application::completion::CompletionService;
use crate::application::execution::ExecutionService;
use crate::application::monitor::{MonitorBounds, MonitorRequest, MonitorService};
use crate::application::plan::PlanMutationRequest;
use crate::commands::CommandResponse;
use crate::domain::{
    CheckId, CheckpointKind, CriterionId, EvidenceId, PlanId, RequestId, TaskId, Timestamp,
};
use crate::{ErrorCategory, MinoError};

#[derive(Debug, Subcommand)]
pub(crate) enum ExecAction {
    /// Start the first eligible task in declared order.
    Start(StartArguments),
    /// Record one typed execution checkpoint.
    Checkpoint(CheckpointArguments),
    /// Run one planned verification check.
    Check(CheckArguments),
    /// Bind compatible immutable evidence to acceptance criteria.
    Criterion(CriterionArguments),
    /// Complete the active task after every evidence and scope gate passes.
    Complete(CompleteArguments),
    /// Block the current plan with a resumable reason.
    Block(BlockArguments),
    /// Resume a plan from its recorded blocked state.
    Resume(ResumeArguments),
    /// Finish global verification and move the plan to Review.
    Finish(FinishArguments),
}

#[derive(Clone, Debug, Args)]
struct MutationArguments {
    /// Target plan identifier.
    #[arg(long)]
    plan: String,
    /// Required optimistic-concurrency revision.
    #[arg(long)]
    expect_revision: u64,
    /// Idempotency UUID for this execution request.
    #[arg(long)]
    request_id: String,
    /// Actor recorded in the event and evidence logs.
    #[arg(long, default_value = "user")]
    actor: String,
}

#[derive(Debug, Args)]
pub(crate) struct StartArguments {
    #[command(flatten)]
    mutation: MutationArguments,
    /// Eligible task identifier.
    #[arg(long)]
    task: String,
}

#[derive(Debug, Args)]
pub(crate) struct CheckpointArguments {
    #[command(flatten)]
    mutation: MutationArguments,
    /// Active task identifier.
    #[arg(long)]
    task: String,
    /// Stable checkpoint classification.
    #[arg(long, value_enum)]
    kind: CheckpointKindArgument,
    /// Human-meaningful completed milestone.
    #[arg(long)]
    summary: String,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum CheckpointKindArgument {
    Inspection,
    Approach,
    Implementation,
    Verification,
    Blocker,
    Deviation,
}

impl From<CheckpointKindArgument> for CheckpointKind {
    fn from(value: CheckpointKindArgument) -> Self {
        match value {
            CheckpointKindArgument::Inspection => Self::Inspection,
            CheckpointKindArgument::Approach => Self::Approach,
            CheckpointKindArgument::Implementation => Self::Implementation,
            CheckpointKindArgument::Verification => Self::Verification,
            CheckpointKindArgument::Blocker => Self::Blocker,
            CheckpointKindArgument::Deviation => Self::Deviation,
        }
    }
}

#[derive(Debug, Args)]
pub(crate) struct CheckArguments {
    #[command(subcommand)]
    action: CheckAction,
}

#[derive(Debug, Subcommand)]
enum CheckAction {
    /// Execute, journal, and attach evidence for one planned check.
    Run(CheckRunArguments),
    /// Re-run one planned check under finite attempt, interval, and deadline bounds.
    Monitor(CheckMonitorArguments),
}

#[derive(Debug, Args)]
struct CheckRunArguments {
    #[command(flatten)]
    mutation: MutationArguments,
    /// Globally unique planned check identifier.
    #[arg(long)]
    check: String,
}

#[derive(Debug, Args)]
struct CheckMonitorArguments {
    #[command(flatten)]
    mutation: MutationArguments,
    /// Globally unique planned check identifier.
    #[arg(long)]
    check: String,
    /// Maximum number of check invocations, from 1 through 100.
    #[arg(long)]
    max_attempts: u32,
    /// Delay between failed attempts, from 1 through 60000 milliseconds.
    #[arg(long)]
    interval_milliseconds: u64,
    /// Complete attempt-and-wait elapsed bound, up to 24 hours.
    #[arg(long)]
    deadline_milliseconds: u64,
    /// Optional project-relative regular file whose presence requests cancellation.
    #[arg(long)]
    cancel_file: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub(crate) struct CriterionArguments {
    #[command(subcommand)]
    action: CriterionAction,
}

#[derive(Debug, Subcommand)]
enum CriterionAction {
    /// Mark a criterion satisfied by one compatible evidence record.
    Pass(CriterionPassArguments),
}

#[derive(Debug, Args)]
struct CriterionPassArguments {
    #[command(flatten)]
    mutation: MutationArguments,
    /// Globally unique acceptance-criterion identifier.
    #[arg(long)]
    criterion: String,
    /// Immutable evidence identifier to bind.
    #[arg(long)]
    evidence: String,
}

#[derive(Debug, Args)]
pub(crate) struct CompleteArguments {
    #[command(flatten)]
    mutation: MutationArguments,
    /// Active task identifier.
    #[arg(long)]
    task: String,
}

#[derive(Debug, Args)]
pub(crate) struct BlockArguments {
    #[command(flatten)]
    mutation: MutationArguments,
    /// Non-empty reason required before execution can resume.
    #[arg(long)]
    reason: String,
}

#[derive(Debug, Args)]
pub(crate) struct ResumeArguments {
    #[command(flatten)]
    mutation: MutationArguments,
}

#[derive(Debug, Args)]
pub(crate) struct FinishArguments {
    #[command(flatten)]
    mutation: MutationArguments,
}

pub(crate) fn execute(start: &Path, action: ExecAction) -> Result<CommandResponse, MinoError> {
    let service = ExecutionService::discover(start)?;
    match action {
        ExecAction::Start(arguments) => {
            let command = mutation_command(
                &["start"],
                &arguments.mutation,
                vec!["--task".to_owned(), arguments.task.clone()],
            );
            let request = mutation_request(arguments.mutation, command)?;
            let report = service.start_task(request, parse_task_id(&arguments.task)?)?;
            response_with_guidance(start, "Task execution started.", report)
        }
        ExecAction::Checkpoint(arguments) => {
            let kind_name = arguments
                .kind
                .to_possible_value()
                .map_or_else(|| "unknown".to_owned(), |value| value.get_name().to_owned());
            let command = mutation_command(
                &["checkpoint"],
                &arguments.mutation,
                vec![
                    "--task".to_owned(),
                    arguments.task.clone(),
                    "--kind".to_owned(),
                    kind_name,
                    "--summary".to_owned(),
                    arguments.summary.clone(),
                ],
            );
            let request = mutation_request(arguments.mutation, command)?;
            let report = service.checkpoint(
                request,
                parse_task_id(&arguments.task)?,
                arguments.kind.into(),
                arguments.summary,
            )?;
            response_with_guidance(start, "Execution checkpoint recorded.", report)
        }
        ExecAction::Check(arguments) => match arguments.action {
            CheckAction::Run(arguments) => run_check(start, &service, arguments),
            CheckAction::Monitor(arguments) => monitor_check(start, arguments),
        },
        ExecAction::Criterion(arguments) => match arguments.action {
            CriterionAction::Pass(arguments) => pass_criterion(start, arguments),
        },
        ExecAction::Complete(arguments) => complete_task(start, arguments),
        ExecAction::Block(arguments) => {
            let command = mutation_command(
                &["block"],
                &arguments.mutation,
                vec!["--reason".to_owned(), arguments.reason.clone()],
            );
            let request = mutation_request(arguments.mutation, command)?;
            let report = service.block(request, arguments.reason)?;
            response_with_guidance(start, "Plan execution blocked.", report)
        }
        ExecAction::Resume(arguments) => {
            let command = mutation_command(&["resume"], &arguments.mutation, Vec::new());
            let request = mutation_request(arguments.mutation, command)?;
            let report = service.resume(request)?;
            response_with_guidance(start, "Plan execution resumed.", report)
        }
        ExecAction::Finish(arguments) => finish(start, arguments),
    }
}

fn pass_criterion(
    start: &Path,
    arguments: CriterionPassArguments,
) -> Result<CommandResponse, MinoError> {
    let command = mutation_command(
        &["criterion", "pass"],
        &arguments.mutation,
        vec![
            "--criterion".to_owned(),
            arguments.criterion.clone(),
            "--evidence".to_owned(),
            arguments.evidence.clone(),
        ],
    );
    let request = mutation_request(arguments.mutation, command)?;
    let report = CompletionService::discover(start)?.pass_criterion(
        request,
        parse_criterion_id(&arguments.criterion)?,
        parse_evidence_id(&arguments.evidence)?,
    )?;
    response_with_guidance(start, "Criterion evidence attached.", report)
}

fn complete_task(start: &Path, arguments: CompleteArguments) -> Result<CommandResponse, MinoError> {
    let command = mutation_command(
        &["complete"],
        &arguments.mutation,
        vec!["--task".to_owned(), arguments.task.clone()],
    );
    let request = mutation_request(arguments.mutation, command)?;
    let report = CompletionService::discover(start)?
        .complete_task(request, parse_task_id(&arguments.task)?)?;
    response_with_guidance(start, "Task execution completed.", report)
}

fn finish(start: &Path, arguments: FinishArguments) -> Result<CommandResponse, MinoError> {
    let command = mutation_command(&["finish"], &arguments.mutation, Vec::new());
    let request = mutation_request(arguments.mutation, command)?;
    let report = CompletionService::discover(start)?.finish(request)?;
    response_with_guidance(start, "Plan execution finished and entered Review.", report)
}

fn run_check(
    start: &Path,
    service: &ExecutionService,
    arguments: CheckRunArguments,
) -> Result<CommandResponse, MinoError> {
    let command = mutation_command(
        &["check", "run"],
        &arguments.mutation,
        vec!["--check".to_owned(), arguments.check.clone()],
    );
    let request = mutation_request(arguments.mutation, command)?;
    let check_id = parse_check_id(&arguments.check)?;
    let report = service.run_check(&request, &check_id)?;
    let guidance = AgentService::discover(start)?.context()?;
    if !report.is_success() {
        return Err(MinoError::new(
            ErrorCategory::CheckFailed,
            format!(
                "Verification check {check_id} completed with {:?}",
                report.run().outcome()
            ),
        )
        .with_remediation(
            vec![format!("verification_checks.{check_id}")],
            guidance.next_actions,
        )
        .with_details(serde_json::json!({ "execution": report })));
    }
    response(
        "Verification check passed and evidence was attached.",
        report,
        guidance.next_actions,
    )
}

fn monitor_check(
    start: &Path,
    arguments: CheckMonitorArguments,
) -> Result<CommandResponse, MinoError> {
    let bounds = MonitorBounds::new(
        arguments.max_attempts,
        arguments.interval_milliseconds,
        arguments.deadline_milliseconds,
    )?;
    let mut extra = vec![
        "--check".to_owned(),
        arguments.check.clone(),
        "--max-attempts".to_owned(),
        arguments.max_attempts.to_string(),
        "--interval-milliseconds".to_owned(),
        arguments.interval_milliseconds.to_string(),
        "--deadline-milliseconds".to_owned(),
        arguments.deadline_milliseconds.to_string(),
    ];
    if let Some(path) = &arguments.cancel_file {
        extra.extend([
            "--cancel-file".to_owned(),
            path.to_string_lossy().into_owned(),
        ]);
    }
    let command = mutation_command(&["check", "monitor"], &arguments.mutation, extra);
    let check_id = parse_check_id(&arguments.check)?;
    let report = MonitorService::discover(start)?.run(MonitorRequest {
        plan_id: parse_plan_id(&arguments.mutation.plan)?,
        expected_revision: arguments.mutation.expect_revision,
        request_id: parse_request_id(&arguments.mutation.request_id)?,
        actor: arguments.mutation.actor,
        command,
        check_id: check_id.clone(),
        bounds,
        cancel_file: arguments.cancel_file,
    })?;
    let guidance = AgentService::discover(start)?.context()?;
    if !report.is_success() {
        return Err(MinoError::new(
            ErrorCategory::CheckFailed,
            format!(
                "Bounded monitoring for check {check_id} stopped with {:?}",
                report.terminal_reason
            ),
        )
        .with_remediation(
            vec![format!("verification_checks.{check_id}")],
            guidance.next_actions,
        )
        .with_details(serde_json::json!({ "monitor": report })));
    }
    response(
        "Bounded monitoring stopped after the planned check passed.",
        report,
        guidance.next_actions,
    )
}

fn mutation_request(
    arguments: MutationArguments,
    command: Vec<String>,
) -> Result<PlanMutationRequest, MinoError> {
    Ok(PlanMutationRequest {
        plan_id: parse_plan_id(&arguments.plan)?,
        expected_revision: arguments.expect_revision,
        request_id: parse_request_id(&arguments.request_id)?,
        actor: arguments.actor,
        command,
        updated_at: Timestamp::now_utc(),
    })
}

fn mutation_command(
    path: &[&str],
    arguments: &MutationArguments,
    extra: Vec<String>,
) -> Vec<String> {
    let mut command = vec!["mino".to_owned(), "exec".to_owned()];
    command.extend(path.iter().map(|part| (*part).to_owned()));
    command.extend([
        "--plan".to_owned(),
        arguments.plan.clone(),
        "--expect-revision".to_owned(),
        arguments.expect_revision.to_string(),
        "--request-id".to_owned(),
        arguments.request_id.clone(),
        "--actor".to_owned(),
        arguments.actor.clone(),
    ]);
    command.extend(extra);
    command
}

fn response_with_guidance<T: Serialize>(
    start: &Path,
    message: &str,
    payload: T,
) -> Result<CommandResponse, MinoError> {
    let guidance = AgentService::discover(start)?.context()?;
    response(message, payload, guidance.next_actions)
}

fn response<T: Serialize>(
    message: &str,
    payload: T,
    next_actions: Vec<crate::NextAction>,
) -> Result<CommandResponse, MinoError> {
    let payload = serde_json::to_value(payload).map_err(|error| {
        MinoError::new(
            ErrorCategory::EnvironmentUnavailable,
            format!("Failed to serialize execution result: {error}"),
        )
    })?;
    Ok(CommandResponse {
        message: message.to_owned(),
        complete: true,
        payload,
        missing: Vec::new(),
        next_actions,
    })
}

fn parse_plan_id(value: &str) -> Result<PlanId, MinoError> {
    PlanId::parse(value).map_err(|error| domain_error(&error))
}

fn parse_task_id(value: &str) -> Result<TaskId, MinoError> {
    TaskId::parse(value).map_err(|error| domain_error(&error))
}

fn parse_check_id(value: &str) -> Result<CheckId, MinoError> {
    CheckId::parse(value).map_err(|error| domain_error(&error))
}

fn parse_criterion_id(value: &str) -> Result<CriterionId, MinoError> {
    CriterionId::parse(value).map_err(|error| domain_error(&error))
}

fn parse_evidence_id(value: &str) -> Result<EvidenceId, MinoError> {
    EvidenceId::parse(value).map_err(|error| domain_error(&error))
}

fn parse_request_id(value: &str) -> Result<RequestId, MinoError> {
    RequestId::parse(value).map_err(|error| domain_error(&error))
}

fn domain_error(error: &crate::domain::DomainError) -> MinoError {
    MinoError::new(ErrorCategory::IncompleteOrValidation, error.to_string())
}
