//! Acceptance tests for inert scheduler-neutral task handoff specifications.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use mino::ErrorCategory;
use mino::application::execution::ExecutionService;
use mino::application::monitor::MonitorBounds;
use mino::application::plan::PlanMutationRequest;
use mino::domain::{
    AcceptanceCriterion, Approval, CheckId, CriterionId, GitFlowConsent, GitReadiness,
    GitReadinessObservation, GitReadinessState, GitRepositoryMode, GitSetupDecision, Plan,
    PlanDraftSeed, PlanId, RequestId, Task, TaskId, Timestamp, VerificationCheck,
};
use mino::project::initialize;
use mino::render::{render_plan, write_projection};
use mino::schedule::{
    SCHEDULE_SPEC_KIND, ScheduleSpecRequest, ScheduleSpecService, ScheduledTaskSpec,
};
use mino::store::{MutationRequest, PlanStore, canonical_json_bytes, sha256_digest};
use serde_json::Value;

static NEXT_TEMP_DIRECTORY: AtomicU64 = AtomicU64::new(1);

struct TestProject {
    path: PathBuf,
    revision: u64,
}

impl TestProject {
    fn new(label: &str) -> Self {
        let sequence = NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "mino-schedule-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("temporary project should be created");
        fs::write(
            path.join("Cargo.toml"),
            "[package]\nname = \"schedule-fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .expect("fixture manifest should be written");
        fs::create_dir(path.join("src")).expect("fixture source directory should be created");
        fs::write(path.join("src/lib.rs"), "pub fn fixture() -> u8 { 1 }\n")
            .expect("fixture source should be written");
        fs::create_dir(path.join("scheduled-results")).expect("result parent should be created");
        initialize(&path).expect("temporary project should initialize");
        let path = path.canonicalize().expect("project root should resolve");
        let revision = create_started_plan(&path);
        Self { path, revision }
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
                .is_some_and(|name| name.to_string_lossy().starts_with("mino-schedule-"))
        {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

fn create_started_plan(root: &Path) -> u64 {
    let global = VerificationCheck::new(
        check_id("GLOBAL-CHECK"),
        vec!["cargo".to_owned(), "test".to_owned(), "--lib".to_owned()],
        ".",
        0,
        true,
    );
    let mut plan = Plan::from_draft_seed(
        PlanDraftSeed {
            id: plan_id(),
            name: "Scheduled handoff contract".to_owned(),
            trigger: "durable".to_owned(),
            original_request: "Emit an inert complete scheduled task handoff.".to_owned(),
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
    let mut task = Task::new(task_id(), "Emit one scheduler handoff", Vec::new());
    task.add_acceptance_criterion(AcceptanceCriterion::new(
        criterion_id(),
        "The specification is complete and inert",
    ))
    .expect("criterion should be added");
    task.add_verification_check(VerificationCheck::new(
        check_id("TASK-CHECK"),
        vec!["cargo".to_owned(), "test".to_owned(), "--lib".to_owned()],
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
            "chat:schedule-approval",
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
            .expect("store request should validate"),
            mutation,
        )
        .expect("plan mutation should persist");
}

fn schedule_request(project: &TestProject) -> ScheduleSpecRequest {
    ScheduleSpecRequest {
        plan_id: plan_id(),
        expected_revision: project.revision,
        check_id: check_id("TASK-CHECK"),
        execution_request_id: request_id(20),
        actor: "codex".to_owned(),
        execution_environment: "local-test".to_owned(),
        monitor_bounds: MonitorBounds::new(3, 1_000, 10_000)
            .expect("monitor bounds should validate"),
        trigger_at: Timestamp::parse("2026-07-27T00:00:00Z").expect("trigger should validate"),
        expires_at: Timestamp::parse("2026-07-27T00:01:00Z").expect("expiry should validate"),
        max_dispatch_attempts: 2,
        dispatch_retry_milliseconds: 5_000,
        success_condition: "The planned check reports passed".to_owned(),
        stop_condition: "Stop after any terminal monitor report or expiry".to_owned(),
        failure_handling: "Preserve the report and notify the plan owner".to_owned(),
        result_destination: PathBuf::from("scheduled-results/result.json"),
    }
}

fn plan_id() -> PlanId {
    PlanId::parse("2026-07-26-schedule-contract").expect("plan ID should be valid")
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
    RequestId::parse(format!("90000000-0000-0000-0000-{sequence:012x}"))
        .expect("request ID should be valid")
}

fn timestamp(minute: u8) -> Timestamp {
    Timestamp::parse(format!("2026-07-26T09:{minute:02}:00Z")).expect("timestamp should be valid")
}

fn projection_relative() -> &'static str {
    "docs/plan/schedule-contract.md"
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

fn cli_arguments(project: &TestProject) -> Vec<String> {
    vec![
        "exec".to_owned(),
        "schedule".to_owned(),
        "spec".to_owned(),
        "--plan".to_owned(),
        plan_id().to_string(),
        "--expect-revision".to_owned(),
        project.revision.to_string(),
        "--check".to_owned(),
        "TASK-CHECK".to_owned(),
        "--execution-request-id".to_owned(),
        request_id(20).to_string(),
        "--actor".to_owned(),
        "codex".to_owned(),
        "--execution-environment".to_owned(),
        "local-test".to_owned(),
        "--max-attempts".to_owned(),
        "3".to_owned(),
        "--interval-milliseconds".to_owned(),
        "1000".to_owned(),
        "--deadline-milliseconds".to_owned(),
        "10000".to_owned(),
        "--trigger-at".to_owned(),
        "2026-07-27T00:00:00Z".to_owned(),
        "--expires-at".to_owned(),
        "2026-07-27T00:01:00Z".to_owned(),
        "--max-dispatch-attempts".to_owned(),
        "2".to_owned(),
        "--dispatch-retry-milliseconds".to_owned(),
        "5000".to_owned(),
        "--success-condition".to_owned(),
        "The planned check reports passed".to_owned(),
        "--stop-condition".to_owned(),
        "Stop after any terminal monitor report or expiry".to_owned(),
        "--failure-handling".to_owned(),
        "Preserve the report and notify the plan owner".to_owned(),
        "--result-destination".to_owned(),
        "scheduled-results/result.json".to_owned(),
    ]
}

fn assert_schedule_schema() {
    let schema = ScheduledTaskSpec::schema();
    let schema_bytes = canonical_json_bytes(&schema).expect("schema should canonicalize");
    assert_eq!(
        sha256_digest(&schema_bytes),
        "sha256:64cd28195b9f142ff0588d360811078feb8a198afba91840cf7510c71d515f8c"
    );
    let schema_value: Value = serde_json::from_slice(&schema_bytes).expect("schema should parse");
    assert_eq!(schema_value["title"], "ScheduledTaskSpec");
    for required in [
        "spec_kind",
        "spec_digest",
        "project",
        "execution",
        "trigger",
        "outcome",
        "authorization",
        "emission_side_effects",
    ] {
        assert!(
            schema_value["required"]
                .as_array()
                .is_some_and(|fields| fields.iter().any(|field| field == required)),
            "schema is missing required field {required}"
        );
    }
}

#[test]
fn schema_and_generated_spec_are_complete_canonical_and_digest_bound() {
    assert_schedule_schema();
    let project = TestProject::new("complete");
    let before = tree_snapshot(project.path());
    let service = ScheduleSpecService::discover(project.path()).expect("service should discover");
    let spec = service
        .generate(schedule_request(&project))
        .expect("schedule spec should generate");
    assert_eq!(spec.spec_kind, SCHEDULE_SPEC_KIND);
    assert!(spec.verify_digest().expect("digest should verify"));
    assert_eq!(spec.project.plan_id, plan_id());
    assert_eq!(spec.project.plan_revision, project.revision);
    assert_eq!(spec.project.task_id, Some(task_id()));
    assert!(spec.project.plan_digest.starts_with("sha256:"));
    assert!(spec.project.projection_digest.starts_with("sha256:"));
    assert!(spec.project.check_digest.starts_with("sha256:"));
    assert_eq!(spec.execution.instruction_kind, "command");
    assert_eq!(spec.execution.environment, "local-test");
    assert_eq!(
        spec.execution.working_directory,
        project.path.to_string_lossy()
    );
    assert_eq!(spec.execution.check_working_directory, ".");
    assert_eq!(spec.execution.expected_check_exit_code, 0);
    assert_eq!(spec.execution.monitor.max_attempts, 3);
    assert_eq!(spec.execution.monitor.interval_milliseconds, 1_000);
    assert_eq!(spec.execution.monitor.deadline_milliseconds, 10_000);
    assert_eq!(spec.execution.monitor.check_timeout_milliseconds, 2_666);
    assert_eq!(
        spec.execution.argv,
        vec![
            "mino",
            "--root",
            project.path.to_string_lossy().as_ref(),
            "--format",
            "json",
            "--no-input",
            "exec",
            "check",
            "monitor",
            "--plan",
            plan_id().as_str(),
            "--check",
            "TASK-CHECK",
            "--expect-revision",
            &project.revision.to_string(),
            "--request-id",
            request_id(20).as_str(),
            "--actor",
            "codex",
            "--max-attempts",
            "3",
            "--interval-milliseconds",
            "1000",
            "--deadline-milliseconds",
            "10000",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>()
    );
    assert_eq!(spec.trigger.trigger_kind, "once");
    assert_eq!(spec.trigger.window_milliseconds, 60_000);
    assert_eq!(spec.trigger.max_dispatch_attempts, 2);
    assert_eq!(spec.trigger.dispatch_retry_milliseconds, 5_000);
    assert_eq!(spec.trigger.required_budget_milliseconds, 25_000);
    assert_eq!(
        spec.outcome.result_destination,
        "scheduled-results/result.json"
    );
    assert!(spec.authorization.external_creation_required);
    assert!(!spec.authorization.authorization_granted);
    assert!(!spec.emission_side_effects.scheduler_mutated);
    assert!(!spec.emission_side_effects.network_accessed);
    assert!(!spec.emission_side_effects.mino_state_mutated);
    let canonical = spec.canonical_bytes().expect("spec should canonicalize");
    assert!(canonical.ends_with(b"\n"));
    let decoded: ScheduledTaskSpec =
        serde_json::from_slice(&canonical).expect("canonical spec should decode");
    assert_eq!(decoded, spec);
    let second = service
        .generate(schedule_request(&project))
        .expect("identical schedule spec should regenerate");
    assert_eq!(second, spec);
    assert_eq!(tree_snapshot(project.path()), before);
    assert!(!project.path.join("scheduled-results/result.json").exists());
}

#[test]
fn schedule_bounds_text_identity_and_revision_are_strict() {
    let project = TestProject::new("validation");
    let service = ScheduleSpecService::discover(project.path()).expect("service should discover");
    let before = tree_snapshot(project.path());

    let mut request = schedule_request(&project);
    request.expected_revision += 1;
    assert_eq!(
        service
            .generate(request)
            .expect_err("stale revision should fail")
            .category(),
        ErrorCategory::RevisionConflict
    );

    let mut request = schedule_request(&project);
    request.check_id = check_id("MISSING-CHECK");
    assert_eq!(
        service
            .generate(request)
            .expect_err("missing check should fail")
            .category(),
        ErrorCategory::IncompleteOrValidation
    );

    let invalid_requests = [
        {
            let mut request = schedule_request(&project);
            request.expires_at =
                Timestamp::parse("2026-07-26T23:59:59Z").expect("timestamp should validate");
            request
        },
        {
            let mut request = schedule_request(&project);
            request.expires_at =
                Timestamp::parse("2026-08-28T00:00:00Z").expect("timestamp should validate");
            request
        },
        {
            let mut request = schedule_request(&project);
            request.expires_at =
                Timestamp::parse("2026-07-27T00:00:05Z").expect("timestamp should validate");
            request
        },
        {
            let mut request = schedule_request(&project);
            request.max_dispatch_attempts = 0;
            request
        },
        {
            let mut request = schedule_request(&project);
            request.max_dispatch_attempts = 101;
            request
        },
        {
            let mut request = schedule_request(&project);
            request.dispatch_retry_milliseconds = 0;
            request
        },
        {
            let mut request = schedule_request(&project);
            request.dispatch_retry_milliseconds = 86_400_001;
            request
        },
        {
            let mut request = schedule_request(&project);
            request.success_condition = " \t".to_owned();
            request
        },
        {
            let mut request = schedule_request(&project);
            request.stop_condition = "stop\nnow".to_owned();
            request
        },
        {
            let mut request = schedule_request(&project);
            request.failure_handling = String::new();
            request
        },
        {
            let mut request = schedule_request(&project);
            request.execution_environment = "local\nremote".to_owned();
            request
        },
    ];
    for request in invalid_requests {
        assert_eq!(
            service
                .generate(request)
                .expect_err("invalid scheduled-task input should fail")
                .category(),
            ErrorCategory::IncompleteOrValidation
        );
    }
    assert_eq!(tree_snapshot(project.path()), before);
}

#[test]
fn result_destination_rejects_managed_escaping_or_non_regular_paths() {
    let project = TestProject::new("paths");
    let service = ScheduleSpecService::discover(project.path()).expect("service should discover");
    let invalid_paths = [
        project.path.join("absolute-result.json"),
        PathBuf::from("../outside-result.json"),
        PathBuf::from(".mino/result.json"),
        PathBuf::from("docs/plan/result.json"),
        PathBuf::from("missing-parent/result.json"),
        PathBuf::from("scheduled-results"),
    ];
    for path in invalid_paths {
        let before = tree_snapshot(project.path());
        let mut request = schedule_request(&project);
        request.result_destination = path;
        let error = service
            .generate(request)
            .expect_err("unsafe result path should fail");
        assert!(matches!(
            error.category(),
            ErrorCategory::IncompleteOrValidation | ErrorCategory::PolicyViolation
        ));
        assert_eq!(tree_snapshot(project.path()), before);
    }

    let existing = project.path.join("scheduled-results/existing.json");
    fs::write(&existing, "existing\n").expect("existing result should be written");
    let before = tree_snapshot(project.path());
    let mut request = schedule_request(&project);
    request.result_destination = PathBuf::from("scheduled-results/existing.json");
    let spec = service
        .generate(request)
        .expect("existing regular destination should be described without writing");
    assert_eq!(
        spec.outcome.result_destination,
        "scheduled-results/existing.json"
    );
    assert_eq!(tree_snapshot(project.path()), before);

    let outside_parent = project.path.join("outside-parent");
    let linked_parent = project.path.join("linked-parent");
    fs::create_dir(&outside_parent).expect("symlink target should be created");
    if create_directory_symlink(&outside_parent, &linked_parent) {
        let before = tree_snapshot(project.path());
        let mut request = schedule_request(&project);
        request.result_destination = PathBuf::from("linked-parent/result.json");
        assert_eq!(
            service
                .generate(request)
                .expect_err("symbolic parent should fail")
                .category(),
            ErrorCategory::IncompleteOrValidation
        );
        assert_eq!(tree_snapshot(project.path()), before);
    }
}

#[test]
fn cli_emits_only_the_spec_and_rejects_missing_or_stale_context() {
    let project = TestProject::new("cli");
    let before = tree_snapshot(project.path());
    let arguments = cli_arguments(&project);
    let output = run_mino(&project, &arguments);
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).expect("success should be JSON");
    assert_eq!(value["spec_kind"], SCHEDULE_SPEC_KIND);
    assert_eq!(value["project"]["plan_revision"], project.revision);
    assert_eq!(value["execution"]["instruction_kind"], "command");
    assert_eq!(value["authorization"]["external_creation_required"], true);
    assert_eq!(value["authorization"]["authorization_granted"], false);
    assert_eq!(value["emission_side_effects"]["scheduler_mutated"], false);
    assert_eq!(value["emission_side_effects"]["network_accessed"], false);
    assert_eq!(value["emission_side_effects"]["mino_state_mutated"], false);
    assert_eq!(value["next_actions"], serde_json::json!([]));
    assert_eq!(tree_snapshot(project.path()), before);

    let mut missing = cli_arguments(&project);
    let position = missing
        .iter()
        .position(|value| value == "--failure-handling")
        .expect("failure flag should exist");
    missing.drain(position..=position + 1);
    let output = run_mino(&project, &missing);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(tree_snapshot(project.path()), before);

    let mut stale = cli_arguments(&project);
    let position = stale
        .iter()
        .position(|value| value == "--expect-revision")
        .expect("revision flag should exist");
    stale[position + 1] = (project.revision + 1).to_string();
    let output = run_mino(&project, &stale);
    assert_eq!(output.status.code(), Some(3));
    let value: Value = serde_json::from_slice(&output.stdout).expect("failure should be JSON");
    assert_eq!(value["error"]["code"], "revision_conflict");
    assert_eq!(tree_snapshot(project.path()), before);
}

#[test]
fn drifted_projection_is_reported_without_repair_or_scheduler_mutation() {
    let project = TestProject::new("drift");
    let projection = project.path.join(projection_relative());
    fs::write(&projection, "drifted projection\n").expect("projection should be drifted");
    let before = tree_snapshot(project.path());
    let error = ScheduleSpecService::discover(project.path())
        .expect("service should discover")
        .generate(schedule_request(&project))
        .expect_err("projection drift should fail");
    assert_eq!(error.category(), ErrorCategory::DriftDetected);
    assert_eq!(tree_snapshot(project.path()), before);
}
