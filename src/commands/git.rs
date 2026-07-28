//! Git inspection and active-binding CLI adapter.

use std::path::Path;

use clap::{Args, Subcommand, ValueEnum};

use crate::application::git_binding::GitBindingService;
use crate::application::git_branch::GitBranchService;
use crate::application::git_commit::GitCommitService;
use crate::application::git_hooks::GitHookService;
use crate::application::git_readiness::GitReadinessService;
use crate::application::plan::PlanMutationRequest;
use crate::commands::CommandResponse;
use crate::domain::{PlanId, RequestId, TaskId, Timestamp};
use crate::git::GitHookName;
use crate::{ErrorCategory, MinoError};

#[derive(Debug, Subcommand)]
pub(crate) enum GitAction {
    /// Inspect repository, worktree, HEAD, index, and active binding facts.
    Inspect(InspectArguments),
    /// Bind one plan to the exact current worktree and branch or detached HEAD.
    Bind(BindArguments),
    /// Propose or create the deterministic Git branch for a plan.
    Branch(BranchArguments),
    /// Create or recover one approved task-level commit.
    Commit(CommitArguments),
    /// Manage required task commit-gate exceptions.
    Gate(GateArguments),
    /// Inspect, install, or invoke optional advisory repository hooks.
    Hook(HookArguments),
    /// Inspect or explicitly refresh plan-bound live Git readiness.
    Readiness(ReadinessArguments),
}

