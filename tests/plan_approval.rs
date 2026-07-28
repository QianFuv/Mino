//! Contract tests for plan finalization, show, revision-bound review, and approval.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use mino::domain::{DomainErrorKind, GitFlowConsent, Plan, PlanId, TaskId, TaskStatus, Timestamp};
use mino::project::initialize;
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
            "mino-plan-approval-{label}-{}-{sequence}",
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
        initialize(&path).expect("temporary project should initialize");
        let project = Self {
            path: path.canonicalize().expect("project root should resolve"),
        };
        initialize_git(&project);
        project
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
                .is_some_and(|name| name.to_string_lossy().starts_with("mino-plan-approval-"))
        {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/drafts")
        .join(name)
}

fn run_mino(arguments: &[String]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mino"))
        .args(arguments)
        .stdin(Stdio::null())
        .output()
        .expect("Mino binary should run")
}

fn base_arguments(project: &TestProject) -> Vec<String> {
    vec![
        "--root".to_owned(),
        project.path().to_string_lossy().into_owned(),
        "--format".to_owned(),
        "json".to_owned(),
        "--no-input".to_owned(),
    ]
}

fn request_id(number: u64) -> String {
    format!("20000000-0000-0000-0000-{number:012}")
}

fn parse_json(output: &Output) -> Value {
    assert!(
        output.stderr.is_empty(),
        "stderr must be empty in JSON mode"
    );
    serde_json::from_slice(&output.stdout).expect("stdout should be JSON")
}

fn parse_success(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    parse_json(output)
}

fn assert_readiness_drift(output: &Output, expected_mismatches: &[&str]) {
    assert_eq!(output.status.code(), Some(8));
    let value = parse_json(output);
    assert_eq!(value["error"]["code"], "drift_detected");
    assert_eq!(
        value["readiness_mismatches"],
        serde_json::json!(expected_mismatches)
    );
    assert_eq!(value["next_actions"][0]["id"], "git.readiness.refresh");
}

fn create_plan(project: &TestProject, name: &str, request_number: u64) -> String {
    let request_file = project.path().join(format!("request-{request_number}.md"));
    fs::write(&request_file, "Finalize and approve a complete plan.\n")
        .expect("request fixture should be written");
    let mut arguments = base_arguments(project);
    arguments.extend([
        "plan".to_owned(),
        "create".to_owned(),
        "--name".to_owned(),
        name.to_owned(),
        "--trigger".to_owned(),
        "durable".to_owned(),
        "--request-file".to_owned(),
        request_file.to_string_lossy().into_owned(),
        "--request-id".to_owned(),
        request_id(request_number),
        "--actor".to_owned(),
        "codex".to_owned(),
    ]);
    let value = parse_success(&run_mino(&arguments));
    value["plan_id"]
        .as_str()
        .expect("create should return a plan ID")
        .to_owned()
}

fn apply_complete(
    project: &TestProject,
    plan_id: &str,
    expected_revision: u64,
    request_number: u64,
) {
    let mut arguments = mutation_arguments(
        project,
        &["plan", "apply"],
        plan_id,
        expected_revision,
        request_number,
    );
    arguments.extend([
        "--file".to_owned(),
        fixture_path("complete.yaml").to_string_lossy().into_owned(),
    ]);
    parse_success(&run_mino(&arguments));
}

fn mutation_arguments(
    project: &TestProject,
    command: &[&str],
    plan_id: &str,
    expected_revision: u64,
    request_number: u64,
) -> Vec<String> {
    let mut arguments = base_arguments(project);
    arguments.extend(command.iter().map(|part| (*part).to_owned()));
    arguments.extend([
        "--plan".to_owned(),
        plan_id.to_owned(),
        "--expect-revision".to_owned(),
        expected_revision.to_string(),
        "--request-id".to_owned(),
        request_id(request_number),
        "--actor".to_owned(),
        "codex".to_owned(),
    ]);
    arguments
}

fn read_arguments(project: &TestProject, command: &str, plan_id: &str) -> Vec<String> {
    let mut arguments = base_arguments(project);
    arguments.extend([
        "plan".to_owned(),
        command.to_owned(),
        "--plan".to_owned(),
        plan_id.to_owned(),
    ]);
    arguments
}

