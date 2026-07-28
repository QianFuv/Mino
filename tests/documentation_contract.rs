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
        &[
            "init", "show", "doctor", "scan", "migrate", "import", "help",
        ],
    ),
    (&["project", "migrate", "--help"], &["legacy", "help"]),
    (&["project", "import", "--help"], &["legacy", "help"]),
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
            "fork",
            "diff",
            "alternatives",
            "select",
            "archive",
            "metadata",
            "summary",
            "outcome",
            "scan",
            "context",
            "scope",
            "decision",
            "edge-case",
            "task",
            "file",
            "verification",
            "help",
        ],
    ),
    (&["plan", "metadata", "--help"], &["set", "help"]),
    (&["plan", "summary", "--help"], &["set", "help"]),
    (&["plan", "outcome", "--help"], &["set", "help"]),
    (&["plan", "scan", "--help"], &["accept", "help"]),
    (&["plan", "context", "--help"], &["add", "help"]),
    (&["plan", "scope", "--help"], &["set", "add", "help"]),
    (
        &["plan", "decision", "--help"],
        &["add", "update", "remove", "help"],
    ),
    (
        &["plan", "edge-case", "--help"],
        &["update", "remove", "help"],
    ),
    (
        &["plan", "task", "--help"],
        &[
            "add",
            "update",
            "remove",
            "move",
            "step",
            "criterion",
            "verification",
            "help",
        ],
    ),
    (
        &["plan", "task", "step", "--help"],
        &["add", "update", "remove", "help"],
    ),
    (
        &["plan", "task", "criterion", "--help"],
        &["add", "update", "remove", "help"],
    ),
    (
        &["plan", "task", "verification", "--help"],
        &["add", "update", "remove", "help"],
    ),
    (
        &["plan", "file", "--help"],
        &["add", "update", "remove", "help"],
    ),
    (
        &["plan", "verification", "--help"],
        &["add", "update", "remove", "help"],
    ),
    (
        &["plan", "amend", "--help"],
        &[
            "propose", "approve", "reject", "withdraw", "cancel", "apply", "help",
        ],
    ),
    (
        &["standards", "--help"],
        &[
            "detect",
            "recommend",
            "apply",
            "sync",
            "catalog",
            "conflict",
            "help",
        ],
    ),
    (
        &["standards", "catalog", "--help"],
        &["init", "validate", "build", "help"],
    ),
    (
        &["standards", "conflict", "--help"],
        &["list", "refresh", "resolve", "help"],
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
            "deviation",
            "check",
            "schedule",
            "criterion",
            "complete",
            "rework",
            "block",
            "resume",
            "finish",
            "help",
        ],
    ),
    (&["exec", "check", "--help"], &["run", "monitor", "help"]),
    (
        &["exec", "deviation", "--help"],
        &["record", "list", "resolve", "reject", "supersede", "help"],
    ),
    (&["exec", "schedule", "--help"], &["spec", "help"]),
    (&["exec", "criterion", "--help"], &["pass", "help"]),
    (
        &["git", "--help"],
        &[
            "inspect",
            "bind",
            "branch",
            "commit",
            "gate",
            "hook",
            "readiness",
            "help",
        ],
    ),
    (&["git", "branch", "--help"], &["propose", "create", "help"]),
    (&["git", "commit", "--help"], &["record-manual", "help"]),
    (&["git", "gate", "--help"], &["skip", "help"]),
    (
        &["git", "hook", "--help"],
        &["propose", "status", "install", "run", "help"],
    ),
    (&["git", "readiness", "--help"], &["refresh", "help"]),
    (
        &["review", "--help"],
        &[
            "record",
            "rework",
            "resolve",
            "disposition",
            "accept",
            "help",
        ],
    ),
    (&["review", "disposition", "--help"], &["revise", "help"]),
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
    "exec check monitor",
    "exec check run",
    "exec checkpoint",
    "exec complete",
    "exec criterion pass",
    "exec deviation list",
    "exec deviation record",
    "exec deviation reject",
    "exec deviation resolve",
    "exec deviation supersede",
    "exec finish",
    "exec resume",
    "exec rework",
    "exec schedule spec",
    "exec start",
    "git bind",
    "git branch create",
    "git branch propose",
    "git commit",
    "git commit record-manual",
    "git gate skip",
    "git hook install",
    "git hook propose",
    "git hook run",
    "git hook status",
    "git inspect",
    "git readiness refresh",
    "plan apply",
    "plan amend apply",
    "plan amend approve",
    "plan amend cancel",
    "plan amend propose",
    "plan amend reject",
    "plan amend withdraw",
    "plan alternatives",
    "plan approve",
    "plan archive",
    "plan context add",
    "plan create",
    "plan decision add",
    "plan decision remove",
    "plan decision update",
    "plan diff",
    "plan edge-case remove",
    "plan edge-case update",
    "plan file add",
    "plan file remove",
    "plan file update",
    "plan finalize",
    "plan fork",
    "plan metadata set",
    "plan next",
    "plan outcome set",
    "plan review",
    "plan scan accept",
    "plan select",
    "plan scope add",
    "plan scope set",
    "plan show",
    "plan summary set",
    "plan task add",
    "plan task criterion add",
    "plan task criterion remove",
    "plan task criterion update",
    "plan task move",
    "plan task remove",
    "plan task step add",
    "plan task step remove",
    "plan task step update",
    "plan task update",
    "plan task verification add",
    "plan task verification remove",
    "plan task verification update",
    "plan validate",
    "plan verification add",
    "plan verification remove",
    "plan verification update",
    "project doctor",
    "project import legacy",
    "project init",
    "project migrate legacy",
    "project scan",
    "project show",
    "protocol migrate",
    "protocol status",
    "review accept",
    "review disposition",
    "review disposition revise",
    "review record",
    "review resolve",
    "review rework",
    "standards apply",
    "standards catalog build",
    "standards catalog init",
    "standards catalog validate",
    "standards conflict list",
    "standards conflict refresh",
    "standards conflict resolve",
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
            .find(|action| action["id"] == "review.disposition.revise")
            .is_some_and(|action| {
                action["mutates"] == true && action["approval_boundary"] == true
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
            "exec.deviation.reject",
            "git.branch.create",
            "git.commit.record-manual",
            "git.gate.skip",
            "git.hook.install",
            "plan.amend.approve",
            "plan.amend.cancel",
            "plan.amend.reject",
            "plan.approve",
            "plan.archive",
            "plan.scan.accept",
            "plan.select",
            "review.accept",
            "review.disposition",
            "review.disposition.revise",
            "standards.conflict.resolve"
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
        "mino.deviation-list/v1",
        "mino.monitor/v1",
        "mino.scheduled-task-spec/v1",
        "mino.plan-diff/v1",
        "mino.git-hook-status/v1",
        "mino.git-hook-proposal/v1",
        "mino.git-hook-install/v1",
        "mino.git-hook-runtime/v1",
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
        ".mino/plans/<plan-id>/monitors/<request-id>/summary.json",
        "docs/plan/",
        ".agents/skills/mino/",
    ] {
        assert!(architecture.contains(path), "missing owned path {path}");
    }
    assert!(commands.contains("doc-contract: no-arbitrary-status-setter"));
    assert!(security.contains("doc-contract: no-hidden-git-mutation"));
    assert!(security.contains("doc-contract: monitor-no-background-service"));
    assert!(security.contains("doc-contract: schedule-no-external-mutation"));
    assert!(security.contains("doc-contract: no-protocol-template-fallback"));
    assert!(security.contains("doc-contract: managed-state-no-manual-edit"));
    for marker in [
        "doc-contract: explicit-file-map-overrides-ignore",
        "doc-contract: expected-git-entry",
        "doc-contract: final-plan-delta-gate",
    ] {
        assert!(
            architecture.contains(marker),
            "architecture is missing {marker}"
        );
        assert!(
            security.contains(marker),
            "security guide is missing {marker}"
        );
    }
    for marker in [
        "doc-contract: material-amendment-operations",
        "doc-contract: next-actions-subset",
        "doc-contract: non-ascii-plan-id",
        "doc-contract: review-decision-revision",
        "doc-contract: standards-reconciliation-action",
    ] {
        assert!(
            commands.contains(marker),
            "command contract is missing {marker}"
        );
    }
}

