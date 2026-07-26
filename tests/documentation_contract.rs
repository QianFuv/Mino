//! Executable inventory checks for the authoritative command documentation.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::Value;

const HELP_CASES: &[(&[&str], &[&str])] = &[
    (
        &["--help"],
        &[
            "project",
            "plan",
            "standards",
            "agent",
            "evidence",
            "exec",
            "git",
            "review",
            "protocol",
            "help",
        ],
    ),
    (
        &["project", "--help"],
        &["init", "show", "doctor", "scan", "migrate", "help"],
    ),
    (&["project", "migrate", "--help"], &["legacy", "help"]),
    (
        &["plan", "--help"],
        &[
            "create",
            "next",
            "validate",
            "show",
            "finalize",
            "review",
            "approve",
            "apply",
            "amend",
            "metadata",
            "summary",
            "context",
            "scope",
            "decision",
            "task",
            "file",
            "verification",
            "help",
        ],
    ),
    (&["plan", "metadata", "--help"], &["set", "help"]),
    (&["plan", "summary", "--help"], &["set", "help"]),
    (&["plan", "context", "--help"], &["add", "help"]),
    (&["plan", "scope", "--help"], &["set", "add", "help"]),
    (&["plan", "decision", "--help"], &["add", "help"]),
    (
        &["plan", "task", "--help"],
        &["add", "step", "criterion", "verification", "help"],
    ),
    (&["plan", "task", "step", "--help"], &["add", "help"]),
    (&["plan", "task", "criterion", "--help"], &["add", "help"]),
    (
        &["plan", "task", "verification", "--help"],
        &["add", "help"],
    ),
    (&["plan", "file", "--help"], &["add", "help"]),
    (&["plan", "verification", "--help"], &["add", "help"]),
    (
        &["plan", "amend", "--help"],
        &["propose", "approve", "apply", "help"],
    ),
    (
        &["standards", "--help"],
        &["detect", "recommend", "apply", "sync", "help"],
    ),
    (
        &["agent", "--help"],
        &["context", "next", "capabilities", "help"],
    ),
    (&["evidence", "--help"], &["add", "list", "show", "help"]),
    (
        &["exec", "--help"],
        &[
            "start",
            "checkpoint",
            "check",
            "criterion",
            "complete",
            "block",
            "resume",
            "finish",
            "help",
        ],
    ),
    (&["exec", "check", "--help"], &["run", "help"]),
    (&["exec", "criterion", "--help"], &["pass", "help"]),
    (
        &["git", "--help"],
        &["inspect", "bind", "branch", "commit", "help"],
    ),
    (&["git", "branch", "--help"], &["propose", "create", "help"]),
    (
        &["review", "--help"],
        &["record", "rework", "resolve", "accept", "help"],
    ),
    (&["protocol", "--help"], &["status", "migrate", "help"]),
];

const LEAF_COMMANDS: &[&str] = &[
    "agent capabilities",
    "agent context",
    "agent next",
    "evidence add",
    "evidence list",
    "evidence show",
    "exec block",
    "exec check run",
    "exec checkpoint",
    "exec complete",
    "exec criterion pass",
    "exec finish",
    "exec resume",
    "exec start",
    "git bind",
    "git branch create",
    "git branch propose",
    "git commit",
    "git inspect",
    "plan apply",
    "plan amend apply",
    "plan amend approve",
    "plan amend propose",
    "plan approve",
    "plan context add",
    "plan create",
    "plan decision add",
    "plan file add",
    "plan finalize",
    "plan metadata set",
    "plan next",
    "plan review",
    "plan scope add",
    "plan scope set",
    "plan show",
    "plan summary set",
    "plan task add",
    "plan task criterion add",
    "plan task step add",
    "plan task verification add",
    "plan validate",
    "plan verification add",
    "project doctor",
    "project init",
    "project migrate legacy",
    "project scan",
    "project show",
    "protocol migrate",
    "protocol status",
    "review accept",
    "review record",
    "review resolve",
    "review rework",
    "standards apply",
    "standards detect",
    "standards recommend",
    "standards sync",
];

fn repository_path(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
}

fn read(relative: &str) -> String {
    fs::read_to_string(repository_path(relative)).expect("document should be readable")
}

fn run_mino(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mino"))
        .args(arguments)
        .stdin(Stdio::null())
        .output()
        .expect("Mino binary should run")
}

fn help_commands(output: &Output) -> Vec<String> {
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let help = std::str::from_utf8(&output.stdout).expect("help should be UTF-8");
    let commands = help
        .split_once("Commands:\n")
        .expect("help should have a Commands section")
        .1
        .split_once("\n\nOptions:")
        .expect("help should end commands before Options")
        .0;
    commands
        .lines()
        .filter_map(|line| line.strip_prefix("  "))
        .filter_map(|line| line.split_whitespace().next())
        .map(str::to_owned)
        .collect()
}

#[test]
fn recursive_help_inventory_is_exact_and_includes_review_group() {
    for (arguments, expected) in HELP_CASES {
        assert_eq!(
            help_commands(&run_mino(arguments)),
            expected.iter().map(ToString::to_string).collect::<Vec<_>>(),
            "unexpected command inventory for {arguments:?}"
        );
    }
    let top_level = help_commands(&run_mino(&["--help"]));
    assert!(top_level.contains(&"review".to_owned()));
    assert!(top_level.contains(&"git".to_owned()));
}

