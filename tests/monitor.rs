//! Acceptance tests for finite foreground planned-check monitoring.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use mino::ErrorCategory;
use mino::application::execution::ExecutionService;
use mino::application::monitor::{
    MONITOR_KIND, MonitorAttemptDisposition, MonitorBounds, MonitorRequest, MonitorService,
    MonitorTerminalReason,
};
use mino::application::plan::PlanMutationRequest;
use mino::domain::{
    AcceptanceCriterion, Approval, CheckId, CheckRunLimits, CheckRunOutcome, CriterionId,
    GitFlowConsent, GitReadiness, GitReadinessObservation, GitReadinessState, GitRepositoryMode,
    GitSetupDecision, Plan, PlanDraftSeed, PlanId, RequestId, Task, TaskId, Timestamp,
    VerificationCheck,
};
use mino::evidence::EvidenceStore;
use mino::project::initialize;
use mino::render::{render_plan, write_projection};
use mino::store::{MutationRequest, PlanStore, sha256_digest};
use serde_json::Value;

static NEXT_TEMP_DIRECTORY: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy)]
enum Fixture {
    PassAfter(u32),
    Fail,
    Sleep(u64),
    CancelAfterFailure,
}

struct TestProject {
    path: PathBuf,
    marker: PathBuf,
    base_revision: u64,
}

