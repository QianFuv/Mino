//! Acceptance tests for ordered execution, checkpoints, evidence, and retry recovery.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use mino::ErrorCategory;
use mino::application::execution::{CheckExecutionDisposition, ExecutionService};
use mino::application::plan::PlanMutationRequest;
use mino::domain::{
    AcceptanceCriterion, Approval, CheckId, CheckRunContext, CheckRunLease, CheckRunLimits,
    CheckStatus, CheckpointKind, CriterionId, GitFlowConsent, GitReadiness, Plan, PlanDraftSeed,
    PlanId, RequestId, Task, TaskId, Timestamp, VerificationCheck,
};
use mino::evidence::EvidenceStore;
use mino::project::initialize;
use mino::render::{render_plan, write_projection};
use mino::runner::{CheckRunJournal, Redactor, RunEnvironment};
use mino::store::{MutationRequest, PlanStore, sha256_digest};
use serde_json::Value;

static NEXT_TEMP_DIRECTORY: AtomicU64 = AtomicU64::new(1);

struct TestProject {
    path: PathBuf,
    helper: PathBuf,
    base_revision: u64,
}

impl TestProject {
    fn new(label: &str, first_mode: &str, marker: Option<&Path>) -> Self {
        let sequence = NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "mino-execution-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("temporary project should be created");
        fs::write(
            path.join("Cargo.toml"),
            "[package]\nname = \"execution-fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .expect("fixture manifest should be written");
        fs::create_dir(path.join("src")).expect("fixture source directory should be created");
        fs::write(path.join("src/lib.rs"), "pub fn fixture() -> u8 { 1 }\n")
            .expect("fixture source should be written");
        initialize(&path).expect("temporary project should initialize");
        let path = path.canonicalize().expect("project root should resolve");
        let helper = compile_helper(&path);
        let base_revision = create_approved_plan(&path, &helper, first_mode, marker);
        Self {
            path,
            helper,
            base_revision,
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
                .is_some_and(|name| name.to_string_lossy().starts_with("mino-execution-"))
        {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

fn compile_helper(root: &Path) -> PathBuf {
    let source = root.join("execution-helper.rs");
    let executable = root.join(if cfg!(windows) {
        "execution-helper.exe"
    } else {
        "execution-helper"
    });
    fs::write(
        &source,
        r#"use std::env;
use std::fs::OpenOptions;
use std::io::Write;

fn main() {
    let mut arguments = env::args().skip(1);
    let mode = arguments.next().expect("mode is required");
    if mode.contains("mark") {
        let marker = arguments.next().expect("marker is required");
        let mut file = OpenOptions::new().create(true).append(true).open(marker).expect("marker should open");
        file.write_all(b"executed\n").expect("marker should write");
    }
    match mode.as_str() {
        "pass" | "pass-mark" => println!("planned success"),
        "fail" | "fail-mark" => {
            eprintln!("planned failure");
            std::process::exit(9);
        }
        _ => std::process::exit(10),
    }
}
"#,
    )
    .expect("helper source should be written");
    let output = Command::new("rustc")
        .args([source.as_os_str(), "-o".as_ref(), executable.as_os_str()])
        .output()
        .expect("rustc should run");
    assert!(
        output.status.success(),
        "rustc stdout: {}\nrustc stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    executable
}

fn create_approved_plan(
    root: &Path,
    helper: &Path,
    first_mode: &str,
    marker: Option<&Path>,
) -> u64 {
    let global = VerificationCheck::new(
        check_id("GLOBAL-CHECK"),
        helper_command(helper, "pass", None),
        ".",
        0,
        true,
    );
    let plan = Plan::from_draft_seed(
        PlanDraftSeed {
            id: plan_id(),
            name: "Execution contract".to_owned(),
            trigger: "durable".to_owned(),
            original_request: "Execute tasks in order with immutable evidence.".to_owned(),
            branch: None,
            markdown_path: projection_relative().to_owned(),
            git_readiness: GitReadiness::detected("Present", "Clean", None, None, "Clean", true),
            standards: Vec::new(),
            verification_plan: vec![global],
        },
        timestamp(0),
    );
    let store = PlanStore::new(root);
    store
        .create_plan(
            &plan,
            request_id(1),
            "codex",
            vec!["mino".to_owned(), "plan".to_owned(), "create".to_owned()],
        )
        .expect("initial plan should persist");
    let first_task = configured_task(
        task_id("T1"),
        Vec::new(),
        check_id("TASK-CHECK"),
        helper_command(helper, first_mode, marker),
    );
    commit(&store, 1, 2, vec!["tasks"], move |plan| {
        plan.add_task(first_task, timestamp(1))
    });
    let second_task = configured_task(
        task_id("T2"),
        vec![task_id("T1")],
        check_id("SECOND-CHECK"),
        helper_command(helper, "pass", None),
    );
    commit(&store, 2, 3, vec!["tasks"], move |plan| {
        plan.add_task(second_task, timestamp(2))
    });
    commit(&store, 3, 4, vec!["tasks.T1.status"], |plan| {
        plan.mark_task_ready(&task_id("T1"), timestamp(3))
    });
    commit(&store, 4, 5, vec!["tasks.T2.status"], |plan| {
        plan.mark_task_ready(&task_id("T2"), timestamp(4))
    });
    commit(&store, 5, 6, vec!["status"], |plan| {
        plan.finalize(timestamp(5))
    });
    commit(&store, 6, 7, vec!["approvals"], |plan| {
        plan.record_approval(Approval::plan(
            "user",
            "chat:execution-approval",
            timestamp(6),
            GitFlowConsent::Approved,
        ))
    });
    let plan = store
        .load_plan(&plan_id())
        .expect("approved plan should load");
    let rendered = render_plan(&plan).expect("approved plan should render");
    write_projection(&root.join(projection_relative()), &rendered, None)
        .expect("projection should be written");
    plan.revision()
}

fn configured_task(
    id: TaskId,
    dependencies: Vec<TaskId>,
    check_id: CheckId,
    command: Vec<String>,
) -> Task {
    let criterion_id =
        CriterionId::parse(format!("{id}-A1")).expect("criterion ID should be valid");
    let title = format!("Execute {id}");
    let mut task = Task::new(id, title, dependencies);
    task.add_acceptance_criterion(AcceptanceCriterion::new(
        criterion_id,
        "The task behavior is observable",
    ))
    .expect("criterion should be added");
    task.add_verification_check(VerificationCheck::new(check_id, command, ".", 0, true))
        .expect("verification should be added");
    task
}

fn helper_command(helper: &Path, mode: &str, marker: Option<&Path>) -> Vec<String> {
    let mut command = vec![helper.to_string_lossy().into_owned(), mode.to_owned()];
    if let Some(marker) = marker {
        command.push(marker.to_string_lossy().into_owned());
    }
    command
}

fn commit<F>(
    store: &PlanStore,
    expected_revision: u64,
    request_sequence: u64,
    changed_fields: Vec<&str>,
    mutation: F,
) where
    F: FnOnce(&mut Plan) -> Result<(), mino::domain::DomainError>,
{
    store
        .commit(
            &plan_id(),
            MutationRequest::new(
                expected_revision,
                request_id(request_sequence),
                "codex",
                vec!["test".to_owned(), request_sequence.to_string()],
                changed_fields.into_iter().map(str::to_owned).collect(),
            )
            .expect("store request should be valid"),
            mutation,
        )
        .expect("plan mutation should persist");
}

fn plan_id() -> PlanId {
    PlanId::parse("2026-07-25-execution-contract").expect("plan ID should be valid")
}

fn task_id(value: &str) -> TaskId {
    TaskId::parse(value).expect("task ID should be valid")
}

fn check_id(value: &str) -> CheckId {
    CheckId::parse(value).expect("check ID should be valid")
}

fn request_id(sequence: u64) -> RequestId {
    RequestId::parse(format!("40000000-0000-0000-0000-{sequence:012x}"))
        .expect("request ID should be valid")
}

fn timestamp(minute: u8) -> Timestamp {
    Timestamp::parse(format!("2026-07-25T14:{minute:02}:00Z")).expect("timestamp should be valid")
}

fn projection_relative() -> &'static str {
    "docs/plan/execution-contract.md"
}

fn mutation(
    expected_revision: u64,
    request_sequence: u64,
    command: Vec<String>,
    minute: u8,
) -> PlanMutationRequest {
    PlanMutationRequest {
        plan_id: plan_id(),
        expected_revision,
        request_id: request_id(request_sequence),
        actor: "codex".to_owned(),
        command,
        updated_at: timestamp(minute),
    }
}

fn start_command(expected_revision: u64, request_sequence: u64, task: &str) -> Vec<String> {
    vec![
        "mino".to_owned(),
        "exec".to_owned(),
        "start".to_owned(),
        "--plan".to_owned(),
        plan_id().to_string(),
        "--expect-revision".to_owned(),
        expected_revision.to_string(),
        "--request-id".to_owned(),
        request_id(request_sequence).to_string(),
        "--actor".to_owned(),
        "codex".to_owned(),
        "--task".to_owned(),
        task.to_owned(),
    ]
}

fn check_command(expected_revision: u64, request_sequence: u64) -> Vec<String> {
    vec![
        "mino".to_owned(),
        "exec".to_owned(),
        "check".to_owned(),
        "run".to_owned(),
        "--plan".to_owned(),
        plan_id().to_string(),
        "--expect-revision".to_owned(),
        expected_revision.to_string(),
        "--request-id".to_owned(),
        request_id(request_sequence).to_string(),
        "--actor".to_owned(),
        "codex".to_owned(),
        "--check".to_owned(),
        "TASK-CHECK".to_owned(),
    ]
}

#[test]
fn execution_order_checkpoints_and_block_resume_are_revision_checked() {
    let project = TestProject::new("state", "pass", None);
    let service = ExecutionService::discover(project.path()).expect("service should discover");
    let wrong_task = service
        .start_task(
            mutation(
                project.base_revision,
                20,
                start_command(project.base_revision, 20, "T2"),
                20,
            ),
            task_id("T2"),
        )
        .expect_err("later task should not start first");
    assert_eq!(wrong_task.category(), ErrorCategory::PolicyViolation);
    let started = service
        .start_task(
            mutation(
                project.base_revision,
                21,
                start_command(project.base_revision, 21, "T1"),
                21,
            ),
            task_id("T1"),
        )
        .expect("first task should start");
    let checkpointed = service
        .checkpoint(
            mutation(
                started.revision,
                22,
                vec![
                    "mino".to_owned(),
                    "exec".to_owned(),
                    "checkpoint".to_owned(),
                ],
                22,
            ),
            task_id("T1"),
            CheckpointKind::Inspection,
            "Inspected the declared task surface".to_owned(),
        )
        .expect("checkpoint should persist");
    let plan = PlanStore::new(project.path())
        .load_plan(&plan_id())
        .expect("plan should load");
    let execution = plan
        .execution_state()
        .expect("execution state should decode");
    assert_eq!(execution.checkpoints().len(), 1);
    assert_eq!(execution.checkpoints()[0].sequence(), 1);
    let blocked = service
        .block(
            mutation(
                checkpointed.revision,
                23,
                vec!["mino".to_owned(), "exec".to_owned(), "block".to_owned()],
                23,
            ),
            "Waiting for an external dependency".to_owned(),
        )
        .expect("plan should block");
    let resumed = service
        .resume(mutation(
            blocked.revision,
            24,
            vec!["mino".to_owned(), "exec".to_owned(), "resume".to_owned()],
            24,
        ))
        .expect("plan should resume");
    assert_eq!(resumed.revision, project.base_revision + 4);
}

#[test]
fn successful_check_is_evidenced_once_and_replayed_without_process_execution() {
    let marker = std::env::temp_dir().join(format!(
        "mino-execution-marker-{}-{}",
        std::process::id(),
        NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed)
    ));
    let project = TestProject::new("replay", "pass-mark", Some(&marker));
    let service = ExecutionService::discover(project.path()).expect("service should discover");
    let started = service
        .start_task(
            mutation(
                project.base_revision,
                30,
                start_command(project.base_revision, 30, "T1"),
                30,
            ),
            task_id("T1"),
        )
        .expect("task should start");
    let request = mutation(
        started.revision,
        31,
        check_command(started.revision, 31),
        31,
    );
    let first = service
        .run_check(&request, &check_id("TASK-CHECK"))
        .expect("check should run");
    assert!(first.is_success());
    assert_eq!(first.disposition(), CheckExecutionDisposition::Executed);
    assert_eq!(first.evidence().id().as_str(), "E0001");
    assert_eq!(
        fs::read_to_string(&marker).expect("marker should exist"),
        "executed\n"
    );
    let replay = service
        .run_check(&request, &check_id("TASK-CHECK"))
        .expect("check should replay");
    assert!(replay.is_success());
    assert_eq!(replay.disposition(), CheckExecutionDisposition::Replayed);
    assert_eq!(replay.evidence().id().as_str(), "E0001");
    assert_eq!(replay.plan().revision, first.plan().revision);
    assert_eq!(
        fs::read_to_string(&marker).expect("marker should exist"),
        "executed\n"
    );
    assert_eq!(
        EvidenceStore::new(project.path())
            .list(&plan_id())
            .expect("evidence should list")
            .len(),
        1
    );
    let plan = PlanStore::new(project.path())
        .load_plan(&plan_id())
        .expect("plan should load");
    assert_eq!(
        plan.task(&task_id("T1"))
            .expect("task should exist")
            .verification_checks()[0]
            .status(),
        CheckStatus::Passed
    );
    let _ = fs::remove_file(marker);
}

#[test]
fn failed_cli_check_returns_exit_six_after_persisting_terminal_evidence() {
    let marker = std::env::temp_dir().join(format!(
        "mino-execution-failure-marker-{}-{}",
        std::process::id(),
        NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed)
    ));
    let project = TestProject::new("failure", "fail-mark", Some(&marker));
    let start = run_mino(
        &project,
        &[
            "exec",
            "start",
            "--plan",
            plan_id().as_str(),
            "--task",
            "T1",
            "--expect-revision",
            &project.base_revision.to_string(),
            "--request-id",
            request_id(40).as_str(),
            "--actor",
            "codex",
        ],
    );
    assert!(
        start.status.success(),
        "{}",
        String::from_utf8_lossy(&start.stdout)
    );
    let active_revision = project.base_revision + 1;
    let failed = run_mino(
        &project,
        &[
            "exec",
            "check",
            "run",
            "--plan",
            plan_id().as_str(),
            "--check",
            "TASK-CHECK",
            "--expect-revision",
            &active_revision.to_string(),
            "--request-id",
            request_id(41).as_str(),
            "--actor",
            "codex",
        ],
    );
    assert_eq!(failed.status.code(), Some(6));
    let payload: Value = serde_json::from_slice(&failed.stdout).expect("failure should be JSON");
    assert_eq!(payload["error"]["code"], "check_failed");
    assert_eq!(payload["execution"]["run"]["outcome"], "unexpected_exit");
    assert_eq!(payload["execution"]["evidence"]["exit_code"], 9);
    assert_eq!(
        fs::read_to_string(&marker).expect("marker should exist"),
        "executed\n"
    );
    let evidence = EvidenceStore::new(project.path())
        .list(&plan_id())
        .expect("evidence should list");
    assert_eq!(evidence.len(), 1);
    assert_eq!(evidence[0].check_id(), Some(&check_id("TASK-CHECK")));
    let plan = PlanStore::new(project.path())
        .load_plan(&plan_id())
        .expect("plan should load");
    assert_eq!(plan.revision(), active_revision + 2);
    assert_eq!(
        plan.task(&task_id("T1"))
            .expect("task should exist")
            .verification_checks()[0]
            .status(),
        CheckStatus::Failed
    );
    let _ = fs::remove_file(marker);
}

#[test]
fn abandoned_lease_becomes_interrupted_evidence_on_retry() {
    let project = TestProject::new("interrupted", "pass", None);
    let service = ExecutionService::discover(project.path()).expect("service should discover");
    let started = service
        .start_task(
            mutation(
                project.base_revision,
                50,
                start_command(project.base_revision, 50, "T1"),
                40,
            ),
            task_id("T1"),
        )
        .expect("task should start");
    let root_request = mutation(
        started.revision,
        51,
        check_command(started.revision, 51),
        41,
    );
    prepare_abandoned_lease(&project, &root_request);
    let report = service
        .run_check(&root_request, &check_id("TASK-CHECK"))
        .expect("abandoned lease should reconcile");
    assert!(!report.is_success());
    assert_eq!(
        report.disposition(),
        CheckExecutionDisposition::RecoveredInterrupted
    );
    assert_eq!(
        report.run().outcome(),
        mino::domain::CheckRunOutcome::Interrupted
    );
    assert!(
        report
            .evidence()
            .output_summary()
            .is_some_and(|summary| summary.contains("Previous invocation ended"))
    );
}

fn prepare_abandoned_lease(project: &TestProject, request: &PlanMutationRequest) {
    let store = PlanStore::new(project.path());
    let prior = store.load_plan(&plan_id()).expect("prior plan should load");
    let begin_request_id = phase_request_id(&request.request_id, "begin");
    let mut begin_command = request.command.clone();
    begin_command.extend(["--mino-phase".to_owned(), "begin".to_owned()]);
    store
        .commit(
            &plan_id(),
            MutationRequest::new(
                request.expected_revision,
                begin_request_id,
                "codex",
                begin_command,
                vec!["tasks.T1.verification_checks.TASK-CHECK.status".to_owned()],
            )
            .expect("begin request should be valid"),
            |plan| plan.begin_check_run(&check_id("TASK-CHECK"), timestamp(41)),
        )
        .expect("begin phase should persist");
    let leased = store
        .load_plan(&plan_id())
        .expect("leased plan should load");
    let prior_rendered = render_plan(&prior).expect("prior plan should render");
    let leased_rendered = render_plan(&leased).expect("leased plan should render");
    write_projection(
        &project.path().join(projection_relative()),
        &leased_rendered,
        Some(&prior_rendered),
    )
    .expect("leased projection should publish");
    let environment = RunEnvironment::minimal();
    let redactor = Redactor::default();
    let check = leased
        .task(&task_id("T1"))
        .expect("task should exist")
        .verification_checks()[0]
        .clone();
    let lease = CheckRunLease::new(
        CheckRunContext::new(
            plan_id(),
            leased.revision(),
            Some(task_id("T1")),
            phase_request_id(&request.request_id, "run"),
            "codex",
            leased.metadata().updated_at().clone(),
        )
        .expect("context should be valid"),
        &check,
        CheckRunLimits::new(Duration::from_mins(5), 1024 * 1024).expect("limits should be valid"),
        environment.variable_names(),
        environment.digest(),
        redactor.policy_digest(),
    )
    .expect("lease should be valid");
    let journal_directory = PathBuf::from(".mino")
        .join("plans")
        .join(plan_id().as_str())
        .join("runs");
    CheckRunJournal::new(project.path(), &journal_directory)
        .expect("journal should be valid")
        .begin(&lease)
        .expect("abandoned lease should persist");
}

fn phase_request_id(request_id: &RequestId, phase: &str) -> RequestId {
    let digest = sha256_digest(format!("{request_id}:{phase}").as_bytes());
    let value = &digest["sha256:".len().."sha256:".len() + 32];
    RequestId::parse(format!(
        "{}-{}-{}-{}-{}",
        &value[0..8],
        &value[8..12],
        &value[12..16],
        &value[16..20],
        &value[20..32]
    ))
    .expect("phase request ID should be valid")
}

fn run_mino(project: &TestProject, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mino"))
        .args([
            "--root",
            project.path().to_string_lossy().as_ref(),
            "--format",
            "json",
            "--no-input",
        ])
        .args(arguments)
        .stdin(Stdio::null())
        .output()
        .expect("Mino binary should run")
}

#[test]
fn helper_binary_is_an_explicit_non_shell_test_fixture() {
    let project = TestProject::new("helper", "pass", None);
    let output = Command::new(&project.helper)
        .arg("pass")
        .output()
        .expect("helper should run");
    assert!(output.status.success());
}