#[test]
fn every_leaf_command_is_documented_once_and_matches_agent_capabilities() {
    let command_contract = read("docs/command-contract.md");
    for command in LEAF_COMMANDS {
        let marker = format!("`mino {command}`");
        assert!(
            command_contract.contains(&marker),
            "command contract is missing {marker}"
        );
    }
    let capabilities = run_mino(&["agent", "capabilities", "--format", "json", "--no-input"]);
    assert!(capabilities.status.success());
    let value: Value = serde_json::from_slice(&capabilities.stdout)
        .expect("capabilities should return direct JSON");
    let actions = value["actions"]
        .as_array()
        .expect("actions should be an array");
    let actual = actions
        .iter()
        .map(|action| action["id"].as_str().expect("action ID should be text"))
        .collect::<Vec<_>>();
    assert!(actual.windows(2).all(|pair| pair[0] < pair[1]));
    let expected = LEAF_COMMANDS
        .iter()
        .map(|command| command.replace(' ', "."))
        .collect::<BTreeSet<_>>();
    assert_eq!(
        actual
            .iter()
            .map(|action| (*action).to_owned())
            .collect::<BTreeSet<_>>(),
        expected
    );
    assert!(
        actions
            .iter()
            .find(|action| action["id"] == "evidence.add")
            .is_some_and(|action| action["mutates"] == true)
    );
    assert!(
        actions
            .iter()
            .find(|action| action["id"] == "git.bind")
            .is_some_and(|action| action["mutates"] == false)
    );
    assert!(
        actions
            .iter()
            .find(|action| action["id"] == "git.branch.create")
            .is_some_and(|action| {
                action["mutates"] == false && action["approval_boundary"] == true
            })
    );
    assert!(
        actions
            .iter()
            .find(|action| action["id"] == "review.accept")
            .is_some_and(|action| {
                action["mutates"] == true && action["approval_boundary"] == true
            })
    );
    assert!(
        actions
            .iter()
            .find(|action| action["id"] == "git.commit")
            .is_some_and(|action| {
                action["mutates"] == false && action["approval_boundary"] == false
            })
    );
    assert_eq!(
        actions
            .iter()
            .filter(|action| action["approval_boundary"] == true)
            .map(|action| action["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "git.branch.create",
            "plan.amend.approve",
            "plan.approve",
            "review.accept"
        ]
    );
}

#[test]
fn stable_schemas_exits_states_paths_and_prohibitions_are_documented() {
    let architecture = read("docs/architecture.md");
    let commands = read("docs/command-contract.md");
    let security = read("docs/security.md");
    for schema in [
        "mino.result/v1",
        "mino.agent-context/v1",
        "mino.agent-next/v1",
        "mino.agent-capabilities/v1",
        "mino.validation/v1",
        "mino.plan-review/v1",
        "mino.check-run/v1",
    ] {
        assert!(commands.contains(schema), "missing schema {schema}");
    }
    for (exit_code, code) in [
        (2, "incomplete_or_validation"),
        (3, "revision_conflict"),
        (4, "approval_required"),
        (5, "policy_violation"),
        (6, "check_failed"),
        (7, "environment_unavailable"),
        (8, "drift_detected"),
    ] {
        assert!(commands.contains(&format!("| {exit_code} | `{code}` |")));
    }
    for state in [
        "Draft",
        "Ready",
        "In Progress",
        "Blocked",
        "Review",
        "Done",
        "Accepted Exception",
    ] {
        assert!(commands.contains(state), "missing state {state}");
    }
    for path in [
        ".mino/config.toml",
        ".mino/protocol.lock",
        ".mino/standards.lock",
        ".mino/active.json",
        ".mino/git/branches/",
        ".mino/git/commits/",
        ".mino/plans/",
        "docs/plan/",
        ".agents/skills/mino/",
    ] {
        assert!(architecture.contains(path), "missing owned path {path}");
    }
    assert!(commands.contains("No command accepts an arbitrary status value"));
    assert!(security.contains("There is no hidden Git mutation path"));
    assert!(security.contains("copy the protocol template as a fallback"));
    assert!(security.contains("Do not edit it manually"));
}

#[test]
fn protocol_manifest_and_document_links_are_current() {
    let manifest: Value = serde_json::from_str(&read("assets/protocol/2026-05-11/manifest.json"))
        .expect("protocol manifest should parse");
    let migration = read("docs/migration.md");
    for field in [
        "protocol_version",
        "protocol_revision",
        "schema_version",
        "renderer_version",
    ] {
        let value = &manifest[field];
        let rendered = value
            .as_str()
            .map_or_else(|| value.to_string(), str::to_owned);
        assert!(
            migration.contains(&rendered),
            "missing manifest value {field}"
        );
    }
    for resource in manifest["resources"]
        .as_array()
        .expect("resources should be an array")
    {
        let digest = resource["sha256"]
            .as_str()
            .expect("resource digest should be text")
            .trim_start_matches("sha256:");
        assert!(migration.contains(digest));
    }

    let readme = read("README.md");
    for relative in [
        "docs/architecture.md",
        "docs/command-contract.md",
        "docs/migration.md",
        "docs/security.md",
    ] {
        assert!(readme.contains(relative));
        assert!(repository_path(relative).is_file());
    }
}
