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
    CriterionStatus, DeviationStatus, Evidence, EvidenceId, EvidenceType, FileChange, FileMapEntry,
    GitFlowConsent, GitReadiness, Plan, PlanDraftSeed, PlanId, PlanStatus, RequestId, Task, TaskId,
    TaskStatus, Timestamp, VerificationCheck, WorkspaceRepositoryMode,
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
        Self::new_with_repository(label, task_mode, true, false)
    }

    fn new_without_git(label: &str, task_mode: &str) -> Self {
        Self::new_with_repository(label, task_mode, false, false)
    }

    fn new_with_two_tasks(label: &str, task_mode: &str) -> Self {
        Self::new_with_repository(label, task_mode, true, true)
    }

    fn new_with_ignored_file_map(label: &str, task_mode: &str, has_git_repository: bool) -> Self {
        Self::new_with_repository_and_file_map(
            label,
            task_mode,
            has_git_repository,
            false,
            "dist/**",
        )
    }

    fn new_with_repository(
        label: &str,
        task_mode: &str,
        has_git_repository: bool,
        has_second_task: bool,
    ) -> Self {
        Self::new_with_repository_and_file_map(
            label,
            task_mode,
            has_git_repository,
            has_second_task,
            "src/feature.rs",
        )
    }

    fn new_with_repository_and_file_map(
        label: &str,
        task_mode: &str,
        has_git_repository: bool,
        has_second_task: bool,
        task_file_map: &str,
    ) -> Self {
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
        fs::write(
            path.join(".gitignore"),
            ".mino/\ndocs/plan/\ndist/\n*.pdb\n",
        )
        .expect("ignore rules should be written");
        let helper = compile_helper(&path);
        initialize(&path).expect("Mino project should initialize");
        if has_git_repository {
            initialize_git(&path, &helper);
        }
        let path = path.canonicalize().expect("project root should resolve");
        let base_revision = create_approved_plan(
            &path,
            &helper,
            task_mode,
            has_git_repository,
            has_second_task,
            task_file_map,
        );
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

fn create_approved_plan(
    root: &Path,
    helper: &Path,
    task_mode: &str,
    has_git_repository: bool,
    has_second_task: bool,
    task_file_map: &str,
) -> u64 {
    let plan = fixture_plan(helper, has_git_repository);
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
        task_file_map,
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
    let mut revision = 3;
    let mut request_sequence = 4;
    if has_second_task {
        (revision, request_sequence) =
            append_second_task(&store, helper, task_mode, revision, request_sequence);
    }
    commit(&store, revision, request_sequence, vec!["status"], |plan| {
        plan.finalize(timestamp(5))
    });
    revision += 1;
    request_sequence += 1;
    commit(
        &store,
        revision,
        request_sequence,
        vec!["approvals"],
        |plan| {
            plan.record_approval(Approval::plan(
                "user",
                "chat:completion-approval",
                timestamp(6),
                GitFlowConsent::Disabled,
            ))
        },
    );
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

fn fixture_plan(helper: &Path, has_git_repository: bool) -> Plan {
    let git_readiness = if has_git_repository {
        GitReadiness::detected(
            "Present",
            "Clean",
            Some("master".to_owned()),
            None,
            "Clean",
            false,
        )
    } else {
        GitReadiness::detected(
            "Missing",
            "Not Applicable",
            None,
            None,
            "No Git repository",
            false,
        )
    };
    Plan::from_draft_seed(
        PlanDraftSeed {
            id: plan_id(),
            name: "Completion contract".to_owned(),
            trigger: "durable".to_owned(),
            original_request: "Complete only with compatible evidence.".to_owned(),
            branch: Some("master".to_owned()),
            markdown_path: projection_relative().to_owned(),
            git_readiness,
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
    )
}

fn append_second_task(
    store: &PlanStore,
    helper: &Path,
    task_mode: &str,
    revision: u64,
    request_sequence: u64,
) -> (u64, u64) {
    let second_task_id = TaskId::parse("T2").expect("second task ID should be valid");
    let mut second_task = Task::new(
        second_task_id.clone(),
        "Implement the second feature",
        vec![task_id()],
    );
    second_task
        .add_acceptance_criterion(AcceptanceCriterion::new(
            CriterionId::parse("T2-A1").expect("second criterion ID should be valid"),
            "The second feature is observable",
        ))
        .expect("second criterion should be added");
    second_task
        .add_verification_check(VerificationCheck::new(
            check_id("SECOND-CHECK"),
            helper_command(helper, task_mode),
            ".",
            0,
            true,
        ))
        .expect("second check should be added");
    second_task
        .add_file_map_entry(FileMapEntry::new(
            "src/second.rs",
            FileChange::Create,
            "Own the second feature implementation",
            second_task_id.clone(),
        ))
        .expect("second file map should be added");
    commit(
        store,
        revision,
        request_sequence,
        vec!["tasks"],
        move |plan| plan.add_task(second_task, timestamp(3)),
    );
    commit(
        store,
        revision + 1,
        request_sequence + 1,
        vec!["tasks.T2.status"],
        move |plan| plan.mark_task_ready(&second_task_id, timestamp(4)),
    );
    (revision + 2, request_sequence + 2)
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

#[test]
fn final_outcome_is_required_after_current_global_verification() {
    let project = TestProject::new("outcome-gate", "pass");
    let completed = complete_task_after_scope_and_freshness_rejections(&project);
    let global = parse_success(&run_mino(
        &project,
        &check_arguments(result_revision(&completed), 18, "GLOBAL-CHECK"),
    ));

    let missing = run_mino(
        &project,
        &mutation_arguments("finish", result_revision(&global), 19, &[]),
    );
    assert_eq!(missing.status.code(), Some(2));
    let missing: Value = serde_json::from_slice(&missing.stdout).expect("failure should be JSON");
    assert!(
        missing["message"]
            .as_str()
            .is_some_and(|message| message.contains("Final Outcome"))
    );

    let outcome = parse_success(&run_mino(
        &project,
        &outcome_arguments(result_revision(&global), 20),
    ));
    let finished = parse_success(&run_mino(
        &project,
        &mutation_arguments("finish", result_revision(&outcome), 21, &[]),
    ));
    assert_eq!(finished["status"], "Review");
}

#[test]
fn non_git_project_completes_a_real_file_change_from_its_task_baseline() {
    let project = TestProject::new_without_git("non-git", "pass");
    let started = start_active_task(&project, 50);
    fs::write(
        project.path().join("src/feature.rs"),
        "pub fn feature() -> u8 { 5 }\n",
    )
    .expect("planned non-Git change should be written");

    complete_started_task(&project, started, 51);

    let plan = PlanStore::new(project.path())
        .load_plan(&plan_id())
        .expect("completed non-Git plan should load");
    assert_eq!(
        plan.task(&task_id()).expect("task should exist").status(),
        TaskStatus::Done
    );
}

#[test]
fn ignored_file_map_glob_is_fingerprinted_and_directory_delta_is_detected() {
    let project = TestProject::new_with_ignored_file_map("ignored-git", "pass", true);
    let (_, evidence) = capture_ignored_file_change(&project, 60);
    let fingerprint = evidence
        .workspace_fingerprint()
        .expect("ignored file check should capture a workspace fingerprint");
    assert_eq!(fingerprint.repository_mode(), WorkspaceRepositoryMode::Git);
    assert!(
        fingerprint
            .file_snapshots()
            .iter()
            .any(|snapshot| snapshot.path() == "dist/generated.txt")
    );
    let ignored = Command::new("git")
        .args(["check-ignore", "--quiet", "--", "dist/generated.txt"])
        .current_dir(project.path())
        .output()
        .expect("Git ignored-path probe should run");
    assert!(ignored.status.success());
}

#[test]
fn non_git_ignored_file_map_glob_is_detected() {
    let project = TestProject::new_with_ignored_file_map("ignored-non-git", "pass", false);
    let (_, evidence) = capture_ignored_file_change(&project, 65);
    let fingerprint = evidence
        .workspace_fingerprint()
        .expect("ignored file check should capture a workspace fingerprint");
    assert_eq!(
        fingerprint.repository_mode(),
        WorkspaceRepositoryMode::NonGit
    );
    assert!(
        fingerprint
            .file_snapshots()
            .iter()
            .any(|snapshot| snapshot.path() == "dist/generated.txt")
    );
}

#[test]
fn ignored_file_change_stales_check_evidence() {
    let project = TestProject::new_with_ignored_file_map("ignored-stale", "pass", true);
    let (revision, _) = capture_ignored_file_change(&project, 70);
    fs::write(
        project.path().join("dist/generated.txt"),
        "changed after verification\n",
    )
    .expect("ignored file should change after verification");

    let error = CompletionService::discover(project.path())
        .expect("completion service should discover")
        .complete_task(
            mutation(
                revision,
                72,
                vec!["mino".to_owned(), "exec".to_owned(), "complete".to_owned()],
            ),
            task_id(),
        )
        .expect_err("changed ignored file should stale its check evidence");
    assert_eq!(error.category(), ErrorCategory::IncompleteOrValidation);
    let plan = PlanStore::new(project.path())
        .load_plan(&plan_id())
        .expect("stale plan should load");
    assert_eq!(
        plan.task(&task_id())
            .expect("task should exist")
            .verification_checks()[0]
            .status(),
        CheckStatus::Stale
    );
}

#[test]
fn ignored_file_map_override_preserves_resource_limits() {
    let project = TestProject::new_with_ignored_file_map("ignored-budget", "pass", false);
    fs::create_dir(project.path().join("dist")).expect("ignored directory should be created");
    fs::write(
        project.path().join("dist/oversized.bin"),
        vec![0_u8; 16 * 1024 * 1024 + 1],
    )
    .expect("ignored oversized fixture should be written");

    let error = ExecutionService::discover(project.path())
        .expect("execution service should discover")
        .start_task(
            mutation(
                project.base_revision,
                75,
                vec!["mino".to_owned(), "exec".to_owned(), "start".to_owned()],
            ),
            task_id(),
        )
        .expect_err("ignored explicit scope must retain the file-size limit");
    assert_eq!(error.category(), ErrorCategory::EnvironmentUnavailable);
    assert!(error.message().contains("16777216-byte limit"));
}

#[test]
fn ignored_file_map_override_rejects_symlinks() {
    let project = TestProject::new_with_ignored_file_map("ignored-symlink", "pass", false);
    fs::create_dir(project.path().join("dist")).expect("ignored directory should be created");
    let target = project.path().join("outside-target.txt");
    fs::write(&target, "outside explicit scope\n").expect("symlink target should be written");
    if !create_file_symlink(&target, &project.path().join("dist/linked.txt")) {
        return;
    }

    let error = ExecutionService::discover(project.path())
        .expect("execution service should discover")
        .start_task(
            mutation(
                project.base_revision,
                78,
                vec!["mino".to_owned(), "exec".to_owned(), "start".to_owned()],
            ),
            task_id(),
        )
        .expect_err("ignored explicit scope must reject symbolic links");
    assert_eq!(error.category(), ErrorCategory::PolicyViolation);
    assert!(error.message().contains("symbolic link"));
}

fn capture_ignored_file_change(project: &TestProject, sequence: u64) -> (u64, Evidence) {
    let started = start_active_task(project, sequence);
    let baseline_plan = PlanStore::new(project.path())
        .load_plan(&plan_id())
        .expect("started plan should load");
    let workspace = baseline_plan
        .workspace_state()
        .expect("workspace state should decode");
    let baseline = workspace
        .task_baseline(&task_id())
        .expect("task baseline should exist");
    assert!(
        baseline
            .file_snapshots()
            .iter()
            .all(|snapshot| snapshot.path() != "dist/generated.txt")
    );
    fs::create_dir(project.path().join("dist")).expect("ignored directory should be created");
    fs::write(
        project.path().join("dist/generated.txt"),
        "generated after task start\n",
    )
    .expect("ignored file should be written");
    let report = ExecutionService::discover(project.path())
        .expect("execution service should discover")
        .run_check(
            &mutation(
                started,
                sequence + 1,
                vec!["mino".to_owned(), "exec".to_owned(), "check".to_owned()],
            ),
            &check_id("TASK-CHECK"),
        )
        .expect("ignored file check should pass");
    (report.plan().revision, report.evidence().clone())
}

#[cfg(unix)]
fn create_file_symlink(target: &Path, link: &Path) -> bool {
    std::os::unix::fs::symlink(target, link).is_ok()
}

#[cfg(windows)]
fn create_file_symlink(target: &Path, link: &Path) -> bool {
    std::os::windows::fs::symlink_file(target, link).is_ok()
}

#[cfg(not(any(unix, windows)))]
fn create_file_symlink(_target: &Path, _link: &Path) -> bool {
    false
}

#[test]
fn unchanged_dirty_baseline_files_are_not_attributed_to_the_task() {
    let project = TestProject::new("dirty-baseline", "pass");
    fs::write(
        project.path().join("preexisting-dirty.txt"),
        "present before task start\n",
    )
    .expect("dirty baseline should be written");
    let started = start_active_task(&project, 50);
    fs::write(
        project.path().join("src/feature.rs"),
        "pub fn feature() -> u8 { 6 }\n",
    )
    .expect("planned change should be written");

    complete_started_task(&project, started, 51);

    assert!(project.path().join("preexisting-dirty.txt").exists());
    let plan = PlanStore::new(project.path())
        .load_plan(&plan_id())
        .expect("completed dirty-baseline plan should load");
    assert_eq!(
        plan.task(&task_id()).expect("task should exist").status(),
        TaskStatus::Done
    );
}

#[test]
fn disabled_git_flow_attributes_each_uncommitted_task_to_its_own_baseline() {
    let project = TestProject::new_with_two_tasks("disabled-git-flow", "pass");
    let first_started = start_active_task(&project, 50);
    fs::write(
        project.path().join("src/feature.rs"),
        "pub fn feature() -> u8 { 7 }\n",
    )
    .expect("first planned change should be written");
    let first_done = complete_task_workflow(
        &project,
        first_started,
        51,
        task_id(),
        &check_id("TASK-CHECK"),
        criterion_id(),
    );

    let second_task_id = TaskId::parse("T2").expect("second task ID should be valid");
    let second_started = ExecutionService::discover(project.path())
        .expect("execution service should discover")
        .start_task(
            mutation(
                first_done,
                54,
                vec!["mino".to_owned(), "exec".to_owned(), "start".to_owned()],
            ),
            second_task_id.clone(),
        )
        .expect("second task should start without committing the first change")
        .revision;
    fs::write(
        project.path().join("src/second.rs"),
        "pub fn second() -> u8 { 8 }\n",
    )
    .expect("second planned change should be written");
    complete_task_workflow(
        &project,
        second_started,
        55,
        second_task_id.clone(),
        &check_id("SECOND-CHECK"),
        CriterionId::parse("T2-A1").expect("second criterion ID should be valid"),
    );

    let plan = PlanStore::new(project.path())
        .load_plan(&plan_id())
        .expect("two-task plan should load");
    assert_eq!(
        plan.task(&task_id())
            .expect("first task should exist")
            .status(),
        TaskStatus::Done
    );
    assert_eq!(
        plan.task(&second_task_id)
            .expect("second task should exist")
            .status(),
        TaskStatus::Done
    );
}

#[test]
fn workspace_baseline_capture_fails_loudly_when_a_file_exceeds_its_budget() {
    let project = TestProject::new_without_git("baseline-budget", "pass");
    fs::write(
        project.path().join("oversized.bin"),
        vec![0_u8; 16 * 1024 * 1024 + 1],
    )
    .expect("oversized fixture should be written");

    let error = ExecutionService::discover(project.path())
        .expect("execution service should discover")
        .start_task(
            mutation(
                project.base_revision,
                50,
                vec!["mino".to_owned(), "exec".to_owned(), "start".to_owned()],
            ),
            task_id(),
        )
        .expect_err("oversized baseline must not be silently truncated");

    assert_eq!(error.category(), ErrorCategory::EnvironmentUnavailable);
    assert!(error.message().contains("16777216-byte limit"));
}

fn complete_started_task(project: &TestProject, started: u64, sequence: u64) -> u64 {
    complete_task_workflow(
        project,
        started,
        sequence,
        task_id(),
        &check_id("TASK-CHECK"),
        criterion_id(),
    )
}

fn complete_task_workflow(
    project: &TestProject,
    started: u64,
    sequence: u64,
    selected_task_id: TaskId,
    selected_check_id: &CheckId,
    selected_criterion_id: CriterionId,
) -> u64 {
    let execution = ExecutionService::discover(project.path()).expect("service should discover");
    let check = execution
        .run_check(
            &mutation(
                started,
                sequence,
                vec!["mino".to_owned(), "exec".to_owned(), "check".to_owned()],
            ),
            selected_check_id,
        )
        .expect("task check should pass");
    assert!(check.is_success());
    let completion =
        CompletionService::discover(project.path()).expect("completion service should discover");
    let criterion = completion
        .pass_criterion(
            mutation(
                check.plan().revision,
                sequence + 1,
                vec!["mino".to_owned(), "exec".to_owned(), "criterion".to_owned()],
            ),
            selected_criterion_id,
            check.evidence().id().clone(),
        )
        .expect("criterion should pass");
    completion
        .complete_task(
            mutation(
                criterion.revision,
                sequence + 2,
                vec!["mino".to_owned(), "exec".to_owned(), "complete".to_owned()],
            ),
            selected_task_id,
        )
        .expect("task-local delta should satisfy completion")
        .revision
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
    assert_eq!(global["next_actions"], serde_json::json!([]));
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
    let outcome = parse_success(&run_mino(
        project,
        &outcome_arguments(result_revision(&reglobal), 25),
    ));
    let finish_arguments = mutation_arguments("finish", result_revision(&outcome), 26, &[]);
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
    accept_arguments.extend(common_mutation_arguments(result_revision(&finished), 27));
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
#[allow(clippy::too_many_lines)]
fn incomplete_evidence_and_open_deviation_block_until_evidence_resolution() {
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
    let open_plan = PlanStore::new(project.path())
        .load_plan(&plan_id())
        .expect("plan should load");
    let open_execution = open_plan
        .execution_state()
        .expect("execution state should decode");
    assert_eq!(open_execution.deviations()[0].id(), "D1");
    assert_eq!(
        open_execution.deviations()[0].status(),
        DeviationStatus::Open
    );
    assert_eq!(
        open_execution.deviations()[0].legacy_checkpoint_sequence(),
        Some(1)
    );
    let missing_evidence = execution
        .resolve_deviation(
            mutation(
                checkpoint.revision,
                47,
                vec!["mino".to_owned(), "exec".to_owned(), "deviation".to_owned()],
            ),
            "D1".to_owned(),
            "Resolved in the declared scope".to_owned(),
            vec![EvidenceId::parse("E9999").expect("evidence ID should parse")],
        )
        .expect_err("missing evidence should reject resolution");
    assert_eq!(
        missing_evidence.category(),
        ErrorCategory::IncompleteOrValidation
    );
    let resolved = execution
        .resolve_deviation(
            mutation(
                checkpoint.revision,
                48,
                vec!["mino".to_owned(), "exec".to_owned(), "deviation".to_owned()],
            ),
            "D1".to_owned(),
            "Resolved in the declared scope".to_owned(),
            vec![check.evidence().id().clone()],
        )
        .expect("current task evidence should resolve deviation");
    let resolved_plan = PlanStore::new(project.path())
        .load_plan(&plan_id())
        .expect("plan should load");
    assert_eq!(
        resolved_plan
            .execution_state()
            .expect("execution should decode")
            .deviations()[0]
            .status(),
        DeviationStatus::Resolved
    );
    completion
        .complete_task(
            mutation(
                resolved.revision,
                49,
                vec!["mino".to_owned(), "exec".to_owned(), "complete".to_owned()],
            ),
            task_id(),
        )
        .expect("resolved deviation should no longer block completion");
    assert_eq!(
        PlanStore::new(project.path())
            .load_plan(&plan_id())
            .expect("plan should load")
            .task(&task_id())
            .expect("task should exist")
            .status(),
        TaskStatus::Done
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

fn outcome_arguments(revision: u64, sequence: u64) -> Vec<String> {
    let mut arguments = vec!["plan".to_owned(), "outcome".to_owned(), "set".to_owned()];
    arguments.extend(common_mutation_arguments(revision, sequence));
    arguments.extend([
        "--summary".to_owned(),
        "Completion contract verified".to_owned(),
        "--remaining-risk".to_owned(),
        "N/A".to_owned(),
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
