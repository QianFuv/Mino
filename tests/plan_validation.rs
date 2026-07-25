//! Contract tests for fixed-order plan validation and canonical remediation.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use mino::domain::{GitReadiness, Plan, PlanDraftSeed, PlanId, TaskId, Timestamp};
use mino::input::yaml;
use mino::project::initialize;
use mino::store::PlanStore;
use mino::validation::validate_plan;
use serde_json::Value;

static NEXT_TEMP_DIRECTORY: AtomicU64 = AtomicU64::new(1);

struct TestProject {
    path: PathBuf,
}

impl TestProject {
    fn new(label: &str) -> Self {
        let sequence = NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "mino-plan-validation-{label}-{}-{sequence}",
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
}

impl Drop for TestProject {
    fn drop(&mut self) {
        let temporary_root = std::env::temp_dir();
        if self.path.starts_with(&temporary_root)
            && self
                .path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("mino-plan-validation-"))
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
    format!("10000000-0000-0000-0000-{number:012}")
}

fn parse_json(output: &Output) -> Value {
    assert!(
        output.stderr.is_empty(),
        "stderr must be empty in JSON mode"
    );
    serde_json::from_slice(&output.stdout).expect("stdout should be JSON")
}

fn create_plan(project: &TestProject, name: &str, request_number: u64) -> String {
    let request_file = project.path().join(format!("request-{request_number}.md"));
    fs::write(&request_file, "Validate a complete authored plan.\n")
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
    let output = run_mino(&arguments);
    assert!(
        output.status.success(),
        "create stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    parse_json(&output)["plan_id"]
        .as_str()
        .expect("create should return a plan ID")
        .to_owned()
}

fn apply_fixture(project: &TestProject, plan_id: &str, fixture: &str, request_number: u64) {
    let mut arguments = base_arguments(project);
    arguments.extend([
        "plan".to_owned(),
        "apply".to_owned(),
        "--plan".to_owned(),
        plan_id.to_owned(),
        "--expect-revision".to_owned(),
        "1".to_owned(),
        "--request-id".to_owned(),
        request_id(request_number),
        "--actor".to_owned(),
        "codex".to_owned(),
        "--file".to_owned(),
        fixture_path(fixture).to_string_lossy().into_owned(),
    ]);
    let output = run_mino(&arguments);
    assert!(
        output.status.success(),
        "apply stderr: {}\nstdout: {}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
}

fn validate(project: &TestProject, plan_id: &str) -> Output {
    let mut arguments = base_arguments(project);
    arguments.extend([
        "plan".to_owned(),
        "validate".to_owned(),
        "--plan".to_owned(),
        plan_id.to_owned(),
    ]);
    run_mino(&arguments)
}

fn finding_ids(value: &Value) -> Vec<&str> {
    value["findings"]
        .as_array()
        .expect("findings should be an array")
        .iter()
        .map(|finding| finding["id"].as_str().expect("finding ID should be text"))
        .collect()
}

fn assert_fixed_layer_order(value: &Value) {
    let mut previous = 0_u8;
    for finding in value["findings"]
        .as_array()
        .expect("findings should be an array")
    {
        let current = match finding["layer"]
            .as_str()
            .expect("finding layer should be text")
        {
            "schema" => 0,
            "semantic" => 1,
            "graph" => 2,
            "policy" => 3,
            layer => panic!("unexpected validation layer {layer}"),
        };
        assert!(current >= previous, "validation layers must be fixed-order");
        previous = current;
    }
}

#[test]
fn valid_plan_has_zero_findings_stable_actions_and_no_writes() {
    let project = TestProject::new("valid");
    let plan_id = create_plan(&project, "Valid plan", 1);
    apply_fixture(&project, &plan_id, "complete.yaml", 2);
    let typed_id = PlanId::parse(&plan_id).expect("plan ID should parse");
    let store = PlanStore::new(project.path());
    let state_path = store.paths().current_plan(&typed_id);
    let events_path = store.paths().event_log(&typed_id);
    let projection_path = project
        .path()
        .join("docs")
        .join("plan")
        .join(format!("{typed_id}.md"));
    let before = [
        fs::read(&state_path).expect("state should exist"),
        fs::read(&events_path).expect("events should exist"),
        fs::read(&projection_path).expect("projection should exist"),
    ];

    let first_output = validate(&project, &plan_id);
    assert!(
        first_output.status.success(),
        "validation stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&first_output.stdout),
        String::from_utf8_lossy(&first_output.stderr)
    );
    let first = parse_json(&first_output);
    let second = parse_json(&validate(&project, &plan_id));
    assert_eq!(first, second);
    assert_eq!(first["validation_kind"], "mino.validation/v1");
    assert_eq!(first["valid"], true);
    assert_eq!(first["complete"], false);
    assert_eq!(first["findings"], Value::Array(Vec::new()));
    assert_eq!(first["missing"], Value::Array(Vec::new()));
    assert_eq!(first["next_actions"][0]["id"], "plan.finalize");
    let argv = first["next_actions"][0]["argv"]
        .as_array()
        .expect("finalize argv should be an array");
    assert!(argv.contains(&Value::from("--expect-revision")));
    assert!(argv.contains(&Value::from("--request-id")));
    assert!(argv.contains(&Value::from("--no-input")));
    let after = [
        fs::read(state_path).expect("state should remain"),
        fs::read(events_path).expect("events should remain"),
        fs::read(projection_path).expect("projection should remain"),
    ];
    assert_eq!(before, after);
}

#[test]
fn incomplete_and_schema_invalid_drafts_return_structured_exit_two_findings() {
    let incomplete_project = TestProject::new("incomplete");
    let incomplete_id = create_plan(&incomplete_project, "Incomplete plan", 10);
    let first_output = validate(&incomplete_project, &incomplete_id);
    assert_eq!(first_output.status.code(), Some(2));
    let first = parse_json(&first_output);
    let second = parse_json(&validate(&incomplete_project, &incomplete_id));
    assert_eq!(first, second);
    assert_eq!(first["error"]["code"], "incomplete_or_validation");
    assert_eq!(first["validation_kind"], "mino.validation/v1");
    assert_eq!(first["valid"], false);
    assert_fixed_layer_order(&first);
    let ids = finding_ids(&first);
    assert!(ids.contains(&"SCHEMA-PLACEHOLDER-UNRESOLVED"));
    assert!(ids.contains(&"SEMANTIC-SUMMARY-MISSING"));
    assert_eq!(first["next_actions"][0]["id"], "plan.apply");

    let schema_project = TestProject::new("schema");
    let schema_id = create_plan(&schema_project, "Schema defects", 20);
    apply_fixture(&schema_project, &schema_id, "schema-invalid.yaml", 21);
    let schema_output = validate(&schema_project, &schema_id);
    assert_eq!(schema_output.status.code(), Some(2));
    let schema = parse_json(&schema_output);
    let schema_ids = finding_ids(&schema);
    assert!(schema_ids.contains(&"SCHEMA-CHECK-ID-DUPLICATE"));
    assert!(schema_ids.contains(&"SCHEMA-PATH-INVALID"));
    assert!(schema_ids.contains(&"SCHEMA-PLACEHOLDER-UNRESOLVED"));
    assert!(schema_ids.contains(&"SCHEMA-REFERENCE-UNKNOWN"));
    assert!(
        yaml::parse_draft(
            &fs::read_to_string(fixture_path("schema-type-invalid.yaml"))
                .expect("invalid type fixture should be readable")
        )
        .is_err()
    );
}

#[test]
fn semantic_validation_accumulates_every_required_completeness_rule() {
    let project = TestProject::new("semantic");
    let source = fs::read_to_string(fixture_path("semantic-invalid.yaml"))
        .expect("semantic fixture should be readable");
    let input = yaml::parse_draft(&source).expect("semantic fixture should parse");
    let mut plan = Plan::new(
        PlanId::parse("2026-07-25-semantic-defects").expect("plan ID should parse"),
        "Validate semantic completeness",
        Timestamp::parse("2026-07-25T15:00:00Z").expect("timestamp should parse"),
    );
    plan.apply_draft_input(
        input,
        Timestamp::parse("2026-07-25T15:01:00Z").expect("timestamp should parse"),
    )
    .expect("semantic fixture should apply");
    let report = validate_plan(project.path(), &plan).expect("validation should return a report");
    let ids = report
        .findings
        .iter()
        .map(|finding| finding.id.as_str())
        .collect::<Vec<_>>();
    for expected in [
        "SEMANTIC-GOAL-MISSING",
        "SEMANTIC-DELIVERABLES-MISSING",
        "SEMANTIC-IN-SCOPE-MISSING",
        "SEMANTIC-OUT-OF-SCOPE-MISSING",
        "SEMANTIC-BLOCKING-QUESTION-OPEN",
        "SEMANTIC-TASK-CRITERIA-MISSING",
        "SEMANTIC-TASK-VERIFICATION-MISSING",
        "SEMANTIC-GLOBAL-VERIFICATION-MISSING",
        "GRAPH-TASK-BOUNDARY-MISSING",
    ] {
        assert!(ids.contains(&expected), "missing finding {expected}");
    }
}

#[test]
fn graph_validation_reports_missing_ordering_and_cycle_defects() {
    let cycle_project = TestProject::new("cycle");
    let cycle_id = create_plan(&cycle_project, "Cycle defects", 30);
    apply_fixture(&cycle_project, &cycle_id, "graph-invalid.yaml", 31);
    let cycle_output = validate(&cycle_project, &cycle_id);
    assert_eq!(cycle_output.status.code(), Some(2));
    let cycle = parse_json(&cycle_output);
    let cycle_ids = finding_ids(&cycle);
    assert!(cycle_ids.contains(&"GRAPH-DEPENDENCY-ORDER"));
    assert!(cycle_ids.contains(&"GRAPH-DEPENDENCY-CYCLE"));
    assert_fixed_layer_order(&cycle);
    let mut cycle_plan = PlanStore::new(cycle_project.path())
        .load_plan(&PlanId::parse(&cycle_id).expect("plan ID should parse"))
        .expect("cycle plan should load");
    let first_id = TaskId::parse("T1").expect("task ID should parse");
    let second_id = TaskId::parse("T2").expect("task ID should parse");
    cycle_plan
        .mark_task_ready(
            &first_id,
            Timestamp::parse("2026-07-25T15:02:00Z").expect("timestamp should parse"),
        )
        .expect("task definition should become Ready");
    cycle_plan
        .mark_task_ready(
            &second_id,
            Timestamp::parse("2026-07-25T15:03:00Z").expect("timestamp should parse"),
        )
        .expect("task definition should become Ready");
    assert!(
        cycle_plan
            .finalize(Timestamp::parse("2026-07-25T15:04:00Z").expect("timestamp should parse"))
            .is_err(),
        "domain finalization must not bypass graph validation"
    );

    let missing_project = TestProject::new("missing");
    let missing_id = create_plan(&missing_project, "Missing dependency", 40);
    apply_fixture(&missing_project, &missing_id, "missing-dependency.yaml", 41);
    let missing_output = validate(&missing_project, &missing_id);
    assert_eq!(missing_output.status.code(), Some(2));
    let missing = parse_json(&missing_output);
    assert!(finding_ids(&missing).contains(&"GRAPH-DEPENDENCY-MISSING"));
}

#[test]
fn policy_validation_reports_commit_scope_and_missing_language_standards() {
    let project = TestProject::new("policy");
    let plan_id = create_plan(&project, "Policy defects", 50);
    apply_fixture(&project, &plan_id, "policy-invalid.yaml", 51);
    let output = validate(&project, &plan_id);
    assert_eq!(output.status.code(), Some(2));
    let value = parse_json(&output);
    let ids = finding_ids(&value);
    assert!(ids.contains(&"POLICY-COMMIT-MESSAGE-INVALID"));
    assert!(ids.contains(&"POLICY-COMMIT-SCOPE-OUTSIDE-FILE-MAP"));
    assert!(ids.contains(&"POLICY-FILE-MAP-OUTSIDE-COMMIT-SCOPE"));
    assert!(ids.contains(&"POLICY-STANDARD-REQUIRED"));
    assert!(ids.contains(&"POLICY-STANDARD-CHECK-MISSING"));
    assert_eq!(value["next_actions"][0]["id"], "standards.recommend");
    assert_eq!(value["next_actions"][1]["id"], "plan.apply");
    assert_fixed_layer_order(&value);
}

#[test]
fn policy_validation_requires_decided_git_and_git_flow_commit_gates() {
    let project = TestProject::new("git-policy");
    let source = fs::read_to_string(fixture_path("complete-without-gate.yaml"))
        .expect("Git Flow fixture should be readable");
    let input = yaml::parse_draft(&source).expect("Git Flow fixture should parse");
    let created_at = Timestamp::parse("2026-07-25T15:00:00Z").expect("timestamp should parse");
    let updated_at = Timestamp::parse("2026-07-25T15:01:00Z").expect("timestamp should parse");

    let mut undecided = Plan::new(
        PlanId::parse("2026-07-25-undecided-git").expect("plan ID should parse"),
        "Validate undecided Git readiness",
        created_at.clone(),
    );
    undecided
        .apply_draft_input(input.clone(), updated_at.clone())
        .expect("fixture should apply");
    let undecided_report =
        validate_plan(project.path(), &undecided).expect("validation should return a report");
    assert!(
        undecided_report
            .findings
            .iter()
            .any(|finding| finding.id == "POLICY-GIT-READINESS-UNDECIDED")
    );

    let seed = PlanDraftSeed {
        id: PlanId::parse("2026-07-25-git-flow-gate").expect("plan ID should parse"),
        name: "Git Flow gate".to_owned(),
        trigger: "durable".to_owned(),
        original_request: "Validate Git Flow commit gates".to_owned(),
        branch: Some("main".to_owned()),
        markdown_path: "docs/plan/2026-07-25-git-flow-gate.md".to_owned(),
        git_readiness: GitReadiness::detected(
            "Present",
            "Clean",
            Some("main".to_owned()),
            Some("abc1234".to_owned()),
            "Clean: git status --short returned empty",
            true,
        ),
        standards: Vec::new(),
        verification_plan: Vec::new(),
    };
    let mut git_flow = Plan::from_draft_seed(seed, created_at);
    git_flow
        .apply_draft_input(input, updated_at)
        .expect("fixture should apply");
    let git_flow_report =
        validate_plan(project.path(), &git_flow).expect("validation should return a report");
    assert!(
        git_flow_report
            .findings
            .iter()
            .any(|finding| finding.id == "POLICY-COMMIT-GATE-MISSING")
    );
}
