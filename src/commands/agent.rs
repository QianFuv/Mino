//! Strict JSON and no-input command adapter for Agent protocol queries.

use std::path::Path;

use clap::Subcommand;
use serde::Serialize;
use serde_json::Value;

use crate::application::agent::AgentService;
use crate::{ErrorCategory, MinoError, NextAction, OutputFormat};

#[derive(Clone, Copy, Debug, Subcommand)]
pub(crate) enum AgentAction {
    /// Return complete dynamic project and active-plan context.
    Context,
    /// Return only the current approval boundary and next commands.
    Next,
    /// Return the static machine-use protocol contract.
    Capabilities,
}

pub(crate) fn execute(
    start: &Path,
    action: AgentAction,
    no_input: bool,
    format: OutputFormat,
) -> Result<Value, MinoError> {
    require_agent_mode(action, no_input, format)?;
    match action {
        AgentAction::Context => serialize(AgentService::discover(start)?.context()?),
        AgentAction::Next => serialize(AgentService::discover(start)?.next()?),
        AgentAction::Capabilities => serialize(AgentService::capabilities()),
    }
}

fn require_agent_mode(
    action: AgentAction,
    no_input: bool,
    format: OutputFormat,
) -> Result<(), MinoError> {
    if no_input && format == OutputFormat::Json {
        return Ok(());
    }
    let missing = [
        (!no_input).then(|| "--no-input".to_owned()),
        (format != OutputFormat::Json).then(|| "--format json".to_owned()),
    ]
    .into_iter()
    .flatten()
    .collect();
    Err(MinoError::new(
        ErrorCategory::PolicyViolation,
        "Agent commands require --format json and --no-input",
    )
    .with_remediation(
        missing,
        vec![NextAction {
            id: format!("agent.{}", action_name(action)),
            argv: vec![
                "mino".to_owned(),
                "agent".to_owned(),
                action_name(action).to_owned(),
                "--format".to_owned(),
                "json".to_owned(),
                "--no-input".to_owned(),
            ],
        }],
    ))
}

fn serialize<T: Serialize>(value: T) -> Result<Value, MinoError> {
    serde_json::to_value(value).map_err(|error| {
        MinoError::new(
            ErrorCategory::EnvironmentUnavailable,
            format!("Failed to serialize Agent response: {error}"),
        )
    })
}

const fn action_name(action: AgentAction) -> &'static str {
    match action {
        AgentAction::Context => "context",
        AgentAction::Next => "next",
        AgentAction::Capabilities => "capabilities",
    }
}