fn finalize_arguments(
    project: &TestProject,
    plan_id: &str,
    expected_revision: u64,
    request_number: u64,
) -> Vec<String> {
    mutation_arguments(
        project,
        &["plan", "finalize"],
        plan_id,
        expected_revision,
        request_number,
    )
}

fn approve_arguments(
    project: &TestProject,
    plan_id: &str,
    expected_revision: u64,
    request_number: u64,
    consent: &str,
) -> Vec<String> {
    let mut arguments = mutation_arguments(
        project,
        &["plan", "approve"],
        plan_id,
        expected_revision,
        request_number,
    );
    arguments.extend([
        "--approval-ref".to_owned(),
        "chat:explicit-approval".to_owned(),
        "--git-flow-consent".to_owned(),
        consent.to_owned(),
    ]);
    arguments
}

fn refresh_arguments(
    project: &TestProject,
    plan_id: &str,
    expected_revision: u64,
    request_number: u64,
) -> Vec<String> {
    mutation_arguments(
        project,
        &["git", "readiness", "refresh"],
        plan_id,
        expected_revision,
        request_number,
    )
}

fn initialize_git(project: &TestProject) {
    fs::write(
        project.path().join(".gitignore"),
        "/.mino/\n/docs/plan/\n/request-*.md\n",
    )
    .expect("Git ignore fixture should be written");
    git(
        project.path(),
        &["init", "--quiet", "--initial-branch", "main"],
    );
    git(project.path(), &["add", "."]);
    git(
        project.path(),
        &[
            "-c",
            "user.name=Mino Tests",
            "-c",
            "user.email=mino-tests@example.invalid",
            "commit",
            "--quiet",
            "-m",
            "chore: establish readiness fixture",
        ],
    );
}

