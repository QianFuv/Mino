//! Contract tests for project root discovery, initialization, show, and doctor.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(unix)]
use std::os::unix::fs::symlink;
#[cfg(windows)]
use std::os::windows::fs::symlink_dir;

use mino::domain::{Plan, PlanId, RequestId, Timestamp};
use mino::integration::{
    IntegrationFailurePoint, IntegrationOptions, integrate_project_with_failure,
};
use mino::project::{
    FindingSeverity, PlanSelectionRequest, ProjectConfig, ProjectLayout, ProjectPlanSelectionStore,
    ProtocolLock, RootSource, StandardsLock, discover, doctor, initialize, initialize_with_options,
    show,
};
use mino::render::{render_plan, write_projection};
use mino::store::PlanStore;
use serde_json::Value;

static NEXT_TEMP_DIRECTORY: AtomicU64 = AtomicU64::new(1);

struct TestProject {
    path: PathBuf,
}

impl TestProject {
    fn new(label: &str) -> Self {
        let sequence = NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "mino-project-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("temporary project should be created");
        Self { path }
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
                .is_some_and(|name| name.to_string_lossy().starts_with("mino-project-"))
        {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

fn timestamp() -> Timestamp {
    Timestamp::parse("2026-07-25T14:00:00Z").expect("test timestamp should be valid")
}

fn plan_id() -> PlanId {
    PlanId::parse("2026-07-25-project-doctor").expect("test plan ID should be valid")
}

fn request_id() -> RequestId {
    RequestId::parse("00000000-0000-0000-0000-000000000001")
        .expect("test request ID should be valid")
}

fn command(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| (*part).to_owned()).collect()
}

fn run_mino(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mino"))
        .args(arguments)
        .output()
        .expect("Mino binary should run")
}

fn finding_codes(findings: &[mino::project::DoctorFinding]) -> Vec<&str> {
    findings
        .iter()
        .map(|finding| finding.code.as_str())
        .collect()
}

fn apply_integrations(root: &Path) {
    initialize_with_options(
        root,
        IntegrationOptions {
            apply_agents_block: true,
            apply_gitignore_block: true,
        },
    )
    .expect("repository integrations should apply");
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum TreeEntrySnapshot {
    Directory,
    File(Vec<u8>),
    Other,
}

fn snapshot_tree(root: &Path) -> BTreeMap<PathBuf, TreeEntrySnapshot> {
    let mut snapshot = BTreeMap::new();
    let mut directories = vec![root.to_path_buf()];
    while let Some(directory) = directories.pop() {
        let mut entries = fs::read_dir(&directory)
            .expect("snapshot directory should be readable")
            .collect::<Result<Vec<_>, _>>()
            .expect("snapshot entries should be readable");
        entries.sort_by_key(std::fs::DirEntry::path);
        for entry in entries {
            let path = entry.path();
            let relative = path
                .strip_prefix(root)
                .expect("snapshot path should be relative")
                .to_path_buf();
            let metadata = fs::symlink_metadata(&path).expect("snapshot metadata should read");
            if metadata.is_dir() {
                snapshot.insert(relative, TreeEntrySnapshot::Directory);
                directories.push(path);
            } else if metadata.is_file() {
                snapshot.insert(
                    relative,
                    TreeEntrySnapshot::File(fs::read(path).expect("snapshot file should read")),
                );
            } else {
                snapshot.insert(relative, TreeEntrySnapshot::Other);
            }
        }
    }
    snapshot
}

#[cfg(any(unix, windows))]
fn create_directory_symlink(target: &Path, link: &Path) -> bool {
    #[cfg(unix)]
    let result = symlink(target, link);
    #[cfg(windows)]
    let result = symlink_dir(target, link);
    match result {
        Ok(()) => true,
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => false,
        Err(error) => panic!("managed symlink should be created: {error}"),
    }
}

#[cfg(any(unix, windows))]
fn assert_init_rejects_directory_symlink(relative: &str) {
    let project = TestProject::new(&format!("symlink-{}", relative.replace('/', "-")));
    let external = TestProject::new("symlink-external");
    let sentinel = external.path().join("sentinel.txt");
    fs::write(&sentinel, b"outside\n").expect("outside sentinel should be written");
    let link = project.path().join(relative);
    if let Some(parent) = link.parent() {
        fs::create_dir_all(parent).expect("managed parent should be created");
    }
    if !create_directory_symlink(external.path(), &link) {
        return;
    }

    let error = initialize(project.path()).expect_err("managed symlink must be rejected");
    assert_eq!(error.category(), mino::ErrorCategory::DriftDetected);
    assert_eq!(
        fs::read(&sentinel).expect("outside sentinel should remain readable"),
        b"outside\n"
    );
    assert_eq!(
        fs::read_dir(external.path())
            .expect("outside directory should remain readable")
            .count(),
        1
    );
}

#[test]
fn fresh_and_repeated_init_are_local_idempotent_and_non_destructive() {
    let project = TestProject::new("init");
    let first = initialize(project.path()).expect("fresh project should initialize");
    assert_eq!(
        first.root,
        project.path().canonicalize().expect("root should resolve")
    );
    assert_eq!(first.root_source, RootSource::InitializationFallback);
    assert_eq!(first.created_files.len(), 3);
    assert!(first.existing_files.is_empty());
    assert!(first.is_healthy());
    assert_eq!(
        finding_codes(&first.findings),
        vec!["agents_block_missing", "gitignore_block_missing"]
    );
    let layout = ProjectLayout::new(&first.root);
    assert!(layout.skill_file().is_file());
    let config_before = fs::read(layout.config()).expect("config should exist");
    let protocol_before = fs::read(layout.protocol_lock()).expect("protocol lock should exist");
    let standards_before = fs::read(layout.standards_lock()).expect("standards lock should exist");
    let config: ProjectConfig =
        toml::from_str(std::str::from_utf8(&config_before).expect("config should be UTF-8"))
            .expect("config should parse");
    let protocol: ProtocolLock = toml::from_str(
        std::str::from_utf8(&protocol_before).expect("protocol lock should be UTF-8"),
    )
    .expect("protocol lock should parse");
    let standards: StandardsLock = toml::from_str(
        std::str::from_utf8(&standards_before).expect("standards lock should be UTF-8"),
    )
    .expect("standards lock should parse");
    assert!(config.is_supported());
    assert_eq!(protocol, ProtocolLock::default());
    assert!(standards.is_supported());

    let second = initialize(project.path()).expect("repeated init should succeed");
    assert!(second.created_files.is_empty());
    assert_eq!(second.existing_files.len(), 3);
    assert_eq!(
        fs::read(layout.config()).expect("config should remain"),
        config_before
    );
    assert_eq!(
        fs::read(layout.protocol_lock()).expect("protocol lock should remain"),
        protocol_before
    );
    assert_eq!(
        fs::read(layout.standards_lock()).expect("standards lock should remain"),
        standards_before
    );
}

#[cfg(any(unix, windows))]
#[test]
fn init_rejects_symlinked_managed_directory_ancestors() {
    for relative in [".mino", ".mino/plans", ".mino/cache"] {
        assert_init_rejects_directory_symlink(relative);
    }
}

#[cfg(any(unix, windows))]
#[test]
fn doctor_reports_a_symlinked_mino_directory_without_following_it() {
    let project = TestProject::new("doctor-symlink");
    initialize(project.path()).expect("project should initialize");
    let external = TestProject::new("doctor-symlink-external");
    let external_state = external.path().join("state");
    fs::rename(project.path().join(".mino"), &external_state)
        .expect("managed state should move outside for the fixture");
    let sentinel = external_state.join("sentinel.txt");
    fs::write(&sentinel, b"outside\n").expect("outside sentinel should be written");
    if !create_directory_symlink(&external_state, &project.path().join(".mino")) {
        return;
    }

    let report = doctor(project.path()).expect("doctor should report unsafe managed state");
    assert!(finding_codes(&report.findings).contains(&"managed_path_unsafe"));
    assert_eq!(
        fs::read(&sentinel).expect("outside sentinel should remain readable"),
        b"outside\n"
    );
}

#[test]
fn doctor_reports_pending_integration_replacement_without_changing_any_tree_entry() {
    let project = TestProject::new("doctor-integration-transaction");
    apply_integrations(project.path());
    let agents = project.path().join("AGENTS.md");
    let stale = fs::read_to_string(&agents)
        .expect("AGENTS should be readable")
        .replace(
            "Invoke `$mino` for an explicitly requested formal plan",
            "Invoke a stale workflow for an explicitly requested formal plan",
        );
    fs::write(&agents, stale).expect("owned AGENTS drift should be written");
    integrate_project_with_failure(
        project.path(),
        IntegrationOptions {
            apply_agents_block: true,
            apply_gitignore_block: true,
        },
        IntegrationFailurePoint::AfterBackup,
    )
    .expect_err("integration replacement should stop after backup");
    assert!(!agents.exists());

    let before = snapshot_tree(project.path());
    let report = doctor(project.path()).expect("doctor should inspect pending replacement");
    assert!(finding_codes(&report.findings).contains(&"integration_transaction_pending"));
    assert_eq!(snapshot_tree(project.path()), before);

    initialize(project.path()).expect("init should recover the pending replacement");
    assert!(agents.is_file());
    assert!(
        doctor(project.path())
            .expect("doctor should run after recovery")
            .findings
            .iter()
            .all(|finding| !finding.code.starts_with("integration_transaction_"))
    );
}

#[test]
fn init_repairs_only_missing_files_and_preserves_corrupt_or_configured_files() {
    let project = TestProject::new("partial");
    initialize(project.path()).expect("project should initialize");
    let layout = ProjectLayout::new(project.path().canonicalize().expect("root should resolve"));
    fs::remove_file(layout.protocol_lock()).expect("protocol lock should be removed");
    fs::write(layout.standards_lock(), "not = [valid").expect("corruption should be injected");
    let corrupt_standards = fs::read(layout.standards_lock()).expect("corrupt lock should exist");
    let report = initialize(project.path()).expect("partial init should complete");
    assert_eq!(report.created_files, vec![layout.protocol_lock()]);
    assert!(finding_codes(&report.findings).contains(&"standards_lock_drift_corrupt"));
    assert_eq!(
        fs::read(layout.standards_lock()).expect("corrupt lock should remain"),
        corrupt_standards
    );

    fs::write(
        layout.standards_lock(),
        toml::to_string_pretty(&StandardsLock::default()).expect("lock should serialize"),
    )
    .expect("standards lock should be restored for test setup");
    let mut config: ProjectConfig =
        toml::from_str(&fs::read_to_string(layout.config()).expect("config should be readable"))
            .expect("config should parse");
    config.catalog.url = Some("https://catalog.example/manifest.toml".to_owned());
    fs::write(
        layout.config(),
        toml::to_string_pretty(&config).expect("configured file should serialize"),
    )
    .expect("configured file should be written");
    let configured_bytes = fs::read(layout.config()).expect("configured file should exist");
    let configured = initialize(project.path()).expect("configured init should remain valid");
    assert!(!finding_codes(&configured.findings).contains(&"config_drift"));
    assert_eq!(
        fs::read(layout.config()).expect("configured file should remain"),
        configured_bytes
    );
}

#[test]
fn doctor_distinguishes_transactions_render_drift_locks_and_integrations() {
    let project = TestProject::new("doctor");
    initialize(project.path()).expect("project should initialize");
    let layout = ProjectLayout::new(project.path());
    apply_integrations(project.path());
    let store = PlanStore::new(project.path());
    let plan = Plan::new(plan_id(), "Diagnose projections.", timestamp());
    store
        .create_plan(
            &plan,
            request_id(),
            "codex",
            command(&["mino", "plan", "create"]),
        )
        .expect("plan should be stored");
    let projection = project
        .path()
        .join("docs/plan/2026-07-25-project-doctor.md");
    write_projection(
        &projection,
        &render_plan(&plan).expect("plan should render"),
        None,
    )
    .expect("projection should be written");
    assert!(
        doctor(project.path())
            .expect("doctor should run")
            .is_complete()
    );

    fs::create_dir(store.paths().plan_directory(plan.id()).join("transaction"))
        .expect("incomplete transaction should be injected");
    fs::write(&projection, "manual edit\n").expect("render drift should be injected");
    fs::write(layout.protocol_lock(), "lock_version = [").expect("lock corruption should inject");
    fs::remove_file(layout.skill_file()).expect("skill should be removed");
    let report = doctor(project.path()).expect("doctor should report findings");
    let codes = finding_codes(&report.findings);
    assert!(codes.contains(&"incomplete_transaction"));
    assert!(codes.contains(&"render_drift"));
    assert!(codes.contains(&"protocol_lock_corrupt"));
    assert!(codes.contains(&"mino_skill_conflict"));
    assert!(
        report
            .findings
            .iter()
            .any(|finding| finding.severity == FindingSeverity::Error)
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn project_plan_selection_is_revision_checked_bounded_and_doctor_visible() {
    let project = TestProject::new("plan-selection");
    initialize(project.path()).expect("project should initialize");
    let store = PlanStore::new(project.path());
    let first_id = PlanId::parse("2026-07-25-selection-first").expect("plan ID should parse");
    let second_id = PlanId::parse("2026-07-25-selection-second").expect("plan ID should parse");
    for (plan_id, request) in [
        (first_id.clone(), "90000000-0000-0000-0000-000000000001"),
        (second_id.clone(), "90000000-0000-0000-0000-000000000002"),
    ] {
        let plan = Plan::new(plan_id.clone(), "Selection fixture", timestamp());
        store
            .create_plan(
                &plan,
                RequestId::parse(request).expect("request ID should parse"),
                "codex",
                command(&["mino", "plan", "create"]),
            )
            .expect("selection fixture plan should persist");
        write_projection(
            &project
                .path()
                .join("docs/plan")
                .join(format!("{plan_id}.md")),
            &render_plan(&plan).expect("plan should render"),
            None,
        )
        .expect("selection fixture projection should publish");
    }
    let selection_store = ProjectPlanSelectionStore::new(project.path());
    let live = vec![first_id.clone(), second_id.clone()];
    let legacy = selection_store
        .resolve(&live)
        .expect("legacy alternatives should resolve");
    assert_eq!(legacy.selection_revision, 0);
    assert_eq!(legacy.selected_plan, None);
    assert_eq!(legacy.alternatives, live);
    let request = PlanSelectionRequest {
        plan_id: second_id.clone(),
        expected_selection_revision: 0,
        request_id: RequestId::parse("90000000-0000-0000-0000-000000000003")
            .expect("request ID should parse"),
        actor: "codex".to_owned(),
        approval_reference: "chat:selection".to_owned(),
        reason: "Select the second plan".to_owned(),
        command: command(&["mino", "plan", "select"]),
        selected_at: timestamp(),
    };
    let selected = selection_store
        .select(request.clone(), &live)
        .expect("selection should persist");
    assert!(!selected.replayed);
    assert_eq!(selected.selection.selection_revision, 1);
    assert_eq!(selected.selection.selected_plan, Some(second_id));
    let replayed = selection_store
        .select(request, &live)
        .expect("exact selection should replay");
    assert!(replayed.replayed);
    let stale = selection_store
        .select(
            PlanSelectionRequest {
                plan_id: first_id,
                expected_selection_revision: 0,
                request_id: RequestId::parse("90000000-0000-0000-0000-000000000004")
                    .expect("request ID should parse"),
                actor: "codex".to_owned(),
                approval_reference: "chat:stale-selection".to_owned(),
                reason: "Attempt a stale choice".to_owned(),
                command: command(&["mino", "plan", "select"]),
                selected_at: timestamp(),
            },
            &live,
        )
        .expect_err("stale selection revision should fail");
    assert_eq!(stale.category(), mino::ErrorCategory::RevisionConflict);
    let persisted: Value = serde_json::from_slice(
        &fs::read(selection_store.path()).expect("selection file should be readable"),
    )
    .expect("selection file should be JSON");
    assert_eq!(persisted["schema_version"], 1);
    assert_eq!(persisted["selection_revision"], 1);
    assert!(
        fs::read_dir(project.path().join(".mino"))
            .expect("Mino directory should be readable")
            .all(|entry| {
                let path = entry.expect("Mino entry should be readable").path();
                path.extension().is_none_or(|extension| {
                    !extension.eq_ignore_ascii_case("tmp") && !extension.eq_ignore_ascii_case("bak")
                })
            })
    );

    fs::write(selection_store.path(), vec![b'x'; 1024 * 1024 + 1])
        .expect("oversized selection should be injected");
    let oversized = selection_store
        .inspect()
        .expect_err("oversized selection must be rejected before parsing");
    assert_eq!(oversized.category(), mino::ErrorCategory::DriftDetected);
    assert!(
        finding_codes(
            &doctor(project.path())
                .expect("doctor should inspect oversized selection")
                .findings
        )
        .contains(&"plan_selection_corrupt")
    );
}

#[test]
fn root_discovery_prefers_git_and_falls_back_to_supported_manifests() {
    let git_project = TestProject::new("git-root");
    let status = Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(git_project.path())
        .status()
        .expect("git should run");
    assert!(status.success());
    let nested = git_project.path().join("a/b");
    fs::create_dir_all(&nested).expect("nested directory should exist");
    let head_before = fs::read(git_project.path().join(".git/HEAD")).expect("HEAD should exist");
    let root = discover(&nested).expect("Git root should be discovered");
    assert_eq!(root.source(), RootSource::Git);
    assert_eq!(
        root.path(),
        git_project
            .path()
            .canonicalize()
            .expect("root should resolve")
    );
    initialize(&nested).expect("Git project should initialize");
    assert_eq!(
        fs::read(git_project.path().join(".git/HEAD")).expect("HEAD should remain"),
        head_before
    );
    assert!(!git_project.path().join(".git/index").exists());

    let manifest_project = TestProject::new("manifest-root");
    fs::write(manifest_project.path().join("Cargo.toml"), "[workspace]\n")
        .expect("manifest should be written");
    let nested = manifest_project.path().join("nested");
    fs::create_dir(&nested).expect("nested directory should exist");
    let root = discover(&nested).expect("manifest root should be discovered");
    assert_eq!(root.source(), RootSource::Manifest);
    assert_eq!(
        root.path(),
        manifest_project
            .path()
            .canonicalize()
            .expect("root should resolve")
    );
}

#[test]
fn project_cli_returns_stable_agent_json_for_init_show_and_doctor() {
    let project = TestProject::new("cli");
    let root = project.path().to_str().expect("test path should be UTF-8");
    let init = run_mino(&[
        "project",
        "init",
        "--root",
        root,
        "--format",
        "json",
        "--no-input",
    ]);
    assert!(init.status.success());
    assert!(init.stderr.is_empty());
    let value: Value = serde_json::from_slice(&init.stdout).expect("init should return JSON");
    assert_eq!(value["kind"], "mino.result/v1");
    assert_eq!(value["ok"], true);
    assert_eq!(value["complete"], false);
    assert_eq!(value["created_files"].as_array().map(Vec::len), Some(3));
    assert!(
        value["missing"]
            .as_array()
            .is_some_and(|missing| !missing.is_empty())
    );

    let show_output = run_mino(&["project", "show", "--root", root, "--format", "json"]);
    assert!(show_output.status.success());
    let show_value: Value =
        serde_json::from_slice(&show_output.stdout).expect("show should return JSON");
    assert_eq!(show_value["config"]["schema_version"], 1);
    let doctor_output = run_mino(&["project", "doctor", "--root", root]);
    assert!(doctor_output.status.success());
    assert!(
        String::from_utf8(doctor_output.stdout)
            .expect("human output should be UTF-8")
            .contains("Project doctor completed")
    );
    assert!(
        show(project.path())
            .expect("show service should work")
            .config
            .is_some()
    );
}
