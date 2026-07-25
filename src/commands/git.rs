//! Git inspection and active-binding CLI adapter.

use std::path::Path;

use clap::{Args, Subcommand};

use crate::application::git_binding::GitBindingService;
use crate::commands::CommandResponse;
use crate::domain::PlanId;
use crate::{ErrorCategory, MinoError};

#[derive(Debug, Subcommand)]
pub(crate) enum GitAction {
    /// Inspect repository, worktree, HEAD, index, and active binding facts.
    Inspect(InspectArguments),
    /// Bind one plan to the exact current worktree and branch or detached HEAD.
    Bind(BindArguments),
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

fn parse_plan_id(value: &str) -> Result<PlanId, MinoError> {
    PlanId::parse(value)
        .map_err(|error| MinoError::new(ErrorCategory::IncompleteOrValidation, error.to_string()))
}
