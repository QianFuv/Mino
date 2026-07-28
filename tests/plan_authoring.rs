//! Contract tests for canonical Draft creation, granular authoring, YAML, and wizard input.

use std::fs;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use mino::domain::{DraftPlanInput, Plan, PlanId, RequestId, TaskId, Timestamp};
use mino::git::{ActiveBindingStore, GitAdapter};
use mino::input::{wizard, yaml};
use mino::project::{ProjectLayout, initialize};
use mino::store::{PlanStore, StoreErrorKind};
use serde_json::Value;

static NEXT_TEMP_DIRECTORY: AtomicU64 = AtomicU64::new(1);

struct TestProject {
    path: PathBuf,
}

impl TestProject {
    fn new(label: &str) -> Self {
        let sequence = NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "mino-plan-authoring-{label}-{}-{sequence}",
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
        Self {
            path: path.canonicalize().expect("project root should resolve"),
        }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn layout(&self) -> ProjectLayout {
        ProjectLayout::new(&self.path)
    }
}

impl Drop for TestProject {
    fn drop(&mut self) {
        let temporary_root = std::env::temp_dir();
        if self.path.starts_with(&temporary_root)
            && self
                .path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("mino-plan-authoring-"))
        {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

fn run_mino(arguments: &[String]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mino"))
        .args(arguments)
        .stdin(Stdio::null())
        .output()
        .expect("Mino binary should run")
}

fn retain_binding_after_git_removal(project: &TestProject, plan_id: &str, revision: u64) {
    let initialized = Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(project.path())
        .output()
        .expect("Git should initialize the binding fixture");
    assert!(initialized.status.success());
    let facts = GitAdapter::new(project.path())
        .inspect()
        .expect("Git facts should inspect");
    ActiveBindingStore::new(project.path())
        .bind(
            &facts,
            PlanId::parse(plan_id).expect("bound plan ID should parse"),
            revision,
            Timestamp::parse("2026-07-27T05:20:00Z").expect("binding timestamp should parse"),
        )
        .expect("active binding should be written");
    fs::remove_dir_all(project.path().join(".git"))
        .expect("Git repository should be removed from the fixture");
}

fn run_mino_with_input(arguments: &[String], input: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_mino"))
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Mino binary should start");
    child
        .stdin
        .take()
        .expect("child stdin should be piped")
        .write_all(input.as_bytes())
        .expect("test input should be written");
    child.wait_with_output().expect("Mino binary should finish")
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
    format!("00000000-0000-0000-0000-{number:012}")
}

fn create_arguments(
    project: &TestProject,
    name: &str,
    request_file: &Path,
    request_number: u64,
) -> Vec<String> {
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
    arguments
}

fn mutation_arguments(
    project: &TestProject,
    command: &[&str],
    plan_id: &str,
    revision: u64,
    request_number: u64,
) -> Vec<String> {
    let mut arguments = base_arguments(project);
    arguments.extend(command.iter().map(|part| (*part).to_owned()));
    arguments.extend([
        "--plan".to_owned(),
        plan_id.to_owned(),
        "--expect-revision".to_owned(),
        revision.to_string(),
        "--request-id".to_owned(),
        request_id(request_number),
        "--actor".to_owned(),
        "codex".to_owned(),
    ]);
    arguments
}

fn parse_success(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    serde_json::from_slice(&output.stdout).expect("stdout should be JSON")
}

fn plan_id_from(value: &Value) -> String {
    value["plan_id"]
        .as_str()
        .expect("result should contain a plan ID")
        .to_owned()
}

fn load_plan(project: &TestProject, plan_id: &str) -> Plan {
    PlanStore::new(project.path())
        .load_plan(&PlanId::parse(plan_id).expect("plan ID should parse"))
        .expect("plan should load")
}

fn projection_path(project: &TestProject, plan_id: &str) -> PathBuf {
    project
        .path()
        .join("docs")
        .join("plan")
        .join(format!("{plan_id}.md"))
}

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/drafts")
        .join(name)
}

fn run_plan_mutation(
    project: &TestProject,
    command: &[&str],
    plan_id: &str,
    revision: u64,
    request_number: u64,
    extra: &[&str],
) -> Value {
    let mut arguments = mutation_arguments(project, command, plan_id, revision, request_number);
    arguments.extend(extra.iter().map(|value| (*value).to_owned()));
    parse_success(&run_mino(&arguments))
}

fn create_complete_plan(
    project: &TestProject,
    name: &str,
    create_request: u64,
    apply_request: u64,
) -> String {
    let request_file = project.path().join(format!("{name}-request.md"));
    fs::write(&request_file, "Edit every authored Draft collection.\n")
        .expect("request should be written");
    let created = parse_success(&run_mino(&create_arguments(
        project,
        name,
        &request_file,
        create_request,
    )));
    let plan_id = plan_id_from(&created);
    let mut apply = mutation_arguments(project, &["plan", "apply"], &plan_id, 1, apply_request);
    apply.extend([
        "--file".to_owned(),
        fixture_path("complete.yaml").to_string_lossy().into_owned(),
    ]);
    assert_eq!(parse_success(&run_mino(&apply))["revision"], 2);
    plan_id
}

fn create_named_draft(
    label: &str,
    name: &str,
    request_number: u64,
) -> (TestProject, Vec<String>, Value) {
    let project = TestProject::new(label);
    let request_file = project.path().join("request.md");
    fs::write(&request_file, "Create a Unicode-named plan.\n").expect("request should be written");
    let arguments = create_arguments(&project, name, &request_file, request_number);
    let created = parse_success(&run_mino(&arguments));
    (project, arguments, created)
}

fn plan_slug(value: &Value) -> &str {
    value["plan_id"]
        .as_str()
        .expect("plan ID should be a string")
        .get(11..)
        .expect("plan ID should contain a date prefix")
}

#[test]
fn unicode_names_use_stable_hash_fallback_without_changing_ascii_slugs() {
    let chinese_name = "修复网络重试策略";
    let (chinese_project, chinese_arguments, chinese) =
        create_named_draft("unicode-chinese", chinese_name, 200);
    assert_eq!(plan_slug(&chinese), "plan-9be17b54");
    let chinese_id = plan_id_from(&chinese);
    assert_eq!(
        load_plan(&chinese_project, &chinese_id).metadata().name(),
        chinese_name
    );
    let replay = parse_success(&run_mino(&chinese_arguments));
    assert_eq!(replay["plan_id"], chinese_id);
    assert_eq!(replay["replayed"], true);
    let collision = run_mino(&create_arguments(
        &chinese_project,
        chinese_name,
        &chinese_project.path().join("request.md"),
        201,
    ));
    assert_eq!(collision.status.code(), Some(2));

    let (_, _, different) = create_named_draft("unicode-different", "改进网络重试策略", 202);
    assert_eq!(plan_slug(&different), "plan-cd683e41");
    assert_ne!(plan_slug(&different), plan_slug(&chinese));

    let (_, _, punctuation) = create_named_draft("unicode-punctuation", "！！！", 203);
    assert_eq!(plan_slug(&punctuation), "plan-bd03faef");

    let (_, _, mixed) = create_named_draft("unicode-mixed", "修复 Retry 策略", 204);
    assert_eq!(plan_slug(&mixed), "retry");
    let long_ascii_name = format!("{} trailing words", "A".repeat(120));
    let (_, _, long_ascii) = create_named_draft("unicode-long-ascii", &long_ascii_name, 205);
    assert_eq!(plan_slug(&long_ascii), "a".repeat(96));
}

#[test]
fn create_is_incomplete_collision_safe_and_request_idempotent() {
    let project = TestProject::new("create");
    let request_file = project.path().join("request.md");
    fs::write(&request_file, "Implement deterministic plan authoring.\n")
        .expect("request should be written");
    let arguments = create_arguments(&project, "Plan authoring", &request_file, 1);

    let first = parse_success(&run_mino(&arguments));
    assert_eq!(first["message"], "Plan draft initialized.");
    assert_eq!(first["status"], "Draft");
    assert_eq!(first["revision"], 1);
    assert_eq!(first["complete"], false);
    assert_eq!(first["replayed"], false);
    assert_eq!(first["missing"][0], "summary");
    assert_eq!(first["next_actions"][0]["id"], "plan.summary.set");
    let next_argv = first["next_actions"][0]["argv"]
        .as_array()
        .expect("next argv should be an array");
    assert!(next_argv.contains(&Value::from("--expect-revision")));
    assert!(next_argv.contains(&Value::from("--request-id")));
    assert!(next_argv.contains(&Value::from("--no-input")));
    let plan_id = plan_id_from(&first);
    assert!(projection_path(&project, &plan_id).is_file());
    let store = PlanStore::new(project.path());
    let typed_id = PlanId::parse(&plan_id).expect("plan ID should parse");
    assert_eq!(
        store.events(&typed_id).expect("events should load").len(),
        1
    );

    let replay = parse_success(&run_mino(&arguments));
    assert_eq!(replay["revision"], 1);
    assert_eq!(replay["replayed"], true);
    assert_eq!(
        store.events(&typed_id).expect("events should load").len(),
        1
    );

    let state_before = fs::read(store.paths().current_plan(&typed_id)).expect("state should exist");
    let projection_before =
        fs::read(projection_path(&project, &plan_id)).expect("projection should exist");
    fs::write(&request_file, "Different request bytes.\n").expect("request should change");
    let conflicting_retry = run_mino(&arguments);
    assert_eq!(conflicting_retry.status.code(), Some(3));
    assert_eq!(
        fs::read(store.paths().current_plan(&typed_id)).expect("state should remain"),
        state_before
    );
    assert_eq!(
        fs::read(projection_path(&project, &plan_id)).expect("projection should remain"),
        projection_before
    );

    let collision = run_mino(&create_arguments(
        &project,
        "Plan authoring",
        &request_file,
        2,
    ));
    assert_eq!(collision.status.code(), Some(2));
    let collision_json: Value =
        serde_json::from_slice(&collision.stdout).expect("collision should be JSON");
    assert_eq!(
        collision_json["next_actions"][0]["id"],
        "plan.create.choose-name"
    );
    assert_eq!(
        store.events(&typed_id).expect("events should load").len(),
        1
    );
}

#[test]
fn retained_git_binding_cannot_bypass_non_git_active_plan_creation_policy() {
    let project = TestProject::new("non-git-active");
    let first_request = project.path().join("first-request.md");
    fs::write(&first_request, "Create the first active plan.\n")
        .expect("first request should be written");
    let first = parse_success(&run_mino(&create_arguments(
        &project,
        "First active plan",
        &first_request,
        3,
    )));
    let first_plan_id = plan_id_from(&first);
    retain_binding_after_git_removal(&project, &first_plan_id, 1);
    let second_request = project.path().join("second-request.md");
    fs::write(&second_request, "Create a second active plan.\n")
        .expect("second request should be written");

    let second = run_mino(&create_arguments(
        &project,
        "Second active plan",
        &second_request,
        4,
    ));
    assert_eq!(second.status.code(), Some(5));
    let failure: Value = serde_json::from_slice(&second.stdout).expect("failure should be JSON");
    assert_eq!(failure["error"]["code"], "policy_violation");
    assert_eq!(failure["missing"], serde_json::json!(["active_plan"]));
    assert_eq!(
        fs::read_dir(project.layout().plans_directory())
            .expect("plans directory should be readable")
            .filter_map(Result::ok)
            .filter(|entry| entry.path().is_dir())
            .count(),
        1
    );
}

#[test]
fn strict_yaml_apply_is_atomic_replayable_and_rejects_execution_fields() {
    let project = TestProject::new("yaml");
    let request_file = project.path().join("request.md");
    fs::write(&request_file, "Apply a strict authored draft.\n").expect("request should exist");
    let created = parse_success(&run_mino(&create_arguments(
        &project,
        "Strict YAML",
        &request_file,
        10,
    )));
    let plan_id = plan_id_from(&created);
    let complete_fixture = fixture_path("complete.yaml");
    let mut apply = mutation_arguments(&project, &["plan", "apply"], &plan_id, 1, 11);
    apply.extend([
        "--file".to_owned(),
        complete_fixture.to_string_lossy().into_owned(),
    ]);

    let applied = parse_success(&run_mino(&apply));
    assert_eq!(applied["revision"], 2);
    assert_eq!(applied["replayed"], false);
    let plan = load_plan(&project, &plan_id);
    assert_eq!(
        plan.summary(),
        "Add deterministic draft authoring through one source-of-truth aggregate."
    );
    assert_eq!(plan.tasks().len(), 1);
    assert_eq!(plan.tasks()[0].id().as_str(), "T1");
    assert_eq!(plan.tasks()[0].steps().len(), 2);
    assert_eq!(plan.tasks()[0].file_map().len(), 1);
    assert_eq!(plan.approach().file_map().len(), 1);
    assert!(
        plan.global_verification()
            .iter()
            .any(|check| check.id().as_str() == "GLOBAL-SMOKE")
    );
    let store = PlanStore::new(project.path());
    let typed_id = PlanId::parse(&plan_id).expect("plan ID should parse");
    assert_eq!(
        store.events(&typed_id).expect("events should load").len(),
        2
    );

    let replay = parse_success(&run_mino(&apply));
    assert_eq!(replay["revision"], 2);
    assert_eq!(replay["replayed"], true);
    assert_eq!(
        store.events(&typed_id).expect("events should load").len(),
        2
    );

    let state_before = fs::read(store.paths().current_plan(&typed_id)).expect("state should exist");
    let projection_before =
        fs::read(projection_path(&project, &plan_id)).expect("projection should exist");
    let mut invalid = mutation_arguments(&project, &["plan", "apply"], &plan_id, 2, 12);
    invalid.extend([
        "--file".to_owned(),
        fixture_path("execution-field.yaml")
            .to_string_lossy()
            .into_owned(),
    ]);
    let rejected = run_mino(&invalid);
    assert_eq!(rejected.status.code(), Some(2));
    assert_eq!(
        fs::read(store.paths().current_plan(&typed_id)).expect("state should remain"),
        state_before
    );
    assert_eq!(
        fs::read(projection_path(&project, &plan_id)).expect("projection should remain"),
        projection_before
    );
    assert_eq!(
        store.events(&typed_id).expect("events should load").len(),
        2
    );
}

#[test]
fn granular_commands_require_revisions_and_return_assigned_identifiers() {
    let project = TestProject::new("granular");
    let request_file = project.path().join("request.md");
    fs::write(&request_file, "Author a plan through granular commands.\n")
        .expect("request should exist");
    let created = parse_success(&run_mino(&create_arguments(
        &project,
        "Granular commands",
        &request_file,
        20,
    )));
    let plan_id = plan_id_from(&created);
    author_summary_and_scope(&project, &plan_id);
    let step = author_task_and_criterion(&project, &plan_id);
    author_checks_and_file(&project, &plan_id);

    let plan = load_plan(&project, &plan_id);
    assert_eq!(plan.revision(), 9);
    assert_eq!(plan.summary(), "Granular summary");
    assert_eq!(plan.tasks()[0].steps(), ["Implement authoring"]);
    assert_eq!(
        plan.tasks()[0].acceptance_criteria()[0].id().as_str(),
        "T1-A1"
    );
    assert_eq!(plan.tasks()[0].file_map().len(), 1);
    assert_eq!(plan.approach().file_map().len(), 1);

    let stale = run_mino(&step);
    assert_eq!(stale.status.code(), Some(3));
    assert_eq!(load_plan(&project, &plan_id).revision(), 9);
}

#[test]
#[allow(clippy::too_many_lines)]
fn draft_collection_commands_update_remove_and_replay_atomically() {
    let project = TestProject::new("collection-edits");
    let plan_id = create_complete_plan(&project, "Collection edits", 100, 101);

    let decision_update =
        mutation_arguments(&project, &["plan", "decision", "update"], &plan_id, 2, 102);
    let mut decision_update = decision_update;
    decision_update.extend(
        [
            "--position",
            "1",
            "--item",
            "Replacement decision",
            "--type",
            "Assumption",
            "--decision",
            "Use stable selectors",
            "--reason",
            "Retries must target one item",
            "--status",
            "Accepted",
        ]
        .map(str::to_owned),
    );
    let updated = parse_success(&run_mino(&decision_update));
    assert_eq!(updated["revision"], 3);
    assert_eq!(
        load_plan(&project, &plan_id).decisions()[0].item(),
        "Replacement decision"
    );
    let replay = parse_success(&run_mino(&decision_update));
    assert_eq!(replay["revision"], 3);
    assert_eq!(replay["replayed"], true);

    assert_eq!(
        run_plan_mutation(
            &project,
            &["plan", "decision", "remove"],
            &plan_id,
            3,
            103,
            &["--position", "1"],
        )["revision"],
        4
    );
    assert_eq!(
        run_plan_mutation(
            &project,
            &["plan", "edge-case", "update"],
            &plan_id,
            4,
            104,
            &[
                "--position",
                "1",
                "--case",
                "A stale selector is retried",
                "--expected-behavior",
                "Reject without changing the revision",
                "--covered-by",
                "T1-A1",
            ],
        )["revision"],
        5
    );
    assert_eq!(
        load_plan(&project, &plan_id).edge_cases()[0].case(),
        "A stale selector is retried"
    );
    assert_eq!(
        run_plan_mutation(
            &project,
            &["plan", "edge-case", "remove"],
            &plan_id,
            5,
            105,
            &["--position", "1"],
        )["revision"],
        6
    );
    assert_eq!(
        run_plan_mutation(
            &project,
            &["plan", "task", "update"],
            &plan_id,
            6,
            106,
            &[
                "--task",
                "T1",
                "--title",
                "Implement editable authoring",
                "--clear-commit-gate",
            ],
        )["revision"],
        7
    );
    assert_eq!(
        run_plan_mutation(
            &project,
            &["plan", "task", "step", "update"],
            &plan_id,
            7,
            107,
            &[
                "--task",
                "T1",
                "--position",
                "1",
                "--value",
                "Replace inputs"
            ],
        )["revision"],
        8
    );

    let mut invalid_position = mutation_arguments(
        &project,
        &["plan", "task", "step", "update"],
        &plan_id,
        8,
        108,
    );
    invalid_position
        .extend(["--task", "T1", "--position", "0", "--value", "Invalid"].map(str::to_owned));
    assert_eq!(run_mino(&invalid_position).status.code(), Some(2));
    assert_eq!(load_plan(&project, &plan_id).revision(), 8);
    assert_eq!(
        run_plan_mutation(
            &project,
            &["plan", "task", "step", "remove"],
            &plan_id,
            8,
            109,
            &["--task", "T1", "--position", "1"],
        )["revision"],
        9
    );
    assert_eq!(
        run_plan_mutation(
            &project,
            &["plan", "task", "criterion", "update"],
            &plan_id,
            9,
            110,
            &[
                "--task",
                "T1",
                "--criterion",
                "T1-A1",
                "--description",
                "Edited content persists",
            ],
        )["revision"],
        10
    );
    assert_eq!(
        run_plan_mutation(
            &project,
            &["plan", "task", "criterion", "remove"],
            &plan_id,
            10,
            111,
            &["--task", "T1", "--criterion", "T1-A1"],
        )["revision"],
        11
    );
    assert_eq!(
        run_plan_mutation(
            &project,
            &["plan", "task", "verification", "update"],
            &plan_id,
            11,
            112,
            &[
                "--task",
                "T1",
                "--check",
                "TASK-TEST",
                "--command",
                "cargo",
                "--command",
                "check",
                "--required",
                "true",
            ],
        )["revision"],
        12
    );
    assert_eq!(
        run_plan_mutation(
            &project,
            &["plan", "task", "verification", "remove"],
            &plan_id,
            12,
            113,
            &["--task", "T1", "--check", "TASK-TEST"],
        )["revision"],
        13
    );
    assert_eq!(
        run_plan_mutation(
            &project,
            &["plan", "file", "update"],
            &plan_id,
            13,
            114,
            &[
                "--task",
                "T1",
                "--position",
                "1",
                "--path",
                "src/domain/plan.rs",
                "--change",
                "modify",
                "--reason",
                "Own collection edits",
            ],
        )["revision"],
        14
    );
    let file_updated = load_plan(&project, &plan_id);
    assert_eq!(
        file_updated.tasks()[0].file_map()[0].path(),
        "src/domain/plan.rs"
    );
    assert_eq!(
        file_updated.approach().file_map()[0].path(),
        "src/domain/plan.rs"
    );
    assert_eq!(
        run_plan_mutation(
            &project,
            &["plan", "file", "remove"],
            &plan_id,
            14,
            115,
            &["--task", "T1", "--position", "1"],
        )["revision"],
        15
    );
    assert_eq!(
        run_plan_mutation(
            &project,
            &["plan", "verification", "update"],
            &plan_id,
            15,
            116,
            &[
                "--check",
                "GLOBAL-SMOKE",
                "--command",
                "cargo",
                "--command",
                "check",
                "--required",
                "true",
            ],
        )["revision"],
        16
    );
    assert_eq!(
        run_plan_mutation(
            &project,
            &["plan", "verification", "remove"],
            &plan_id,
            16,
            117,
            &["--check", "GLOBAL-SMOKE"],
        )["revision"],
        17
    );

    let plan = load_plan(&project, &plan_id);
    let task = &plan.tasks()[0];
    assert!(plan.decisions().is_empty());
    assert!(plan.edge_cases().is_empty());
    assert_eq!(task.title(), "Implement editable authoring");
    assert_eq!(task.steps(), ["Persist one semantic revision"]);
    assert!(task.acceptance_criteria().is_empty());
    assert!(task.verification_checks().is_empty());
    assert!(task.file_map().is_empty());
    assert!(task.commit_gate().is_none());
    assert!(plan.approach().file_map().is_empty());
    assert!(
        plan.global_verification()
            .iter()
            .all(|check| check.id().as_str() != "GLOBAL-SMOKE")
    );
}

#[test]
fn task_move_and_remove_reject_broken_dependencies_without_state_changes() {
    let project = TestProject::new("task-order-edits");
    let plan_id = create_complete_plan(&project, "Task order edits", 120, 121);
    let second = run_plan_mutation(
        &project,
        &["plan", "task", "add"],
        &plan_id,
        2,
        122,
        &["--title", "Second task"],
    );
    assert_eq!(second["assigned_id"], "T2");
    let third = run_plan_mutation(
        &project,
        &["plan", "task", "add"],
        &plan_id,
        3,
        123,
        &["--title", "Dependent task", "--depends-on", "T2"],
    );
    assert_eq!(third["assigned_id"], "T3");

    let mut invalid_move =
        mutation_arguments(&project, &["plan", "task", "move"], &plan_id, 4, 124);
    invalid_move.extend(["--task", "T3", "--position", "2"].map(str::to_owned));
    assert_eq!(run_mino(&invalid_move).status.code(), Some(2));
    assert_eq!(
        load_plan(&project, &plan_id)
            .task_order()
            .iter()
            .map(TaskId::as_str)
            .collect::<Vec<_>>(),
        ["T1", "T2", "T3"]
    );

    let mut invalid_remove =
        mutation_arguments(&project, &["plan", "task", "remove"], &plan_id, 4, 125);
    invalid_remove.extend(["--task", "T2"].map(str::to_owned));
    assert_eq!(run_mino(&invalid_remove).status.code(), Some(2));
    assert_eq!(load_plan(&project, &plan_id).revision(), 4);

    assert_eq!(
        run_plan_mutation(
            &project,
            &["plan", "task", "update"],
            &plan_id,
            4,
            126,
            &["--task", "T3", "--clear-dependencies"],
        )["revision"],
        5
    );
    assert_eq!(
        run_plan_mutation(
            &project,
            &["plan", "task", "move"],
            &plan_id,
            5,
            127,
            &["--task", "T3", "--position", "1"],
        )["revision"],
        6
    );
    assert_eq!(
        run_plan_mutation(
            &project,
            &["plan", "task", "remove"],
            &plan_id,
            6,
            128,
            &["--task", "T2"],
        )["revision"],
        7
    );
    let plan = load_plan(&project, &plan_id);
    assert_eq!(
        plan.task_order()
            .iter()
            .map(TaskId::as_str)
            .collect::<Vec<_>>(),
        ["T3", "T1"]
    );
    assert!(
        plan.task(&TaskId::parse("T2").expect("task ID should parse"))
            .is_none()
    );
}

fn author_summary_and_scope(project: &TestProject, plan_id: &str) {
    let mut summary = mutation_arguments(project, &["plan", "summary", "set"], plan_id, 1, 21);
    summary.push("--stdin".to_owned());
    let summary_result = parse_success(&run_mino_with_input(&summary, "Granular summary\n"));
    assert_eq!(summary_result["revision"], 2);
    let replay = parse_success(&run_mino_with_input(&summary, "Granular summary\n"));
    assert_eq!(replay["revision"], 2);
    assert_eq!(replay["replayed"], true);
    let conflicting = run_mino_with_input(&summary, "Different summary\n");
    assert_eq!(conflicting.status.code(), Some(3));

    let mut scope = mutation_arguments(project, &["plan", "scope", "set"], plan_id, 2, 22);
    scope.extend([
        "--goal".to_owned(),
        "Author the complete Draft".to_owned(),
        "--deliverable".to_owned(),
        "CLI commands".to_owned(),
        "--in-scope".to_owned(),
        "Draft fields".to_owned(),
        "--out-of-scope".to_owned(),
        "Execution".to_owned(),
    ]);
    assert_eq!(parse_success(&run_mino(&scope))["revision"], 3);
}

fn author_task_and_criterion(project: &TestProject, plan_id: &str) -> Vec<String> {
    let mut task = mutation_arguments(project, &["plan", "task", "add"], plan_id, 3, 23);
    task.extend([
        "--title".to_owned(),
        "Implement the Draft".to_owned(),
        "--commit-required".to_owned(),
        "--planned-commit-message".to_owned(),
        "feat(plan): implement draft".to_owned(),
        "--commit-scope".to_owned(),
        "src/application/plan.rs".to_owned(),
    ]);
    let task_result = parse_success(&run_mino(&task));
    assert_eq!(task_result["revision"], 4);
    assert_eq!(task_result["assigned_id"], "T1");

    let mut step = mutation_arguments(project, &["plan", "task", "step", "add"], plan_id, 4, 24);
    step.extend([
        "--task".to_owned(),
        "T1".to_owned(),
        "--value".to_owned(),
        "Implement authoring".to_owned(),
    ]);
    assert_eq!(parse_success(&run_mino(&step))["revision"], 5);

    let mut criterion = mutation_arguments(
        project,
        &["plan", "task", "criterion", "add"],
        plan_id,
        5,
        25,
    );
    criterion.extend([
        "--task".to_owned(),
        "T1".to_owned(),
        "--description".to_owned(),
        "Draft is persisted".to_owned(),
    ]);
    let criterion_result = parse_success(&run_mino(&criterion));
    assert_eq!(criterion_result["revision"], 6);
    assert_eq!(criterion_result["assigned_id"], "T1-A1");
    step
}

fn author_checks_and_file(project: &TestProject, plan_id: &str) {
    let mut verification = mutation_arguments(
        project,
        &["plan", "task", "verification", "add"],
        plan_id,
        6,
        26,
    );
    verification.extend([
        "--task".to_owned(),
        "T1".to_owned(),
        "--id".to_owned(),
        "TASK-SMOKE".to_owned(),
        "--command".to_owned(),
        "cargo".to_owned(),
        "--command".to_owned(),
        "test".to_owned(),
    ]);
    assert_eq!(parse_success(&run_mino(&verification))["revision"], 7);

    let mut file = mutation_arguments(project, &["plan", "file", "add"], plan_id, 7, 27);
    file.extend([
        "--task".to_owned(),
        "T1".to_owned(),
        "--path".to_owned(),
        "src/application/plan.rs".to_owned(),
        "--change".to_owned(),
        "modify".to_owned(),
        "--reason".to_owned(),
        "Own authoring orchestration".to_owned(),
    ]);
    assert_eq!(parse_success(&run_mino(&file))["revision"], 8);

    let mut global = mutation_arguments(project, &["plan", "verification", "add"], plan_id, 8, 28);
    global.extend([
        "--id".to_owned(),
        "GLOBAL-CUSTOM".to_owned(),
        "--command".to_owned(),
        "cargo".to_owned(),
        "--command".to_owned(),
        "test".to_owned(),
    ]);
    assert_eq!(parse_success(&run_mino(&global))["revision"], 9);
}

#[test]
fn batch_and_granular_paths_produce_identical_authored_bytes_and_wizard_is_confirmed() {
    let source = fs::read_to_string(fixture_path("complete.yaml"))
        .expect("complete fixture should be readable");
    let input = yaml::parse_draft(&source).expect("complete fixture should parse");
    let created_at =
        Timestamp::parse("2026-07-25T15:00:00Z").expect("creation timestamp should parse");
    let updated_at =
        Timestamp::parse("2026-07-25T15:01:00Z").expect("update timestamp should parse");
    let plan_id = PlanId::parse("2026-07-25-equivalent-authoring").expect("plan ID should parse");
    let mut batch = Plan::new(plan_id.clone(), "Equivalent request", created_at.clone());
    batch
        .apply_draft_input(input.clone(), updated_at.clone())
        .expect("batch input should apply");
    let mut granular = Plan::new(plan_id, "Equivalent request", created_at);
    apply_granular(&mut granular, input, &updated_at);
    let mut batch_value = serde_json::to_value(batch).expect("batch plan should serialize");
    let mut granular_value =
        serde_json::to_value(granular).expect("granular plan should serialize");
    batch_value["revision"] = Value::from(0);
    granular_value["revision"] = Value::from(0);
    assert_eq!(batch_value, granular_value);

    let wizard_input = b"Equivalent authoring\n\nEquivalent request\n.\nyes\n";
    let mut reader = Cursor::new(wizard_input);
    let mut output = Vec::new();
    let collected = wizard::collect(&mut reader, &mut output)
        .expect("wizard should succeed")
        .expect("wizard should be confirmed");
    assert_eq!(collected.name, "Equivalent authoring");
    assert_eq!(collected.trigger, "durable");
    assert_eq!(collected.original_request, "Equivalent request");
    assert!(
        String::from_utf8(output)
            .expect("preview should be UTF-8")
            .contains("Preview")
    );

    let mut cancelled_reader = Cursor::new(b"Cancelled\ndurable\nDo nothing\n.\nn\n");
    let mut cancelled_output = Vec::new();
    assert_eq!(
        wizard::collect(&mut cancelled_reader, &mut cancelled_output)
            .expect("cancelled wizard should succeed"),
        None
    );
    assert!(yaml::parse_draft("summary: valid\nstatus: Ready\n").is_err());
    assert!(yaml::parse_draft("summary: one\n---\nsummary: two\n").is_err());
}

#[test]
fn interactive_cli_never_prompts_in_agent_mode_or_without_a_terminal() {
    let project = TestProject::new("interactive");
    let mut no_input = base_arguments(&project);
    no_input.extend([
        "plan".to_owned(),
        "create".to_owned(),
        "--interactive".to_owned(),
        "--request-id".to_owned(),
        request_id(40),
    ]);
    let rejected = run_mino(&no_input);
    assert_eq!(rejected.status.code(), Some(5));

    let without_agent_flag = vec![
        "--root".to_owned(),
        project.path().to_string_lossy().into_owned(),
        "--format".to_owned(),
        "json".to_owned(),
        "plan".to_owned(),
        "create".to_owned(),
        "--interactive".to_owned(),
        "--request-id".to_owned(),
        request_id(41),
    ];
    let non_terminal = run_mino(&without_agent_flag);
    assert_eq!(non_terminal.status.code(), Some(7));
    let plans = fs::read_dir(project.layout().plans_directory())
        .expect("plans directory should be readable")
        .collect::<Result<Vec<_>, _>>()
        .expect("plan entries should be readable");
    assert!(plans.is_empty());
}

fn apply_granular(plan: &mut Plan, input: DraftPlanInput, updated_at: &Timestamp) {
    let DraftPlanInput {
        metadata,
        summary,
        context,
        scope,
        decisions,
        approach,
        interfaces,
        edge_cases,
        tasks,
        verification_plan,
    } = input;
    if let Some(metadata) = metadata {
        plan.author_metadata(metadata, updated_at.clone())
            .expect("metadata should apply");
    }
    if let Some(summary) = summary {
        plan.author_summary(summary, updated_at.clone())
            .expect("summary should apply");
    }
    for context in context {
        plan.author_context(context, updated_at.clone())
            .expect("context should apply");
    }
    if let Some(scope) = scope {
        plan.author_scope(scope, updated_at.clone())
            .expect("scope should apply");
    }
    for decision in decisions {
        plan.author_decision(decision, updated_at.clone())
            .expect("decision should apply");
    }
    if let Some(approach) = approach {
        plan.author_approach(approach, updated_at.clone())
            .expect("approach should apply");
    }
    if let Some(interfaces) = interfaces {
        plan.author_interfaces(interfaces, updated_at.clone())
            .expect("interfaces should apply");
    }
    for edge_case in edge_cases {
        plan.author_edge_case(edge_case, updated_at.clone())
            .expect("edge case should apply");
    }
    for task in tasks {
        plan.author_task(task, updated_at.clone())
            .expect("task should apply");
    }
    for verification in verification_plan {
        plan.author_global_verification(verification, updated_at.clone())
            .expect("verification should apply");
    }
}

#[test]
fn request_identifiers_used_by_fixtures_are_valid() {
    for number in [1_u64, 11, 21, 41] {
        RequestId::parse(request_id(number)).expect("fixture request ID should parse");
    }
}

#[test]
fn storage_rejects_same_create_request_for_different_initial_bytes() {
    let project = TestProject::new("store-replay");
    let store = PlanStore::new(project.path());
    let plan_id = PlanId::parse("2026-07-25-store-create-replay").expect("plan ID should parse");
    let timestamp = Timestamp::parse("2026-07-25T16:00:00Z").expect("timestamp should parse");
    let request_id = RequestId::parse(request_id(50)).expect("request ID should parse");
    let command = vec!["mino".to_owned(), "plan".to_owned(), "create".to_owned()];
    store
        .create_plan(
            &Plan::new(plan_id.clone(), "First request", timestamp.clone()),
            request_id.clone(),
            "codex",
            command.clone(),
        )
        .expect("initial plan should persist");
    let conflict = store
        .create_plan(
            &Plan::new(plan_id, "Different request", timestamp),
            request_id,
            "codex",
            command,
        )
        .expect_err("different initial bytes must not replay");
    assert_eq!(conflict.kind(), StoreErrorKind::RequestConflict);
}

#[test]
fn truncated_scan_requires_digest_bound_acceptance_before_authoring_can_advance() {
    let project = TestProject::new("truncated-scan");
    let huge_source = project.path().join("huge.js");
    fs::File::create(&huge_source)
        .expect("large source should be created")
        .set_len(4 * 1024 * 1024 + 1)
        .expect("large source should exceed the per-file scan budget");
    let request_file = project.path().join("truncated-request.md");
    fs::write(
        &request_file,
        "Plan from an explicitly accepted partial scan.\n",
    )
    .expect("request fixture should be written");
    let created = parse_success(&run_mino(&create_arguments(
        &project,
        "Truncated scan acceptance",
        &request_file,
        60,
    )));
    let plan_id = plan_id_from(&created);
    assert_eq!(created["missing"][0], "scan.acceptance");
    assert_eq!(created["next_actions"], serde_json::json!([]));
    let initial = load_plan(&project, &plan_id);
    let summary = initial
        .project_scan_summary()
        .expect("scan summary should decode")
        .expect("scan summary should be persisted");
    assert!(summary.is_incomplete());
    assert_eq!(summary.truncation_reasons(), ["per_file_byte_limit"]);
    assert!(summary.digest().starts_with("sha256:"));
    assert!(
        fs::read_to_string(projection_path(&project, &plan_id))
            .expect("projection should be readable")
            .contains("## Project Scan")
    );

    let mut setup = mutation_arguments(&project, &["git", "setup", "decide"], &plan_id, 1, 62);
    setup.extend([
        "--decision".to_owned(),
        "continue-without-git".to_owned(),
        "--approval-ref".to_owned(),
        "chat:continue-without-git".to_owned(),
    ]);
    assert_eq!(parse_success(&run_mino(&setup))["revision"], 2);

    let mut context_arguments = base_arguments(&project);
    context_arguments.extend(["agent".to_owned(), "context".to_owned()]);
    let context = parse_success(&run_mino(&context_arguments));
    assert_eq!(context["scan_incomplete"], true);
    assert_eq!(context["approval_required"], true);
    assert!(
        context["allowed_actions"]
            .as_array()
            .is_some_and(|actions| actions.contains(&Value::from("plan.scan.accept")))
    );

    let mut validate_arguments = base_arguments(&project);
    validate_arguments.extend([
        "plan".to_owned(),
        "validate".to_owned(),
        "--plan".to_owned(),
        plan_id.clone(),
    ]);
    let validation_output = run_mino(&validate_arguments);
    assert_eq!(validation_output.status.code(), Some(2));
    let validation: Value = serde_json::from_slice(&validation_output.stdout)
        .expect("validation failure should be JSON");
    assert!(validation["findings"].as_array().is_some_and(|findings| {
        findings
            .iter()
            .any(|finding| finding["id"] == "POLICY-SCAN-INCOMPLETE")
    }));

    let mut acceptance = mutation_arguments(&project, &["plan", "scan", "accept"], &plan_id, 2, 61);
    acceptance.extend([
        "--decision-ref".to_owned(),
        "chat:accept-partial-scan".to_owned(),
        "--reason".to_owned(),
        "The bounded scan is sufficient for this plan".to_owned(),
    ]);
    let accepted = parse_success(&run_mino(&acceptance));
    assert_eq!(accepted["revision"], 3);
    assert_eq!(accepted["missing"][0], "summary");
    assert_eq!(accepted["next_actions"][0]["id"], "plan.summary.set");
    let replayed = parse_success(&run_mino(&acceptance));
    assert_eq!(replayed["revision"], 3);
    assert_eq!(replayed["replayed"], true);

    let accepted_plan = load_plan(&project, &plan_id);
    let accepted_summary = accepted_plan
        .project_scan_summary()
        .expect("accepted scan summary should decode")
        .expect("accepted scan summary should remain persisted");
    let audit = accepted_summary
        .acceptance()
        .expect("scan acceptance should be recorded");
    assert_eq!(audit.scan_digest(), accepted_summary.digest());
    assert_eq!(audit.actor(), "codex");
    assert_eq!(audit.reference(), "chat:accept-partial-scan");
}
