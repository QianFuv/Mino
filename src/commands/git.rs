//! Git inspection and active-binding CLI adapter.

use std::path::Path;

use clap::{Args, Subcommand, ValueEnum};

use crate::application::git_binding::GitBindingService;
use crate::application::git_branch::GitBranchService;
use crate::application::git_commit::GitCommitService;
use crate::application::git_hooks::GitHookService;
use crate::commands::CommandResponse;
use crate::domain::{PlanId, TaskId};
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
    /// Inspect, install, or invoke optional advisory repository hooks.
    Hook(HookArguments),
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
    /// Approved plan containing the task commit gate.
    #[arg(long)]
    plan: String,
    /// Done task whose exact commit gate should execute.
    #[arg(long)]
    task: String,
}

#[derive(Debug, Args)]
pub(crate) struct HookArguments {
    #[command(subcommand)]
    action: GitHookAction,
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
        GitAction::Commit(arguments) => (
            "Task commit created, evidenced, and recorded.",
            serde_json::to_value(GitCommitService::discover(start)?.commit(
                &parse_plan_id(&arguments.plan)?,
                &parse_task_id(&arguments.task)?,
            )?),
        ),
        GitAction::Hook(arguments) => return execute_hook(start, arguments.action),
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