#[test]
fn shipped_skill_references_match_plugin_source_byte_for_byte() {
    for name in ["approval-boundaries.md", "command-contract.md"] {
        let canonical = fs::read(repository_path(&format!(
            "assets/skill/mino/references/{name}"
        )))
        .expect("canonical Skill reference should be readable");
        let plugin = fs::read(repository_path(&format!(
            "plugins/mino/skills/mino/references/{name}"
        )))
        .expect("plugin Skill reference should be readable");
        assert_eq!(canonical, plugin, "Skill reference differs for {name}");
    }
}

#[test]
fn primary_ci_runs_the_complete_pipeline_on_three_platforms() {
    let workflow = read(".github/workflows/ci.yml");
    for marker in [
        "name: Stable (${{ matrix.name }})",
        "fail-fast: false",
        "runner: windows-latest",
        "runner: ubuntu-24.04",
        "runner: macos-15",
        "binary: mino.exe",
        "binary: mino",
        "cargo fmt --all -- --check",
        "cargo clippy --all-targets --all-features -- -D warnings",
        "cargo sort --check",
        "cargo +nightly miri test --lib",
        "cargo install --path .",
        "MINO_E2E_BINARY:",
        "cargo test --offline --test e2e_v0_1 -- --test-threads=1",
        "cargo test --release --offline --all-targets --all-features",
        "cargo doc --offline --all-features --no-deps",
    ] {
        assert!(workflow.contains(marker), "CI workflow is missing {marker}");
    }
    assert_eq!(workflow.matches("runner: ").count(), 3);
    assert_eq!(workflow.matches("binary: ").count(), 3);

    let readme = read("README.md");
    let documentation_index = read("docs/README.md");
    let architecture = read("docs/architecture.md");
    assert!(readme.contains("Windows、Linux 和 macOS"));
    assert!(documentation_index.contains("Windows、Linux、macOS"));
    assert!(architecture.contains("doc-contract: three-platform-full-ci"));
    assert!(!architecture.contains("当前普通完整 CI 仍只在 Windows job 运行"));
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
    assert!(readme.contains("docs/README.md"));
    let documentation_index = read("docs/README.md");
    for relative in [
        "architecture.md",
        "command-contract.md",
        "distribution.md",
        "migration.md",
        "security.md",
        "team-catalog.md",
    ] {
        assert!(documentation_index.contains(relative));
        assert!(repository_path("docs").join(relative).is_file());
    }
    assert!(!repository_path("docs/v0-3.md").exists());
}

