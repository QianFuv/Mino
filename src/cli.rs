//! Nested command-line parser, dispatcher, and output routing.

use std::io::{self, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use serde::Serialize;
use serde_json::{Value, json};

use crate::commands::project::{self, ProjectAction};
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
}

#[derive(Debug, Args)]
struct ProjectArguments {
    #[command(subcommand)]
    action: ProjectAction,
}

/// Parses arguments, executes one command, writes one result, and returns its exit code.
#[must_use]
pub fn main_entry() -> ExitCode {
    let cli = Cli::parse();
    let format = cli.format;
    match execute(cli) {
        Ok(result) => match write_result(&result, format) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => write_failure(&error, format),
        },
        Err(error) => write_failure(&error, format),
    }
}

fn execute(cli: Cli) -> Result<MinoResult<Value>, MinoError> {
    let _ = cli.no_input;
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
            .map(crate::commands::CommandResponse::into_result),
        None => Ok(MinoResult::success(
            "Mino CLI initialized.",
            true,
            json!({}),
        )),
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
            let failure = json!({
                "kind": "mino.result/v1",
                "ok": false,
                "complete": false,
                "message": error.message(),
                "error": {
                    "code": error.category().code(),
                    "exit_code": error.category().exit_code_value()
                },
                "missing": [],
                "next_actions": []
            });
            if let Ok(mut rendered) = serde_json::to_string(&failure) {
                rendered.push('\n');
                let _ = io::stdout().lock().write_all(rendered.as_bytes());
            }
        }
    }
    error.exit_code()
}
