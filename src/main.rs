//! Binary entry point for the Mino command-line interface.

use std::io::{self, Write};
use std::process::ExitCode;

use clap::Parser;
use mino::{EmptyPayload, ErrorCategory, MinoError, MinoResult, OutputFormat};

#[derive(Debug, Parser)]
#[command(name = "mino")]
#[command(version)]
#[command(about = "A local plan protocol engine for coding agents")]
struct Cli {
    #[arg(long, value_enum, default_value_t, global = true)]
    format: OutputFormat,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = MinoResult::success("Mino CLI initialized.", true, EmptyPayload::default());

    match write_result(&result, cli.format) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let _ = writeln!(io::stderr().lock(), "{error}");
            error.exit_code()
        }
    }
}

fn write_result(result: &MinoResult<EmptyPayload>, format: OutputFormat) -> Result<(), MinoError> {
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
