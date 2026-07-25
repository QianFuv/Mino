//! Git inspection and active-binding CLI adapter.

use std::path::Path;

use clap::{Args, Subcommand};

use crate::application::git_binding::GitBindingService;
use crate::application::git_branch::GitBranchService;
use crate::application::git_commit::GitCommitService;
use crate::commands::CommandResponse;
use crate::domain::{PlanId, TaskId};
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
