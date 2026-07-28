//! Contract tests for project root discovery, initialization, show, and doctor.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(unix)]
use std::os::unix::fs::symlink;
#[cfg(unix)]
use std::os::unix::fs::symlink as symlink_file;
#[cfg(windows)]
use std::os::windows::fs::{symlink_dir, symlink_file};

use mino::application::plan::{CreatePlanRequest, PlanService};
use mino::domain::{Plan, PlanId, RequestId, Timestamp};
use mino::integration::{
    IntegrationFailurePoint, IntegrationOptions, integrate_project_with_failure,
};
use mino::project::{
    FindingSeverity, PlanSelectionRequest, PlanningAuthorityApplyRequest,
    PlanningAuthorityDecision, PlanningAuthorityDecisionRequest, PlanningAuthorityService,
    ProjectConfig, ProjectLayout, ProjectPlanSelectionStore, ProtocolLock, RootSource,
    StandardsLock, discover, doctor, initialize, initialize_with_options, show,
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

fn authority_fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/integration/agents-user.md")
}

fn prepare_authority_project(label: &str) -> TestProject {
    let project = TestProject::new(label);
    fs::copy(authority_fixture(), project.path().join("AGENTS.md"))
        .expect("authority AGENTS fixture should be copied");
    apply_integrations(project.path());
    project
}

fn authority_request_id(number: u64) -> RequestId {
    RequestId::parse(format!("91000000-0000-0000-0000-{number:012}"))
        .expect("authority request ID should parse")
}

fn create_durable_plan(
    project: &TestProject,
    name: &str,
    number: u64,
) -> Result<mino::application::plan::PlanOperationReport, mino::MinoError> {
    PlanService::discover(project.path())?.create(CreatePlanRequest {
        name: name.to_owned(),
        trigger: "durable".to_owned(),
        original_request: "Exercise the planning authority gate.".to_owned(),
        request_id: authority_request_id(number),
        actor: "codex".to_owned(),
        command: command(&["mino", "plan", "create"]),
        created_at: timestamp(),
    })
}

fn authority_decision_request(
    status: &mino::project::PlanningAuthorityStatus,
    decision: PlanningAuthorityDecision,
    number: u64,
) -> PlanningAuthorityDecisionRequest {
    PlanningAuthorityDecisionRequest {
        expected_revision: status.authority_revision,
        expected_source_digest: status
            .source_digest
            .clone()
            .expect("authority source digest should exist"),
        decision,
        request_id: authority_request_id(number),
        actor: "codex".to_owned(),
        approval_reference: format!("chat:authority-{number}"),
        command: command(&["mino", "project", "authority", "decide"]),
        decided_at: timestamp(),
    }
}

