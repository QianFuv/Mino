//! Golden and CLI contract tests for stable Agent context and next-action APIs.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use mino::application::{AGENT_EXECUTOR_IDENTITY, agent::build_agent_context};
use mino::domain::{
    AmendmentPatch, Approval, CheckId, CriterionId, DeviationClassification, DraftCommitGateInput,
    DraftCriterionInput, DraftFileInput, DraftMetadataInput, DraftPlanInput, DraftScopeInput,
    DraftTaskInput, DraftVerificationInput, EvidenceId, FileChange, GitFlowConsent, GitReadiness,
    Plan, PlanDraftSeed, PlanId, StandardSelection, TaskId, Timestamp, VerificationCheck,
};
use mino::git::{ActiveBindingStore, GitAdapter};
use mino::project::initialize;
use mino::standards::EmbeddedCatalog;
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
            "mino-agent-context-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("temporary project should be created");
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
    configured_draft_with_git(
        GitReadiness::detected(
            "Missing",
            "Not Applicable",
            None,
            None,
            "Not Applicable: lifecycle fixture",
            false,
        ),
        None,
    )
}

fn configured_draft_with_git(
    git_readiness: GitReadiness,
    commit_gate: Option<DraftCommitGateInput>,
) -> Plan {
    let catalog = EmbeddedCatalog::load().expect("embedded standards should load");
    let common = catalog
        .package("common")
        .expect("Common standards should exist");
    let mut plan = Plan::from_draft_seed(
        PlanDraftSeed {
            id: plan_id(),
            name: "Build Agent context fixtures".to_owned(),
            trigger: "durable".to_owned(),
            original_request: "Build Agent context fixtures".to_owned(),
            branch: None,
            markdown_path: "docs/plan/2026-07-26-agent-fixture.md".to_owned(),
            git_readiness,
            standards: vec![StandardSelection::new(
                common.package_id(),
                common.version(),
                common.digest(),
                "embedded",
            )],
            verification_plan: vec![VerificationCheck::new(
                check_id("GLOBAL-V1"),
                vec!["cargo".to_owned(), "test".to_owned()],
                ".",
                0,
                true,
            )],
        },
        timestamp(0),
    );
    plan.apply_draft_input(
        DraftPlanInput {
            metadata: Some(DraftMetadataInput {
                priority: Some("P1".to_owned()),
                area: Some("agent".to_owned()),
                owner: Some("codex".to_owned()),
                ..DraftMetadataInput::default()
            }),
            summary: Some("Exercise every stable Agent lifecycle state.".to_owned()),
            scope: Some(DraftScopeInput {
                goal: Some("Keep Agent lifecycle guidance deterministic.".to_owned()),
                deliverables: Some(vec!["Stable Agent context fixtures".to_owned()]),
                in_scope: Some(vec!["Agent lifecycle state".to_owned()]),
                out_of_scope: Some(vec!["External side effects".to_owned()]),
            }),
            approach: Some("Build one complete in-memory lifecycle aggregate.".to_owned()),
            interfaces: Some(
                "Agent context projects one plan state into canonical actions.".to_owned(),
            ),
            ..DraftPlanInput::default()
        },
        timestamp(1),
    )
    .expect("authored plan fields should be added");
    plan.author_task(
        DraftTaskInput {
            id: Some(task_id()),
            title: "Implement the fixture".to_owned(),
            depends_on: Vec::new(),
            steps: vec!["Implement deterministic behavior".to_owned()],
            files: vec![DraftFileInput {
                path: "fixture.txt".to_owned(),
                change: FileChange::Modify,
                reason: "Own the fixture behavior".to_owned(),
            }],
            acceptance_criteria: vec![DraftCriterionInput {
                id: Some(criterion_id()),
                description: "The behavior is observable".to_owned(),
            }],
            verification: vec![DraftVerificationInput {
                id: check_id("TASK-V1"),
                command: vec!["cargo".to_owned(), "test".to_owned()],
                cwd: ".".to_owned(),
                expected_exit_code: 0,
                required: true,
            }],
            commit_gate,
        },
        timestamp(2),
    )
    .expect("task should be added");
    plan
}

