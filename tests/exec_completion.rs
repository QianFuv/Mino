//! Acceptance tests for criterion, File Map, task, and plan completion gates.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use mino::ErrorCategory;
use mino::application::completion::CompletionService;
use mino::application::execution::ExecutionService;
use mino::application::plan::PlanMutationRequest;
use mino::domain::{
    AcceptanceCriterion, Approval, CheckId, CheckStatus, CheckpointKind, CriterionId,
    CriterionStatus, EvidenceId, EvidenceType, FileChange, FileMapEntry, GitFlowConsent,
    GitReadiness, Plan, PlanDraftSeed, PlanId, PlanStatus, RequestId, Task, TaskId, TaskStatus,
    Timestamp, VerificationCheck,
};
use mino::evidence::{AddEvidenceRequest, EvidenceRequestContext, EvidenceSource, EvidenceStore};
use mino::git::matches_file_map_path;
use mino::project::initialize;
use mino::render::{render_plan, write_projection};
use mino::runner::Redactor;
use mino::store::{MutationRequest, PlanStore};
use serde_json::Value;

static NEXT_TEMP_DIRECTORY: AtomicU64 = AtomicU64::new(1);

struct TestProject {
    path: PathBuf,
    base_revision: u64,
}

impl TestProject {
    fn new(label: &str, task_mode: &str) -> Self {
        let sequence = NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "mino-completion-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("temporary project should be created");
        fs::write(
            path.join("Cargo.toml"),
            "[package]\nname = \"completion-fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .expect("fixture manifest should be written");
        fs::create_dir(path.join("src")).expect("fixture source should be created");
        fs::write(path.join("src/lib.rs"), "pub fn fixture() -> u8 { 1 }\n")
            .expect("fixture library should be written");
        fs::write(path.join(".gitignore"), ".mino/\ndocs/plan/\n*.pdb\n")
            .expect("ignore rules should be written");
        let helper = compile_helper(&path);
        initialize(&path).expect("Mino project should initialize");
        initialize_git(&path, &helper);
        let path = path.canonicalize().expect("project root should resolve");
        let base_revision = create_approved_plan(&path, &helper, task_mode);
        Self {
            path,
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
                .is_some_and(|name| name.to_string_lossy().starts_with("mino-completion-"))
        {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

fn compile_helper(root: &Path) -> PathBuf {
    let source = root.join("completion-helper.rs");
    let executable = root.join(if cfg!(windows) {
        "completion-helper.exe"
    } else {
        "completion-helper"
    });
    fs::write(
        &source,
        r#"use std::env;

fn main() {
    match env::args().nth(1).as_deref() {
        Some("pass") => println!("planned success"),
        Some("fail") => {
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

fn initialize_git(root: &Path, helper: &Path) {
    git(root, &["init"]);
    git(root, &["config", "user.name", "Mino Test"]);
    git(root, &["config", "user.email", "mino@example.invalid"]);
    let helper_name = helper
        .file_name()
        .and_then(|name| name.to_str())
        .expect("helper file name should be UTF-8");
    git(
        root,
        &[
            "add",
            "--",
            ".gitignore",
            ".agents/skills/mino",
            "Cargo.toml",
            "src/lib.rs",
            "completion-helper.rs",
            helper_name,
        ],
    );
    git(root, &["commit", "-m", "test: create completion fixture"]);
}

fn git(root: &Path, arguments: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(arguments)
        .output()
        .expect("Git should run");
    assert!(
        output.status.success(),
        "git stdout: {}\ngit stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn create_approved_plan(root: &Path, helper: &Path, task_mode: &str) -> u64 {
    let plan = Plan::from_draft_seed(
        PlanDraftSeed {
            id: plan_id(),
            name: "Completion contract".to_owned(),
            trigger: "durable".to_owned(),
            original_request: "Complete only with compatible evidence.".to_owned(),
            branch: Some("master".to_owned()),
            markdown_path: projection_relative().to_owned(),
            git_readiness: GitReadiness::detected(
                "Present",
                "Clean",
                Some("master".to_owned()),
                None,
                "Clean",
                false,
            ),
            standards: Vec::new(),
            verification_plan: vec![VerificationCheck::new(
                check_id("GLOBAL-CHECK"),
                helper_command(helper, "pass"),
                ".",
                0,
                true,
            )],
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
    let mut task = Task::new(task_id(), "Implement the feature", Vec::new());
    task.add_acceptance_criterion(AcceptanceCriterion::new(
        criterion_id(),
        "The feature is observable",
    ))
    .expect("criterion should be added");
    task.add_verification_check(VerificationCheck::new(
        check_id("TASK-CHECK"),
        helper_command(helper, task_mode),
        ".",
        0,
        true,
    ))
    .expect("task check should be added");
    task.add_file_map_entry(FileMapEntry::new(
        "src/feature.rs",
        FileChange::Create,
        "Own the feature implementation",
        task_id(),
    ))
    .expect("file map should be added");
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
            "chat:completion-approval",
            timestamp(4),
            GitFlowConsent::Disabled,
        ))
    });
    let plan = store
        .load_plan(&plan_id())
        .expect("approved plan should load");
    write_projection(
        &root.join(projection_relative()),
        &render_plan(&plan).expect("plan should render"),
        None,
    )
    .expect("projection should write");
    plan.revision()
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
            .expect("mutation request should be valid"),
            mutation,
        )
        .expect("plan mutation should persist");
}

fn helper_command(helper: &Path, mode: &str) -> Vec<String> {
    vec![helper.to_string_lossy().into_owned(), mode.to_owned()]
}

fn plan_id() -> PlanId {
    PlanId::parse("2026-07-25-completion-contract").expect("plan ID should be valid")
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
    RequestId::parse(format!("50000000-0000-0000-0000-{sequence:012x}"))
        .expect("request ID should be valid")
}

fn timestamp(minute: u8) -> Timestamp {
    Timestamp::parse(format!("2026-07-25T15:{minute:02}:00Z")).expect("timestamp should be valid")
}

fn projection_relative() -> &'static str {
    "docs/plan/completion-contract.md"
}

fn mutation(expected_revision: u64, sequence: u64, command: Vec<String>) -> PlanMutationRequest {
    PlanMutationRequest {
        plan_id: plan_id(),
        expected_revision,
        request_id: request_id(sequence),
        actor: "codex".to_owned(),
        command,
        updated_at: timestamp(u8::try_from(sequence).unwrap_or(59).min(59)),
    }
}

fn start_active_task(project: &TestProject, sequence: u64) -> u64 {
    ExecutionService::discover(project.path())
        .expect("execution service should discover")
        .start_task(
            mutation(
                project.base_revision,
                sequence,
                vec!["mino".to_owned(), "exec".to_owned(), "start".to_owned()],
            ),
            task_id(),
        )
        .expect("task should start")
        .revision
}

#[test]
fn cli_completion_flow_rejects_scope_drift_then_enters_review() {
    let project = TestProject::new("flow", "pass");
    let completed = complete_task_after_scope_and_freshness_rejections(&project);
    assert_final_and_review_freshness(&project, &completed);
}

fn complete_task_after_scope_and_freshness_rejections(project: &TestProject) -> Value {
    let started = parse_success(&run_mino(
        project,
        &mutation_arguments("start", project.base_revision, 10, &["--task", "T1"]),
    ));
    assert_eq!(started["revision"], project.base_revision + 1);
    let task_check = parse_success(&run_mino(
        project,
        &check_arguments(project.base_revision + 1, 11, "TASK-CHECK"),
    ));
    assert_eq!(task_check["evidence"]["id"], "E0001");
    let criterion = parse_success(&run_mino(
        project,
        &criterion_arguments(project.base_revision + 3, 12, "E0001"),
    ));
    assert_eq!(criterion["revision"], project.base_revision + 4);

    fs::write(project.path().join("outside.txt"), "outside\n")
        .expect("outside change should be written");
    let rejected = run_mino(
        project,
        &mutation_arguments("complete", project.base_revision + 4, 13, &["--task", "T1"]),
    );
    assert_eq!(rejected.status.code(), Some(5));
    let rejected: Value = serde_json::from_slice(&rejected.stdout).expect("failure should be JSON");
    assert_eq!(rejected["error"]["code"], "policy_violation");
    assert_eq!(rejected["missing"], serde_json::json!(["outside.txt"]));
    assert_eq!(rejected["next_actions"][0]["id"], "exec.checkpoint");
    fs::remove_file(project.path().join("outside.txt")).expect("outside change should be removed");
    fs::write(
        project.path().join("src/feature.rs"),
        "pub fn feature() -> u8 { 2 }\n",
    )
    .expect("planned change should be written");

    let stale = run_mino(
        project,
        &mutation_arguments("complete", project.base_revision + 4, 14, &["--task", "T1"]),
    );
    assert_eq!(stale.status.code(), Some(2));
    let stale: Value = serde_json::from_slice(&stale.stdout).expect("failure should be JSON");
    assert_eq!(stale["error"]["code"], "incomplete_or_validation");
    assert_eq!(stale["missing"], serde_json::json!(["TASK-CHECK"]));
    let stale_plan = PlanStore::new(project.path())
        .load_plan(&plan_id())
        .expect("stale transition should persist");
    assert_eq!(stale_plan.revision(), project.base_revision + 5);
    assert_eq!(
        stale_plan
            .task(&task_id())
            .expect("task should exist")
            .verification_checks()[0]
            .status(),
        CheckStatus::Stale
    );
    assert_eq!(
        stale_plan
            .task(&task_id())
            .expect("task should exist")
            .acceptance_criteria()[0]
            .status(),
        CriterionStatus::Pending
    );
    let rerun = parse_success(&run_mino(
        project,
        &check_arguments(project.base_revision + 5, 15, "TASK-CHECK"),
    ));
    assert_eq!(rerun["evidence"]["id"], "E0002");
    let rebound = parse_success(&run_mino(
        project,
        &criterion_arguments(project.base_revision + 7, 16, "E0002"),
    ));
    let completed = parse_success(&run_mino(
        project,
        &mutation_arguments("complete", result_revision(&rebound), 17, &["--task", "T1"]),
    ));
    assert_eq!(completed["revision"], project.base_revision + 9);
    assert_eq!(completed["next_actions"][0]["id"], "exec.check.run");
    completed
}

fn assert_final_and_review_freshness(project: &TestProject, completed: &Value) {
    let global = parse_success(&run_mino(
        project,
        &check_arguments(result_revision(completed), 18, "GLOBAL-CHECK"),
    ));
    assert_eq!(global["evidence"]["id"], "E0003");
    assert_eq!(global["next_actions"][0]["id"], "exec.finish");
    fs::write(
        project.path().join("src/feature.rs"),
        "pub fn feature() -> u8 { 3 }\n",
    )
    .expect("post-global change should be written");
    let stale_finish = run_mino(
        project,
        &mutation_arguments("finish", project.base_revision + 11, 19, &[]),
    );
    assert_eq!(stale_finish.status.code(), Some(2));
    let stale_finish: Value =
        serde_json::from_slice(&stale_finish.stdout).expect("failure should be JSON");
    assert_eq!(
        stale_finish["missing"],
        serde_json::json!(["GLOBAL-CHECK", "TASK-CHECK"])
    );
    let stale_plan = PlanStore::new(project.path())
        .load_plan(&plan_id())
        .expect("stale final evidence should persist");
    assert_eq!(stale_plan.revision(), project.base_revision + 12);
    assert_eq!(
        stale_plan
            .task(&task_id())
            .expect("task should exist")
            .status(),
        TaskStatus::Ready
    );
    let restarted = parse_success(&run_mino(
        project,
        &mutation_arguments("start", project.base_revision + 12, 20, &["--task", "T1"]),
    ));
    let rechecked = parse_success(&run_mino(
        project,
        &check_arguments(result_revision(&restarted), 21, "TASK-CHECK"),
    ));
    let recriterion = parse_success(&run_mino(
        project,
        &criterion_arguments(result_revision(&rechecked), 22, "E0004"),
    ));
    let recompleted = parse_success(&run_mino(
        project,
        &mutation_arguments(
            "complete",
            result_revision(&recriterion),
            23,
            &["--task", "T1"],
        ),
    ));
    let reglobal = parse_success(&run_mino(
        project,
        &check_arguments(result_revision(&recompleted), 24, "GLOBAL-CHECK"),
    ));
    assert_eq!(reglobal["evidence"]["id"], "E0005");
    let finish_arguments = mutation_arguments("finish", result_revision(&reglobal), 25, &[]);
    let finished = parse_success(&run_mino(project, &finish_arguments));
    assert_eq!(finished["status"], "Review");
    fs::remove_file(project.path().join(projection_relative()))
        .expect("projection should be removed for replay recovery");
    let replay = parse_success(&run_mino(project, &finish_arguments));
    assert_eq!(replay["replayed"], true);
    assert!(project.path().join(projection_relative()).exists());
    assert_eq!(
        PlanStore::new(project.path())
            .load_plan(&plan_id())
            .expect("plan should load")
            .status(),
        PlanStatus::Review
    );
    fs::write(
        project.path().join("src/feature.rs"),
        "pub fn feature() -> u8 { 4 }\n",
    )
    .expect("post-review change should be written");
    let mut accept_arguments = vec!["review".to_owned(), "accept".to_owned()];
    accept_arguments.extend(common_mutation_arguments(result_revision(&finished), 26));
    accept_arguments.extend(["--approval-ref".to_owned(), "chat:stale-review".to_owned()]);
    let stale_review = run_mino(project, &accept_arguments);
    assert_eq!(stale_review.status.code(), Some(2));
    let reopened = PlanStore::new(project.path())
        .load_plan(&plan_id())
        .expect("review drift should persist");
    assert_eq!(reopened.status(), PlanStatus::InProgress);
    assert_eq!(
        reopened
            .task(&task_id())
            .expect("task should exist")
            .status(),
        TaskStatus::Ready
    );
}

#[test]
fn failed_check_evidence_cannot_satisfy_a_criterion() {
    let project = TestProject::new("failed", "fail");
    let started = start_active_task(&project, 20);
    let execution = ExecutionService::discover(project.path()).expect("service should discover");
    let report = execution
        .run_check(
            &mutation(
                started,
                21,
                vec!["mino".to_owned(), "exec".to_owned(), "check".to_owned()],
            ),
            &check_id("TASK-CHECK"),
        )
        .expect("failed process should still produce a report");
    assert!(!report.is_success());
    let current_revision = report.plan().revision;
    let error = CompletionService::discover(project.path())
        .expect("completion service should discover")
        .pass_criterion(
            mutation(
                current_revision,
                22,
                vec!["mino".to_owned(), "exec".to_owned(), "criterion".to_owned()],
            ),
            criterion_id(),
            report.evidence().id().clone(),
        )
        .expect_err("failed check evidence should be rejected");
    assert_eq!(error.category(), ErrorCategory::PolicyViolation);
    let plan = PlanStore::new(project.path())
        .load_plan(&plan_id())
        .expect("plan should load");
    assert_eq!(
        plan.task(&task_id())
            .expect("task should exist")
            .acceptance_criteria()[0]
            .status(),
        CriterionStatus::Pending
    );
}

#[test]
fn incomplete_evidence_and_unresolved_deviation_never_complete_a_task() {
    let project = TestProject::new("gates", "pass");
    let started = start_active_task(&project, 40);
    let completion = CompletionService::discover(project.path()).expect("service should discover");
    let incomplete = completion
        .complete_task(
            mutation(
                started,
                41,
                vec!["mino".to_owned(), "exec".to_owned(), "complete".to_owned()],
            ),
            task_id(),
        )
        .expect_err("pending verification should block completion");
    assert_eq!(incomplete.category(), ErrorCategory::IncompleteOrValidation);
    let execution = ExecutionService::discover(project.path()).expect("service should discover");
    fs::write(
        project.path().join("src/feature.rs"),
        "pub fn feature() {}\n",
    )
    .expect("planned path should be written before verification");
    let check = execution
        .run_check(
            &mutation(
                started,
                42,
                vec!["mino".to_owned(), "exec".to_owned(), "check".to_owned()],
            ),
            &check_id("TASK-CHECK"),
        )
        .expect("check should pass");
    let missing_criterion = completion
        .complete_task(
            mutation(
                check.plan().revision,
                43,
                vec!["mino".to_owned(), "exec".to_owned(), "complete".to_owned()],
            ),
            task_id(),
        )
        .expect_err("pending criterion should block completion");
    assert_eq!(
        missing_criterion.category(),
        ErrorCategory::IncompleteOrValidation
    );
    let criterion = completion
        .pass_criterion(
            mutation(
                check.plan().revision,
                44,
                vec!["mino".to_owned(), "exec".to_owned(), "criterion".to_owned()],
            ),
            criterion_id(),
            check.evidence().id().clone(),
        )
        .expect("criterion should bind");
    let checkpoint = execution
        .checkpoint(
            mutation(
                criterion.revision,
                45,
                vec![
                    "mino".to_owned(),
                    "exec".to_owned(),
                    "checkpoint".to_owned(),
                ],
            ),
            task_id(),
            CheckpointKind::Deviation,
            "An undeclared file would be required".to_owned(),
        )
        .expect("deviation should record");
    let deviation = completion
        .complete_task(
            mutation(
                checkpoint.revision,
                46,
                vec!["mino".to_owned(), "exec".to_owned(), "complete".to_owned()],
            ),
            task_id(),
        )
        .expect_err("unresolved deviation should block completion");
    assert_eq!(deviation.category(), ErrorCategory::IncompleteOrValidation);
    assert_eq!(
        PlanStore::new(project.path())
            .load_plan(&plan_id())
            .expect("plan should load")
            .revision(),
        checkpoint.revision
    );
}

fn result_revision(value: &Value) -> u64 {
    value["revision"]
        .as_u64()
        .or_else(|| value["plan"]["revision"].as_u64())
        .expect("operation result should include a revision")
}

#[test]
fn superseded_exception_is_rejected_and_its_correction_is_accepted() {
    let project = TestProject::new("exception", "pass");
    let revision = start_active_task(&project, 30);
    let store = EvidenceStore::new(project.path());
    let first = exception_request(revision, 31, None);
    let first = store
        .add(&first, &Redactor::default())
        .expect("first exception should persist")
        .evidence()
        .clone();
    let correction = exception_request(revision, 32, Some(first.id().clone()));
    let correction = store
        .add(&correction, &Redactor::default())
        .expect("correction should persist")
        .evidence()
        .clone();
    let completion = CompletionService::discover(project.path()).expect("service should discover");
    let rejected = completion
        .pass_criterion(
            mutation(
                revision,
                33,
                vec!["mino".to_owned(), "exec".to_owned(), "criterion".to_owned()],
            ),
            criterion_id(),
            first.id().clone(),
        )
        .expect_err("superseded evidence should be rejected");
    assert_eq!(rejected.category(), ErrorCategory::IncompleteOrValidation);
    completion
        .pass_criterion(
            mutation(
                revision,
                34,
                vec!["mino".to_owned(), "exec".to_owned(), "criterion".to_owned()],
            ),
            criterion_id(),
            correction.id().clone(),
        )
        .expect("corrected exception should bind");
    let plan = PlanStore::new(project.path())
        .load_plan(&plan_id())
        .expect("plan should load");
    assert_eq!(
        plan.task(&task_id())
            .expect("task should exist")
            .acceptance_criteria()[0]
            .status(),
        CriterionStatus::AcceptedException
    );
}

#[test]
fn file_map_matching_is_narrow_and_rejects_traversal() {
    assert!(matches_file_map_path("src/**", "src/a/b.rs"));
    assert!(matches_file_map_path("src/*.rs", "src/lib.rs"));
    assert!(!matches_file_map_path("src/*.rs", "src/a/lib.rs"));
    assert!(!matches_file_map_path("../src/**", "src/lib.rs"));
    assert!(!matches_file_map_path("src/**", "../src/lib.rs"));
}

fn exception_request(
    revision: u64,
    sequence: u64,
    supersedes: Option<EvidenceId>,
) -> AddEvidenceRequest {
    let context = EvidenceRequestContext::new(
        plan_id(),
        revision,
        request_id(sequence),
        "reviewer",
        vec!["mino".to_owned(), "evidence".to_owned(), "add".to_owned()],
        timestamp(u8::try_from(sequence).unwrap_or(59).min(59)),
    )
    .expect("evidence context should be valid");
    let mut request = AddEvidenceRequest::new(
        context,
        EvidenceType::AcceptedException,
        EvidenceSource::Reference(format!("chat:exception-{sequence}")),
        "Approved exception for the fixture criterion",
    )
    .expect("exception request should be valid")
    .with_criterion(task_id(), criterion_id());
    if let Some(supersedes) = supersedes {
        request = request.superseding(supersedes);
    }
    request
}

fn mutation_arguments(
    action: &str,
    revision: u64,
    sequence: u64,
    additional: &[&str],
) -> Vec<String> {
    let mut arguments = vec!["exec".to_owned(), action.to_owned()];
    arguments.extend(common_mutation_arguments(revision, sequence));
    arguments.extend(additional.iter().map(|value| (*value).to_owned()));
    arguments
}

fn check_arguments(revision: u64, sequence: u64, check: &str) -> Vec<String> {
    let mut arguments = vec!["exec".to_owned(), "check".to_owned(), "run".to_owned()];
    arguments.extend(common_mutation_arguments(revision, sequence));
    arguments.extend(["--check".to_owned(), check.to_owned()]);
    arguments
}

fn criterion_arguments(revision: u64, sequence: u64, evidence: &str) -> Vec<String> {
    let mut arguments = vec!["exec".to_owned(), "criterion".to_owned(), "pass".to_owned()];
    arguments.extend(common_mutation_arguments(revision, sequence));
    arguments.extend([
        "--criterion".to_owned(),
        criterion_id().to_string(),
        "--evidence".to_owned(),
        evidence.to_owned(),
    ]);
    arguments
}

fn common_mutation_arguments(revision: u64, sequence: u64) -> Vec<String> {
    vec![
        "--plan".to_owned(),
        plan_id().to_string(),
        "--expect-revision".to_owned(),
        revision.to_string(),
        "--request-id".to_owned(),
        request_id(sequence).to_string(),
        "--actor".to_owned(),
        "codex".to_owned(),
    ]
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

fn parse_success(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("success should be JSON")
}