fn authority_apply_request(
    proposal: &mino::project::PlanningAuthorityProposal,
    number: u64,
) -> PlanningAuthorityApplyRequest {
    PlanningAuthorityApplyRequest {
        expected_revision: proposal.authority_revision,
        expected_source_digest: proposal.source_digest.clone(),
        expected_replacement_digest: proposal.replacement_digest.clone(),
        is_confirmed: true,
        request_id: authority_request_id(number),
        actor: "codex".to_owned(),
        approval_reference: format!("chat:authority-apply-{number}"),
        command: command(&["mino", "project", "authority", "apply"]),
        decided_at: timestamp(),
    }
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

#[test]
#[allow(clippy::too_many_lines)]
fn authority_decisions_deterministically_gate_durable_planning() {
    let coexistence_project = prepare_authority_project("authority-coexistence");
    let service = PlanningAuthorityService::discover(coexistence_project.path())
        .expect("authority service should discover");
    let pending = service.status().expect("pending authority should inspect");
    assert_eq!(pending.decision, PlanningAuthorityDecision::Pending);
    assert_eq!(
        pending.block_reason.as_deref(),
        Some("legacy_planning_authority_conflict")
    );
    assert!(
        finding_codes(
            &doctor(coexistence_project.path())
                .expect("doctor should inspect pending authority")
                .findings
        )
        .contains(&"legacy_planning_authority_conflict")
    );
    let context = mino::application::agent::build_agent_context(coexistence_project.path(), None)
        .expect("Agent context should expose the authority gate");
    assert!(context.approval_required);
    assert_eq!(context.next_actions[0].id, "project.authority.status");
    assert!(
        context
            .blocked_actions
            .iter()
            .any(|action| action.action == "plan.create")
    );
    let blocked = create_durable_plan(&coexistence_project, "pending authority", 1)
        .expect_err("pending authority must block durable creation");
    assert_eq!(blocked.category(), mino::ErrorCategory::PolicyViolation);
    assert_eq!(blocked.missing(), ["legacy_planning_authority_conflict"]);
    assert_eq!(blocked.next_actions()[0].id, "project.authority.status");

    let coexistence_request =
        authority_decision_request(&pending, PlanningAuthorityDecision::CoexistenceApproved, 2);
    let approved = service
        .decide(coexistence_request.clone())
        .expect("coexistence should be recorded");
    assert!(!approved.replayed);
    assert!(!approved.status.blocks_durable_planning);
    assert_eq!(approved.status.decided_by.as_deref(), Some("codex"));
    assert_eq!(
        approved.status.decision_reference.as_deref(),
        Some("chat:authority-2")
    );
    let mut decision_retry = coexistence_request;
    decision_retry.decided_at =
        Timestamp::parse("2026-07-25T14:01:00Z").expect("retry timestamp should parse");
    assert!(
        service
            .decide(decision_retry)
            .expect("CLI-style retry should replay")
            .replayed
    );
    create_durable_plan(&coexistence_project, "approved coexistence", 3)
        .expect("coexistence should permit durable creation");

    fs::write(
        coexistence_project.path().join("AGENTS.md"),
        format!(
            "{}\nRepository-local change.\n",
            fs::read_to_string(coexistence_project.path().join("AGENTS.md"))
                .expect("AGENTS should read")
        ),
    )
    .expect("source drift should be injected");
    let stale = service.status().expect("stale decision should inspect");
    assert!(stale.decision_is_stale);
    assert_eq!(
        stale.block_reason.as_deref(),
        Some("planning_authority_decision_stale")
    );
    assert_eq!(
        stale
            .state_refresh_action
            .as_ref()
            .expect("stale state should expose a canonical refresh")
            .id,
        "project.init"
    );
    initialize(coexistence_project.path()).expect("init should refresh stale authority state");
    let refreshed = service
        .status()
        .expect("refreshed authority should inspect");
    assert_eq!(refreshed.decision, PlanningAuthorityDecision::Pending);
    assert!(!refreshed.decision_is_stale);
    assert_eq!(
        refreshed.block_reason.as_deref(),
        Some("legacy_planning_authority_conflict")
    );

    let declined_project = prepare_authority_project("authority-declined");
    let declined_service = PlanningAuthorityService::discover(declined_project.path())
        .expect("authority service should discover");
    let pending = declined_service
        .status()
        .expect("pending authority should inspect");
    let declined = declined_service
        .decide(authority_decision_request(
            &pending,
            PlanningAuthorityDecision::Declined,
            4,
        ))
        .expect("decline should be recorded");
    assert_eq!(
        declined.status.block_reason.as_deref(),
        Some("mino_durable_planning_declined")
    );
    assert!(
        finding_codes(
            &doctor(declined_project.path())
                .expect("doctor should inspect declined authority")
                .findings
        )
        .contains(&"mino_durable_planning_declined")
    );
    create_durable_plan(&declined_project, "declined authority", 5)
        .expect_err("declined authority must block durable creation");
}

#[test]
fn authority_status_and_proposal_cli_are_stable_and_read_only() {
    let project = prepare_authority_project("authority-cli-read-only");
    let agents_path = project.path().join("AGENTS.md");
    let authority_path = project.path().join(".mino/authority.json");
    let agents_before = fs::read(&agents_path).expect("AGENTS should read");
    let authority_before = fs::read(&authority_path).expect("authority state should read");
    let root = project
        .path()
        .to_str()
        .expect("project path should be UTF-8");

    let status = run_mino(&[
        "--root",
        root,
        "--format",
        "json",
        "--no-input",
        "project",
        "authority",
        "status",
    ]);
    assert!(status.status.success());
    let status: Value = serde_json::from_slice(&status.stdout).expect("status should return JSON");
    assert_eq!(status["kind"], "mino.result/v1");
    assert_eq!(status["authority_kind"], "mino.planning-authority/v1");
    assert_eq!(status["decision"], "pending");
    assert_eq!(status["complete"], false);
    assert_eq!(status["block_reason"], "legacy_planning_authority_conflict");

    let proposal = run_mino(&[
        "--root",
        root,
        "--format",
        "json",
        "--no-input",
        "project",
        "authority",
        "propose",
    ]);
    assert!(proposal.status.success());
    let proposal: Value =
        serde_json::from_slice(&proposal.stdout).expect("proposal should return JSON");
    assert_eq!(proposal["kind"], "mino.result/v1");
    assert_eq!(
        proposal["proposal_kind"],
        "mino.planning-authority-proposal/v1"
    );
    assert!(proposal["source_digest"].as_str().is_some());
    assert!(proposal["replacement_digest"].as_str().is_some());
    assert_eq!(
        fs::read(agents_path).expect("AGENTS should remain"),
        agents_before
    );
    assert_eq!(
        fs::read(authority_path).expect("authority state should remain"),
        authority_before
    );
}

#[test]
fn fenced_mino_markers_do_not_create_a_planning_authority_conflict() {
    let project = TestProject::new("authority-fenced-mino");
    fs::write(
        project.path().join("AGENTS.md"),
        "## Planning Documents\n\n- **Formal Plan Trigger**: Use the legacy template.\n\n```markdown\n<!-- mino:workflow:start -->\nFenced example only.\n<!-- mino:workflow:end -->\n```\n",
    )
    .expect("fenced authority fixture should be written");
    initialize(project.path()).expect("fenced authority project should initialize");
    let status = PlanningAuthorityService::discover(project.path())
        .expect("authority service should discover")
        .status()
        .expect("authority status should inspect");
    assert!(status.legacy_planning_rules_detected);
    assert!(!status.mino_workflow_active);
    assert!(!status.blocks_durable_planning);
}

#[test]
#[allow(clippy::too_many_lines)]
fn authority_rewrite_recovers_every_interruption_and_preserves_other_sections() {
    let failure_points = [
        IntegrationFailurePoint::BeforeBackup,
        IntegrationFailurePoint::AfterBackup,
        IntegrationFailurePoint::BeforePublish,
        IntegrationFailurePoint::AfterPublish,
        IntegrationFailurePoint::BeforeBackupRemoval,
    ];
    for (index, failure_point) in failure_points.into_iter().enumerate() {
        let project = prepare_authority_project(&format!("authority-recovery-{index}"));
        let agents_path = project.path().join("AGENTS.md");
        let before = fs::read_to_string(&agents_path).expect("AGENTS should read");
        let service = PlanningAuthorityService::discover(project.path())
            .expect("authority service should discover");
        let proposal = service.propose().expect("rewrite should be proposed");
        let request = authority_apply_request(&proposal, 100 + index as u64);
        let interrupted = service
            .apply_with_failure(&request, failure_point)
            .expect_err("rewrite should stop at the injected boundary");
        assert_eq!(
            interrupted.category(),
            mino::ErrorCategory::EnvironmentUnavailable
        );
        let pending = service.status().expect("pending rewrite should inspect");
        assert!(pending.rewrite_pending);
        assert_eq!(
            pending.block_reason.as_deref(),
            Some("planning_authority_rewrite_pending")
        );
        let recovery_action = pending
            .recovery_action
            .as_ref()
            .expect("pending rewrite should expose its exact approved retry");
        assert_eq!(recovery_action.id, "project.authority.apply");
        assert!(
            recovery_action
                .argv
                .windows(2)
                .any(|parts| parts == ["--approval-ref", request.approval_reference.as_str()])
        );
        if index == 0 {
            let context = mino::application::agent::build_agent_context(project.path(), None)
                .expect("Agent context should expose the persisted rewrite retry");
            assert!(!context.approval_required);
            assert_eq!(context.next_actions[0].id, "project.authority.apply");
            let competing = service
                .decide(authority_decision_request(
                    &pending,
                    PlanningAuthorityDecision::CoexistenceApproved,
                    150,
                ))
                .expect_err("pending rewrite must block a competing decision");
            assert_eq!(competing.category(), mino::ErrorCategory::PolicyViolation);
            assert_eq!(competing.next_actions()[0].id, "project.authority.apply");
            service
                .propose()
                .expect_err("pending rewrite must block a replacement proposal");
        }

        let mut retry = request.clone();
        retry.decided_at =
            Timestamp::parse("2026-07-25T14:01:00Z").expect("retry timestamp should parse");
        let recovered = service
            .apply(&retry)
            .expect("exact retry should recover and finalize");
        assert!(!recovered.replayed);
        assert_eq!(
            recovered.status.decision,
            PlanningAuthorityDecision::Superseded
        );
        assert!(!recovered.status.blocks_durable_planning);
        assert!(!recovered.status.legacy_planning_rules_detected);
        assert_eq!(
            recovered.status.applied_rewrite_digest.as_deref(),
            Some(proposal.replacement_digest.as_str())
        );
        retry.decided_at =
            Timestamp::parse("2026-07-25T14:02:00Z").expect("second retry timestamp should parse");
        let replayed = service
            .apply(&retry)
            .expect("completed exact retry should replay");
        assert!(replayed.replayed);

        let after = fs::read_to_string(&agents_path).expect("rewritten AGENTS should read");
        assert!(after.contains("For durable plans, Mino supersedes the legacy template"));
        assert!(!after.contains("Fetch and instantiate the legacy planning template"));
        for preserved in [
            "Keep this repository-specific rule exactly as written.",
            "Keep this coding rule byte-identical.",
            "Keep this Git rule byte-identical.",
            "Keep this MCP rule byte-identical.",
            "<!-- mino:workflow:start -->",
            "<!-- mino:workflow:end -->",
        ] {
            assert!(before.contains(preserved));
            assert!(after.contains(preserved));
        }
        let transaction_root = project.path().join(".mino/integration-transactions");
        assert!(
            !transaction_root.exists()
                || fs::read_dir(transaction_root)
                    .expect("transaction root should read")
                    .next()
                    .is_none()
        );
        let authority: Value = serde_json::from_slice(
            &fs::read(project.path().join(".mino/authority.json"))
                .expect("authority state should read"),
        )
        .expect("authority state should be JSON");
        assert_eq!(authority["decision"], "superseded");
        assert_eq!(authority["decided_by"], "codex");
        assert_eq!(
            authority["applied_rewrite_digest"],
            proposal.replacement_digest
        );
        if index == 0 {
            fs::write(&agents_path, format!("{after}\nPost-rewrite change.\n"))
                .expect("post-rewrite drift should be injected");
            let stale = service.status().expect("stale supersession should inspect");
            assert!(stale.decision_is_stale);
            assert_eq!(
                stale.block_reason.as_deref(),
                Some("planning_authority_decision_stale")
            );
            assert_eq!(
                stale
                    .state_refresh_action
                    .as_ref()
                    .expect("stale supersession should expose a refresh")
                    .id,
                "project.init"
            );
            initialize(project.path()).expect("init should refresh stale supersession state");
            let refreshed = service
                .status()
                .expect("refreshed authority should inspect");
            assert_eq!(refreshed.decision, PlanningAuthorityDecision::Pending);
            assert!(!refreshed.blocks_durable_planning);
        }
    }
}

#[test]
fn authority_rewrite_targets_fail_closed_on_drift_and_unsafe_entries() {
    let drift_project = prepare_authority_project("authority-source-drift");
    let drift_service = PlanningAuthorityService::discover(drift_project.path())
        .expect("authority service should discover");
    let proposal = drift_service.propose().expect("rewrite should be proposed");
    let agents_path = drift_project.path().join("AGENTS.md");
    let changed = format!(
        "{}\nConcurrent repository edit.\n",
        fs::read_to_string(&agents_path).expect("AGENTS should read")
    );
    fs::write(&agents_path, &changed).expect("source drift should be injected");
    let digest_error = drift_service
        .apply(&authority_apply_request(&proposal, 200))
        .expect_err("changed source digest must fail closed");
    assert_eq!(digest_error.category(), mino::ErrorCategory::DriftDetected);
    assert_eq!(
        fs::read_to_string(&agents_path).expect("changed source should remain"),
        changed
    );

    let concurrent_project = prepare_authority_project("authority-concurrent-edit");
    let concurrent_service = PlanningAuthorityService::discover(concurrent_project.path())
        .expect("authority service should discover");
    let proposal = concurrent_service
        .propose()
        .expect("rewrite should be proposed");
    let request = authority_apply_request(&proposal, 201);
    concurrent_service
        .apply_with_failure(&request, IntegrationFailurePoint::BeforeBackup)
        .expect_err("rewrite should leave a prepared transaction");
    let concurrent_path = concurrent_project.path().join("AGENTS.md");
    fs::write(&concurrent_path, "externally replaced\n")
        .expect("concurrent source edit should be injected");
    let concurrent_error = concurrent_service
        .apply(&request)
        .expect_err("concurrent edit must block recovery");
    assert_eq!(
        concurrent_error.category(),
        mino::ErrorCategory::DriftDetected
    );
    assert_eq!(
        fs::read_to_string(concurrent_path).expect("external edit should remain"),
        "externally replaced\n"
    );

    let oversized_project = prepare_authority_project("authority-oversized");
    fs::write(
        oversized_project.path().join("AGENTS.md"),
        vec![b'x'; 1024 * 1024 + 1],
    )
    .expect("oversized source should be injected");
    let oversized = PlanningAuthorityService::discover(oversized_project.path())
        .expect("authority service should discover")
        .status()
        .expect_err("oversized source must fail closed");
    assert_eq!(oversized.category(), mino::ErrorCategory::DriftDetected);

    let directory_project = prepare_authority_project("authority-directory");
    let directory_path = directory_project.path().join("AGENTS.md");
    fs::remove_file(&directory_path).expect("AGENTS should be removed");
    fs::create_dir(&directory_path).expect("directory target should be injected");
    PlanningAuthorityService::discover(directory_project.path())
        .expect("authority service should discover")
        .status()
        .expect_err("directory target must fail closed");

    #[cfg(unix)]
    {
        let special_project = prepare_authority_project("authority-special-file");
        let special_path = special_project.path().join("AGENTS.md");
        fs::remove_file(&special_path).expect("AGENTS should be removed");
        let _listener = std::os::unix::net::UnixListener::bind(&special_path)
            .expect("socket target should be injected");
        PlanningAuthorityService::discover(special_project.path())
            .expect("authority service should discover")
            .status()
            .expect_err("special-file target must fail closed");
    }

    let symlink_project = prepare_authority_project("authority-symlink");
    let symlink_path = symlink_project.path().join("AGENTS.md");
    let external = symlink_project.path().join("external-agents.md");
    fs::write(&external, "external instructions\n").expect("external target should be written");
    fs::remove_file(&symlink_path).expect("AGENTS should be removed");
    if symlink_file(&external, &symlink_path).is_ok() {
        let symlink_error = PlanningAuthorityService::discover(symlink_project.path())
            .expect("authority service should discover")
            .status()
            .expect_err("symlink target must fail closed");
        assert_eq!(symlink_error.category(), mino::ErrorCategory::DriftDetected);
        assert_eq!(
            fs::read_to_string(external).expect("external target should remain"),
            "external instructions\n"
        );
    }
}