#[test]
fn operator_guides_cover_operations_recovery_verification_and_product_boundaries() {
    let catalog = read("docs/team-catalog.md");
    for marker in [
        "mino standards catalog init",
        "mino standards catalog validate",
        "mino standards catalog build",
        "mino standards sync --all",
        "catalog-manifest.json",
        "HTTPS",
        "doc-contract: trust-and-recovery",
        "doc-contract: deliberate-non-goals",
    ] {
        assert!(
            catalog.contains(marker),
            "catalog guide is missing {marker}"
        );
    }

    let distribution = read("docs/distribution.md");
    for marker in [
        "cargo run --release --locked --bin xtask -- package-plugin",
        "mino.plugin-artifact-manifest/v1",
        "SHA256SUMS",
        "x86_64-pc-windows-msvc",
        "aarch64-unknown-linux-gnu",
        "x86_64-apple-darwin",
        "aarch64-apple-darwin",
        "doc-contract: upgrade-rollback-publication",
        "doc-contract: deliberate-non-goals",
    ] {
        assert!(
            distribution.contains(marker),
            "distribution guide is missing {marker}"
        );
    }

    let security = read("docs/security.md");
    for marker in [
        "doc-contract: no-llm-execution",
        "doc-contract: no-daemon",
        "doc-contract: no-cloud-control-plane",
        "doc-contract: no-built-in-scheduler",
        "doc-contract: no-auto-update",
        "doc-contract: no-arbitrary-plugin-runtime",
        "doc-contract: no-git-remote-or-destructive",
        "doc-contract: no-plan-merge",
    ] {
        assert!(
            security.contains(marker),
            "security guide is missing {marker}"
        );
    }

    let migration = read("docs/migration.md");
    assert!(migration.contains("doc-contract: upgrade-and-rollback"));

    let documentation_index = read("docs/README.md");
    assert!(documentation_index.contains("doc-contract: verification-strategy"));
}
