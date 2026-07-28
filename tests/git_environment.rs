//! Regression tests for ambient Git environment isolation.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use mino::domain::{GitSetupDecision, PlanId, PlanStatus, PrePlanCleanupDecision};
use mino::git::inspect_changes;
use mino::project::initialize;
use mino::store::PlanStore;
use serde_json::Value;

static NEXT_TEMP_DIRECTORY: AtomicU64 = AtomicU64::new(1);

const HELPER_ENVIRONMENT: &str = "MINO_GIT_ENVIRONMENT_HELPER";
const HELPER_ROOT: &str = "MINO_GIT_ENVIRONMENT_ROOT";
const GIT_ENVIRONMENT_NAMES: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_CONFIG_COUNT",
    "GIT_CONFIG_KEY_0",
    "GIT_CONFIG_VALUE_0",
];

struct TestArea {
    root: PathBuf,
}

impl TestArea {
    fn new(label: &str) -> Self {
        let sequence = NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "mino-git-environment-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&root).expect("temporary Git environment area should be created");
        Self { root }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }
}

impl Drop for TestArea {
    fn drop(&mut self) {
        let temporary_root = std::env::temp_dir();
        if self.root.starts_with(&temporary_root)
            && self
                .root
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("mino-git-environment-"))
        {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

#[test]
fn cli_root_inspection_and_readiness_ignore_all_ambient_git_controls() {
    let area = TestArea::new("cli");
    let primary = committed_repository(&area.path("primary"), "primary", true);
    let foreign = committed_repository(&area.path("foreign"), "foreign", false);
    fs::write(primary.join("primary-change.txt"), "primary\n")
        .expect("primary change should be written");
    fs::write(foreign.join("foreign-change.txt"), "foreign\n")
        .expect("foreign change should be written");

    let inspected = json_success(&run_poisoned_mino(&primary, &foreign, &["git", "inspect"]));
    assert_eq!(
        PathBuf::from(
            inspected["facts"]["worktree"]
                .as_str()
                .expect("Git inspection should report a worktree")
        ),
        primary
    );
    assert_eq!(
        inspected["facts"]["untracked_paths"],
        Value::from(vec!["primary-change.txt"])
    );

    fs::remove_file(primary.join("primary-change.txt")).expect("primary change should be removed");
    let base_commit = git_text(&primary, &["rev-parse", "HEAD"]);
    let request_file = primary.join("request.md").to_string_lossy().into_owned();
    let created = json_success(&run_poisoned_mino(
        &primary,
        &foreign,
        &[
            "plan",
            "create",
            "--name",
            "Git environment isolation",
            "--trigger",
            "durable",
            "--request-file",
            &request_file,
            "--request-id",
            "81000000-0000-0000-0000-000000000001",
            "--actor",
            "codex",
        ],
    ));
    let plan_id = created["plan_id"]
        .as_str()
        .expect("plan creation should report an identifier");
    let plan_id = PlanId::parse(plan_id).expect("plan identifier should parse");
    let plan = PlanStore::new(&primary)
        .load_plan(&plan_id)
        .expect("created plan should load");
    assert_eq!(plan.git_readiness().repository(), "Present");
    assert_eq!(plan.git_readiness().working_tree(), "Clean");
    assert_eq!(plan.git_readiness().branch(), Some("main"));
    assert_eq!(
        plan.git_readiness().base_commit(),
        Some(base_commit.as_str())
    );
    assert!(plan.git_readiness().git_flow_enabled());
}

#[test]
fn discovery_and_readiness_probes_enforce_timeout_and_output_limits() {
    let area = TestArea::new("bounded");
    let shim_directory = area.path("shim");
    fs::create_dir(&shim_directory).expect("Git shim directory should be created");
    compile_git_shim(&shim_directory);

    let missing = initialized_non_git_project(&area.path("missing"));
    let missing_plan = create_plan(&missing, None, 1);
    assert_eq!(missing_plan.git_readiness().repository(), "Missing");
    assert_eq!(
        missing_plan.git_readiness().working_tree(),
        "Not Applicable"
    );
    assert!(!missing_plan.git_readiness().git_flow_enabled());

    let root_timeout = initialized_non_git_project(&area.path("root-timeout"));
    fs::write(root_timeout.join("root-timeout.marker"), "timeout\n")
        .expect("root timeout marker should be written");
    let started = Instant::now();
    let mut command = Command::new(std::env::current_exe().expect("test binary should resolve"));
    command
        .args(["--exact", "root_discovery_helper", "--nocapture"])
        .env(HELPER_ENVIRONMENT, "discover")
        .env(HELPER_ROOT, &root_timeout)
        .env("PATH", &shim_directory)
        .stdin(Stdio::null());
    clear_git_environment(&mut command);
    let output = command.output().expect("root discovery helper should run");
    assert!(
        output.status.success(),
        "root discovery helper failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        started.elapsed() < Duration::from_secs(8),
        "Git root discovery exceeded its bounded timeout"
    );

    let output_limited = initialized_non_git_project(&area.path("output-limited"));
    fs::write(output_limited.join("output-limit.marker"), "limit\n")
        .expect("output limit marker should be written");
    let output_failure = json_failure(
        &run_create_plan(&output_limited, Some(&shim_directory), 2),
        8,
        "drift_detected",
    );
    assert_eq!(
        output_failure["message"],
        "Git returned invalid machine-readable state"
    );

    let probe_timeout = initialized_non_git_project(&area.path("probe-timeout"));
    fs::write(probe_timeout.join("probe-timeout.marker"), "timeout\n")
        .expect("readiness timeout marker should be written");
    let started = Instant::now();
    let timeout_failure = json_failure(
        &run_create_plan(&probe_timeout, Some(&shim_directory), 3),
        7,
        "environment_unavailable",
    );
    assert!(
        started.elapsed() < Duration::from_secs(8),
        "Git readiness exceeded its bounded timeout"
    );
    assert_eq!(
        timeout_failure["message"],
        "Git inspection or operation is unavailable"
    );
}

#[test]
fn missing_repository_setup_decisions_are_auditable_and_recoverable() {
    let area = TestArea::new("setup-decisions");
    let continued = initialized_non_git_project(&area.path("continued"));
    let continued_plan = create_plan(&continued, None, 30);
    let continued_id = continued_plan.id().to_string();
    let continued_state = continued_plan
        .git_readiness_state()
        .expect("setup state should decode")
        .expect("setup state should exist");
    assert_eq!(
        continued_state.setup().decision(),
        GitSetupDecision::Pending
    );
    let context = json_success(&run_mino(&continued, &["agent", "context"]));
    assert_eq!(context["approval_required"], true);
    assert_eq!(context["next_actions"][0]["id"], "git.inspect");

    let continue_arguments = setup_decision_arguments(
        &continued_id,
        1,
        31,
        "continue-without-git",
        "chat:continue-without-git",
    );
    let continued_result = json_success(&run_mino_owned(&continued, &continue_arguments));
    assert_eq!(continued_result["revision"], 2);
    let continued_after = load_plan(&continued, &continued_id);
    assert_eq!(
        continued_after
            .git_readiness_state()
            .expect("setup state should decode")
            .expect("setup state should exist")
            .setup()
            .decision(),
        GitSetupDecision::ContinueWithoutGit
    );
    assert!(!continued_after.git_readiness().git_flow_enabled());
    let replay = json_success(&run_mino_owned(&continued, &continue_arguments));
    assert_eq!(replay["revision"], 2);
    assert_eq!(replay["replayed"], true);

    let blocked = initialized_non_git_project(&area.path("blocked"));
    let blocked_plan = create_plan(&blocked, None, 32);
    let blocked_id = blocked_plan.id().to_string();
    let blocked_result = json_success(&run_mino_owned(
        &blocked,
        &setup_decision_arguments(
            &blocked_id,
            1,
            33,
            "blocked-until-manual-setup",
            "chat:block-until-setup",
        ),
    ));
    assert_eq!(blocked_result["status"], "Blocked");
    assert!(load_plan(&blocked, &blocked_id).is_blocked_for_git_readiness());
    initialize_git_after_plan_creation(&blocked);
    let refreshed = json_success(&run_mino_owned(
        &blocked,
        &readiness_refresh_arguments(&blocked_id, 2, 34),
    ));
    assert_eq!(refreshed["revision"], 3);
    assert_eq!(refreshed["status"], "Draft");
    let blocked_after = load_plan(&blocked, &blocked_id);
    assert!(!blocked_after.is_blocked_for_git_readiness());
    assert!(blocked_after.git_readiness().git_flow_enabled());
}

#[test]
#[allow(clippy::too_many_lines)]
fn cleanup_proposal_approval_recording_and_refresh_verify_external_commits() {
    let area = TestArea::new("cleanup-flow");
    let repository = committed_repository(&area.path("repository"), "cleanup", true);
    fs::write(
        repository.join("src/lib.rs"),
        "pub const VALUE: &str = \"updated\";\n",
    )
    .expect("tracked cleanup path should change");
    fs::write(repository.join("notes.txt"), "pre-plan notes\n")
        .expect("untracked cleanup path should exist");
    let plan = create_plan(&repository, None, 40);
    let plan_id = plan.id().to_string();
    let state = plan
        .git_readiness_state()
        .expect("cleanup state should decode")
        .expect("cleanup state should exist");
    assert_eq!(state.cleanup().decision(), PrePlanCleanupDecision::Pending);
    assert_eq!(
        state.cleanup().observed_paths(),
        ["notes.txt", "src/lib.rs"]
    );
    let base = git_text(&repository, &["rev-parse", "HEAD"]);
    let status_before = git_text(&repository, &["status", "--short"]);

    let incomplete_path = area.path("incomplete-cleanup.yaml");
    fs::write(
        &incomplete_path,
        "items:\n  - logical_change: Update source\n    files: [src/lib.rs]\n    planned_commit_message: \"chore(cleanup): update source\"\n",
    )
    .expect("incomplete proposal should be written");
    let incomplete = run_mino_owned(
        &repository,
        &cleanup_propose_arguments(&plan_id, 1, 41, &incomplete_path),
    );
    json_failure(&incomplete, 2, "incomplete_or_validation");
    assert_eq!(load_plan(&repository, &plan_id).revision(), 1);

    let invalid_message_path = area.path("invalid-message-cleanup.yaml");
    fs::write(
        &invalid_message_path,
        "items:\n  - logical_change: Invalid message\n    files: [notes.txt, src/lib.rs]\n    planned_commit_message: \"cleanup source\"\n",
    )
    .expect("invalid-message proposal should be written");
    let invalid_message = run_mino_owned(
        &repository,
        &cleanup_propose_arguments(&plan_id, 1, 42, &invalid_message_path),
    );
    json_failure(&invalid_message, 2, "incomplete_or_validation");
    assert_eq!(load_plan(&repository, &plan_id).revision(), 1);

    let proposal_path = area.path("cleanup.yaml");
    fs::write(
        &proposal_path,
        "items:\n  - logical_change: Update source\n    files: [src/lib.rs]\n    planned_commit_message: \"chore(cleanup): update source\"\n  - logical_change: Add notes\n    files: [notes.txt]\n    planned_commit_message: \"docs(cleanup): add notes\"\n",
    )
    .expect("complete proposal should be written");
    let proposed = json_success(&run_mino_owned(
        &repository,
        &cleanup_propose_arguments(&plan_id, 1, 43, &proposal_path),
    ));
    assert_eq!(proposed["revision"], 2);
    assert_eq!(git_text(&repository, &["rev-parse", "HEAD"]), base);
    assert_eq!(git_text(&repository, &["status", "--short"]), status_before);

    json_success(&run_mino_owned(
        &repository,
        &cleanup_approve_arguments(&plan_id, 2, 44, "C1"),
    ));
    let reproposal = run_mino_owned(
        &repository,
        &cleanup_propose_arguments(&plan_id, 3, 60, &proposal_path),
    );
    json_failure(&reproposal, 2, "incomplete_or_validation");
    assert_eq!(load_plan(&repository, &plan_id).revision(), 3);
    json_success(&run_mino_owned(
        &repository,
        &cleanup_approve_arguments(&plan_id, 3, 45, "C2"),
    ));
    let wrong_order = run_mino_owned(
        &repository,
        &cleanup_record_arguments(&plan_id, 4, 46, "C2", &base),
    );
    json_failure(&wrong_order, 2, "incomplete_or_validation");
    assert_eq!(load_plan(&repository, &plan_id).revision(), 4);
    git(&repository, &["add", "--", "src/lib.rs"]);
    git_commit(&repository, "chore(cleanup): update source");
    let first_commit = git_text(&repository, &["rev-parse", "HEAD"]);
    let context = json_success(&run_mino_owned(
        &repository,
        &["agent".to_owned(), "context".to_owned()],
    ));
    assert_eq!(context["next_actions"][0]["id"], "git.inspect");
    assert!(
        context["allowed_actions"]
            .as_array()
            .is_some_and(|actions| actions.iter().any(|action| action == "git.cleanup.record"))
    );
    let premature_refresh =
        run_mino_owned(&repository, &readiness_refresh_arguments(&plan_id, 4, 61));
    json_failure(&premature_refresh, 2, "incomplete_or_validation");
    assert_eq!(load_plan(&repository, &plan_id).revision(), 4);
    json_success(&run_mino_owned(
        &repository,
        &cleanup_record_arguments(&plan_id, 4, 47, "C1", &first_commit),
    ));
    git(&repository, &["add", "--", "notes.txt"]);
    git_commit(&repository, "docs(cleanup): add notes");
    let second_commit = git_text(&repository, &["rev-parse", "HEAD"]);
    json_success(&run_mino_owned(
        &repository,
        &cleanup_record_arguments(&plan_id, 5, 48, "C2", &second_commit),
    ));
    assert!(git_text(&repository, &["status", "--short"]).is_empty());

    let refreshed = json_success(&run_mino_owned(
        &repository,
        &readiness_refresh_arguments(&plan_id, 6, 49),
    ));
    assert_eq!(refreshed["revision"], 7);
    let completed = load_plan(&repository, &plan_id);
    let completed_state = completed
        .git_readiness_state()
        .expect("completed cleanup state should decode")
        .expect("completed cleanup state should exist");
    assert_eq!(
        completed_state.cleanup().decision(),
        PrePlanCleanupDecision::Completed
    );
    assert_eq!(
        completed_state.cleanup().items()[0].actual_commit(),
        Some(first_commit.as_str())
    );
    assert_eq!(
        completed_state.cleanup().items()[1].actual_commit(),
        Some(second_commit.as_str())
    );
    assert!(completed.git_readiness().git_flow_enabled());
}

#[test]
fn unapproved_cleanup_proposal_resets_after_a_clean_same_head_refresh() {
    let area = TestArea::new("cleanup-reset");
    let repository = committed_repository(&area.path("repository"), "cleanup-reset", true);
    fs::write(
        repository.join("src/lib.rs"),
        "pub const VALUE: &str = \"changed\";\n",
    )
    .expect("cleanup path should change");
    let plan = create_plan(&repository, None, 70);
    let plan_id = plan.id().to_string();
    let proposal_path = area.path("cleanup-reset.yaml");
    fs::write(
        &proposal_path,
        "items:\n  - logical_change: Restore source\n    files: [src/lib.rs]\n    planned_commit_message: \"chore(cleanup): restore source\"\n",
    )
    .expect("cleanup proposal should be written");
    json_success(&run_mino_owned(
        &repository,
        &cleanup_propose_arguments(&plan_id, 1, 71, &proposal_path),
    ));
    fs::write(
        repository.join("src/lib.rs"),
        "pub const VALUE: &str = \"cleanup-reset\";\n",
    )
    .expect("cleanup path should return to its base content");
    assert!(git_text(&repository, &["status", "--short"]).is_empty());

    json_success(&run_mino_owned(
        &repository,
        &readiness_refresh_arguments(&plan_id, 2, 72),
    ));
    let refreshed = load_plan(&repository, &plan_id);
    let state = refreshed
        .git_readiness_state()
        .expect("refreshed cleanup should decode")
        .expect("refreshed cleanup should exist");
    assert_eq!(
        state.cleanup().decision(),
        PrePlanCleanupDecision::NotRequired
    );
    assert!(state.cleanup().items().is_empty());
    assert!(refreshed.git_readiness().git_flow_enabled());
}

#[test]
fn unsafe_initial_cleanup_state_is_recoverably_blocked() {
    let area = TestArea::new("cleanup-unsafe");
    let repository = committed_repository(&area.path("repository"), "cleanup-unsafe", true);
    git(&repository, &["switch", "-c", "cleanup-conflict"]);
    fs::write(
        repository.join("src/lib.rs"),
        "pub const VALUE: &str = \"branch\";\n",
    )
    .expect("branch conflict content should be written");
    git(&repository, &["add", "--", "src/lib.rs"]);
    git_commit(&repository, "test(git): change conflict branch");
    git(&repository, &["switch", "main"]);
    fs::write(
        repository.join("src/lib.rs"),
        "pub const VALUE: &str = \"main\";\n",
    )
    .expect("main conflict content should be written");
    git(&repository, &["add", "--", "src/lib.rs"]);
    git_commit(&repository, "test(git): change main branch");
    let mut merge = Command::new("git");
    merge
        .args([
            "-c",
            "user.name=Mino Tests",
            "-c",
            "user.email=mino-tests@example.invalid",
            "merge",
            "--no-edit",
            "cleanup-conflict",
        ])
        .current_dir(&repository);
    clear_git_environment(&mut merge);
    let output = merge.output().expect("conflicting merge should run");
    assert!(!output.status.success());

    let plan = create_plan(&repository, None, 80);
    assert_eq!(plan.status(), PlanStatus::Blocked);
    assert!(plan.is_blocked_for_git_readiness());
    let state = plan
        .git_readiness_state()
        .expect("unsafe cleanup state should decode")
        .expect("unsafe cleanup state should exist");
    assert!(
        state
            .cleanup()
            .blockers()
            .iter()
            .any(|blocker| blocker.starts_with("unmerged:"))
    );
    assert!(!plan.git_readiness().git_flow_enabled());
}

#[test]
fn declined_cleanup_with_file_map_overlap_blocks_until_a_clean_refresh() {
    let area = TestArea::new("cleanup-overlap");
    let repository = committed_repository(&area.path("repository"), "overlap", true);
    fs::create_dir_all(repository.join("src/application"))
        .expect("overlap source directory should exist");
    fs::write(
        repository.join("src/application/plan.rs"),
        "pub fn baseline() {}\n",
    )
    .expect("overlap baseline should be written");
    git(&repository, &["add", "--", "src/application/plan.rs"]);
    git_commit(&repository, "test: add overlap baseline");
    fs::write(
        repository.join("src/application/plan.rs"),
        "pub fn changed() {}\n",
    )
    .expect("overlap path should become dirty");
    let plan = create_plan(&repository, None, 50);
    let plan_id = plan.id().to_string();
    let apply = vec![
        "plan".to_owned(),
        "apply".to_owned(),
        "--plan".to_owned(),
        plan_id.clone(),
        "--expect-revision".to_owned(),
        "1".to_owned(),
        "--request-id".to_owned(),
        request_id(51),
        "--actor".to_owned(),
        "codex".to_owned(),
        "--file".to_owned(),
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/drafts/complete.yaml")
            .to_string_lossy()
            .into_owned(),
    ];
    let applied = json_success(&run_mino_owned(&repository, &apply));
    assert_eq!(applied["status"], "Blocked");
    let declined = json_success(&run_mino_owned(
        &repository,
        &cleanup_decline_arguments(&plan_id, 2, 52),
    ));
    assert_eq!(declined["status"], "Blocked");
    let blocked = load_plan(&repository, &plan_id);
    assert!(blocked.is_blocked_for_git_readiness());
    assert!(!blocked.git_readiness().git_flow_enabled());

    fs::write(
        repository.join("src/application/plan.rs"),
        "pub fn baseline() {}\n",
    )
    .expect("overlap source should be restored");
    assert!(git_text(&repository, &["status", "--short"]).is_empty());
    let refreshed = json_success(&run_mino_owned(
        &repository,
        &readiness_refresh_arguments(&plan_id, 3, 53),
    ));
    assert_eq!(refreshed["status"], "Draft");
    let unblocked = load_plan(&repository, &plan_id);
    assert!(!unblocked.is_blocked_for_git_readiness());
    assert_eq!(
        unblocked
            .git_readiness_state()
            .expect("declined cleanup should decode")
            .expect("declined cleanup should exist")
            .cleanup()
            .decision(),
        PrePlanCleanupDecision::Declined
    );
    assert!(!unblocked.git_readiness().git_flow_enabled());
}

#[test]
fn agent_context_only_downgrades_an_explicit_non_repository() {
    let area = TestArea::new("agent-inspection");
    let shim_directory = area.path("shim");
    fs::create_dir(&shim_directory).expect("Git shim directory should be created");
    compile_git_shim(&shim_directory);

    let non_repository = initialized_non_git_project(&area.path("non-repository"));
    let context = json_success(&run_with_git_shim(
        &non_repository,
        &shim_directory,
        &["agent", "context"],
    ));
    assert_eq!(context["git"], Value::Null);

    for (label, marker, exit_code, error_code) in [
        (
            "command-failure",
            "command-failure.marker",
            7,
            "environment_unavailable",
        ),
        ("malformed", "malformed.marker", 8, "drift_detected"),
        ("output-limit", "output-limit.marker", 8, "drift_detected"),
    ] {
        let project = initialized_non_git_project(&area.path(label));
        fs::write(project.join(marker), "trigger\n").expect("Git failure marker should be written");
        let output = run_with_git_shim(&project, &shim_directory, &["agent", "context"]);
        let failure = json_failure(&output, exit_code, error_code);
        assert_eq!(failure["active_plan"], Value::Null);
        assert!(!String::from_utf8_lossy(&output.stdout).contains("gitcommandcredentialvalue"));
    }

    let timeout = initialized_non_git_project(&area.path("timeout"));
    fs::write(timeout.join("probe-timeout.marker"), "timeout\n")
        .expect("Git timeout marker should be written");
    let started = Instant::now();
    let failure = run_with_git_shim(&timeout, &shim_directory, &["agent", "context"]);
    assert!(
        started.elapsed() < Duration::from_secs(8),
        "Agent Git inspection exceeded its bounded timeout"
    );
    json_failure(&failure, 7, "environment_unavailable");

    let empty_path = area.path("missing-git");
    fs::create_dir(&empty_path).expect("empty PATH directory should be created");
    let missing = initialized_non_git_project(&area.path("missing-executable"));
    let failure = run_with_git_shim(&missing, &empty_path, &["agent", "context"]);
    json_failure(&failure, 7, "environment_unavailable");
}

#[test]
fn recorded_git_plan_blocks_agent_context_when_repository_metadata_breaks() {
    let area = TestArea::new("recorded-git-failure");
    let repository = committed_repository(&area.path("repository"), "recorded-git", true);
    let plan = create_plan(&repository, None, 20);
    assert_eq!(plan.git_readiness().repository(), "Present");
    assert!(plan.git_readiness().git_flow_enabled());

    let secret = "gitmetadatacredentialvalue";
    fs::write(repository.join(".git/HEAD"), format!("invalid {secret}\n"))
        .expect("corrupt HEAD should be written");
    let output = run_mino(&repository, &["agent", "context"]);
    json_failure(&output, 7, "environment_unavailable");
    assert!(!String::from_utf8_lossy(&output.stdout).contains(secret));
}

#[test]
fn inspect_changes_uses_project_root_and_default_index_under_each_git_override() {
    let area = TestArea::new("changes");
    let primary = committed_repository(&area.path("primary"), "primary", false);
    let foreign = committed_repository(&area.path("foreign"), "foreign", false);
    fs::write(primary.join("inside-change.txt"), "inside\n")
        .expect("inside change should be written");
    fs::write(foreign.join("foreign-change.txt"), "foreign\n")
        .expect("foreign change should be written");

    for override_name in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_CONFIG_COUNT",
    ] {
        let mut command =
            Command::new(std::env::current_exe().expect("test binary should resolve"));
        command
            .args(["--exact", "inspect_changes_helper", "--nocapture"])
            .env(HELPER_ENVIRONMENT, "inspect_changes")
            .env(HELPER_ROOT, &primary)
            .stdin(Stdio::null());
        clear_git_environment(&mut command);
        apply_single_override(&mut command, override_name, &foreign);
        let output = command.output().expect("inspection helper should run");
        assert!(
            output.status.success(),
            "override {override_name} escaped the project Git boundary\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn inspect_changes_helper() {
    if std::env::var(HELPER_ENVIRONMENT).as_deref() != Ok("inspect_changes") {
        return;
    }
    let root = PathBuf::from(
        std::env::var_os(HELPER_ROOT).expect("inspection helper root should be supplied"),
    );
    let changes = inspect_changes(&root).expect("project changes should inspect");
    assert!(changes.is_repository());
    assert_eq!(changes.files().len(), 1);
    assert_eq!(changes.files()[0].path(), "inside-change.txt");
}

#[test]
fn root_discovery_helper() {
    if std::env::var(HELPER_ENVIRONMENT).as_deref() != Ok("discover") {
        return;
    }
    let root = PathBuf::from(
        std::env::var_os(HELPER_ROOT).expect("root discovery helper path should be supplied"),
    );
    let discovered = mino::project::discover(&root).expect("Mino marker fallback should succeed");
    assert_eq!(discovered.path(), root);
    assert_eq!(
        discovered.source(),
        mino::project::RootSource::MinoDirectory
    );
}

fn committed_repository(path: &Path, contents: &str, initialize_mino: bool) -> PathBuf {
    fs::create_dir_all(path.join("src")).expect("repository source directory should be created");
    fs::write(
        path.join("Cargo.toml"),
        format!("[package]\nname = \"{contents}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n"),
    )
    .expect("repository manifest should be written");
    fs::write(
        path.join("src/lib.rs"),
        format!("pub const VALUE: &str = \"{contents}\";\n"),
    )
    .expect("repository source should be written");
    fs::write(
        path.join("request.md"),
        "Preserve the Git project boundary.\n",
    )
    .expect("plan request should be written");
    fs::write(path.join(".gitignore"), "/.mino/\n/docs/plan/\n")
        .expect("ignore file should be written");
    if initialize_mino {
        initialize(path).expect("Mino project should initialize");
    }
    git(path, &["init", "--quiet", "--initial-branch", "main"]);
    git(path, &["add", "."]);
    git(
        path,
        &[
            "-c",
            "user.name=Mino Tests",
            "-c",
            "user.email=mino-tests@example.invalid",
            "commit",
            "--quiet",
            "-m",
            contents,
        ],
    );
    path.canonicalize().expect("repository should resolve")
}

fn initialized_non_git_project(path: &Path) -> PathBuf {
    fs::create_dir_all(path).expect("non-Git project should be created");
    fs::write(
        path.join("Cargo.toml"),
        "[package]\nname = \"bounded-probe\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("non-Git manifest should be written");
    fs::write(path.join("request.md"), "Bound Git probes.\n")
        .expect("bounded probe request should be written");
    initialize(path).expect("non-Git Mino project should initialize");
    path.canonicalize().expect("non-Git project should resolve")
}

fn create_plan(
    root: &Path,
    shim_directory: Option<&Path>,
    request_number: u64,
) -> mino::domain::Plan {
    let output = run_create_plan(root, shim_directory, request_number);
    let created = json_success(&output);
    let plan_id = created["plan_id"]
        .as_str()
        .expect("plan creation should report an identifier");
    let plan_id = PlanId::parse(plan_id).expect("plan identifier should parse");
    PlanStore::new(root)
        .load_plan(&plan_id)
        .expect("bounded-probe plan should load")
}

fn load_plan(root: &Path, plan_id: &str) -> mino::domain::Plan {
    PlanStore::new(root)
        .load_plan(&PlanId::parse(plan_id).expect("plan ID should parse"))
        .expect("plan should load")
}

fn request_id(number: u64) -> String {
    format!("82000000-0000-0000-0000-{number:012}")
}

fn run_mino_owned(root: &Path, arguments: &[String]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mino"))
        .arg("--root")
        .arg(root)
        .args(["--format", "json", "--no-input"])
        .args(arguments)
        .stdin(Stdio::null())
        .output()
        .expect("Mino binary should run")
}

fn setup_decision_arguments(
    plan_id: &str,
    revision: u64,
    request_number: u64,
    decision: &str,
    approval_reference: &str,
) -> Vec<String> {
    vec![
        "git".to_owned(),
        "setup".to_owned(),
        "decide".to_owned(),
        "--plan".to_owned(),
        plan_id.to_owned(),
        "--decision".to_owned(),
        decision.to_owned(),
        "--approval-ref".to_owned(),
        approval_reference.to_owned(),
        "--expect-revision".to_owned(),
        revision.to_string(),
        "--request-id".to_owned(),
        request_id(request_number),
        "--actor".to_owned(),
        "codex".to_owned(),
    ]
}

fn readiness_refresh_arguments(plan_id: &str, revision: u64, request_number: u64) -> Vec<String> {
    vec![
        "git".to_owned(),
        "readiness".to_owned(),
        "refresh".to_owned(),
        "--plan".to_owned(),
        plan_id.to_owned(),
        "--expect-revision".to_owned(),
        revision.to_string(),
        "--request-id".to_owned(),
        request_id(request_number),
        "--actor".to_owned(),
        "codex".to_owned(),
    ]
}

fn cleanup_propose_arguments(
    plan_id: &str,
    revision: u64,
    request_number: u64,
    file: &Path,
) -> Vec<String> {
    vec![
        "git".to_owned(),
        "cleanup".to_owned(),
        "propose".to_owned(),
        "--plan".to_owned(),
        plan_id.to_owned(),
        "--file".to_owned(),
        file.to_string_lossy().into_owned(),
        "--expect-revision".to_owned(),
        revision.to_string(),
        "--request-id".to_owned(),
        request_id(request_number),
        "--actor".to_owned(),
        "codex".to_owned(),
    ]
}

fn cleanup_approve_arguments(
    plan_id: &str,
    revision: u64,
    request_number: u64,
    item_id: &str,
) -> Vec<String> {
    vec![
        "git".to_owned(),
        "cleanup".to_owned(),
        "approve".to_owned(),
        "--plan".to_owned(),
        plan_id.to_owned(),
        "--item".to_owned(),
        item_id.to_owned(),
        "--approval-ref".to_owned(),
        format!("chat:approve-{item_id}"),
        "--expect-revision".to_owned(),
        revision.to_string(),
        "--request-id".to_owned(),
        request_id(request_number),
        "--actor".to_owned(),
        "codex".to_owned(),
    ]
}

fn cleanup_decline_arguments(plan_id: &str, revision: u64, request_number: u64) -> Vec<String> {
    vec![
        "git".to_owned(),
        "cleanup".to_owned(),
        "approve".to_owned(),
        "--plan".to_owned(),
        plan_id.to_owned(),
        "--decline".to_owned(),
        "--approval-ref".to_owned(),
        "chat:decline-cleanup".to_owned(),
        "--expect-revision".to_owned(),
        revision.to_string(),
        "--request-id".to_owned(),
        request_id(request_number),
        "--actor".to_owned(),
        "codex".to_owned(),
    ]
}

fn cleanup_record_arguments(
    plan_id: &str,
    revision: u64,
    request_number: u64,
    item_id: &str,
    commit: &str,
) -> Vec<String> {
    vec![
        "git".to_owned(),
        "cleanup".to_owned(),
        "record".to_owned(),
        "--plan".to_owned(),
        plan_id.to_owned(),
        "--item".to_owned(),
        item_id.to_owned(),
        "--commit".to_owned(),
        commit.to_owned(),
        "--expect-revision".to_owned(),
        revision.to_string(),
        "--request-id".to_owned(),
        request_id(request_number),
        "--actor".to_owned(),
        "codex".to_owned(),
    ]
}

fn initialize_git_after_plan_creation(root: &Path) {
    fs::write(
        root.join(".gitignore"),
        "/.mino/\n/docs/plan/\n/request.md\n",
    )
    .expect("post-plan Git ignore should be written");
    git(root, &["init", "--quiet", "--initial-branch", "main"]);
    git(root, &["add", "."]);
    git_commit(root, "chore: establish external Git baseline");
}

fn git_commit(root: &Path, message: &str) {
    git(
        root,
        &[
            "-c",
            "user.name=Mino Tests",
            "-c",
            "user.email=mino-tests@example.invalid",
            "commit",
            "--quiet",
            "-m",
            message,
        ],
    );
}

fn run_create_plan(root: &Path, shim_directory: Option<&Path>, request_number: u64) -> Output {
    let request_file = root.join("request.md").to_string_lossy().into_owned();
    let request_id = format!("81000000-0000-0000-0000-{request_number:012}");
    let arguments = [
        "plan",
        "create",
        "--name",
        "Bounded Git probe",
        "--trigger",
        "durable",
        "--request-file",
        &request_file,
        "--request-id",
        &request_id,
        "--actor",
        "codex",
    ];
    if let Some(shim_directory) = shim_directory {
        run_with_git_shim(root, shim_directory, &arguments)
    } else {
        run_mino(root, &arguments)
    }
}

fn compile_git_shim(directory: &Path) {
    let source = directory.join("git-shim.rs");
    let executable = directory.join(if cfg!(windows) { "git.exe" } else { "git" });
    fs::write(
        &source,
        r#"use std::env;
use std::io::{self, Write};
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

fn main() {
    let arguments = env::args_os().collect::<Vec<_>>();
    let root = arguments
        .windows(2)
        .find(|pair| pair[0] == "-C")
        .map(|pair| PathBuf::from(&pair[1]))
        .expect("Git shim should receive -C root");
    let is_root_probe = arguments.iter().any(|argument| argument == "--show-toplevel");
    if is_root_probe && root.join("root-timeout.marker").exists() {
        thread::sleep(Duration::from_secs(30));
        return;
    }
    if is_root_probe {
        println!("{}", root.display());
        return;
    }
    if arguments
        .iter()
        .any(|argument| argument == "--is-bare-repository")
    {
        println!("false");
        return;
    }
    if arguments
        .iter()
        .any(|argument| argument == "--is-inside-work-tree")
    {
        if root.join("command-failure.marker").exists() {
            eprintln!("permission denied: gitcommandcredentialvalue");
            std::process::exit(1);
        }
        if root.join("malformed.marker").exists() {
            println!("invalid-worktree-value");
            return;
        }
        if root.join("probe-timeout.marker").exists() {
            thread::sleep(Duration::from_secs(30));
            return;
        }
        if root.join("output-limit.marker").exists() {
            io::stdout()
                .write_all(&vec![b'x'; 70 * 1024])
                .expect("Git shim output should write");
            return;
        }
        println!("false");
        return;
    }
}
"#,
    )
    .expect("Git shim source should be written");
    let output = Command::new("rustc")
        .args([source.as_os_str(), "-o".as_ref(), executable.as_os_str()])
        .output()
        .expect("rustc should compile the Git shim");
    assert!(
        output.status.success(),
        "Git shim compilation failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn run_with_git_shim(root: &Path, shim_directory: &Path, arguments: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mino"));
    command
        .arg("--root")
        .arg(root)
        .args(["--format", "json", "--no-input"])
        .args(arguments)
        .env("PATH", shim_directory)
        .stdin(Stdio::null());
    clear_git_environment(&mut command);
    command
        .output()
        .expect("Mino binary should run with Git shim")
}

fn run_mino(root: &Path, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mino"))
        .arg("--root")
        .arg(root)
        .args(["--format", "json", "--no-input"])
        .args(arguments)
        .stdin(Stdio::null())
        .output()
        .expect("Mino binary should run")
}

fn run_poisoned_mino(root: &Path, foreign: &Path, arguments: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mino"));
    command
        .arg("--root")
        .arg(root)
        .args(["--format", "json", "--no-input"])
        .args(arguments)
        .stdin(Stdio::null());
    clear_git_environment(&mut command);
    command
        .env("GIT_DIR", foreign.join(".git"))
        .env("GIT_WORK_TREE", foreign)
        .env("GIT_INDEX_FILE", foreign.join(".git/index"))
        .env("GIT_OBJECT_DIRECTORY", foreign.join(".git/objects"))
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "core.worktree")
        .env("GIT_CONFIG_VALUE_0", foreign)
        .output()
        .expect("Mino binary should run")
}

fn apply_single_override(command: &mut Command, name: &str, foreign: &Path) {
    match name {
        "GIT_DIR" => {
            command.env(name, foreign.join(".git"));
        }
        "GIT_WORK_TREE" => {
            command.env(name, foreign);
        }
        "GIT_INDEX_FILE" => {
            command.env(name, foreign.join(".git/index"));
        }
        "GIT_OBJECT_DIRECTORY" => {
            command.env(name, foreign.join(".git/objects"));
        }
        "GIT_CONFIG_COUNT" => {
            command
                .env(name, "1")
                .env("GIT_CONFIG_KEY_0", "core.worktree")
                .env("GIT_CONFIG_VALUE_0", foreign);
        }
        _ => unreachable!("override names are fixed by the test"),
    }
}

fn clear_git_environment(command: &mut Command) {
    for name in GIT_ENVIRONMENT_NAMES {
        command.env_remove(name);
    }
}

fn json_success(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    serde_json::from_slice(&output.stdout).expect("Mino stdout should be JSON")
}

fn json_failure(output: &Output, exit_code: i32, error_code: &str) -> Value {
    assert_eq!(
        output.status.code(),
        Some(exit_code),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let value: Value = serde_json::from_slice(&output.stdout).expect("Mino failure should be JSON");
    assert_eq!(value["error"]["code"], error_code);
    value
}

fn git(path: &Path, arguments: &[&str]) {
    let mut command = Command::new("git");
    command.args(arguments).current_dir(path);
    clear_git_environment(&mut command);
    let output = command.output().expect("Git should run");
    assert!(
        output.status.success(),
        "git {arguments:?} failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_text(path: &Path, arguments: &[&str]) -> String {
    let mut command = Command::new("git");
    command.args(arguments).current_dir(path);
    clear_git_environment(&mut command);
    let output = command.output().expect("Git should run");
    assert!(output.status.success(), "Git text query should succeed");
    String::from_utf8(output.stdout)
        .expect("Git text should be UTF-8")
        .trim()
        .to_owned()
}