impl TestProject {
    fn new(label: &str, fixture: Fixture) -> Self {
        let sequence = NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "mino-monitor-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("temporary project should be created");
        fs::write(
            path.join("Cargo.toml"),
            "[package]\nname = \"monitor-fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .expect("fixture manifest should be written");
        fs::create_dir(path.join("src")).expect("fixture source directory should be created");
        fs::write(path.join("src/lib.rs"), "pub fn fixture() -> u8 { 1 }\n")
            .expect("fixture source should be written");
        initialize(&path).expect("temporary project should initialize");
        let path = path.canonicalize().expect("project root should resolve");
        let helper = compile_helper(&path);
        let marker = path.join("attempts.log");
        let command = fixture_command(&path, &helper, &marker, fixture);
        let base_revision = create_started_plan(&path, command);
        Self {
            path,
            marker,
            base_revision,
        }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn cancellation_relative() -> PathBuf {
        PathBuf::from("stop.flag")
    }

    fn cancellation_absolute(&self) -> PathBuf {
        self.path.join(Self::cancellation_relative())
    }

    fn marker_attempts(&self) -> usize {
        fs::read_to_string(&self.marker).map_or(0, |content| content.lines().count())
    }
}

impl Drop for TestProject {
    fn drop(&mut self) {
        let temporary_root = std::env::temp_dir();
        if self.path.starts_with(&temporary_root)
            && self
                .path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("mino-monitor-"))
        {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

fn compile_helper(root: &Path) -> PathBuf {
    let source = root.join("monitor-helper.rs");
    let executable = root.join(if cfg!(windows) {
        "monitor-helper.exe"
    } else {
        "monitor-helper"
    });
    fs::write(
        &source,
        r#"use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::thread;
use std::time::Duration;

fn append_attempt(path: &str) -> usize {
    let prior = fs::read_to_string(path).unwrap_or_default();
    let attempt = prior.lines().count() + 1;
    let mut file = OpenOptions::new().create(true).append(true).open(path).expect("marker should open");
    file.write_all(b"executed\n").expect("marker should write");
    attempt
}

fn main() {
    let mut arguments = env::args().skip(1);
    let mode = arguments.next().expect("mode is required");
    match mode.as_str() {
        "pass-after" => {
            let marker = arguments.next().expect("marker is required");
            let threshold = arguments.next().expect("threshold is required").parse::<usize>().expect("threshold should parse");
            if append_attempt(&marker) < threshold {
                std::process::exit(9);
            }
        }
        "fail" => {
            let marker = arguments.next().expect("marker is required");
            append_attempt(&marker);
            std::process::exit(9);
        }
        "sleep" => {
            let milliseconds = arguments.next().expect("sleep is required").parse::<u64>().expect("sleep should parse");
            thread::sleep(Duration::from_millis(milliseconds));
            std::process::exit(9);
        }
        "cancel-fail" => {
            let marker = arguments.next().expect("marker is required");
            let cancellation = arguments.next().expect("cancellation path is required");
            append_attempt(&marker);
            fs::write(cancellation, b"cancel\n").expect("cancellation should write");
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

fn fixture_command(root: &Path, helper: &Path, marker: &Path, fixture: Fixture) -> Vec<String> {
    let mut command = vec![helper.to_string_lossy().into_owned()];
    match fixture {
        Fixture::PassAfter(threshold) => command.extend([
            "pass-after".to_owned(),
            marker.to_string_lossy().into_owned(),
            threshold.to_string(),
        ]),
        Fixture::Fail => command.extend(["fail".to_owned(), marker.to_string_lossy().into_owned()]),
        Fixture::Sleep(milliseconds) => {
            command.extend(["sleep".to_owned(), milliseconds.to_string()]);
        }
        Fixture::CancelAfterFailure => command.extend([
            "cancel-fail".to_owned(),
            marker.to_string_lossy().into_owned(),
            root.join("stop.flag").to_string_lossy().into_owned(),
        ]),
    }
    command
}

fn create_started_plan(root: &Path, task_command: Vec<String>) -> u64 {
    let global =
        VerificationCheck::new(check_id("GLOBAL-CHECK"), task_command.clone(), ".", 0, true);
    let mut plan = Plan::from_draft_seed(
        PlanDraftSeed {
            id: plan_id(),
            name: "Monitor contract".to_owned(),
            trigger: "durable".to_owned(),
            original_request: "Observe one planned check within finite bounds.".to_owned(),
            branch: None,
            markdown_path: projection_relative().to_owned(),
            git_readiness: GitReadiness::detected(
                "Missing",
                "Not Applicable",
                None,
                None,
                "No Git repository",
                false,
            ),
            standards: Vec::new(),
            verification_plan: vec![global],
        },
        timestamp(0),
    );
    plan.record_initial_git_readiness(&non_git_readiness_state())
        .expect("initial Git readiness should record");
    let store = PlanStore::new(root);
    store
        .create_plan(
            &plan,
            request_id(1),
            "codex",
            vec!["mino".to_owned(), "plan".to_owned(), "create".to_owned()],
        )
        .expect("initial plan should persist");
    let mut task = Task::new(task_id(), "Observe the planned check", Vec::new());
    task.add_acceptance_criterion(AcceptanceCriterion::new(
        criterion_id(),
        "Every attempt has immutable evidence",
    ))
    .expect("criterion should be added");
    task.add_verification_check(VerificationCheck::new(
        check_id("TASK-CHECK"),
        task_command,
        ".",
        0,
        true,
    ))
    .expect("verification should be added");
    commit(&store, 1, 2, vec!["tasks"], move |plan| {
        plan.add_task(task, timestamp(1))
    });
    commit(&store, 2, 3, vec!["tasks.T1.status"], |plan| {
        plan.mark_task_ready(&task_id(), timestamp(2))
    });
    commit(&store, 3, 4, vec!["status"], |plan| {
        plan.finalize(timestamp(3))
    });
    commit(&store, 4, 5, vec!["approvals"], |plan| {
        plan.record_approval(Approval::plan(
            "user",
            "chat:monitor-approval",
            timestamp(4),
            GitFlowConsent::Disabled,
        ))
    });
    let approved = store
        .load_plan(&plan_id())
        .expect("approved plan should load");
    let rendered = render_plan(&approved).expect("approved plan should render");
    write_projection(&root.join(projection_relative()), &rendered, None)
        .expect("projection should be written");
    ExecutionService::discover(root)
        .expect("execution service should discover")
        .start_task(
            PlanMutationRequest {
                plan_id: plan_id(),
                expected_revision: approved.revision(),
                request_id: request_id(6),
                actor: "codex".to_owned(),
                command: vec!["mino".to_owned(), "exec".to_owned(), "start".to_owned()],
                updated_at: timestamp(5),
            },
            task_id(),
        )
        .expect("task should start")
        .revision
}

fn non_git_readiness_state() -> GitReadinessState {
    let mut state = GitReadinessState::new(
        GitReadinessObservation::new(
            GitRepositoryMode::NotRepository,
            None,
            None,
            None,
            None,
            sha256_digest(b"[]"),
            false,
            timestamp(0),
        )
        .expect("non-Git observation should validate"),
    )
    .expect("non-Git readiness should validate");
    state
        .decide_setup(
            GitSetupDecision::ContinueWithoutGit,
            "user".to_owned(),
            "chat:continue-without-git".to_owned(),
            timestamp(0),
        )
        .expect("non-Git setup decision should record");
    state
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

fn monitor_request(
    project: &TestProject,
    sequence: u64,
    bounds: MonitorBounds,
    cancel_file: Option<PathBuf>,
) -> MonitorRequest {
    let mut command = vec![
        "mino".to_owned(),
        "exec".to_owned(),
        "check".to_owned(),
        "monitor".to_owned(),
        "--plan".to_owned(),
        plan_id().to_string(),
        "--expect-revision".to_owned(),
        project.base_revision.to_string(),
        "--request-id".to_owned(),
        request_id(sequence).to_string(),
        "--actor".to_owned(),
        "codex".to_owned(),
        "--check".to_owned(),
        "TASK-CHECK".to_owned(),
        "--max-attempts".to_owned(),
        bounds.max_attempts().to_string(),
        "--interval-milliseconds".to_owned(),
        bounds.interval_milliseconds().to_string(),
        "--deadline-milliseconds".to_owned(),
        bounds.deadline_milliseconds().to_string(),
    ];
    if let Some(path) = &cancel_file {
        command.extend([
            "--cancel-file".to_owned(),
            path.to_string_lossy().into_owned(),
        ]);
    }
    MonitorRequest {
        plan_id: plan_id(),
        expected_revision: project.base_revision,
        request_id: request_id(sequence),
        actor: "codex".to_owned(),
        command,
        check_id: check_id("TASK-CHECK"),
        bounds,
        cancel_file,
    }
}

fn execute_first_monitor_attempt(project: &TestProject, request: &MonitorRequest) {
    let limits = CheckRunLimits::new(
        Duration::from_millis(request.bounds.check_timeout_milliseconds()),
        1024 * 1024,
    )
    .expect("check limits should validate");
    let mut command = request.command.clone();
    command.extend(["--mino-monitor-attempt".to_owned(), "1".to_owned()]);
    let report = ExecutionService::discover_with_limits(project.path(), limits)
        .expect("execution service should discover")
        .run_check(
            &PlanMutationRequest {
                plan_id: request.plan_id.clone(),
                expected_revision: request.expected_revision,
                request_id: monitor_attempt_request_id(&request.request_id, 1),
                actor: request.actor.clone(),
                command,
                updated_at: timestamp(10),
            },
            &request.check_id,
        )
        .expect("first monitor attempt should persist without a summary");
    assert!(!report.is_success());
}

fn monitor_attempt_request_id(base: &RequestId, number: u32) -> RequestId {
    let digest = sha256_digest(format!("{base}:monitor-attempt:{number}").as_bytes());
    let value = &digest["sha256:".len().."sha256:".len() + 32];
    RequestId::parse(format!(
        "{}-{}-{}-{}-{}",
        &value[0..8],
        &value[8..12],
        &value[12..16],
        &value[16..20],
        &value[20..32]
    ))
    .expect("monitor attempt request ID should be valid")
}

fn plan_id() -> PlanId {
    PlanId::parse("2026-07-26-monitor-contract").expect("plan ID should be valid")
}

fn task_id() -> TaskId {
    TaskId::parse("T1").expect("task ID should be valid")
}

fn criterion_id() -> CriterionId {
    CriterionId::parse("T1-A1").expect("criterion ID should be valid")
}

fn check_id(value: &str) -> CheckId {
    CheckId::parse(value).expect("check ID should be valid")
}

fn request_id(sequence: u64) -> RequestId {
    RequestId::parse(format!("80000000-0000-0000-0000-{sequence:012x}"))
        .expect("request ID should be valid")
}

fn timestamp(minute: u8) -> Timestamp {
    Timestamp::parse(format!("2026-07-26T08:{minute:02}:00Z")).expect("timestamp should be valid")
}

fn projection_relative() -> &'static str {
    "docs/plan/monitor-contract.md"
}

fn summary_path(project: &TestProject, sequence: u64) -> PathBuf {
    project
        .path
        .join(".mino")
        .join("plans")
        .join(plan_id().as_str())
        .join("monitors")
        .join(request_id(sequence).to_string())
        .join("summary.json")
}

fn tree_snapshot(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn visit(root: &Path, directory: &Path, snapshot: &mut BTreeMap<String, Vec<u8>>) {
        let mut entries = fs::read_dir(directory)
            .expect("snapshot directory should read")
            .collect::<Result<Vec<_>, _>>()
            .expect("snapshot entries should read");
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for entry in entries {
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).expect("snapshot metadata should read");
            let relative = path
                .strip_prefix(root)
                .expect("snapshot path should be relative")
                .to_string_lossy()
                .replace('\\', "/");
            if metadata.is_dir() {
                visit(root, &path, snapshot);
            } else if metadata.is_file() {
                snapshot.insert(relative, fs::read(path).expect("snapshot file should read"));
            }
        }
    }

    let mut snapshot = BTreeMap::new();
    visit(root, root, &mut snapshot);
    snapshot
}

#[cfg(unix)]
fn create_file_symlink(target: &Path, link: &Path) -> bool {
    std::os::unix::fs::symlink(target, link).is_ok()
}

#[cfg(windows)]
fn create_file_symlink(target: &Path, link: &Path) -> bool {
    std::os::windows::fs::symlink_file(target, link).is_ok()
}

#[cfg(unix)]
fn create_directory_symlink(target: &Path, link: &Path) -> bool {
    std::os::unix::fs::symlink(target, link).is_ok()
}

#[cfg(windows)]
fn create_directory_symlink(target: &Path, link: &Path) -> bool {
    std::os::windows::fs::symlink_dir(target, link).is_ok()
}

fn run_mino(project: &TestProject, arguments: &[String]) -> Output {
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

fn cli_arguments(project: &TestProject, sequence: u64, attempts: u32) -> Vec<String> {
    vec![
        "exec".to_owned(),
        "check".to_owned(),
        "monitor".to_owned(),
        "--plan".to_owned(),
        plan_id().to_string(),
        "--check".to_owned(),
        "TASK-CHECK".to_owned(),
        "--expect-revision".to_owned(),
        project.base_revision.to_string(),
        "--request-id".to_owned(),
        request_id(sequence).to_string(),
        "--actor".to_owned(),
        "codex".to_owned(),
        "--max-attempts".to_owned(),
        attempts.to_string(),
        "--interval-milliseconds".to_owned(),
        "1".to_owned(),
        "--deadline-milliseconds".to_owned(),
        "10000".to_owned(),
    ]
}

#[test]
fn monitor_bounds_prove_finite_attempt_wait_and_process_budgets() {
    let bounds = MonitorBounds::new(3, 10, 50).expect("bounds should validate");
    assert_eq!(bounds.max_attempts(), 3);
    assert_eq!(bounds.interval_milliseconds(), 10);
    assert_eq!(bounds.deadline_milliseconds(), 50);
    assert_eq!(bounds.check_timeout_milliseconds(), 10);
    assert_eq!(
        bounds.terminal_before_attempt(0, Duration::ZERO, true),
        Some(MonitorTerminalReason::Cancelled)
    );
    assert_eq!(
        bounds.terminal_before_attempt(0, Duration::from_millis(50), false),
        Some(MonitorTerminalReason::DeadlineReached)
    );
    assert_eq!(
        bounds.terminal_before_attempt(3, Duration::from_millis(49), false),
        Some(MonitorTerminalReason::AttemptsExhausted)
    );
    assert_eq!(
        bounds.next_wait(Duration::from_millis(45)),
        Duration::from_millis(5)
    );
    assert_eq!(
        MonitorBounds::new(1, 1, 86_400_000)
            .expect("maximum deadline should validate")
            .check_timeout_milliseconds(),
        300_000
    );
    for invalid in [
        MonitorBounds::new(0, 1, 10),
        MonitorBounds::new(101, 1, 1_000),
        MonitorBounds::new(1, 0, 10),
        MonitorBounds::new(1, 60_001, 70_000),
        MonitorBounds::new(1, 1, 0),
        MonitorBounds::new(1, 1, 86_400_001),
        MonitorBounds::new(2, 11, 10),
        MonitorBounds::new(3, 10, 20),
    ] {
        assert_eq!(
            invalid
                .expect_err("invalid monitor bounds should fail")
                .category(),
            ErrorCategory::IncompleteOrValidation
        );
    }
}

#[test]
fn passing_monitor_records_attempts_and_exact_retry_is_read_only() {
    let project = TestProject::new("pass-replay", Fixture::PassAfter(2));
    let service = MonitorService::discover(project.path()).expect("service should discover");
    let bounds = MonitorBounds::new(3, 1, 10_000).expect("bounds should validate");
    let request = monitor_request(&project, 20, bounds, None);
    let report = service
        .run(request.clone())
        .expect("monitor should eventually pass");
    assert_eq!(report.monitor_kind, MONITOR_KIND);
    assert_eq!(report.terminal_reason, MonitorTerminalReason::Passed);
    assert_eq!(report.expected_revision, project.base_revision);
    assert_eq!(report.final_revision, project.base_revision + 4);
    assert_eq!(report.attempts.len(), 2);
    assert_eq!(report.attempts[0].number, 1);
    assert_eq!(report.attempts[0].outcome, CheckRunOutcome::UnexpectedExit);
    assert_eq!(
        report.attempts[0].disposition,
        MonitorAttemptDisposition::Executed
    );
    assert_eq!(report.attempts[0].evidence_id.as_str(), "E0001");
    assert_eq!(report.attempts[1].number, 2);
    assert_eq!(report.attempts[1].outcome, CheckRunOutcome::Passed);
    assert_eq!(
        report.attempts[1].disposition,
        MonitorAttemptDisposition::Executed
    );
    assert_eq!(report.attempts[1].evidence_id.as_str(), "E0002");
    assert_eq!(project.marker_attempts(), 2);
    assert_eq!(
        EvidenceStore::new(project.path())
            .list(&plan_id())
            .expect("evidence should list")
            .len(),
        2
    );
    let summary = fs::read(summary_path(&project, 20)).expect("summary should exist");
    assert!(summary.ends_with(b"\n"));
    let journal: Value = serde_json::from_slice(&summary).expect("summary should parse");
    assert_eq!(journal["schema_version"], MONITOR_KIND);
    assert_eq!(journal["report"]["terminal_reason"], "passed");
    let before_retry = tree_snapshot(project.path());
    let replay = service
        .run(request.clone())
        .expect("exact retry should return the terminal summary");
    assert_eq!(replay, report);
    assert_eq!(tree_snapshot(project.path()), before_retry);
    assert_eq!(project.marker_attempts(), 2);

    let conflict = service
        .run(MonitorRequest {
            bounds: MonitorBounds::new(2, 1, 10_000).expect("bounds should validate"),
            ..request
        })
        .expect_err("request ID reuse with different bounds should fail");
    assert_eq!(conflict.category(), ErrorCategory::RevisionConflict);
    assert_eq!(tree_snapshot(project.path()), before_retry);
}

#[test]
fn missing_terminal_summary_replays_prior_attempts_before_continuing_or_cancelling() {
    let project = TestProject::new("recover-summary", Fixture::PassAfter(2));
    let bounds = MonitorBounds::new(3, 1, 10_000).expect("bounds should validate");
    let request = monitor_request(&project, 25, bounds, None);
    execute_first_monitor_attempt(&project, &request);
    assert_eq!(project.marker_attempts(), 1);
    assert!(!summary_path(&project, 25).exists());
    let report = MonitorService::discover(project.path())
        .expect("service should discover")
        .run(request)
        .expect("monitor should recover the prior attempt and continue");
    assert_eq!(report.terminal_reason, MonitorTerminalReason::Passed);
    assert_eq!(report.attempts.len(), 2);
    assert_eq!(
        report.attempts[0].disposition,
        MonitorAttemptDisposition::Replayed
    );
    assert_eq!(
        report.attempts[1].disposition,
        MonitorAttemptDisposition::Executed
    );
    assert_eq!(project.marker_attempts(), 2);

    let cancelled = TestProject::new("recover-cancel", Fixture::CancelAfterFailure);
    let request = monitor_request(
        &cancelled,
        26,
        MonitorBounds::new(3, 1, 10_000).expect("bounds should validate"),
        Some(TestProject::cancellation_relative()),
    );
    execute_first_monitor_attempt(&cancelled, &request);
    assert!(cancelled.cancellation_absolute().is_file());
    let report = MonitorService::discover(cancelled.path())
        .expect("service should discover")
        .run(request)
        .expect("monitor should replay evidence before observing cancellation");
    assert_eq!(report.terminal_reason, MonitorTerminalReason::Cancelled);
    assert_eq!(report.attempts.len(), 1);
    assert_eq!(
        report.attempts[0].disposition,
        MonitorAttemptDisposition::Replayed
    );
    assert_eq!(cancelled.marker_attempts(), 1);
}

#[test]
fn failed_monitor_stops_immediately_at_attempt_limit_with_complete_evidence() {
    let project = TestProject::new("attempt-limit", Fixture::Fail);
    let bounds = MonitorBounds::new(2, 1, 10_000).expect("bounds should validate");
    let report = MonitorService::discover(project.path())
        .expect("service should discover")
        .run(monitor_request(&project, 30, bounds, None))
        .expect("terminal check failures should return a report");
    assert_eq!(
        report.terminal_reason,
        MonitorTerminalReason::AttemptsExhausted
    );
    assert!(!report.is_success());
    assert_eq!(report.attempts.len(), 2);
    assert!(
        report
            .attempts
            .iter()
            .all(|attempt| attempt.outcome == CheckRunOutcome::UnexpectedExit)
    );
    assert_eq!(report.final_revision, project.base_revision + 4);
    assert_eq!(project.marker_attempts(), 2);
    assert_eq!(
        EvidenceStore::new(project.path())
            .list(&plan_id())
            .expect("evidence should list")
            .len(),
        2
    );
    assert!(summary_path(&project, 30).is_file());
}

#[test]
fn slow_process_reaches_deadline_under_derived_per_attempt_timeout() {
    let project = TestProject::new("deadline", Fixture::Sleep(500));
    let bounds = MonitorBounds::new(3, 50, 300).expect("bounds should validate");
    assert_eq!(bounds.check_timeout_milliseconds(), 66);
    let started = Instant::now();
    let report = MonitorService::discover(project.path())
        .expect("service should discover")
        .run(monitor_request(&project, 40, bounds, None))
        .expect("deadline should return a terminal report");
    assert_eq!(
        report.terminal_reason,
        MonitorTerminalReason::DeadlineReached
    );
    assert!(!report.attempts.is_empty());
    assert!(report.attempts.len() <= 3);
    assert!(
        report
            .attempts
            .iter()
            .all(|attempt| attempt.outcome == CheckRunOutcome::TimedOut)
    );
    assert!(report.elapsed_milliseconds >= 300);
    assert!(started.elapsed() < Duration::from_secs(10));
    assert_eq!(
        EvidenceStore::new(project.path())
            .list(&plan_id())
            .expect("evidence should list")
            .len(),
        report.attempts.len()
    );
}

#[test]
fn cancellation_records_zero_or_one_attempt_without_losing_evidence() {
    let project = TestProject::new("cancel-after", Fixture::CancelAfterFailure);
    let bounds = MonitorBounds::new(5, 50, 10_000).expect("bounds should validate");
    let request = monitor_request(
        &project,
        50,
        bounds,
        Some(TestProject::cancellation_relative()),
    );
    let report = MonitorService::discover(project.path())
        .expect("service should discover")
        .run(request.clone())
        .expect("cancellation should return a terminal report");
    assert_eq!(report.terminal_reason, MonitorTerminalReason::Cancelled);
    assert_eq!(report.attempts.len(), 1);
    assert_eq!(report.attempts[0].outcome, CheckRunOutcome::UnexpectedExit);
    assert_eq!(report.final_revision, project.base_revision + 2);
    assert_eq!(project.marker_attempts(), 1);
    assert!(project.cancellation_absolute().is_file());
    let replay = MonitorService::discover(project.path())
        .expect("service should discover")
        .run(request)
        .expect("cancelled summary should replay");
    assert_eq!(replay, report);
    assert_eq!(project.marker_attempts(), 1);

    let pre_cancelled = TestProject::new("cancel-before", Fixture::Fail);
    fs::write(pre_cancelled.cancellation_absolute(), "cancel\n")
        .expect("cancellation file should be written");
    let plan_path = pre_cancelled
        .path
        .join(".mino/plans")
        .join(plan_id().as_str())
        .join("plan.json");
    let events_path = plan_path.with_file_name("events.jsonl");
    let plan_before = fs::read(&plan_path).expect("plan bytes should read");
    let events_before = fs::read(&events_path).expect("events should read");
    let report = MonitorService::discover(pre_cancelled.path())
        .expect("service should discover")
        .run(monitor_request(
            &pre_cancelled,
            51,
            MonitorBounds::new(5, 50, 10_000).expect("bounds should validate"),
            Some(TestProject::cancellation_relative()),
        ))
        .expect("preexisting cancellation should return a report");
    assert_eq!(report.terminal_reason, MonitorTerminalReason::Cancelled);
    assert!(report.attempts.is_empty());
    assert_eq!(report.final_revision, pre_cancelled.base_revision);
    assert_eq!(
        fs::read(plan_path).expect("plan bytes should read"),
        plan_before
    );
    assert_eq!(
        fs::read(events_path).expect("events should read"),
        events_before
    );
    assert!(
        EvidenceStore::new(pre_cancelled.path())
            .list(&plan_id())
            .expect("evidence should list")
            .is_empty()
    );
}

#[test]
fn unsafe_cancellation_and_summary_paths_fail_before_plan_mutation() {
    let project = TestProject::new("unsafe-path", Fixture::Fail);
    let service = MonitorService::discover(project.path()).expect("service should discover");
    let bounds = MonitorBounds::new(2, 1, 10_000).expect("bounds should validate");
    let before = tree_snapshot(project.path());
    for path in [
        project.cancellation_absolute(),
        PathBuf::from("../stop.flag"),
    ] {
        let error = service
            .run(monitor_request(&project, 60, bounds, Some(path)))
            .expect_err("unsafe cancellation path should fail");
        assert_eq!(error.category(), ErrorCategory::IncompleteOrValidation);
        assert_eq!(tree_snapshot(project.path()), before);
    }

    fs::create_dir(project.path.join("stop-directory"))
        .expect("cancellation directory should be created");
    let before_directory = tree_snapshot(project.path());
    let error = service
        .run(monitor_request(
            &project,
            61,
            bounds,
            Some(PathBuf::from("stop-directory")),
        ))
        .expect_err("non-file cancellation path should fail");
    assert_eq!(error.category(), ErrorCategory::IncompleteOrValidation);
    assert_eq!(tree_snapshot(project.path()), before_directory);

    let cancellation_target = project.path.join("cancellation-target");
    let cancellation_link = project.path.join("cancellation-link");
    fs::write(&cancellation_target, "cancel\n").expect("cancellation target should be written");
    if create_file_symlink(&cancellation_target, &cancellation_link) {
        let plan_path = project
            .path
            .join(".mino/plans")
            .join(plan_id().as_str())
            .join("plan.json");
        let plan_before = fs::read(&plan_path).expect("plan should read");
        let error = service
            .run(monitor_request(
                &project,
                62,
                bounds,
                Some(PathBuf::from("cancellation-link")),
            ))
            .expect_err("symbolic cancellation file should fail");
        assert_eq!(error.category(), ErrorCategory::IncompleteOrValidation);
        assert_eq!(fs::read(plan_path).expect("plan should read"), plan_before);
        assert_eq!(project.marker_attempts(), 0);
    }

    let monitors = project
        .path
        .join(".mino/plans")
        .join(plan_id().as_str())
        .join("monitors");
    fs::write(&monitors, "not a directory\n").expect("unsafe monitor path should be written");
    let plan_path = monitors.with_file_name("plan.json");
    let plan_before = fs::read(&plan_path).expect("plan should read");
    let error = service
        .run(monitor_request(&project, 63, bounds, None))
        .expect_err("unsafe monitor directory should fail");
    assert_eq!(error.category(), ErrorCategory::DriftDetected);
    assert_eq!(fs::read(plan_path).expect("plan should read"), plan_before);
    assert_eq!(project.marker_attempts(), 0);
}

#[test]
fn symlinked_monitor_directory_cannot_publish_an_external_summary() {
    let project = TestProject::new("monitor-symlink", Fixture::Fail);
    let external = TestProject::new("monitor-symlink-external", Fixture::Fail);
    let monitors = project
        .path
        .join(".mino/plans")
        .join(plan_id().as_str())
        .join("monitors");
    let sentinel = external.path.join("sentinel.txt");
    fs::write(&sentinel, b"outside\n").expect("outside sentinel should be written");
    if !create_directory_symlink(external.path(), &monitors) {
        return;
    }
    let service = MonitorService::discover(project.path()).expect("service should discover");
    let bounds = MonitorBounds::new(1, 1, 10_000).expect("bounds should validate");

    let error = service
        .run(monitor_request(&project, 64, bounds, None))
        .expect_err("symlinked monitor directory must be rejected");
    assert_eq!(error.category(), ErrorCategory::DriftDetected);
    assert_eq!(project.marker_attempts(), 0);
    assert_eq!(
        fs::read(&sentinel).expect("outside sentinel should remain readable"),
        b"outside\n"
    );
    assert!(!external.path.join(request_id(64).as_str()).exists());
}

#[test]
fn cli_returns_success_for_pass_and_exit_six_with_terminal_failure_details() {
    let passing = TestProject::new("cli-pass", Fixture::PassAfter(1));
    let output = run_mino(&passing, &cli_arguments(&passing, 70, 2));
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let payload: Value = serde_json::from_slice(&output.stdout).expect("success should be JSON");
    assert_eq!(payload["terminal_reason"], "passed");
    assert_eq!(payload["attempts"].as_array().map(Vec::len), Some(1));

    let failing = TestProject::new("cli-fail", Fixture::Fail);
    let output = run_mino(&failing, &cli_arguments(&failing, 71, 2));
    assert_eq!(output.status.code(), Some(6));
    let payload: Value = serde_json::from_slice(&output.stdout).expect("failure should be JSON");
    assert_eq!(payload["error"]["code"], "check_failed");
    assert_eq!(payload["monitor"]["terminal_reason"], "attempts_exhausted");
    assert_eq!(
        payload["monitor"]["attempts"].as_array().map(Vec::len),
        Some(2)
    );
    assert_eq!(failing.marker_attempts(), 2);
}
