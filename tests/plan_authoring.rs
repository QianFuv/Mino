//! Contract tests for canonical Draft creation, granular authoring, YAML, and wizard input.

use std::fs;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use mino::domain::{DraftPlanInput, Plan, PlanId, RequestId, Timestamp};
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