#[derive(Debug, Args)]
pub(crate) struct InspectArguments {
    /// Optional plan whose binding relationship should be reported.
    #[arg(long)]
    plan: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct BindArguments {
    /// Plan to bind to the current worktree identity.
    #[arg(long)]
    plan: String,
    /// Explicitly select the current worktree; no other target is supported.
    #[arg(long)]
    current: bool,
}

#[derive(Debug, Args)]
pub(crate) struct BranchArguments {
    #[command(subcommand)]
    action: GitBranchAction,
}

#[derive(Debug, Subcommand)]
enum GitBranchAction {
    /// Return a deterministic branch proposal without mutation.
    Propose(BranchProposeArguments),
    /// Create the proposed branch after an explicit approval reference.
    Create(BranchCreateArguments),
}

#[derive(Debug, Args)]
struct BranchProposeArguments {
    /// Plan for which a Git branch should be proposed.
    #[arg(long)]
    plan: String,
}

#[derive(Debug, Args)]
struct BranchCreateArguments {
    /// Plan for which the approved Git branch should be created.
    #[arg(long)]
    plan: String,
    /// Auditable external reference for explicit branch approval.
    #[arg(long)]
    approval_ref: String,
    /// Optional explicit branch name, which must equal the proposal.
    #[arg(long)]
    branch: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct CommitArguments {
    #[command(subcommand)]
    action: Option<GitCommitAction>,
    /// Approved plan containing the task commit gate.
    #[arg(long)]
    plan: Option<String>,
    /// Done task whose exact commit gate should execute.
    #[arg(long)]
    task: Option<String>,
}

#[derive(Debug, Subcommand)]
enum GitCommitAction {
    /// Verify and record a commit created outside Mino.
    RecordManual(RecordManualArguments),
}

#[derive(Debug, Args)]
struct RecordManualArguments {
    /// Approved plan containing the task commit gate.
    #[arg(long)]
    plan: String,
    /// Done task whose required gate should record the commit.
    #[arg(long)]
    task: String,
    /// Full commit object ID already at current branch HEAD.
    #[arg(long)]
    commit: String,
    /// Auditable external reference approving this manual commit.
    #[arg(long)]
    approval_ref: String,
    /// Required optimistic-concurrency revision.
    #[arg(long)]
    expect_revision: u64,
    /// Idempotency UUID for this record request.
    #[arg(long)]
    request_id: String,
    /// Actor recorded in the event and evidence logs.
    #[arg(long, default_value = "user")]
    actor: String,
}

#[derive(Debug, Args)]
pub(crate) struct GateArguments {
    #[command(subcommand)]
    action: GitGateAction,
}

#[derive(Debug, Subcommand)]
enum GitGateAction {
    /// Satisfy a required gate with an explicitly approved exception.
    Skip(SkipGateArguments),
}

#[derive(Debug, Args)]
struct SkipGateArguments {
    /// Approved plan containing the task commit gate.
    #[arg(long)]
    plan: String,
    /// Done task whose required commit gate should be skipped.
    #[arg(long)]
    task: String,
    /// Auditable external reference approving the exception.
    #[arg(long)]
    approval_ref: String,
    /// Human-readable reason for the exception.
    #[arg(long)]
    reason: String,
    /// Required optimistic-concurrency revision.
    #[arg(long)]
    expect_revision: u64,
    /// Idempotency UUID for this skip request.
    #[arg(long)]
    request_id: String,
    /// Actor recorded in the event and evidence logs.
    #[arg(long, default_value = "user")]
    actor: String,
}

#[derive(Debug, Args)]
pub(crate) struct HookArguments {
    #[command(subcommand)]
    action: GitHookAction,
}

#[derive(Debug, Args)]
pub(crate) struct ReadinessArguments {
    #[command(subcommand)]
    action: GitReadinessAction,
}

#[derive(Debug, Subcommand)]
enum GitReadinessAction {
    /// Capture current Git facts as one revisioned plan mutation.
    Refresh(ReadinessRefreshArguments),
}

#[derive(Debug, Args)]
struct ReadinessRefreshArguments {
    /// Draft or Ready plan whose readiness should be refreshed.
    #[arg(long)]
    plan: String,
    /// Required optimistic-concurrency revision.
    #[arg(long)]
    expect_revision: u64,
    /// Idempotency UUID for this refresh request.
    #[arg(long)]
    request_id: String,
    /// Actor recorded in the event log.
    #[arg(long, default_value = "user")]
    actor: String,
}

#[derive(Debug, Subcommand)]
enum GitHookAction {
    /// Return the hash-bound installation proposal without mutation.
    Propose,
    /// Return current hook ownership and content status without mutation.
    Status,
    /// Install or repair only Mino-owned hooks after explicit approval.
    Install(HookInstallArguments),
    /// Emit advisory staged/commit observations without mutation.
    Run(HookRunArguments),
}

#[derive(Debug, Args)]
struct HookInstallArguments {
    /// Exact current digest returned by `git hook propose`.
    #[arg(long)]
    proposal_hash: String,
    /// Auditable external reference for explicit hook installation approval.
    #[arg(long)]
    approval_ref: String,
}

#[derive(Debug, Args)]
struct HookRunArguments {
    /// Advisory Git hook being invoked.
    #[arg(long, value_enum)]
    hook: HookNameArgument,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum HookNameArgument {
    PreCommit,
    PostCommit,
}

impl From<HookNameArgument> for GitHookName {
    fn from(value: HookNameArgument) -> Self {
        match value {
            HookNameArgument::PreCommit => Self::PreCommit,
            HookNameArgument::PostCommit => Self::PostCommit,
        }
    }
}

pub(crate) fn execute(start: &Path, action: GitAction) -> Result<CommandResponse, MinoError> {
    let service = GitBindingService::discover(start)?;
    let (message, payload) = match action {
        GitAction::Inspect(arguments) => (
            "Git repository and active binding inspected.",
            serde_json::to_value(
                service.inspect(arguments.plan.as_deref().map(parse_plan_id).transpose()?)?,
            ),
        ),
        GitAction::Bind(arguments) => {
            if !arguments.current {
                return Err(MinoError::new(
                    ErrorCategory::IncompleteOrValidation,
                    "Git binding requires --current",
                ));
            }
            (
                "Plan bound to the current Git worktree.",
                serde_json::to_value(service.bind_current(parse_plan_id(&arguments.plan)?)?),
            )
        }
        GitAction::Branch(arguments) => execute_branch(start, arguments.action)?,
        GitAction::Commit(arguments) => execute_commit(start, arguments)?,
        GitAction::Gate(arguments) => execute_gate(start, arguments.action)?,
        GitAction::Hook(arguments) => return execute_hook(start, arguments.action),
        GitAction::Readiness(arguments) => execute_readiness(start, arguments.action)?,
    };
    let payload = payload.map_err(|error| {
        MinoError::new(
            ErrorCategory::EnvironmentUnavailable,
            format!("Failed to serialize Git result: {error}"),
        )
    })?;
    Ok(CommandResponse {
        message: message.to_owned(),
        complete: true,
        payload,
        missing: Vec::new(),
        next_actions: Vec::new(),
    })
}

fn execute_readiness(
    start: &Path,
    action: GitReadinessAction,
) -> Result<(&'static str, Result<serde_json::Value, serde_json::Error>), MinoError> {
    let service = GitReadinessService::discover(start)?;
    match action {
        GitReadinessAction::Refresh(arguments) => {
            let request = PlanMutationRequest {
                plan_id: parse_plan_id(&arguments.plan)?,
                expected_revision: arguments.expect_revision,
                request_id: parse_request_id(&arguments.request_id)?,
                actor: arguments.actor.clone(),
                command: vec![
                    "mino".to_owned(),
                    "git".to_owned(),
                    "readiness".to_owned(),
                    "refresh".to_owned(),
                    "--plan".to_owned(),
                    arguments.plan,
                    "--expect-revision".to_owned(),
                    arguments.expect_revision.to_string(),
                    "--request-id".to_owned(),
                    arguments.request_id,
                    "--actor".to_owned(),
                    arguments.actor,
                ],
                updated_at: Timestamp::now_utc(),
            };
            Ok((
                "Git readiness refreshed as a plan revision.",
                serde_json::to_value(service.refresh(request)?),
            ))
        }
    }
}

fn execute_commit(
    start: &Path,
    arguments: CommitArguments,
) -> Result<(&'static str, Result<serde_json::Value, serde_json::Error>), MinoError> {
    let service = GitCommitService::discover(start)?;
    match arguments.action {
        Some(GitCommitAction::RecordManual(arguments)) => {
            let request = manual_commit_request(&arguments)?;
            Ok((
                "Manual task commit verified, evidenced, and recorded.",
                serde_json::to_value(service.record_manual(
                    request,
                    &parse_task_id(&arguments.task)?,
                    &arguments.commit,
                    &arguments.approval_ref,
                )?),
            ))
        }
        None => {
            let plan = arguments.plan.as_deref().ok_or_else(|| {
                MinoError::new(
                    ErrorCategory::IncompleteOrValidation,
                    "Automatic git commit requires --plan",
                )
            })?;
            let task = arguments.task.as_deref().ok_or_else(|| {
                MinoError::new(
                    ErrorCategory::IncompleteOrValidation,
                    "Automatic git commit requires --task",
                )
            })?;
            Ok((
                "Task commit created, evidenced, and recorded.",
                serde_json::to_value(service.commit(&parse_plan_id(plan)?, &parse_task_id(task)?)?),
            ))
        }
    }
}

fn execute_gate(
    start: &Path,
    action: GitGateAction,
) -> Result<(&'static str, Result<serde_json::Value, serde_json::Error>), MinoError> {
    let service = GitCommitService::discover(start)?;
    match action {
        GitGateAction::Skip(arguments) => {
            let request = skip_gate_request(&arguments)?;
            Ok((
                "Required task commit gate skipped with approved evidence.",
                serde_json::to_value(service.skip_gate(
                    request,
                    &parse_task_id(&arguments.task)?,
                    &arguments.approval_ref,
                    &arguments.reason,
                )?),
            ))
        }
    }
}

fn manual_commit_request(
    arguments: &RecordManualArguments,
) -> Result<PlanMutationRequest, MinoError> {
    Ok(PlanMutationRequest {
        plan_id: parse_plan_id(&arguments.plan)?,
        expected_revision: arguments.expect_revision,
        request_id: parse_request_id(&arguments.request_id)?,
        actor: arguments.actor.clone(),
        command: vec![
            "mino".to_owned(),
            "git".to_owned(),
            "commit".to_owned(),
            "record-manual".to_owned(),
            "--plan".to_owned(),
            arguments.plan.clone(),
            "--task".to_owned(),
            arguments.task.clone(),
            "--commit".to_owned(),
            arguments.commit.clone(),
            "--approval-ref".to_owned(),
            arguments.approval_ref.clone(),
            "--expect-revision".to_owned(),
            arguments.expect_revision.to_string(),
            "--request-id".to_owned(),
            arguments.request_id.clone(),
            "--actor".to_owned(),
            arguments.actor.clone(),
        ],
        updated_at: Timestamp::now_utc(),
    })
}

fn skip_gate_request(arguments: &SkipGateArguments) -> Result<PlanMutationRequest, MinoError> {
    Ok(PlanMutationRequest {
        plan_id: parse_plan_id(&arguments.plan)?,
        expected_revision: arguments.expect_revision,
        request_id: parse_request_id(&arguments.request_id)?,
        actor: arguments.actor.clone(),
        command: vec![
            "mino".to_owned(),
            "git".to_owned(),
            "gate".to_owned(),
            "skip".to_owned(),
            "--plan".to_owned(),
            arguments.plan.clone(),
            "--task".to_owned(),
            arguments.task.clone(),
            "--approval-ref".to_owned(),
            arguments.approval_ref.clone(),
            "--reason".to_owned(),
            arguments.reason.clone(),
            "--expect-revision".to_owned(),
            arguments.expect_revision.to_string(),
            "--request-id".to_owned(),
            arguments.request_id.clone(),
            "--actor".to_owned(),
            arguments.actor.clone(),
        ],
        updated_at: Timestamp::now_utc(),
    })
}

fn execute_hook(start: &Path, action: GitHookAction) -> Result<CommandResponse, MinoError> {
    let service = GitHookService::discover(start)?;
    let (message, payload, next_actions) = match action {
        GitHookAction::Propose => (
            "Advisory Git hook proposal generated without mutation.".to_owned(),
            serde_json::to_value(service.propose()?),
            Vec::new(),
        ),
        GitHookAction::Status => (
            "Advisory Git hook status inspected without mutation.".to_owned(),
            serde_json::to_value(service.status()?),
            Vec::new(),
        ),
        GitHookAction::Install(arguments) => (
            "Approved advisory Git hooks installed and verified.".to_owned(),
            serde_json::to_value(
                service.install(&arguments.proposal_hash, &arguments.approval_ref)?,
            ),
            Vec::new(),
        ),
        GitHookAction::Run(arguments) => {
            let report = service.run(arguments.hook.into())?;
            let message = format!(
                "Advisory {} observation: {}",
                report.hook.as_str(),
                report
                    .diagnostics
                    .iter()
                    .map(|diagnostic| diagnostic.message.as_str())
                    .collect::<Vec<_>>()
                    .join("; ")
            );
            let next_actions = report.next_actions.clone();
            (message, serde_json::to_value(report), next_actions)
        }
    };
    let payload = payload.map_err(|error| {
        MinoError::new(
            ErrorCategory::EnvironmentUnavailable,
            format!("Failed to serialize Git hook result: {error}"),
        )
    })?;
    Ok(CommandResponse {
        message,
        complete: true,
        payload,
        missing: Vec::new(),
        next_actions,
    })
}

fn execute_branch(
    start: &Path,
    action: GitBranchAction,
) -> Result<(&'static str, Result<serde_json::Value, serde_json::Error>), MinoError> {
    let service = GitBranchService::discover(start)?;
    Ok(match action {
        GitBranchAction::Propose(arguments) => (
            "Git branch proposal generated without mutation.",
            serde_json::to_value(service.propose(&parse_plan_id(&arguments.plan)?)?),
        ),
        GitBranchAction::Create(arguments) => (
            "Approved Git branch created and bound.",
            serde_json::to_value(service.create(
                &parse_plan_id(&arguments.plan)?,
                &arguments.approval_ref,
                arguments.branch.as_deref(),
            )?),
        ),
    })
}

fn parse_plan_id(value: &str) -> Result<PlanId, MinoError> {
    PlanId::parse(value)
        .map_err(|error| MinoError::new(ErrorCategory::IncompleteOrValidation, error.to_string()))
}

fn parse_task_id(value: &str) -> Result<TaskId, MinoError> {
    TaskId::parse(value)
        .map_err(|error| MinoError::new(ErrorCategory::IncompleteOrValidation, error.to_string()))
}

fn parse_request_id(value: &str) -> Result<RequestId, MinoError> {
    RequestId::parse(value)
        .map_err(|error| MinoError::new(ErrorCategory::IncompleteOrValidation, error.to_string()))
}
