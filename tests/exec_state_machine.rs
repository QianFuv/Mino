//! Acceptance tests for ordered execution, checkpoints, evidence, and retry recovery.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use mino::ErrorCategory;
use mino::application::agent::AgentService;
use mino::application::completion::CompletionService;
use mino::application::execution::{CheckExecutionDisposition, ExecutionService};
use mino::application::plan::PlanMutationRequest;
use mino::domain::{
    AcceptanceCriterion, Approval, CheckId, CheckRunContext, CheckRunLease, CheckRunLimits,
    CheckStatus, CheckpointKind, CommitStatus, CriterionId, CriterionStatus, GitFlowConsent,
    GitReadiness, Plan, PlanDraftSeed, PlanId, RequestId, Task, TaskId, TaskStatus, Timestamp,
    VerificationCheck,
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
        Self::new_with_global_mode(label, first_mode, "pass", marker)
    }

    fn new_with_global_mode(
        label: &str,
        first_mode: &str,
        global_mode: &str,
        marker: Option<&Path>,
    ) -> Self {
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
        let base_revision = create_approved_plan(&path, &helper, first_mode, global_mode, marker);
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
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

fn main() {
    let mut arguments = env::args().skip(1);
    let mode = arguments.next().expect("mode is required");
    if mode == "block-mark" {
        let marker = PathBuf::from(arguments.next().expect("marker is required"));
        let mut file = OpenOptions::new().create(true).append(true).open(&marker).expect("marker should open");
        file.write_all(b"executed\n").expect("marker should write");
        fs::write(marker.with_extension("ready"), b"ready").expect("ready marker should write");
        let release = marker.with_extension("release");
        let deadline = Instant::now() + Duration::from_secs(10);
        while !release.exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        if !release.exists() {
            std::process::exit(11);
        }
        println!("planned success");
        return;
    }
    if mode.contains("mark") {
        let marker = arguments.next().expect("marker is required");
        let mut file = OpenOptions::new().create(true).append(true).open(marker).expect("marker should open");
        file.write_all(b"executed\n").expect("marker should write");
    }
    match mode.as_str() {
        "pass" | "pass-mark" => println!("planned success"),
        "residual-secret" => println!("client_secret executioncredentialvalue"),
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

#[test]
fn residual_secret_capture_fails_the_check_without_publishing_evidence() {
    let project = TestProject::new("capture-blocked", "residual-secret", None);
    let service = ExecutionService::discover(project.path()).expect("service should discover");
    let started = service
        .start_task(
            mutation(
                project.base_revision,
                42,
                start_command(project.base_revision, 42, "T1"),
                40,
            ),
            task_id("T1"),
        )
        .expect("task should start");
    let error = service
        .run_check(
            &mutation(
                started.revision,
                43,
                check_command(started.revision, 43),
                41,
            ),
            &check_id("TASK-CHECK"),
        )
        .expect_err("residual credential capture must fail closed");
    assert_eq!(error.category(), ErrorCategory::PolicyViolation);
    assert!(error.message().contains("residual credential scan"));
    assert!(!error.message().contains("executioncredentialvalue"));
    assert!(
        EvidenceStore::new(project.path())
            .list(&plan_id())
            .expect("evidence should list")
            .is_empty()
    );
    let plan = PlanStore::new(project.path())
        .load_plan(&plan_id())
        .expect("plan should load");
    let check = &plan
        .task(&task_id("T1"))
        .expect("task should exist")
        .verification_checks()[0];
    assert_eq!(check.status(), CheckStatus::Failed);
    assert!(check.evidence_refs().is_empty());
    let managed_bytes = recursive_managed_bytes(&project.path().join(".mino"));
    assert!(!String::from_utf8_lossy(&managed_bytes).contains("executioncredentialvalue"));
}

fn recursive_managed_bytes(root: &Path) -> Vec<u8> {
    let mut output = Vec::new();
    let mut directories = vec![root.to_path_buf()];
    while let Some(directory) = directories.pop() {
        let Ok(entries) = fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_dir() {
                directories.push(path);
            } else if path.is_file() {
                output.extend(fs::read(path).expect("managed fixture file should be readable"));
            }
        }
    }
    output
}

fn create_approved_plan(
    root: &Path,
    helper: &Path,
    first_mode: &str,
    global_mode: &str,
    marker: Option<&Path>,
) -> u64 {
    let global = VerificationCheck::new(
        check_id("GLOBAL-CHECK"),
        helper_command(helper, global_mode, None),
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
fn failed_global_verification_reopens_an_owned_task_for_fresh_execution() {
    let project = TestProject::new_with_global_mode("global-rework", "pass", "fail", None);
    let service = ExecutionService::discover(project.path()).expect("service should discover");
    let first_done = complete_task_workflow(
        &project,
        &service,
        project.base_revision,
        100,
        ("T1", "TASK-CHECK", "T1-A1"),
        30,
    );
    let second_done = complete_task_workflow(
        &project,
        &service,
        first_done,
        110,
        ("T2", "SECOND-CHECK", "T2-A1"),
        35,
    );
    let failed = service
        .run_check(
            &mutation(
                second_done,
                120,
                vec!["mino".to_owned(), "exec".to_owned(), "check".to_owned()],
                40,
            ),
            &check_id("GLOBAL-CHECK"),
        )
        .expect("failed global check should persist its result");
    assert!(!failed.is_success());
    let guidance = AgentService::discover(project.path())
        .expect("Agent service should discover")
        .context()
        .expect("failed global check should produce Agent guidance");
    assert!(
        guidance
            .allowed_actions
            .iter()
            .any(|action| action == "exec.rework")
    );
    assert_eq!(
        guidance
            .next_actions
            .iter()
            .filter(|action| action.id == "exec.rework")
            .count(),
        2
    );

    let reworked = service
        .rework_failed_global_verification(
            mutation(
                failed.plan().revision,
                121,
                vec!["mino".to_owned(), "exec".to_owned(), "rework".to_owned()],
                41,
            ),
            task_id("T1"),
            "GLOBAL-CHECK exposed a defect owned by T1".to_owned(),
        )
        .expect("failed final verification should reopen the owning task");
    assert_global_rework_state(&project, reworked.revision, failed.plan().revision);

    service
        .start_task(
            mutation(
                reworked.revision,
                122,
                start_command(reworked.revision, 122, "T1"),
                42,
            ),
            task_id("T1"),
        )
        .expect("reopened task should return to normal execution");
}

fn assert_global_rework_state(project: &TestProject, reworked_revision: u64, failed_revision: u64) {
    let plan = PlanStore::new(project.path())
        .load_plan(&plan_id())
        .expect("reworked plan should load");
    let first = plan.task(&task_id("T1")).expect("first task should exist");
    assert_eq!(reworked_revision, failed_revision + 1);
    assert_eq!(first.status(), TaskStatus::Ready);
    assert_eq!(
        first.acceptance_criteria()[0].status(),
        CriterionStatus::Pending
    );
    assert_eq!(
        first.verification_checks()[0].status(),
        CheckStatus::Pending
    );
    assert!(!first.verification_checks()[0].evidence_refs().is_empty());
    assert!(
        first
            .commit_gate()
            .is_none_or(|gate| gate.status() == CommitStatus::NotRequired)
    );
    assert_eq!(plan.global_verification()[0].status(), CheckStatus::Pending);
    assert!(!plan.global_verification()[0].evidence_refs().is_empty());
    assert!(
        plan.workspace_state()
            .expect("workspace state should decode")
            .task_baseline(&task_id("T1"))
            .is_none()
    );
    assert!(
        first
            .implementation_notes()
            .last()
            .is_some_and(|note| note.contains("GLOBAL-CHECK exposed a defect"))
    );
}

fn complete_task_workflow(
    project: &TestProject,
    service: &ExecutionService,
    revision: u64,
    sequence: u64,
    identity: (&str, &str, &str),
    minute: u8,
) -> u64 {
    let (task, check, criterion) = identity;
    let started = service
        .start_task(
            mutation(
                revision,
                sequence,
                start_command(revision, sequence, task),
                minute,
            ),
            task_id(task),
        )
        .expect("task should start");
    let checked = service
        .run_check(
            &mutation(
                started.revision,
                sequence + 1,
                vec!["mino".to_owned(), "exec".to_owned(), "check".to_owned()],
                minute + 1,
            ),
            &check_id(check),
        )
        .expect("task check should run");
    assert!(checked.is_success());
    let completion =
        CompletionService::discover(project.path()).expect("completion service should discover");
    let criterion = completion
        .pass_criterion(
            mutation(
                checked.plan().revision,
                sequence + 2,
                vec!["mino".to_owned(), "exec".to_owned(), "criterion".to_owned()],
                minute + 2,
            ),
            CriterionId::parse(criterion).expect("criterion ID should be valid"),
            checked.evidence().id().clone(),
        )
        .expect("criterion should pass");
    completion
        .complete_task(
            mutation(
                criterion.revision,
                sequence + 3,
                vec!["mino".to_owned(), "exec".to_owned(), "complete".to_owned()],
                minute + 3,
            ),
            task_id(task),
        )
        .expect("task should complete")
        .revision
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
fn concurrent_exact_retry_keeps_the_running_plan_and_persists_one_passed_result() {
    let marker = std::env::temp_dir().join(format!(
        "mino-execution-concurrent-{}-{}",
        std::process::id(),
        NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed)
    ));
    let ready = marker.with_extension("ready");
    let release = marker.with_extension("release");
    let project = TestProject::new("concurrent", "block-mark", Some(&marker));
    let limits = CheckRunLimits::new(Duration::from_secs(10), 1024 * 1024)
        .expect("concurrent limits should be valid");
    let service = ExecutionService::discover_with_limits(project.path(), limits)
        .expect("service should discover");
    let started = service
        .start_task(
            mutation(
                project.base_revision,
                60,
                start_command(project.base_revision, 60, "T1"),
                42,
            ),
            task_id("T1"),
        )
        .expect("task should start");
    let request = mutation(
        started.revision,
        61,
        check_command(started.revision, 61),
        43,
    );
    let winner_service = service.clone();
    let winner_request = request.clone();
    let winner =
        thread::spawn(move || winner_service.run_check(&winner_request, &check_id("TASK-CHECK")));
    if !wait_for_path(&ready, Duration::from_secs(30)) {
        fs::write(&release, b"release").expect("blocked check should be released");
        let _ = winner.join();
        panic!("blocking check did not signal readiness");
    }

    let retry = service.run_check(&request, &check_id("TASK-CHECK"));
    assert_live_retry_state(&project, &request, started.revision + 1);

    fs::write(&release, b"release").expect("blocked check should be released");
    let completed = winner
        .join()
        .expect("winner thread should join")
        .expect("winner should complete");
    let retry = retry.expect_err("live retry should report a revision conflict");
    assert_eq!(retry.category(), ErrorCategory::RevisionConflict);
    assert!(retry.message().contains("already running"));
    assert!(completed.is_success());
    assert_eq!(completed.disposition(), CheckExecutionDisposition::Executed);
    assert_eq!(completed.evidence().id().as_str(), "E0001");
    let final_plan = PlanStore::new(project.path())
        .load_plan(&plan_id())
        .expect("completed plan should load");
    assert_eq!(final_plan.revision(), started.revision + 2);
    assert_eq!(
        final_plan
            .task(&task_id("T1"))
            .expect("task should exist")
            .verification_checks()[0]
            .status(),
        CheckStatus::Passed
    );
    assert_eq!(
        fs::read_to_string(&marker).expect("execution marker should exist"),
        "executed\n"
    );
    assert_eq!(
        EvidenceStore::new(project.path())
            .list(&plan_id())
            .expect("evidence should list")
            .len(),
        1
    );
    let replay = service
        .run_check(&request, &check_id("TASK-CHECK"))
        .expect("completed check should replay");
    assert_eq!(replay.disposition(), CheckExecutionDisposition::Replayed);
    assert_eq!(replay.evidence().id().as_str(), "E0001");
    assert_eq!(
        EvidenceStore::new(project.path())
            .list(&plan_id())
            .expect("evidence should list")
            .len(),
        1
    );
    for path in [&marker, &ready, &release] {
        let _ = fs::remove_file(path);
    }
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

#[test]
#[allow(clippy::too_many_lines)]
fn deviation_cli_records_lists_replays_and_rejects_identified_departures() {
    let project = TestProject::new("deviation-cli", "pass", None);
    let base_revision = project.base_revision.to_string();
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
            &base_revision,
            "--request-id",
            request_id(60).as_str(),
            "--actor",
            "codex",
        ],
    );
    assert!(start.status.success());
    let started: Value = serde_json::from_slice(&start.stdout).expect("start should be JSON");
    let started_revision = started["revision"].as_u64().unwrap().to_string();
    let record_plan_id = plan_id();
    let record_request_id = request_id(61);
    let record_arguments = [
        "exec",
        "deviation",
        "record",
        "--plan",
        record_plan_id.as_str(),
        "--task",
        "T1",
        "--classification",
        "minor",
        "--summary",
        "A task-local implementation departure",
        "--path",
        "support/generated.txt",
        "--expect-revision",
        &started_revision,
        "--request-id",
        record_request_id.as_str(),
        "--actor",
        "codex",
    ];
    let recorded = run_mino(&project, &record_arguments);
    assert!(recorded.status.success());
    let recorded: Value = serde_json::from_slice(&recorded.stdout).expect("record should be JSON");
    assert_eq!(recorded["assigned_id"], "D1");
    let replay = run_mino(&project, &record_arguments);
    let replay: Value = serde_json::from_slice(&replay.stdout).expect("replay should be JSON");
    assert_eq!(replay["replayed"], true);

    let listed = run_mino(
        &project,
        &["exec", "deviation", "list", "--plan", plan_id().as_str()],
    );
    let listed: Value = serde_json::from_slice(&listed.stdout).expect("list should be JSON");
    assert_eq!(listed["kind"], "mino.deviation-list/v1");
    assert_eq!(listed["deviations"][0]["status"], "Open");
    assert_eq!(
        listed["deviations"][0]["affected_paths"],
        serde_json::json!(["support/generated.txt"])
    );
    let recorded_revision = recorded["revision"].as_u64().unwrap().to_string();
    let rejected = run_mino(
        &project,
        &[
            "exec",
            "deviation",
            "reject",
            "--plan",
            plan_id().as_str(),
            "--deviation",
            "D1",
            "--decision-ref",
            "chat:deviation-rejected",
            "--reason",
            "The departure is not accepted",
            "--expect-revision",
            &recorded_revision,
            "--request-id",
            request_id(62).as_str(),
            "--actor",
            "user",
        ],
    );
    assert!(rejected.status.success());
    let listed = run_mino(
        &project,
        &[
            "exec",
            "deviation",
            "list",
            "--plan",
            plan_id().as_str(),
            "--task",
            "T1",
        ],
    );
    let listed: Value = serde_json::from_slice(&listed.stdout).expect("list should be JSON");
    assert_eq!(listed["deviations"][0]["status"], "Rejected");
    assert_eq!(
        listed["deviations"][0]["disposition_reference"],
        "chat:deviation-rejected"
    );
    let projection = fs::read_to_string(project.path().join(projection_relative()))
        .expect("projection should be readable");
    assert!(projection.contains("## Execution Deviations"));
    assert!(projection.contains("support/generated.txt"));
    assert!(projection.contains("chat:deviation-rejected"));
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

fn wait_for_path(path: &Path, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if path.exists() {
            return true;
        }
        thread::sleep(Duration::from_millis(10));
    }
    path.exists()
}

fn assert_live_retry_state(
    project: &TestProject,
    request: &PlanMutationRequest,
    running_revision: u64,
) {
    let running_plan = PlanStore::new(project.path())
        .load_plan(&plan_id())
        .expect("running plan should load");
    assert_eq!(running_plan.revision(), running_revision);
    assert_eq!(
        running_plan
            .task(&task_id("T1"))
            .expect("task should exist")
            .verification_checks()[0]
            .status(),
        CheckStatus::Running
    );
    assert!(
        EvidenceStore::new(project.path())
            .list(&plan_id())
            .expect("evidence should list")
            .is_empty()
    );
    let journal_directory = PathBuf::from(".mino")
        .join("plans")
        .join(plan_id().as_str())
        .join("runs");
    let journal =
        CheckRunJournal::new(project.path(), &journal_directory).expect("run journal should open");
    assert!(
        journal
            .result(&phase_request_id(&request.request_id, "run"))
            .expect("run journal should inspect")
            .is_none()
    );
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