fn initialize_git_flow_baseline(project: &TestProject) -> GitReadiness {
    let initialized = Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(project.path())
        .output()
        .expect("Git should initialize the Agent fixture");
    assert!(initialized.status.success());
    for (key, value) in [
        ("user.name", "Mino Agent Test"),
        ("user.email", "mino-agent@example.invalid"),
    ] {
        let configured = Command::new("git")
            .args(["config", key, value])
            .current_dir(project.path())
            .output()
            .expect("Git identity should configure");
        assert!(configured.status.success());
    }
    fs::write(
        project.path().join(".git/info/exclude"),
        ".agents/\n.mino/\n",
    )
    .expect("Mino state should be excluded from the Git fixture");
    fs::write(project.path().join("fixture.txt"), "baseline\n")
        .expect("baseline fixture should be written");
    let staged = Command::new("git")
        .args(["add", "--", "fixture.txt"])
        .current_dir(project.path())
        .output()
        .expect("baseline fixture should stage");
    assert!(staged.status.success());
    let committed = Command::new("git")
        .args(["commit", "--quiet", "-m", "chore: establish agent baseline"])
        .current_dir(project.path())
        .output()
        .expect("baseline fixture should commit");
    assert!(committed.status.success());
    let facts = GitAdapter::new(project.path())
        .inspect()
        .expect("Git baseline facts should inspect");
    assert!(facts.is_clean);
    GitReadiness::detected(
        "Present",
        "Clean",
        facts.branch,
        facts.head,
        "Clean: git status --short returned empty",
        true,
    )
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
        .set_final_outcome(
            "Agent guidance fixture is verified".to_owned(),
            "N/A".to_owned(),
            Vec::new(),
            timestamp(10),
        )
        .expect("Final Outcome should record");
    ready
        .finish_execution(timestamp(10))
        .expect("plan should enter review");
    let review = ready.clone();
    ready
        .accept_review(
            "reviewer".to_owned(),
            "chat:acceptance".to_owned(),
            timestamp(11),
        )
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

fn assert_agent_action_identity(actions: &[mino::NextAction]) {
    for action in actions {
        let actor_index = action
            .argv
            .iter()
            .position(|argument| argument == "--actor");
        if action
            .argv
            .iter()
            .any(|argument| argument == "--expect-revision")
        {
            let actor_index =
                actor_index.expect("revisioned Agent action should identify its actor");
            assert_eq!(
                action.argv.get(actor_index + 1).map(String::as_str),
                Some(AGENT_EXECUTOR_IDENTITY)
            );
        } else {
            assert!(
                actor_index.is_none(),
                "read-only Agent action {} must not include --actor",
                action.id
            );
        }
    }
}

fn assert_next_actions_are_allowed(context: &mino::application::agent::AgentContext) {
    for action in &context.next_actions {
        assert!(
            context.allowed_actions.contains(&action.id),
            "next action {} must also be allowed",
            action.id
        );
    }
}

#[test]
fn every_lifecycle_state_matches_its_agent_context_golden() {
    let project = TestProject::new("goldens");
    let root = project.path();
    for (name, plan) in lifecycle_contexts() {
        if matches!(name, "ready-unapproved" | "ready-approved") {
            let report = validate_plan(root, plan.as_ref().expect("Ready plan should exist"))
                .expect("Ready plan should be validated");
            assert!(
                report.valid,
                "Ready fixture findings: {:?}",
                report.findings
            );
        }
        let context = build_agent_context(root, plan.as_ref()).expect("context should build");
        assert_eq!(context.executor_identity, AGENT_EXECUTOR_IDENTITY);
        assert_agent_action_identity(&context.next_actions);
        assert_next_actions_are_allowed(&context);
        let actual = normalized(serde_json::to_value(context).expect("context should serialize"));
        let expected: Value = serde_json::from_str(
            &fs::read_to_string(fixture_path(name)).expect("golden should be readable"),
        )
        .expect("golden should be JSON");
        assert_eq!(actual, expected, "Agent context golden {name} changed");
    }
}

#[test]
fn automatic_git_flow_requires_current_plan_binding_before_start_and_commit() {
    let project = TestProject::new("git-binding-guidance");
    let git_readiness = initialize_git_flow_baseline(&project);
    let mut plan = configured_draft_with_git(
        git_readiness,
        Some(DraftCommitGateInput {
            required: true,
            planned_message: "test(agent): verify binding guidance".to_owned(),
            scope: vec!["fixture.txt".to_owned()],
        }),
    );
    plan.finalize(timestamp(3)).expect("plan should finalize");
    plan.record_approval(Approval::plan(
        "user",
        "chat:approval",
        timestamp(4),
        GitFlowConsent::Approved,
    ))
    .expect("plan should be approved");

    let unbound_ready =
        build_agent_context(project.path(), Some(&plan)).expect("Ready context should build");
    assert_next_actions_are_allowed(&unbound_ready);
    assert_eq!(unbound_ready.next_actions[0].id, "git.bind");
    assert!(
        unbound_ready
            .allowed_actions
            .contains(&"git.bind".to_owned())
    );
    assert!(
        !unbound_ready
            .allowed_actions
            .contains(&"exec.start".to_owned())
    );

    let facts = GitAdapter::new(project.path())
        .inspect()
        .expect("Git facts should inspect");
    ActiveBindingStore::new(project.path())
        .bind(&facts, plan.id().clone(), plan.revision(), timestamp(5))
        .expect("current plan should bind");
    let bound_ready =
        build_agent_context(project.path(), Some(&plan)).expect("bound Ready context should build");
    assert_next_actions_are_allowed(&bound_ready);
    assert_eq!(bound_ready.next_actions[0].id, "exec.start");
    assert!(
        bound_ready
            .allowed_actions
            .contains(&"exec.start".to_owned())
    );

    plan.start_task(&task_id(), timestamp(6))
        .expect("task should start");
    plan.record_task_criterion_pass(
        &task_id(),
        &criterion_id(),
        evidence_id("E0001"),
        timestamp(7),
    )
    .expect("criterion should pass");
    plan.record_task_check_pass(
        &task_id(),
        &check_id("TASK-V1"),
        evidence_id("E0002"),
        timestamp(8),
    )
    .expect("task check should pass");
    plan.complete_task(&task_id(), timestamp(9))
        .expect("task should complete");

    let bound_commit = build_agent_context(project.path(), Some(&plan))
        .expect("bound commit context should build");
    assert_next_actions_are_allowed(&bound_commit);
    assert_eq!(bound_commit.next_actions[0].id, "git.commit");
    fs::remove_file(ActiveBindingStore::new(project.path()).path())
        .expect("binding should be removed for the missing-binding fixture");
    let unbound_commit = build_agent_context(project.path(), Some(&plan))
        .expect("unbound commit context should build");
    assert_next_actions_are_allowed(&unbound_commit);
    assert_eq!(unbound_commit.next_actions[0].id, "git.bind");
    assert!(
        unbound_commit
            .allowed_actions
            .contains(&"git.bind".to_owned())
    );
    assert!(
        !unbound_commit
            .allowed_actions
            .contains(&"git.commit".to_owned())
    );
    assert!(
        unbound_commit
            .blocked_actions
            .iter()
            .any(|action| action.action == "git.commit")
    );
}

#[test]
fn verified_execution_requires_outcome_before_finish_guidance() {
    let project = TestProject::new("outcome-guidance");
    let mut plan = configured_draft();
    plan.finalize(timestamp(3)).expect("plan should finalize");
    plan.record_approval(Approval::plan(
        "user",
        "chat:approval",
        timestamp(4),
        GitFlowConsent::Approved,
    ))
    .expect("plan should be approved");
    plan.start_task(&task_id(), timestamp(5))
        .expect("task should start");
    plan.record_task_criterion_pass(
        &task_id(),
        &criterion_id(),
        evidence_id("E0001"),
        timestamp(6),
    )
    .expect("criterion should pass");
    plan.record_task_check_pass(
        &task_id(),
        &check_id("TASK-V1"),
        evidence_id("E0002"),
        timestamp(7),
    )
    .expect("task check should pass");
    plan.complete_task(&task_id(), timestamp(8))
        .expect("task should complete");
    plan.record_global_check_pass(&check_id("GLOBAL-V1"), evidence_id("E0003"), timestamp(9))
        .expect("global check should pass");

    let incomplete = build_agent_context(project.path(), Some(&plan))
        .expect("incomplete outcome context should build");
    assert_eq!(
        incomplete.allowed_actions,
        ["plan.outcome.set", "exec.block", "plan.amend.propose"]
    );
    assert!(incomplete.next_actions.is_empty());
    assert!(
        incomplete
            .blocked_actions
            .iter()
            .any(|action| action.action == "exec.finish")
    );

    plan.set_final_outcome(
        "Agent guidance fixture is verified".to_owned(),
        "N/A".to_owned(),
        Vec::new(),
        timestamp(10),
    )
    .expect("Final Outcome should record");
    let complete = build_agent_context(project.path(), Some(&plan))
        .expect("complete outcome context should build");
    assert_eq!(complete.next_actions[0].id, "exec.finish");
}

#[test]
fn ready_plan_with_open_deviation_exposes_applied_amendment_supersession() {
    let project = TestProject::new("ready-deviation");
    let mut plan = configured_draft();
    plan.finalize(timestamp(3)).expect("plan should finalize");
    plan.record_approval(Approval::plan(
        "user",
        "chat:approval",
        timestamp(4),
        GitFlowConsent::Approved,
    ))
    .expect("plan should approve");
    plan.start_task(&task_id(), timestamp(5))
        .expect("task should start");
    plan.record_deviation(
        &task_id(),
        DeviationClassification::Material,
        "The approved summary must change".to_owned(),
        Vec::new(),
        "codex".to_owned(),
        timestamp(6),
    )
    .expect("deviation should record");
    let patch: AmendmentPatch = serde_json::from_value(serde_json::json!({
        "operations": [{
            "operation": "replace-summary",
            "summary": "Exercise Agent guidance after material deviation amendment."
        }]
    }))
    .expect("patch should parse");
    plan.propose_amendment(
        "Update the approved summary".to_owned(),
        patch,
        None,
        format!("sha256:{}", "a".repeat(64)),
        "codex".to_owned(),
        timestamp(7),
    )
    .expect("amendment should propose");
    plan.approve_amendment(
        "C1",
        "user".to_owned(),
        "chat:amendment-approved".to_owned(),
        timestamp(8),
    )
    .expect("amendment should approve");
    plan.apply_amendment("C1", timestamp(9))
        .expect("amendment should apply");
    let context = build_agent_context(project.path(), Some(&plan)).expect("context should build");
    assert!(
        context
            .allowed_actions
            .contains(&"exec.deviation.list".to_owned())
    );
    assert!(
        context
            .allowed_actions
            .contains(&"exec.deviation.supersede".to_owned())
    );
    assert!(
        !context
            .allowed_actions
            .contains(&"exec.deviation.resolve".to_owned())
    );
}

fn run_mino(arguments: &[String]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mino"))
        .args(arguments)
        .stdin(Stdio::null())
        .output()
        .expect("Mino binary should run")
}

fn run_canonical_action(project: &TestProject, arguments: &[String], input: &str) -> Output {
    assert_eq!(arguments.first().map(String::as_str), Some("mino"));
    let mut child = Command::new(env!("CARGO_BIN_EXE_mino"))
        .args(&arguments[1..])
        .current_dir(project.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Mino canonical action should start");
    child
        .stdin
        .take()
        .expect("canonical action stdin should be piped")
        .write_all(input.as_bytes())
        .expect("canonical action input should be written");
    child
        .wait_with_output()
        .expect("Mino canonical action should finish")
}

fn last_event_actor(project: &TestProject, plan_id: &str) -> String {
    let plan_id = PlanId::parse(plan_id).expect("plan ID should parse");
    let events = PlanStore::new(project.path())
        .events(&plan_id)
        .expect("plan events should load");
    let event = serde_json::to_value(events.last().expect("plan should have an event"))
        .expect("event should serialize");
    event["actor"]
        .as_str()
        .expect("event actor should be a string")
        .to_owned()
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
            Timestamp::parse("2026-07-27T05:22:00Z").expect("binding timestamp should parse"),
        )
        .expect("active binding should be written");
    fs::remove_dir_all(project.path().join(".git"))
        .expect("Git repository should be removed from the fixture");
}

#[test]
fn agent_cli_is_direct_strict_and_uses_the_only_active_plan() {
    let project = TestProject::new("cli");
    let empty = parse_success(&run_mino(&agent_arguments(&project, "context")));
    assert_eq!(empty["kind"], "mino.agent-context/v1");
    assert_eq!(empty["executor_identity"], AGENT_EXECUTOR_IDENTITY);
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
    assert!(
        rejected["next_actions"][0]["argv"]
            .as_array()
            .is_some_and(|arguments| arguments.iter().all(|argument| argument != "--actor"))
    );

    let created = parse_success(&run_mino(&create_arguments(&project, "Active plan", 1)));
    let plan_id = created["plan_id"]
        .as_str()
        .expect("create should return a plan ID");
    retain_binding_after_git_removal(&project, plan_id, 1);
    let context = parse_success(&run_mino(&agent_arguments(&project, "context")));
    assert_eq!(context["active_plan"]["id"], plan_id);
    assert_eq!(context["active_plan"]["revision"], 1);
    assert_eq!(context["next_actions"][0]["id"], "plan.summary.set");
    let next = parse_success(&run_mino(&agent_arguments(&project, "next")));
    assert_eq!(next["kind"], "mino.agent-next/v1");
    assert_eq!(next["executor_identity"], AGENT_EXECUTOR_IDENTITY);
    assert_eq!(next["active_plan"], context["active_plan"]);
    assert_eq!(next["plan_selection"], context["plan_selection"]);
    assert_eq!(next["next_actions"], context["next_actions"]);
    let capabilities = parse_success(&run_mino(&agent_arguments(&project, "capabilities")));
    assert_eq!(capabilities["kind"], "mino.agent-capabilities/v1");
    assert_eq!(capabilities["executor_identity"], AGENT_EXECUTOR_IDENTITY);
    assert_eq!(capabilities["invocation"]["requires_json"], true);
    assert_eq!(capabilities["invocation"]["requires_no_input"], true);
    assert!(
        capabilities["actions"]
            .as_array()
            .is_some_and(|actions| actions.iter().any(|action| action["id"] == "plan.approve"))
    );
    assert!(capabilities["actions"].as_array().is_some_and(|actions| {
        actions.iter().any(|action| {
            action["id"] == "review.disposition.revise" && action["approval_boundary"] == true
        })
    }));

    let second = run_mino(&create_arguments(&project, "Second active plan", 2));
    assert_eq!(second.status.code(), Some(5));
    let second = parse_json(&second);
    assert_eq!(second["missing"], serde_json::json!(["active_plan"]));
    assert_eq!(second["next_actions"][0]["id"], "agent.context");
}

#[test]
fn canonical_agent_mutations_record_codex_while_direct_cli_defaults_to_user() {
    for (label, should_keep_agent_actor, expected_actor) in [
        ("agent-actor", true, AGENT_EXECUTOR_IDENTITY),
        ("human-actor", false, "user"),
    ] {
        let project = TestProject::new(label);
        let created = parse_success(&run_mino(&create_arguments(&project, label, 20)));
        let plan_id = created["plan_id"]
            .as_str()
            .expect("created plan ID should be a string");
        let context = parse_success(&run_mino(&agent_arguments(&project, "context")));
        let mut arguments = context["next_actions"][0]["argv"]
            .as_array()
            .expect("canonical argv should be an array")
            .iter()
            .map(|argument| {
                argument
                    .as_str()
                    .expect("canonical argument should be a string")
                    .to_owned()
            })
            .collect::<Vec<_>>();
        assert_eq!(context["next_actions"][0]["id"], "plan.summary.set");
        let actor_index = arguments
            .iter()
            .position(|argument| argument == "--actor")
            .expect("Agent mutation should identify its actor");
        assert_eq!(arguments[actor_index + 1], AGENT_EXECUTOR_IDENTITY);
        if !should_keep_agent_actor {
            arguments.drain(actor_index..=actor_index + 1);
        }

        let mutation = parse_success(&run_canonical_action(
            &project,
            &arguments,
            "Record the executor identity in this plan.\n",
        ));
        assert_eq!(last_event_actor(&project, plan_id), expected_actor);
        assert_eq!(mutation["next_actions"][0]["id"], "plan.apply");
        let next_arguments = mutation["next_actions"][0]["argv"]
            .as_array()
            .expect("next canonical argv should be an array");
        let next_actor = next_arguments
            .iter()
            .position(|argument| argument == "--actor")
            .expect("next Agent mutation should identify its actor");
        assert_eq!(
            next_arguments[next_actor + 1],
            Value::String(AGENT_EXECUTOR_IDENTITY.to_owned())
        );
    }
}

#[test]
fn validation_finalization_action_uses_the_agent_executor_identity() {
    let project = TestProject::new("validation-actor");
    let plan = configured_draft();
    let report = validate_plan(project.path(), &plan).expect("Draft should validate");
    assert!(report.valid);
    assert_eq!(report.next_actions[0].id, "plan.finalize");
    assert_agent_action_identity(&report.next_actions);
}