fn git(root: &Path, arguments: &[&str]) {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(root)
        .output()
        .expect("Git should run");
    assert!(
        output.status.success(),
        "git {arguments:?} failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn typed_id(plan_id: &str) -> PlanId {
    PlanId::parse(plan_id).expect("plan ID should parse")
}

fn load_plan(project: &TestProject, plan_id: &str) -> Plan {
    PlanStore::new(project.path())
        .load_plan(&typed_id(plan_id))
        .expect("plan should load")
}

fn projection_path(project: &TestProject, plan_id: &str) -> PathBuf {
    project
        .path()
        .join("docs")
        .join("plan")
        .join(format!("{plan_id}.md"))
}

fn stored_bytes(project: &TestProject, plan_id: &str) -> [Vec<u8>; 3] {
    let store = PlanStore::new(project.path());
    let plan_id = typed_id(plan_id);
    [
        fs::read(store.paths().current_plan(&plan_id)).expect("state should exist"),
        fs::read(store.paths().event_log(&plan_id)).expect("events should exist"),
        fs::read(projection_path(project, plan_id.as_str())).expect("projection should exist"),
    ]
}

fn finalize_complete_plan(
    project: &TestProject,
    name: &str,
    create_request: u64,
) -> (String, Vec<String>) {
    let plan_id = create_plan(project, name, create_request);
    apply_complete(project, &plan_id, 1, create_request + 1);
    let arguments = finalize_arguments(project, &plan_id, 2, create_request + 2);
    let value = parse_success(&run_mino(&arguments));
    assert_eq!(value["status"], "Ready");
    assert_eq!(value["revision"], 3);
    (plan_id, arguments)
}

#[test]
fn finalize_rejects_incomplete_or_drifted_drafts_and_commits_once() {
    let incomplete_project = TestProject::new("incomplete");
    let incomplete_id = create_plan(&incomplete_project, "Incomplete lifecycle", 1);
    let before = stored_bytes(&incomplete_project, &incomplete_id);
    let invalid = run_mino(&finalize_arguments(
        &incomplete_project,
        &incomplete_id,
        1,
        2,
    ));
    assert_eq!(invalid.status.code(), Some(2));
    assert_eq!(parse_json(&invalid)["valid"], false);
    assert_eq!(stored_bytes(&incomplete_project, &incomplete_id), before);

    let drift_project = TestProject::new("draft-drift");
    let drift_id = create_plan(&drift_project, "Drifted lifecycle", 10);
    apply_complete(&drift_project, &drift_id, 1, 11);
    fs::write(projection_path(&drift_project, &drift_id), "manual edit\n")
        .expect("projection should be edited");
    let drift_before = stored_bytes(&drift_project, &drift_id);
    let drift = run_mino(&finalize_arguments(&drift_project, &drift_id, 2, 12));
    assert_eq!(drift.status.code(), Some(8));
    assert_eq!(stored_bytes(&drift_project, &drift_id), drift_before);

    let project = TestProject::new("finalize");
    let plan_id = create_plan(&project, "Finalized lifecycle", 20);
    apply_complete(&project, &plan_id, 1, 21);
    let complete_before = stored_bytes(&project, &plan_id);
    let conflicting = run_mino(&finalize_arguments(&project, &plan_id, 1, 29));
    assert_eq!(conflicting.status.code(), Some(3));
    assert_eq!(stored_bytes(&project, &plan_id), complete_before);
    let arguments = finalize_arguments(&project, &plan_id, 2, 22);
    let first = parse_success(&run_mino(&arguments));
    assert_eq!(
        first["message"],
        "Plan created successfully and is ready for review."
    );
    assert_eq!(first["revision"], 3);
    assert_eq!(first["replayed"], false);
    assert_eq!(first["complete"], false);
    assert_eq!(first["missing"], serde_json::json!(["approval"]));
    assert_eq!(first["next_actions"][0]["id"], "plan.review");
    let plan = load_plan(&project, &plan_id);
    assert!(
        plan.tasks()
            .iter()
            .all(|task| task.status() == TaskStatus::Ready)
    );
    assert_eq!(
        PlanStore::new(project.path())
            .events(&typed_id(&plan_id))
            .unwrap()
            .len(),
        3
    );
    let replay = parse_success(&run_mino(&arguments));
    assert_eq!(replay["revision"], 3);
    assert_eq!(replay["replayed"], true);
    assert_eq!(
        PlanStore::new(project.path())
            .events(&typed_id(&plan_id))
            .unwrap()
            .len(),
        3
    );
}

#[test]
fn show_review_and_approval_are_revision_bound_scope_preserving_and_replayable() {
    let project = TestProject::new("approval");
    let (plan_id, _) = finalize_complete_plan(&project, "Approval lifecycle", 30);
    let ready = load_plan(&project, &plan_id);
    let scope_before = serde_json::to_value(ready.scope()).expect("scope should serialize");
    let task_id = TaskId::parse("T1").expect("task ID should parse");
    let mut execution_probe = ready.clone();
    let approval_error = execution_probe
        .start_task(
            &task_id,
            Timestamp::parse("2026-07-25T16:00:00Z").expect("timestamp should parse"),
        )
        .expect_err("execution should require plan approval");
    assert_eq!(approval_error.kind(), DomainErrorKind::ApprovalRequired);

    let show_before = stored_bytes(&project, &plan_id);
    let shown = parse_success(&run_mino(&read_arguments(&project, "show", &plan_id)));
    assert_eq!(shown["status"], "Ready");
    assert_eq!(shown["scope"], scope_before);
    let validated = parse_success(&run_mino(&read_arguments(&project, "validate", &plan_id)));
    assert_eq!(validated["valid"], true);
    assert_eq!(validated["next_actions"], Value::Array(Vec::new()));
    let reviewed = parse_success(&run_mino(&read_arguments(&project, "review", &plan_id)));
    assert_eq!(reviewed["review_kind"], "mino.plan-review/v1");
    assert_eq!(reviewed["revision"], 3);
    assert_eq!(reviewed["approval_required"], true);
    assert_eq!(reviewed["missing"], serde_json::json!(["approval"]));
    assert_eq!(reviewed["reviewed_plan"]["tasks"][0]["id"], "T1");
    assert_eq!(reviewed["reviewed_plan"]["scope"], scope_before);
    assert!(
        reviewed["approval_notice"]
            .as_str()
            .is_some_and(|notice| notice.contains("not cryptographic"))
    );
    assert_eq!(stored_bytes(&project, &plan_id), show_before);

    let mut empty_reference = approve_arguments(&project, &plan_id, 3, 32, "disabled");
    let reference_index = empty_reference
        .iter()
        .position(|argument| argument == "--approval-ref")
        .expect("approval reference flag should exist");
    empty_reference[reference_index + 1] = String::new();
    let empty_reference_result = run_mino(&empty_reference);
    assert_eq!(empty_reference_result.status.code(), Some(4));
    assert_eq!(stored_bytes(&project, &plan_id), show_before);

    let approve = approve_arguments(&project, &plan_id, 3, 34, "disabled");
    let approved = parse_success(&run_mino(&approve));
    assert_eq!(approved["message"], "Plan approval recorded.");
    assert_eq!(approved["revision"], 4);
    assert_eq!(approved["status"], "Ready");
    assert_eq!(approved["complete"], true);
    let plan = load_plan(&project, &plan_id);
    assert_eq!(plan.approvals().len(), 1);
    assert!(plan.has_plan_approval());
    assert_eq!(
        plan.git_readiness().git_flow_consent(),
        GitFlowConsent::Disabled
    );
    assert_eq!(
        serde_json::to_value(plan.scope()).expect("scope should serialize"),
        scope_before
    );
    assert!(
        fs::read_to_string(projection_path(&project, &plan_id))
            .expect("projection should be readable")
            .contains("chat:explicit-approval")
    );
    let replay = parse_success(&run_mino(&approve));
    assert_eq!(replay["revision"], 4);
    assert_eq!(replay["replayed"], true);
    assert_eq!(
        PlanStore::new(project.path())
            .events(&typed_id(&plan_id))
            .unwrap()
            .len(),
        4
    );
    let reviewed_after = parse_success(&run_mino(&read_arguments(&project, "review", &plan_id)));
    assert_eq!(reviewed_after["revision"], 4);
    assert_eq!(reviewed_after["approval_required"], false);
    assert_eq!(reviewed_after["complete"], true);
    assert_eq!(reviewed_after["missing"], Value::Array(Vec::new()));

    let duplicate = run_mino(&approve_arguments(&project, &plan_id, 4, 35, "disabled"));
    assert_eq!(duplicate.status.code(), Some(5));
}

#[test]
fn direct_ready_authoring_is_rejected_without_staling_the_approval() {
    let project = TestProject::new("ready-edit");
    let (plan_id, _) = finalize_complete_plan(&project, "Protected Ready plan", 40);
    let approve = approve_arguments(&project, &plan_id, 3, 43, "disabled");
    parse_success(&run_mino(&approve));
    let before = stored_bytes(&project, &plan_id);
    let plan_before = load_plan(&project, &plan_id);
    let scope_before = serde_json::to_value(plan_before.scope()).expect("scope should serialize");
    let mut edit = mutation_arguments(&project, &["plan", "summary", "set"], &plan_id, 4, 44);
    edit.extend([
        "--value".to_owned(),
        "Attempt to bypass protected amendment policy".to_owned(),
    ]);
    let rejected = run_mino(&edit);
    assert_eq!(rejected.status.code(), Some(5));
    assert_eq!(stored_bytes(&project, &plan_id), before);
    let plan_after = load_plan(&project, &plan_id);
    assert!(plan_after.has_plan_approval());
    assert_eq!(plan_after.revision(), 4);
    assert_eq!(
        serde_json::to_value(plan_after.scope()).expect("scope should serialize"),
        scope_before
    );
}

#[test]
fn projection_drift_blocks_show_review_and_approval_without_state_changes() {
    let project = TestProject::new("ready-drift");
    let (plan_id, _) = finalize_complete_plan(&project, "Drifted Ready plan", 50);
    fs::write(
        projection_path(&project, &plan_id),
        "manually replaced review projection\n",
    )
    .expect("projection should be edited");
    let before = stored_bytes(&project, &plan_id);
    for arguments in [
        read_arguments(&project, "show", &plan_id),
        read_arguments(&project, "review", &plan_id),
        approve_arguments(&project, &plan_id, 3, 53, "disabled"),
    ] {
        let output = run_mino(&arguments);
        assert_eq!(output.status.code(), Some(8));
        assert_eq!(stored_bytes(&project, &plan_id), before);
    }
}

#[test]
fn readiness_drift_requires_explicit_refresh_and_invalidates_ready_approval() {
    let project = TestProject::new("git-readiness-refresh");
    let (plan_id, _) = finalize_complete_plan(&project, "Refresh Git readiness", 60);
    let approved = parse_success(&run_mino(&approve_arguments(
        &project, &plan_id, 3, 63, "approved",
    )));
    assert_eq!(approved["revision"], 4);

    fs::write(
        project.path().join("src/lib.rs"),
        "pub fn fixture() -> u8 { 2 }\n",
    )
    .expect("tracked source should become dirty");
    let drifted = run_mino(&read_arguments(&project, "review", &plan_id));
    assert_eq!(drifted.status.code(), Some(8));
    let drifted = parse_json(&drifted);
    assert_eq!(drifted["error"]["code"], "drift_detected");
    assert_eq!(drifted["next_actions"][0]["id"], "git.readiness.refresh");
    assert_eq!(load_plan(&project, &plan_id).revision(), 4);

    let refresh = refresh_arguments(&project, &plan_id, 4, 64);
    let refreshed = parse_success(&run_mino(&refresh));
    assert_eq!(refreshed["revision"], 5);
    assert_eq!(refreshed["status"], "Ready");
    let plan = load_plan(&project, &plan_id);
    assert!(!plan.has_plan_approval());
    assert_eq!(plan.git_readiness().working_tree(), "Dirty");
    assert!(!plan.git_readiness().git_flow_enabled());
    assert_eq!(
        plan.git_readiness().git_flow_consent(),
        GitFlowConsent::Pending
    );

    fs::write(
        project.path().join("src/lib.rs"),
        "pub fn fixture() -> u8 { 1 }\n",
    )
    .expect("tracked source should be restored");
    let replay = parse_success(&run_mino(&refresh));
    assert_eq!(replay["revision"], 5);
    assert_eq!(replay["replayed"], true);
    let context = parse_success(&run_mino(
        &[
            base_arguments(&project),
            vec!["agent".to_owned(), "context".to_owned()],
        ]
        .concat(),
    ));
    assert_eq!(context["next_actions"][0]["id"], "git.readiness.refresh");

    let refreshed_clean = parse_success(&run_mino(&refresh_arguments(&project, &plan_id, 5, 65)));
    assert_eq!(refreshed_clean["revision"], 6);
    let plan = load_plan(&project, &plan_id);
    assert_eq!(plan.git_readiness().working_tree(), "Clean");
    assert!(plan.git_readiness().git_flow_enabled());
    assert!(!plan.has_plan_approval());
    let reviewed = parse_success(&run_mino(&read_arguments(&project, "review", &plan_id)));
    assert_eq!(reviewed["approval_required"], true);
}

#[test]
fn head_branch_and_worktree_identity_drift_block_review_without_mutation() {
    let head_project = TestProject::new("git-readiness-head");
    let (head_plan_id, _) = finalize_complete_plan(&head_project, "HEAD readiness", 70);
    fs::write(head_project.path().join("head-drift.txt"), "head drift\n")
        .expect("HEAD drift fixture should be written");
    git(head_project.path(), &["add", "--", "head-drift.txt"]);
    git(
        head_project.path(),
        &[
            "-c",
            "user.name=Mino Tests",
            "-c",
            "user.email=mino-tests@example.invalid",
            "commit",
            "--quiet",
            "-m",
            "test: advance readiness head",
        ],
    );
    assert_readiness_drift(
        &run_mino(&read_arguments(&head_project, "review", &head_plan_id)),
        &["head"],
    );
    assert_eq!(load_plan(&head_project, &head_plan_id).revision(), 3);

    let branch_project = TestProject::new("git-readiness-branch");
    let (branch_plan_id, _) = finalize_complete_plan(&branch_project, "Branch readiness", 80);
    git(
        branch_project.path(),
        &["switch", "--quiet", "-c", "readiness-drift"],
    );
    assert_readiness_drift(
        &run_mino(&read_arguments(&branch_project, "review", &branch_plan_id)),
        &["branch"],
    );
    assert_eq!(load_plan(&branch_project, &branch_plan_id).revision(), 3);

    let mut worktree_project = TestProject::new("git-readiness-worktree");
    let (worktree_plan_id, _) = finalize_complete_plan(&worktree_project, "Worktree readiness", 90);
    let original = worktree_project.path.clone();
    let moved = original.with_file_name(format!(
        "{}-moved",
        original
            .file_name()
            .expect("project path should have a file name")
            .to_string_lossy()
    ));
    fs::rename(&original, &moved).expect("worktree fixture should move");
    worktree_project.path = moved
        .canonicalize()
        .expect("moved worktree root should resolve");
    assert_readiness_drift(
        &run_mino(&read_arguments(
            &worktree_project,
            "review",
            &worktree_plan_id,
        )),
        &["common_dir", "worktree"],
    );
    assert_eq!(
        load_plan(&worktree_project, &worktree_plan_id).revision(),
        3
    );
}
