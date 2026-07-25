//! Golden and CLI contract tests for stable Agent context and next-action APIs.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use mino::application::agent::build_agent_context;
use mino::domain::{
    AcceptanceCriterion, Approval, CheckId, CriterionId, EvidenceId, FileChange, FileMapEntry,
    GitFlowConsent, Plan, PlanId, Task, TaskId, Timestamp, VerificationCheck,
};
use mino::project::initialize;
use serde_json::Value;

static NEXT_TEMP_DIRECTORY: AtomicU64 = AtomicU64::new(1);

struct TestProject {
    path: PathBuf,
}

impl TestProject {
    fn new(label: &str) -> Self {
        let sequence = NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "mino-agent-context-{label}-{}-{sequence}",
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
                .is_some_and(|name| name.to_string_lossy().starts_with("mino-agent-context-"))
        {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

fn timestamp(minute: u8) -> Timestamp {
    Timestamp::parse(format!("2026-07-26T00:{minute:02}:00Z")).expect("timestamp should parse")
}

fn plan_id() -> PlanId {
    PlanId::parse("2026-07-26-agent-fixture").expect("plan ID should parse")
}

fn task_id() -> TaskId {
    TaskId::parse("T1").expect("task ID should parse")
}

fn criterion_id() -> CriterionId {
    CriterionId::parse("T1-A1").expect("criterion ID should parse")
}

fn check_id(value: &str) -> CheckId {
    CheckId::parse(value).expect("check ID should parse")
}

fn evidence_id(value: &str) -> EvidenceId {
    EvidenceId::parse(value).expect("evidence ID should parse")
}

fn configured_draft() -> Plan {
    let task_id = task_id();
    let mut task = Task::new(task_id.clone(), "Implement the fixture", Vec::new());
    task.add_step("Implement deterministic behavior")
        .expect("step should be added");
    task.add_file_map_entry(FileMapEntry::new(
        "src/lib.rs",
        FileChange::Modify,
        "Own the fixture behavior",
        task_id.clone(),
    ))
    .expect("file should be added");
    task.add_acceptance_criterion(AcceptanceCriterion::new(
        criterion_id(),
        "The behavior is observable",
    ))
    .expect("criterion should be added");
    task.add_verification_check(VerificationCheck::new(
        check_id("TASK-V1"),
        vec!["cargo".to_owned(), "test".to_owned()],
        ".",
        0,
        true,
    ))
    .expect("task check should be added");
    let mut plan = Plan::new(plan_id(), "Build Agent context fixtures", timestamp(0));
    plan.add_task(task, timestamp(1))
        .expect("task should be added");
    plan.add_global_verification(
        VerificationCheck::new(
            check_id("GLOBAL-V1"),
            vec!["cargo".to_owned(), "test".to_owned()],
            ".",
            0,
            true,
        ),
        timestamp(2),
    )
    .expect("global check should be added");
    plan
}

fn lifecycle_contexts() -> Vec<(&'static str, Option<Plan>)> {
    let draft = Plan::new(plan_id(), "Build Agent context fixtures", timestamp(0));
    let mut ready = configured_draft();
    ready.finalize(timestamp(3)).expect("plan should finalize");
    let ready_unapproved = ready.clone();
    ready
        .record_approval(Approval::plan(
            "user",
            "chat:approval",
            timestamp(4),
            GitFlowConsent::Approved,
        ))
        .expect("plan should be approved");
    let ready_approved = ready.clone();
    ready
        .start_task(&task_id(), timestamp(5))
        .expect("task should start");
    let in_progress = ready.clone();
    let mut blocked = ready.clone();
    blocked
        .block("Waiting for a dependency", timestamp(6))
        .expect("plan should block");

    ready
        .record_task_criterion_pass(
            &task_id(),
            &criterion_id(),
            evidence_id("E0001"),
            timestamp(6),
        )
        .expect("criterion should pass");
    ready
        .record_task_check_pass(
            &task_id(),
            &check_id("TASK-V1"),
            evidence_id("E0002"),
            timestamp(7),
        )
        .expect("task check should pass");
    ready
        .complete_task(&task_id(), timestamp(8))
        .expect("task should complete");
    ready
        .record_global_check_pass(&check_id("GLOBAL-V1"), evidence_id("E0003"), timestamp(9))
        .expect("global check should pass");
    ready
        .finish_execution(timestamp(10))
        .expect("plan should enter review");
    let review = ready.clone();
    ready
        .accept_review(timestamp(11))
        .expect("plan should become Done");

    vec![
        ("none", None),
        ("draft", Some(draft)),
        ("ready-unapproved", Some(ready_unapproved)),
        ("ready-approved", Some(ready_approved)),
        ("in-progress", Some(in_progress)),
        ("blocked", Some(blocked)),
        ("review", Some(review)),
        ("done", Some(ready)),
    ]
}

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/agent")
        .join(format!("{name}.json"))
}

fn normalized(mut value: Value) -> Value {
    value["project"]["root"] = Value::from("<ROOT>");
    if let Some(actions) = value["next_actions"].as_array_mut() {
        for action in actions {
            if let Some(arguments) = action["argv"].as_array_mut()
                && let Some(index) = arguments
                    .iter()
                    .position(|argument| argument == "--request-id")
            {
                arguments[index + 1] = Value::from("<REQUEST_ID>");
            }
        }
    }
    value
}

#[test]
fn every_lifecycle_state_matches_its_agent_context_golden() {
    let root = Path::new("C:/fixture");
    for (name, plan) in lifecycle_contexts() {
        let context = build_agent_context(root, plan.as_ref()).expect("context should build");
        let actual = normalized(serde_json::to_value(context).expect("context should serialize"));
        let expected: Value = serde_json::from_str(
            &fs::read_to_string(fixture_path(name)).expect("golden should be readable"),
        )
        .expect("golden should be JSON");
        assert_eq!(actual, expected, "Agent context golden {name} changed");
    }
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

fn agent_arguments(project: &TestProject, action: &str) -> Vec<String> {
    let mut arguments = base_arguments(project);
    arguments.extend(["agent".to_owned(), action.to_owned()]);
    arguments
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

fn create_arguments(project: &TestProject, name: &str, number: u64) -> Vec<String> {
    let request_file = project.path().join(format!("request-{number}.md"));
    fs::write(&request_file, "Create one active Agent plan.\n").expect("request should be written");
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
        format!("30000000-0000-0000-0000-{number:012}"),
        "--actor".to_owned(),
        "codex".to_owned(),
    ]);
    arguments
}

#[test]
fn agent_cli_is_direct_strict_and_uses_the_only_active_plan() {
    let project = TestProject::new("cli");
    let empty = parse_success(&run_mino(&agent_arguments(&project, "context")));
    assert_eq!(empty["kind"], "mino.agent-context/v1");
    assert!(empty.get("ok").is_none());
    assert_eq!(empty["active_plan"], Value::Null);

    let mut prompt_capable = agent_arguments(&project, "context");
    prompt_capable.retain(|argument| argument != "--no-input");
    let rejected = run_mino(&prompt_capable);
    assert_eq!(rejected.status.code(), Some(5));
    let rejected = parse_json(&rejected);
    assert_eq!(rejected["error"]["code"], "policy_violation");
    assert_eq!(rejected["missing"], serde_json::json!(["--no-input"]));
    assert_eq!(rejected["next_actions"][0]["id"], "agent.context");

    let created = parse_success(&run_mino(&create_arguments(&project, "Active plan", 1)));
    let plan_id = created["plan_id"]
        .as_str()
        .expect("create should return a plan ID");
    let context = parse_success(&run_mino(&agent_arguments(&project, "context")));
    assert_eq!(context["active_plan"]["id"], plan_id);
    assert_eq!(context["active_plan"]["revision"], 1);
    assert_eq!(context["next_actions"][0]["id"], "plan.summary.set");
    let next = parse_success(&run_mino(&agent_arguments(&project, "next")));
    assert_eq!(next["kind"], "mino.agent-next/v1");
    assert_eq!(next["active_plan"], context["active_plan"]);
    assert_eq!(next["next_actions"], context["next_actions"]);
    let capabilities = parse_success(&run_mino(&agent_arguments(&project, "capabilities")));
    assert_eq!(capabilities["kind"], "mino.agent-capabilities/v1");
    assert_eq!(capabilities["invocation"]["requires_json"], true);
    assert_eq!(capabilities["invocation"]["requires_no_input"], true);
    assert!(
        capabilities["actions"]
            .as_array()
            .is_some_and(|actions| actions.iter().any(|action| action["id"] == "plan.approve"))
    );

    let second = run_mino(&create_arguments(&project, "Second active plan", 2));
    assert_eq!(second.status.code(), Some(5));
    let second = parse_json(&second);
    assert_eq!(second["missing"], serde_json::json!(["active_plan"]));
    assert_eq!(second["next_actions"][0]["id"], "agent.context");
}
