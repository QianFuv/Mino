//! Contract tests for the initial Mino command-line interface.

use std::collections::BTreeSet;
use std::process::{Command, Output};

use mino::ErrorCategory;
use serde_json::Value;

fn run_mino(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mino"))
        .args(arguments)
        .output()
        .expect("the Mino test binary should start")
}

#[test]
fn help_and_version_are_deterministic() {
    let first_help = run_mino(&["--help"]);
    let second_help = run_mino(&["--help"]);
    let version = run_mino(&["--version"]);

    assert!(first_help.status.success());
    assert_eq!(first_help.stdout, second_help.stdout);
    assert_eq!(first_help.stderr, second_help.stderr);

    let help = String::from_utf8(first_help.stdout).expect("help should be UTF-8");
    assert!(help.contains("A local plan protocol engine for coding agents"));
    assert!(help.contains("Usage: mino"));
    assert!(help.contains("[OPTIONS]"));
    assert!(help.contains("--format <FORMAT>"));

    assert!(version.status.success());
    assert_eq!(version.stdout, b"mino 0.1.0\n");
    assert!(version.stderr.is_empty());
}

#[test]
fn agent_json_uses_stdout_without_diagnostics() {
    let output = run_mino(&["--format", "json"]);

    assert!(output.status.success());
    assert!(output.stderr.is_empty());

    let value: Value = serde_json::from_slice(&output.stdout).expect("stdout should be JSON");
    assert_eq!(value["kind"], "mino.result/v1");
    assert_eq!(value["ok"], true);
    assert_eq!(value["complete"], true);
    assert_eq!(value["message"], "Mino CLI initialized.");
    assert_eq!(value["missing"], Value::Array(Vec::new()));
    assert_eq!(value["next_actions"], Value::Array(Vec::new()));
}

#[test]
fn diagnostics_do_not_share_stdout() {
    let output = run_mino(&["--format", "json", "--unknown"]);

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.is_empty());
}

#[test]
fn error_categories_map_to_unique_documented_exit_codes() {
    let codes = ErrorCategory::ALL
        .iter()
        .map(|category| category.exit_code_value())
        .collect::<BTreeSet<_>>();
    let names = ErrorCategory::ALL
        .iter()
        .map(|category| category.code())
        .collect::<BTreeSet<_>>();

    assert_eq!(codes, BTreeSet::from([2, 3, 4, 5, 6, 7, 8]));
    assert_eq!(names.len(), ErrorCategory::ALL.len());
}
