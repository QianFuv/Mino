//! Binary entry point for the Mino command-line interface.

use std::process::ExitCode;

fn main() -> ExitCode {
    mino::cli::main_entry()
}
