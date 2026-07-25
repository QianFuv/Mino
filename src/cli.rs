//! Nested command-line parser, dispatcher, and output routing.

use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use serde::Serialize;
use serde_json::{Value, json};

use crate::commands::agent::{self, AgentAction};
use crate::commands::evidence::{self, EvidenceAction};
use crate::commands::exec::{self, ExecAction};
use crate::commands::plan::{self, PlanAction};
use crate::commands::project::{self, ProjectAction};
use crate::commands::protocol::{self, ProtocolAction};
use crate::commands::standards::{self, StandardsAction};
use crate::{ErrorCategory, MinoError, MinoResult, OutputFormat};

#[derive(Debug, Parser)]
#[command(name = "mino")]
#[command(version)]
#[command(about = "A local plan protocol engine for coding agents")]
struct Cli {
    #[arg(long, value_enum, default_value_t, global = true)]
    format: OutputFormat,
    #[arg(long, global = true)]
    no_input: bool,
    #[arg(long, global = true)]
    root: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Discover, initialize, inspect, and diagnose project-local Mino state.
    Project(ProjectArguments),
    /// Create and author revision-checked implementation plans.
    Plan(PlanArguments),
    /// Detect, recommend, resolve, and explicitly synchronize project standards.
    Standards(StandardsArguments),
    /// Return strict, non-interactive machine context and legal next actions.
    Agent(AgentArguments),
    /// Capture and query immutable execution evidence.
    Evidence(EvidenceArguments),
    /// Execute approved plans with ordered evidence gates.
    Exec(ExecArguments),
    /// Inspect embedded protocol compatibility and explicit migrations.
    Protocol(ProtocolArguments),
}

#[derive(Debug, Args)]
struct ProjectArguments {
    #[command(subcommand)]
    action: ProjectAction,
}

#[derive(Debug, Args)]
struct PlanArguments {
    #[command(subcommand)]
    action: PlanAction,
}

#[derive(Debug, Args)]
struct StandardsArguments {
    #[command(subcommand)]
    action: StandardsAction,
}

#[derive(Debug, Args)]
struct AgentArguments {
    #[command(subcommand)]
    action: AgentAction,
}

#[derive(Debug, Args)]
struct EvidenceArguments {
    #[command(subcommand)]
    action: EvidenceAction,
}

#[derive(Debug, Args)]
struct ExecArguments {
    #[command(subcommand)]
    action: ExecAction,
}

#[derive(Debug, Args)]
struct ProtocolArguments {
    #[command(subcommand)]
    action: ProtocolAction,
}

enum CliResponse {
    Result(MinoResult<Value>),
    Direct(Value),
}

/// Parses arguments, executes one command, writes one result, and returns its exit code.
#[must_use]
pub fn main_entry() -> ExitCode {
    let cli = Cli::parse();
    let format = cli.format;
    match execute(cli) {
        Ok(result) => match write_response(&result, format) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => write_failure(&error, format),
        },
        Err(error) => write_failure(&error, format),
    }
}

fn execute(cli: Cli) -> Result<CliResponse, MinoError> {
    let no_input = cli.no_input;
    let format = cli.format;
    let start = match cli.root {
        Some(root) => root,
        None => std::env::current_dir().map_err(|error| {
            MinoError::new(
                ErrorCategory::EnvironmentUnavailable,
                format!("Failed to read the current directory: {error}"),
            )
        })?,
    };
    match cli.command {
        Some(Command::Project(arguments)) => project::execute(&start, arguments.action)
            .map(crate::commands::CommandResponse::into_result)
            .map(CliResponse::Result),
        Some(Command::Plan(arguments)) => plan::execute(&start, arguments.action, no_input)
            .map(crate::commands::CommandResponse::into_result)
            .map(CliResponse::Result),
        Some(Command::Standards(arguments)) => standards::execute(&start, arguments.action)
            .map(crate::commands::CommandResponse::into_result)
            .map(CliResponse::Result),
        Some(Command::Agent(arguments)) => {
            agent::execute(&start, arguments.action, no_input, format).map(CliResponse::Direct)
        }
        Some(Command::Evidence(arguments)) => evidence::execute(&start, arguments.action)
            .map(crate::commands::CommandResponse::into_result)
            .map(CliResponse::Result),
        Some(Command::Exec(arguments)) => exec::execute(&start, arguments.action)
            .map(crate::commands::CommandResponse::into_result)
            .map(CliResponse::Result),
        Some(Command::Protocol(arguments)) => protocol::execute(&start, arguments.action)
            .map(crate::commands::CommandResponse::into_result)
            .map(CliResponse::Result),
        None => Ok(CliResponse::Result(MinoResult::success(
            "Mino CLI initialized.",
            true,
            json!({}),
        ))),
    }
}

fn write_response(response: &CliResponse, format: OutputFormat) -> Result<(), MinoError> {
    match response {
        CliResponse::Result(result) => write_result(result, format),
        CliResponse::Direct(value) => {
            if format != OutputFormat::Json {
                return Err(MinoError::new(
                    ErrorCategory::PolicyViolation,
                    "Direct Agent responses require JSON output",
                ));
            }
            let mut rendered = serde_json::to_string(value).map_err(|error| {
                MinoError::new(
                    ErrorCategory::EnvironmentUnavailable,
                    format!("Failed to serialize the Agent response: {error}"),
                )
            })?;
            rendered.push('\n');
            io::stdout()
                .lock()
                .write_all(rendered.as_bytes())
                .map_err(|error| {
                    MinoError::new(
                        ErrorCategory::EnvironmentUnavailable,
                        format!("Failed to write the Agent response: {error}"),
                    )
                })
        }
    }
}

fn write_result<T: Serialize>(
    result: &MinoResult<T>,
    format: OutputFormat,
) -> Result<(), MinoError> {
    let rendered = result.render(format)?;
    io::stdout()
        .lock()
        .write_all(rendered.as_bytes())
        .map_err(|error| {
            MinoError::new(
                ErrorCategory::EnvironmentUnavailable,
                format!("Failed to write the result: {error}"),
            )
        })
}

fn write_failure(error: &MinoError, format: OutputFormat) -> ExitCode {
    match format {
        OutputFormat::Human => {
            let _ = writeln!(io::stderr().lock(), "{error}");
        }
        OutputFormat::Json => {
            let mut failure = json!({
                "kind": "mino.result/v1",
                "ok": false,
                "complete": false,
                "message": error.message(),
                "error": {
                    "code": error.category().code(),
                    "exit_code": error.category().exit_code_value()
                },
                "missing": error.missing(),
                "next_actions": error.next_actions()
            });
            if let Some(details) = error.details() {
                if let (Some(failure), Some(details)) =
                    (failure.as_object_mut(), details.as_object())
                {
                    for (key, value) in details {
                        if !failure.contains_key(key) {
                            failure.insert(key.clone(), value.clone());
                        }
                    }
                } else if let Some(failure) = failure.as_object_mut() {
                    failure.insert("details".to_owned(), details.clone());
                }
            }
            if let Ok(mut rendered) = serde_json::to_string(&failure) {
                rendered.push('\n');
                let _ = io::stdout().lock().write_all(rendered.as_bytes());
            }
        }
    }
    error.exit_code()
}
