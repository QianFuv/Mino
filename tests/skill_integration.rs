//! Contract tests for the bundled Mino Skill and owned repository integrations.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use mino::ErrorCategory;
use mino::integration::{
    IntegrationFailurePoint, IntegrationOptions, IntegrationStatus, integrate_project_with_failure,
};
use mino::project::{doctor, initialize, initialize_with_options};
use serde_json::Value;

static NEXT_TEMP_DIRECTORY: AtomicU64 = AtomicU64::new(1);

const SKILL_FILES: &[&str] = &[
    "SKILL.md",
    "agents/openai.yaml",
    "references/approval-boundaries.md",
    "references/command-contract.md",
];

struct TestProject {
    path: PathBuf,
}

impl TestProject {
    fn new(label: &str) -> Self {
        let sequence = NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "mino-skill-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("temporary project should be created");
        fs::write(
            path.join("Cargo.toml"),
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .expect("fixture manifest should be written");
        fs::create_dir(path.join("src")).expect("fixture source directory should be created");
        fs::write(path.join("src/lib.rs"), "pub fn fixture() -> u8 { 1 }\n")
            .expect("fixture source should be written");
        Self {
            path: path.canonicalize().expect("project root should resolve"),
        }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TestProject {
    fn drop(&mut self) {
        let temporary_root = std::env::temp_dir();
        if self.path.starts_with(&temporary_root)
            && self
                .path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("mino-skill-"))
        {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

fn fixture_path(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/integration")
        .join(relative)
}

fn bundled_skill_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/skill/mino")
}

fn installed_skill_root(project: &TestProject) -> PathBuf {
    project.path().join(".agents/skills/mino")
}

fn assert_installed_skill_matches_bundle(project: &TestProject) {
    for relative in SKILL_FILES {
        assert_eq!(
            fs::read(installed_skill_root(project).join(relative))
                .expect("installed Skill file should be readable"),
            fs::read(bundled_skill_root().join(relative))
                .expect("bundled Skill file should be readable")
        );
    }
}

fn finding_codes(findings: &[mino::project::DoctorFinding]) -> Vec<&str> {
    findings
        .iter()
        .map(|finding| finding.code.as_str())
        .collect()
}

fn apply_all_options() -> IntegrationOptions {
    IntegrationOptions {
        apply_agents_block: true,
        apply_gitignore_block: true,
    }
}

fn run_mino(project: &TestProject, arguments: &[String]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mino"))
        .args(arguments)
        .current_dir(project.path())
        .stdin(Stdio::null())
        .output()
        .expect("Mino binary should run")
}

fn parse_success(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    serde_json::from_slice(&output.stdout).expect("stdout should be JSON")
}

#[test]
fn skill_metadata_and_workflow_cover_every_required_trigger_and_guardrail() {
    let skill = fs::read_to_string(bundled_skill_root().join("SKILL.md"))
        .expect("bundled SKILL.md should be readable");
    let normalized = skill.to_ascii_lowercase();
    for trigger in [
        "formal plan",
        "durable planning",
        "resume",
        "update",
        "review",
        "rework",
        "evidence",
        "git flow",
    ] {
        assert!(normalized.contains(trigger), "missing trigger: {trigger}");
    }
    assert!(!skill.contains("TODO"));
    assert!(skill.contains("<!-- mino-managed-skill:v1 -->"));
    assert!(skill.contains("mino project doctor --format json --no-input"));
    assert!(skill.contains("mino agent context --format json --no-input"));
    assert!(skill.contains("approval_required"));
    assert!(skill.contains("Never edit `.mino/**`"));
    assert!(skill.contains("Never use bundled protocol Markdown as a fallback"));
    assert!(skill.contains("references/command-contract.md"));
    assert!(skill.contains("references/approval-boundaries.md"));

    let metadata = fs::read_to_string(bundled_skill_root().join("agents/openai.yaml"))
        .expect("Skill UI metadata should be readable");
    assert!(metadata.contains("display_name: \"Mino Planning Workflow\""));
    assert!(
        metadata.contains("short_description: \"Run versioned plans with evidence and Git Flow\"")
    );
    assert!(metadata.contains("default_prompt: \"Use $mino"));
}

#[test]
fn fresh_init_installs_the_skill_and_blocks_require_explicit_apply_flags() {
    let project = TestProject::new("fresh");
    let first = initialize(project.path()).expect("fresh project should initialize");
    assert_installed_skill_matches_bundle(&project);
    assert!(!project.path().join("AGENTS.md").exists());
    assert!(!project.path().join(".gitignore").exists());
    assert_eq!(
        finding_codes(&first.findings),
        ["agents_block_missing", "gitignore_block_missing"]
    );
    assert!(
        first
            .integrations
            .artifacts
            .iter()
            .any(|artifact| artifact.status == IntegrationStatus::Created)
    );

    let applied = initialize_with_options(project.path(), apply_all_options())
        .expect("owned blocks should apply");
    assert!(applied.findings.is_empty());
    assert!(applied.integrations.is_complete());
    let agents_before = fs::read(project.path().join("AGENTS.md")).expect("AGENTS should exist");
    let ignore_before =
        fs::read(project.path().join(".gitignore")).expect(".gitignore should exist");
    let repeated = initialize_with_options(project.path(), apply_all_options())
        .expect("repeated integration should succeed");
    assert!(repeated.findings.is_empty());
    assert!(repeated.integrations.changed_paths.is_empty());
    assert_eq!(
        fs::read(project.path().join("AGENTS.md")).expect("AGENTS should remain"),
        agents_before
    );
    assert_eq!(
        fs::read(project.path().join(".gitignore")).expect(".gitignore should remain"),
        ignore_before
    );
    assert!(
        doctor(project.path())
            .expect("doctor should run")
            .is_complete()
    );
}

#[test]
fn an_unowned_skill_conflict_is_reported_and_preserved_byte_for_byte() {
    let project = TestProject::new("unowned");
    let skill_root = installed_skill_root(&project);
    fs::create_dir_all(&skill_root).expect("custom Skill directory should be created");
    fs::copy(
        fixture_path("unowned-skill/SKILL.md"),
        skill_root.join("SKILL.md"),
    )
    .expect("custom Skill should be copied");
    let before = fs::read(skill_root.join("SKILL.md")).expect("custom Skill should be readable");
    let first = initialize(project.path()).expect("init should report the conflict");
    assert!(finding_codes(&first.findings).contains(&"mino_skill_conflict"));
    assert_eq!(
        fs::read(skill_root.join("SKILL.md")).expect("custom Skill should remain"),
        before
    );
    assert!(!skill_root.join("references").exists());

    let second = initialize(project.path()).expect("repeated init should preserve the conflict");
    assert!(finding_codes(&second.findings).contains(&"mino_skill_conflict"));
    assert_eq!(
        fs::read(skill_root.join("SKILL.md")).expect("custom Skill should remain"),
        before
    );
}

#[test]
fn an_owned_skill_is_repaired_without_removing_unknown_repository_files() {
    let project = TestProject::new("owned-repair");
    initialize(project.path()).expect("project should initialize");
    let skill_root = installed_skill_root(&project);
    let entry = skill_root.join("SKILL.md");
    let drifted = fs::read_to_string(&entry)
        .expect("installed Skill should be readable")
        .replace("Treat the `mino` CLI", "Treat a stale CLI");
    fs::write(&entry, drifted).expect("owned Skill drift should be injected");
    let unknown = skill_root.join("repository-note.txt");
    fs::write(&unknown, "preserve me\n").expect("unknown file should be written");
    assert!(
        finding_codes(
            &doctor(project.path())
                .expect("doctor should detect drift")
                .findings
        )
        .contains(&"mino_skill_drift")
    );

    let repaired = initialize(project.path()).expect("owned Skill should be repaired");
    assert!(!finding_codes(&repaired.findings).contains(&"mino_skill_drift"));
    assert_installed_skill_matches_bundle(&project);
    assert_eq!(
        fs::read_to_string(unknown).expect("unknown file should remain"),
        "preserve me\n"
    );
}

#[test]
fn managed_blocks_preserve_outer_bytes_and_refuse_malformed_markers() {
    let project = TestProject::new("blocks");
    initialize(project.path()).expect("project should initialize");
    let agents = project.path().join("AGENTS.md");
    let gitignore = project.path().join(".gitignore");
    fs::copy(fixture_path("agents-user.md"), &agents).expect("AGENTS fixture should be copied");
    fs::copy(fixture_path("gitignore-user.txt"), &gitignore)
        .expect("ignore fixture should be copied");
    let user_agents = fs::read(&agents).expect("AGENTS fixture should be readable");
    let user_ignore = fs::read(&gitignore).expect("ignore fixture should be readable");
    let inspected = initialize(project.path()).expect("default init should inspect only");
    assert!(finding_codes(&inspected.findings).contains(&"agents_block_missing"));
    assert_eq!(
        fs::read(&agents).expect("AGENTS should remain"),
        user_agents
    );
    assert_eq!(
        fs::read(&gitignore).expect("ignore should remain"),
        user_ignore
    );

    initialize_with_options(project.path(), apply_all_options())
        .expect("owned blocks should apply");
    let applied_agents = fs::read_to_string(&agents).expect("AGENTS should be readable");
    let applied_ignore = fs::read_to_string(&gitignore).expect("ignore should be readable");
    assert!(applied_agents.starts_with(std::str::from_utf8(&user_agents).unwrap()));
    assert!(applied_ignore.starts_with(std::str::from_utf8(&user_ignore).unwrap()));
    assert_eq!(
        applied_agents
            .matches("<!-- mino:workflow:start -->")
            .count(),
        1
    );
    assert_eq!(applied_ignore.matches("# mino:runtime:start").count(), 1);

    let stale_agents = applied_agents.replace(
        "Invoke `$mino` for an explicitly requested formal plan",
        "Invoke a stale workflow for an explicitly requested formal plan",
    );
    let stale_ignore = applied_ignore.replace("/docs/plan/", "/docs/generated-plan/");
    fs::write(&agents, &stale_agents).expect("owned AGENTS drift should be written");
    fs::write(&gitignore, &stale_ignore).expect("owned ignore drift should be written");
    let drifted = initialize(project.path()).expect("owned drift should be inspected");
    let drift_codes = finding_codes(&drifted.findings);
    assert!(drift_codes.contains(&"agents_block_drift"));
    assert!(drift_codes.contains(&"gitignore_block_drift"));
    assert_eq!(fs::read_to_string(&agents).unwrap(), stale_agents);
    assert_eq!(fs::read_to_string(&gitignore).unwrap(), stale_ignore);
    initialize_with_options(project.path(), apply_all_options())
        .expect("owned drift should be repaired explicitly");
    assert!(
        fs::read_to_string(&agents)
            .expect("repaired AGENTS should be readable")
            .contains("Invoke `$mino` for an explicitly requested formal plan")
    );
    assert!(
        fs::read_to_string(&gitignore)
            .expect("repaired ignore should be readable")
            .contains("/docs/plan/")
    );

    fs::copy(fixture_path("agents-malformed.md"), &agents)
        .expect("malformed AGENTS fixture should be copied");
    fs::copy(fixture_path("gitignore-duplicate.txt"), &gitignore)
        .expect("duplicate ignore fixture should be copied");
    let malformed_agents = fs::read(&agents).expect("malformed AGENTS should be readable");
    let malformed_ignore = fs::read(&gitignore).expect("malformed ignore should be readable");
    let refused = initialize_with_options(project.path(), apply_all_options())
        .expect("malformed blocks should be reported without writes");
    let codes = finding_codes(&refused.findings);
    assert!(codes.contains(&"agents_block_malformed"));
    assert!(codes.contains(&"gitignore_block_malformed"));
    assert_eq!(
        fs::read(&agents).expect("malformed AGENTS should remain"),
        malformed_agents
    );
    assert_eq!(
        fs::read(&gitignore).expect("malformed ignore should remain"),
        malformed_ignore
    );
}

#[test]
fn every_replacement_interruption_recovers_one_valid_target_and_cleans_residuals() {
    let cases = [
        (IntegrationFailurePoint::BeforeBackup, true),
        (IntegrationFailurePoint::AfterBackup, false),
        (IntegrationFailurePoint::BeforePublish, true),
        (IntegrationFailurePoint::AfterPublish, false),
        (IntegrationFailurePoint::BeforeBackupRemoval, true),
    ];
    for (index, (failure_point, use_agents)) in cases.into_iter().enumerate() {
        let project = TestProject::new(&format!("recovery-{index}"));
        initialize_with_options(project.path(), apply_all_options())
            .expect("baseline integrations should apply");
        let target = project.path().join(if use_agents {
            "AGENTS.md"
        } else {
            ".gitignore"
        });
        let replacement = fs::read(&target).expect("current integration should be readable");
        let stale = stale_integration_bytes(&replacement, use_agents);
        fs::write(&target, &stale).expect("owned integration drift should be written");

        let failure =
            integrate_project_with_failure(project.path(), apply_all_options(), failure_point)
                .expect_err("integration replacement should stop at the injected boundary");
        assert_eq!(failure.category(), ErrorCategory::EnvironmentUnavailable);
        assert!(
            finding_codes(
                &doctor(project.path())
                    .expect("doctor should inspect the pending transaction")
                    .findings
            )
            .contains(&"integration_transaction_pending")
        );

        initialize(project.path()).expect("the next init should recover before reconciliation");
        let recovered = fs::read(&target).expect("recovered target must exist");
        assert!(recovered == stale || recovered == replacement);
        assert_no_integration_residuals(&project);
    }
}

#[test]
fn unexpected_backup_and_journal_bytes_are_reported_and_preserved() {
    let backup_project = TestProject::new("corrupt-backup");
    let backup_target = prepare_agents_replacement(&backup_project);
    integrate_project_with_failure(
        backup_project.path(),
        apply_all_options(),
        IntegrationFailurePoint::AfterBackup,
    )
    .expect_err("replacement should stop after backup");
    let backup = only_residual_with_extension(&backup_project, "bak");
    fs::write(&backup, b"tampered backup\n").expect("backup should be corrupted");
    let before = snapshot_transaction_artifacts(&backup_project);
    let diagnosed = doctor(backup_project.path()).expect("doctor should inspect corruption");
    assert!(finding_codes(&diagnosed.findings).contains(&"integration_transaction_corrupt"));
    assert_eq!(snapshot_transaction_artifacts(&backup_project), before);
    let error = initialize(backup_project.path()).expect_err("corrupt backup must block recovery");
    assert_eq!(error.category(), ErrorCategory::DriftDetected);
    assert_eq!(snapshot_transaction_artifacts(&backup_project), before);
    assert!(!backup_target.exists());

    let journal_project = TestProject::new("corrupt-journal");
    let journal_target = prepare_agents_replacement(&journal_project);
    integrate_project_with_failure(
        journal_project.path(),
        apply_all_options(),
        IntegrationFailurePoint::BeforeBackup,
    )
    .expect_err("replacement should stop before backup");
    let journal = transaction_phase_file(&journal_project, "prepared.json");
    fs::write(&journal, b"{}\n").expect("journal should be corrupted");
    let before = snapshot_transaction_artifacts(&journal_project);
    let diagnosed = doctor(journal_project.path()).expect("doctor should inspect bad journal");
    assert!(finding_codes(&diagnosed.findings).contains(&"integration_transaction_corrupt"));
    assert_eq!(snapshot_transaction_artifacts(&journal_project), before);
    let error = initialize(journal_project.path()).expect_err("bad journal must block recovery");
    assert_eq!(error.category(), ErrorCategory::DriftDetected);
    assert_eq!(snapshot_transaction_artifacts(&journal_project), before);
    assert!(journal_target.is_file());
}

#[test]
fn interrupted_skill_refresh_recovers_then_finishes_remaining_owned_files() {
    let project = TestProject::new("skill-resume");
    initialize(project.path()).expect("baseline Skill should install");
    let skill_entry = installed_skill_root(&project).join("SKILL.md");
    let metadata = installed_skill_root(&project).join("agents/openai.yaml");
    let stale_entry = fs::read_to_string(&skill_entry)
        .expect("Skill entry should be readable")
        .replace("Treat the `mino` CLI", "Treat an interrupted CLI");
    let stale_metadata = fs::read_to_string(&metadata)
        .expect("Skill metadata should be readable")
        .replace("Mino Planning Workflow", "Interrupted Planning Workflow");
    fs::write(&skill_entry, stale_entry).expect("Skill entry drift should be written");
    fs::write(&metadata, &stale_metadata).expect("Skill metadata drift should be written");

    integrate_project_with_failure(
        project.path(),
        IntegrationOptions::default(),
        IntegrationFailurePoint::AfterPublish,
    )
    .expect_err("Skill refresh should stop after the first publication");
    assert_eq!(
        fs::read(&skill_entry).expect("published Skill entry should exist"),
        fs::read(bundled_skill_root().join("SKILL.md")).expect("bundled entry should read")
    );
    assert_eq!(
        fs::read_to_string(&metadata).expect("unprocessed metadata should exist"),
        stale_metadata
    );

    initialize(project.path()).expect("next init should recover and finish the Skill refresh");
    assert_installed_skill_matches_bundle(&project);
    assert_no_integration_residuals(&project);
}

#[test]
fn project_init_returns_one_canonical_action_that_completes_integrations() {
    let project = TestProject::new("cli");
    let first = parse_success(&run_mino(
        &project,
        &[
            "project".to_owned(),
            "init".to_owned(),
            "--format".to_owned(),
            "json".to_owned(),
            "--no-input".to_owned(),
        ],
    ));
    assert_eq!(first["complete"], false);
    assert_eq!(first["next_actions"][0]["id"], "project.init");
    let argv = first["next_actions"][0]["argv"]
        .as_array()
        .expect("next argv should be an array")
        .iter()
        .map(|value| {
            value
                .as_str()
                .expect("argv should contain strings")
                .to_owned()
        })
        .collect::<Vec<_>>();
    assert_eq!(argv[0], "mino");
    assert!(argv.contains(&"--apply-agents-block".to_owned()));
    assert!(argv.contains(&"--apply-gitignore-block".to_owned()));
    let applied = parse_success(&run_mino(&project, &argv[1..]));
    assert_eq!(applied["complete"], true);
    assert_eq!(applied["missing"], Value::Array(Vec::new()));

    let doctor = parse_success(&run_mino(
        &project,
        &[
            "project".to_owned(),
            "doctor".to_owned(),
            "--format".to_owned(),
            "json".to_owned(),
            "--no-input".to_owned(),
        ],
    ));
    assert_eq!(doctor["complete"], true);
}

fn prepare_agents_replacement(project: &TestProject) -> PathBuf {
    initialize_with_options(project.path(), apply_all_options())
        .expect("baseline integrations should apply");
    let target = project.path().join("AGENTS.md");
    let current = fs::read(&target).expect("AGENTS should be readable");
    fs::write(&target, stale_integration_bytes(&current, true))
        .expect("owned AGENTS drift should be written");
    target
}

fn stale_integration_bytes(current: &[u8], use_agents: bool) -> Vec<u8> {
    let current = std::str::from_utf8(current).expect("integration bytes should be UTF-8");
    if use_agents {
        current
            .replace(
                "Invoke `$mino` for an explicitly requested formal plan",
                "Invoke a stale workflow for an explicitly requested formal plan",
            )
            .into_bytes()
    } else {
        current
            .replace("/docs/plan/", "/docs/interrupted-plan/")
            .into_bytes()
    }
}

fn assert_no_integration_residuals(project: &TestProject) {
    let transaction_root = project.path().join(".mino/integration-transactions");
    if transaction_root.exists() {
        assert_eq!(
            fs::read_dir(&transaction_root)
                .expect("transaction root should be readable")
                .count(),
            0
        );
    }
    assert!(
        recursive_files(project.path())
            .iter()
            .all(|path| !path.to_string_lossy().contains(".mino-integration-"))
    );
}

fn only_residual_with_extension(project: &TestProject, extension: &str) -> PathBuf {
    let matches = recursive_files(project.path())
        .into_iter()
        .filter(|path| path.extension().is_some_and(|value| value == extension))
        .filter(|path| path.to_string_lossy().contains(".mino-integration-"))
        .collect::<Vec<_>>();
    assert_eq!(matches.len(), 1, "expected one {extension} residual");
    matches.into_iter().next().expect("residual should exist")
}

fn transaction_phase_file(project: &TestProject, name: &str) -> PathBuf {
    recursive_files(&project.path().join(".mino/integration-transactions"))
        .into_iter()
        .find(|path| path.file_name().is_some_and(|value| value == name))
        .expect("transaction phase file should exist")
}

fn snapshot_transaction_artifacts(project: &TestProject) -> Vec<(PathBuf, Vec<u8>)> {
    let mut artifacts = recursive_files(project.path())
        .into_iter()
        .filter(|path| {
            path.starts_with(project.path().join(".mino/integration-transactions"))
                || path.to_string_lossy().contains(".mino-integration-")
        })
        .map(|path| {
            let relative = path
                .strip_prefix(project.path())
                .expect("artifact should be project-relative")
                .to_path_buf();
            let bytes = fs::read(&path).expect("transaction artifact should be readable");
            (relative, bytes)
        })
        .collect::<Vec<_>>();
    artifacts.sort_by(|left, right| left.0.cmp(&right.0));
    artifacts
}

fn recursive_files(root: &Path) -> Vec<PathBuf> {
    if !root.exists() {
        return Vec::new();
    }
    let mut files = Vec::new();
    let mut directories = vec![root.to_path_buf()];
    while let Some(directory) = directories.pop() {
        let mut entries = fs::read_dir(directory)
            .expect("test directory should be readable")
            .collect::<Result<Vec<_>, _>>()
            .expect("test entries should be readable");
        entries.sort_by_key(std::fs::DirEntry::path);
        for entry in entries {
            let path = entry.path();
            if entry.file_type().expect("entry type should read").is_dir() {
                directories.push(path);
            } else {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}
