//! Maintainer-only typed tasks for reproducible Mino distribution artifacts.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use mino::MinoError;
use mino::distribution::{PluginPackageRequest, package_plugin};

#[derive(Debug, Parser)]
#[command(name = "xtask", version, about = "Mino maintainer tasks")]
struct XtaskCli {
    #[command(subcommand)]
    command: XtaskCommand,
}

#[derive(Debug, Subcommand)]
enum XtaskCommand {
    /// Build, verify, and smoke one host-native plugin artifact.
    PackagePlugin(PackagePluginArguments),
}

#[derive(Debug, Args)]
struct PackagePluginArguments {
    /// Repository root containing Cargo and canonical plugin sources.
    #[arg(long, default_value = ".")]
    repository: PathBuf,
    /// Exact host-native Mino binary to embed.
    #[arg(long)]
    binary: PathBuf,
    /// Exact native Rust target triple.
    #[arg(long)]
    target: String,
    /// Parent directory for target-specific artifact directories.
    #[arg(long, default_value = "dist")]
    output: PathBuf,
}

fn main() -> ExitCode {
    match execute(XtaskCli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{}: {}", error.category().code(), error);
            error.exit_code()
        }
    }
}

fn execute(cli: XtaskCli) -> Result<(), MinoError> {
    match cli.command {
        XtaskCommand::PackagePlugin(arguments) => {
            let report = package_plugin(&PluginPackageRequest::new(
                arguments.repository,
                arguments.binary,
                arguments.target,
                arguments.output,
            ))?;
            let rendered = serde_json::to_string_pretty(&report).map_err(|error| {
                MinoError::new(
                    mino::ErrorCategory::EnvironmentUnavailable,
                    format!("Failed to serialize packaging report: {error}"),
                )
            })?;
            println!("{rendered}");
            Ok(())
        }
    }
}
