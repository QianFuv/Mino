//! Contract tests for exact, approval-gated, and recoverable task commits.

use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::fs::symlink;
#[cfg(windows)]
use std::os::windows::fs::symlink_dir;

use fs4::FileExt;
use mino::ErrorCategory;
use mino::application::completion::CompletionService;
use mino::application::execution::ExecutionService;
use mino::application::git_binding::GitBindingService;
use mino::application::git_commit::GitCommitService;
use mino::application::plan::PlanMutationRequest;
use mino::domain::{
    AcceptanceCriterion, Approval, CheckId, CommitGate, CommitStatus, CriterionId, EvidenceType,
    FileChange, FileMapEntry, GitFlowConsent, GitReadiness, Plan, PlanDraftSeed, PlanId,
    PlanStatus, RequestId, Task, TaskId, Timestamp, VerificationCheck,
};
use mino::evidence::EvidenceStore;
use mino::git::{GitAdapter, GitCommitJournalStore, GitErrorKind};
use mino::project::initialize;
use mino::render::{render_plan, write_projection};
use mino::store::{MutationRequest, PlanStore};
use serde_json::Value;

const PLAN_ID: &str = "2026-07-26-commit-gate";
const TASK_ID: &str = "T1";
const CRITERION_ID: &str = "T1-A1";
const CHECK_ID: &str = "T1-V1";
const TASK_PATH: &str = "src/feature.rs";
const COMMIT_MESSAGE: &str = "feat(test): commit exact task scope";
static NEXT_TEMP_DIRECTORY: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy)]
enum FixtureState {
    InProgress,
    Done,
}

struct TestRepository {
    root: PathBuf,
    plan_id: PlanId,
}

impl TestRepository {
    fn new(label: &str, state: FixtureState, with_commit_gate: bool) -> Self {
        Self::new_with_git_flow(label, state, with_commit_gate, true)
    }

    fn new_with_git_flow(
        label: &str,
        state: FixtureState,
        with_commit_gate: bool,
        git_flow_enabled: bool,
    ) -> Self {
        let sequence = NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "mino-git-commit-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&root).expect("temporary commit repository should be created");
        fs::create_dir(root.join("src")).expect("fixture source directory should be created");
        fs::write(root.join("src/lib.rs"), "pub fn baseline() -> u8 { 1 }\n")
            .expect("baseline source should be written");
        fs::write(root.join(".gitignore"), ".mino/\ndocs/plan/\n*.pdb\n")
            .expect("fixture ignores should be written");
        initialize(&root).expect("Mino project should initialize");
        initialize_git(&root);
        let root = root.canonicalize().expect("repository root should resolve");
        let plan_id = create_plan(&root, with_commit_gate, git_flow_enabled);
        prepare_task(&root, &plan_id, state);
        Self { root, plan_id }
    }

    fn root(&self) -> &Path {
        &self.root
    }

    fn plan_id(&self) -> &PlanId {
        &self.plan_id
    }

    fn journal(&self) -> GitCommitJournalStore {
        GitCommitJournalStore::new(&self.root)
    }
}

impl Drop for TestRepository {
    fn drop(&mut self) {
        let temporary_root = std::env::temp_dir();
        if self.root.starts_with(&temporary_root)
            && self
                .root
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("mino-git-commit-"))
        {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

#[cfg(any(unix, windows))]
#[test]
fn commit_journal_rejects_a_symlinked_git_state_directory() {
    let repository = TestRepository::new("journal-symlink", FixtureState::Done, true);
    let external = TestRepository::new("journal-symlink-external", FixtureState::Done, true);
    let git_state = repository.root.join(".mino/git");
    #[cfg(unix)]
    let symlink_result = symlink(external.root(), &git_state);
    #[cfg(windows)]
    let symlink_result = symlink_dir(external.root(), &git_state);
    if symlink_result
        .as_ref()
        .is_err_and(|error| error.kind() == std::io::ErrorKind::PermissionDenied)
    {
        return;
    }
    symlink_result.expect("Git state symlink should be created");

    let error = repository
        .journal()
        .lock()
        .expect_err("symlinked commit journal directory must be rejected");
    assert_eq!(error.kind(), GitErrorKind::InvalidOutput);
    assert!(!external.root.join("commit.lock").exists());
}

#[test]
fn valid_commit_is_exact_evidenced_clean_and_idempotent() {
    let repository = TestRepository::new("success", FixtureState::Done, true);
    let base = git_text(repository.root(), &["rev-parse", "HEAD"]);
    let output = json_success(&run_commit(&repository));
    let commit = output["completion"]["commit"]
        .as_str()
        .expect("commit report should contain an object ID")
        .to_owned();

    assert_ne!(commit, base);
    assert_eq!(output["completion"]["message"], COMMIT_MESSAGE);
    assert_eq!(
        output["completion"]["files"],
        serde_json::json!([TASK_PATH])
    );
    assert_eq!(output["replayed"], false);
    assert_eq!(
        git_text(
            repository.root(),
            &["rev-list", "--count", &format!("{base}..HEAD")]
        ),
        "1"
    );
    assert_eq!(
        git_text(repository.root(), &["show", "-s", "--format=%B", "HEAD"]),
        COMMIT_MESSAGE
    );
    assert_eq!(
        git_text(
            repository.root(),
            &["diff-tree", "--no-commit-id", "--name-only", "-r", "HEAD"]
        ),
        TASK_PATH
    );
    assert!(git_status(repository.root()).is_empty());

    let plan = load_plan(&repository);
    let gate = plan
        .task(&task_id())
        .and_then(Task::commit_gate)
        .expect("task commit gate should exist");
    assert_eq!(gate.status(), CommitStatus::Committed);
    assert_eq!(gate.actual_commit(), Some(commit.as_str()));
    assert_eq!(gate.committed_files(), [TASK_PATH]);
    let commit_evidence = EvidenceStore::new(repository.root())
        .list(repository.plan_id())
        .expect("evidence should load")
        .into_iter()
        .filter(|evidence| evidence.kind() == EvidenceType::Commit)
        .collect::<Vec<_>>();
    assert_eq!(commit_evidence.len(), 1);
    assert_eq!(commit_evidence[0].artifact_path(), Some(commit.as_str()));
    assert!(gate.evidence_refs().contains(commit_evidence[0].id()));

    let revision = plan.revision();
    let journal_before = journal_bytes(&repository);
    let replay = json_success(&run_commit(&repository));
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["completion"]["commit"], commit);
    assert_eq!(
        git_text(
            repository.root(),
            &["rev-list", "--count", &format!("{base}..HEAD")]
        ),
        "1"
    );
    assert_eq!(load_plan(&repository).revision(), revision);
    assert_eq!(journal_bytes(&repository), journal_before);
}

#[test]
fn verified_manual_commit_closes_a_disabled_git_flow_gate_idempotently() {
    let repository = TestRepository::new_with_git_flow("manual", FixtureState::Done, true, false);
    git(repository.root(), &["add", "--", TASK_PATH]);
    git(
        repository.root(),
        &["commit", "--quiet", "-m", COMMIT_MESSAGE],
    );
    let commit = git_text(repository.root(), &["rev-parse", "HEAD"]);
    let revision = load_plan(&repository).revision();
    let request = mutation(revision, 90, "record-manual");

    let report = GitCommitService::discover(repository.root())
        .expect("manual commit service should discover")
        .record_manual(
            request.clone(),
            &task_id(),
            &commit,
            "chat:manual-commit-approved",
        )
        .expect("exact manual commit should record");

    assert_eq!(report.commit.commit, commit);
    assert_eq!(report.commit.files, [TASK_PATH]);
    assert_eq!(report.evidence.kind(), EvidenceType::Commit);
    assert!(!report.plan.replayed);
    let plan = load_plan(&repository);
    let gate = plan
        .task(&task_id())
        .and_then(Task::commit_gate)
        .expect("manual task gate should exist");
    assert_eq!(gate.status(), CommitStatus::Committed);
    assert_eq!(gate.actual_commit(), Some(commit.as_str()));
    assert_eq!(gate.committed_files(), [TASK_PATH]);

    let replay = GitCommitService::discover(repository.root())
        .expect("manual commit service should rediscover")
        .record_manual(request, &task_id(), &commit, "chat:manual-commit-approved")
        .expect("manual commit request should replay");
    assert!(replay.plan.replayed);
    assert_eq!(replay.evidence.id(), report.evidence.id());
    assert_eq!(load_plan(&repository).revision(), plan.revision());
}

#[test]
fn manual_commit_rejects_wrong_message_scope_parent_and_verified_content() {
    let wrong_message =
        TestRepository::new_with_git_flow("manual-message", FixtureState::Done, true, false);
    git(wrong_message.root(), &["add", "--", TASK_PATH]);
    git(
        wrong_message.root(),
        &["commit", "--quiet", "-m", "fix(test): wrong message"],
    );
    assert_manual_refusal(&wrong_message, 91, ErrorCategory::PolicyViolation);

    let wrong_scope =
        TestRepository::new_with_git_flow("manual-scope", FixtureState::Done, true, false);
    fs::write(wrong_scope.root().join("outside.txt"), "outside\n")
        .expect("outside path should be written");
    git(wrong_scope.root(), &["add", "--", TASK_PATH, "outside.txt"]);
    git(
        wrong_scope.root(),
        &["commit", "--quiet", "-m", COMMIT_MESSAGE],
    );
    assert_manual_refusal(&wrong_scope, 92, ErrorCategory::PolicyViolation);

    let wrong_parent =
        TestRepository::new_with_git_flow("manual-parent", FixtureState::Done, true, false);
    fs::write(
        wrong_parent.root().join("intermediate.txt"),
        "intermediate\n",
    )
    .expect("intermediate path should be written");
    git(wrong_parent.root(), &["add", "--", "intermediate.txt"]);
    git(
        wrong_parent.root(),
        &["commit", "--quiet", "-m", "test: advance parent"],
    );
    git(wrong_parent.root(), &["add", "--", TASK_PATH]);
    git(
        wrong_parent.root(),
        &["commit", "--quiet", "-m", COMMIT_MESSAGE],
    );
    assert_manual_refusal(&wrong_parent, 93, ErrorCategory::DriftDetected);

    let stale_content =
        TestRepository::new_with_git_flow("manual-stale", FixtureState::Done, true, false);
    fs::write(
        stale_content.root().join(TASK_PATH),
        "pub fn feature() -> u8 { 99 }\n",
    )
    .expect("post-check content should be written");
    git(stale_content.root(), &["add", "--", TASK_PATH]);
    git(
        stale_content.root(),
        &["commit", "--quiet", "-m", COMMIT_MESSAGE],
    );
    assert_manual_refusal(&stale_content, 94, ErrorCategory::IncompleteOrValidation);
}

#[test]
fn approved_skip_records_exception_evidence_and_satisfies_the_gate() {
    let repository = TestRepository::new_with_git_flow("skip", FixtureState::Done, true, false);
    let revision = load_plan(&repository).revision();
    let request = mutation(revision, 95, "skip-gate");

    let report = GitCommitService::discover(repository.root())
        .expect("skip service should discover")
        .skip_gate(
            request.clone(),
            &task_id(),
            "chat:skip-approved",
            "The user will include this change in a later manual commit",
        )
        .expect("approved skip should record");

    assert_eq!(report.evidence.kind(), EvidenceType::AcceptedException);
    assert!(!report.plan.replayed);
    let plan = load_plan(&repository);
    let gate = plan
        .task(&task_id())
        .and_then(Task::commit_gate)
        .expect("skipped gate should exist");
    assert_eq!(gate.status(), CommitStatus::Skipped);
    assert!(gate.is_satisfied());
    assert_eq!(gate.evidence_refs(), [report.evidence.id().clone()]);

    let replay = GitCommitService::discover(repository.root())
        .expect("skip service should rediscover")
        .skip_gate(
            request,
            &task_id(),
            "chat:skip-approved",
            "The user will include this change in a later manual commit",
        )
        .expect("approved skip should replay");
    assert!(replay.plan.replayed);
    assert_eq!(replay.evidence.id(), report.evidence.id());
}

#[test]
fn preflight_refusals_preserve_head_index_and_journal() {
    let incomplete = TestRepository::new("incomplete", FixtureState::InProgress, true);
    assert_preflight_refusal(&incomplete, ErrorCategory::PolicyViolation);

    let missing_gate = TestRepository::new("missing-gate", FixtureState::Done, false);
    assert_preflight_refusal(&missing_gate, ErrorCategory::PolicyViolation);

    let missing_binding = TestRepository::new("missing-binding", FixtureState::Done, true);
    fs::remove_file(missing_binding.root().join(".mino/active.json"))
        .expect("active binding should be removable");
    assert_preflight_refusal(&missing_binding, ErrorCategory::PolicyViolation);

    let staged = TestRepository::new("staged", FixtureState::Done, true);
    git(staged.root(), &["add", "--", TASK_PATH]);
    assert_preflight_refusal(&staged, ErrorCategory::IncompleteOrValidation);

    let mixed = TestRepository::new("mixed", FixtureState::Done, true);
    git(mixed.root(), &["add", "--", TASK_PATH]);
    fs::write(
        mixed.root().join(TASK_PATH),
        "pub fn feature() -> u8 { 3 }\n",
    )
    .expect("mixed worktree content should be written");
    assert_preflight_refusal(&mixed, ErrorCategory::IncompleteOrValidation);

    let outside = TestRepository::new("outside", FixtureState::Done, true);
    fs::write(outside.root().join("outside.txt"), "outside\n")
        .expect("outside file should be written");
    assert_preflight_refusal(&outside, ErrorCategory::PolicyViolation);

    let advanced = TestRepository::new("advanced", FixtureState::Done, true);
    fs::write(advanced.root().join("drift.txt"), "drift\n")
        .expect("drift fixture should be written");
    git(advanced.root(), &["add", "--", "drift.txt"]);
    git(
        advanced.root(),
        &["commit", "--quiet", "-m", "test: advance fixture head"],
    );
    assert_preflight_refusal(&advanced, ErrorCategory::IncompleteOrValidation);
}

#[test]
fn failed_hook_blocks_with_staged_state_and_retry_commits_once() {
    let repository = TestRepository::new("hook", FixtureState::Done, true);
    let base = git_text(repository.root(), &["rev-parse", "HEAD"]);
    install_pre_commit_hook(repository.root());

    let failed = run_commit(&repository);
    assert_json_error(&failed, 7, "environment_unavailable");
    let blocked = load_plan(&repository);
    assert_eq!(blocked.status(), PlanStatus::Blocked);
    assert_eq!(
        blocked
            .task(&task_id())
            .and_then(Task::commit_gate)
            .map(CommitGate::status),
        Some(CommitStatus::Blocked)
    );
    let facts = GitAdapter::new(repository.root())
        .inspect()
        .expect("staged Git facts should inspect");
    assert_eq!(facts.staged_paths, [TASK_PATH]);
    assert!(facts.unstaged_paths.is_empty());
    assert!(
        repository
            .journal()
            .intent_path(repository.plan_id(), &task_id())
            .is_file()
    );
    assert!(
        repository
            .journal()
            .staged_path(repository.plan_id(), &task_id())
            .is_file()
    );
    assert!(
        !repository
            .journal()
            .completion_path(repository.plan_id(), &task_id())
            .exists()
    );

    fs::remove_file(repository.root().join(".git/hooks/pre-commit"))
        .expect("failing hook should be removed");
    let revision = blocked.revision();
    ExecutionService::discover(repository.root())
        .expect("execution service should discover")
        .resume(mutation(revision, 80, "resume"))
        .expect("commit-blocked plan should resume");
    let recovered = json_success(&run_commit(&repository));
    assert_eq!(recovered["replayed"], false);
    assert_eq!(
        git_text(
            repository.root(),
            &["rev-list", "--count", &format!("{base}..HEAD")]
        ),
        "1"
    );
    assert!(git_status(repository.root()).is_empty());
    assert_eq!(
        load_plan(&repository)
            .task(&task_id())
            .and_then(Task::commit_gate)
            .map(CommitGate::status),
        Some(CommitStatus::Committed)
    );
}

#[test]
fn post_commit_plan_lock_interruption_reconciles_without_duplicate_commit() {
    let repository = TestRepository::new("post-commit", FixtureState::Done, true);
    let base = git_text(repository.root(), &["rev-parse", "HEAD"]);
    let marker_path = repository.root().join(".mino/post-commit.marker");
    let ready_path = repository.root().join(".mino/post-commit-lock-ready");
    install_post_commit_pause_hook(repository.root());
    let lock_path = repository
        .root()
        .join(".mino/plans")
        .join(repository.plan_id().as_str())
        .join("store.lock");
    let (locked_sender, locked_receiver) = mpsc::channel();
    let (release_sender, release_receiver) = mpsc::channel();
    let lock_thread = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_mins(3);
        while !marker_path.exists() {
            assert!(
                Instant::now() < deadline,
                "post-commit marker should appear"
            );
            thread::sleep(Duration::from_millis(5));
        }
        let lock = OpenOptions::new()
            .create(true)
            .read(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)
            .expect("plan lock should open");
        FileExt::lock(&lock).expect("plan lock should be held after commit");
        fs::write(ready_path, "locked\n").expect("hook release marker should be written");
        locked_sender
            .send(())
            .expect("lock acquisition should be reported");
        release_receiver
            .recv_timeout(Duration::from_mins(3))
            .expect("plan lock release should be requested");
        FileExt::unlock(&lock).expect("plan lock should release");
    });

    let error = GitCommitService::discover(repository.root())
        .expect("commit service should discover")
        .commit(repository.plan_id(), &task_id())
        .expect_err("plan publication lock should interrupt after Git commit");
    assert_eq!(error.category(), ErrorCategory::EnvironmentUnavailable);
    let created = git_text(repository.root(), &["rev-parse", "HEAD"]);
    assert_ne!(created, base);
    assert_eq!(
        git_text(
            repository.root(),
            &["rev-list", "--count", &format!("{base}..HEAD")]
        ),
        "1"
    );
    assert!(
        !repository
            .journal()
            .completion_path(repository.plan_id(), &task_id())
            .exists()
    );
    locked_receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("post-commit lock should have been acquired");
    release_sender
        .send(())
        .expect("post-commit lock should be releasable");
    lock_thread.join().expect("lock thread should finish");
    fs::remove_file(repository.root().join(".git/hooks/post-commit"))
        .expect("post-commit pause hook should be removed");
    assert_eq!(
        load_plan(&repository)
            .task(&task_id())
            .and_then(Task::commit_gate)
            .map(CommitGate::status),
        Some(CommitStatus::Pending)
    );

    let recovered = GitCommitService::discover(repository.root())
        .expect("commit service should rediscover")
        .commit(repository.plan_id(), &task_id())
        .expect("created commit should reconcile");
    assert!(recovered.reconciled);
    assert_eq!(recovered.completion.commit, created);
    assert_eq!(
        git_text(
            repository.root(),
            &["rev-list", "--count", &format!("{base}..HEAD")]
        ),
        "1"
    );
    assert!(git_status(repository.root()).is_empty());
}

#[test]
fn recorded_gate_without_terminal_journal_replays_to_completion() {
    let repository = TestRepository::new("terminal-publication", FixtureState::Done, true);
    let base = git_text(repository.root(), &["rev-parse", "HEAD"]);
    install_completion_collision_hook(repository.root());

    let error = GitCommitService::discover(repository.root())
        .expect("commit service should discover")
        .commit(repository.plan_id(), &task_id())
        .expect_err("terminal journal collision should interrupt publication");
    assert_eq!(error.category(), ErrorCategory::DriftDetected);
    let created = git_text(repository.root(), &["rev-parse", "HEAD"]);
    assert_ne!(created, base);
    assert_eq!(
        load_plan(&repository)
            .task(&task_id())
            .and_then(Task::commit_gate)
            .map(CommitGate::status),
        Some(CommitStatus::Committed)
    );
    assert_eq!(
        EvidenceStore::new(repository.root())
            .list(repository.plan_id())
            .expect("evidence should load")
            .into_iter()
            .filter(|evidence| evidence.kind() == EvidenceType::Commit)
            .count(),
        1
    );

    fs::remove_file(repository.root().join(".git/hooks/post-commit"))
        .expect("completion collision hook should be removed");
    fs::remove_file(
        repository
            .journal()
            .completion_path(repository.plan_id(), &task_id()),
    )
    .expect("colliding completion file should be removed");
    let recovered = GitCommitService::discover(repository.root())
        .expect("commit service should rediscover")
        .commit(repository.plan_id(), &task_id())
        .expect("committed gate should finish terminal publication");
    assert!(recovered.reconciled);
    assert_eq!(recovered.completion.commit, created);
    assert_eq!(
        git_text(
            repository.root(),
            &["rev-list", "--count", &format!("{base}..HEAD")]
        ),
        "1"
    );
}

fn create_plan(root: &Path, with_commit_gate: bool, git_flow_enabled: bool) -> PlanId {
    let plan_id = plan_id();
    let mut plan = Plan::from_draft_seed(
        PlanDraftSeed {
            id: plan_id.clone(),
            name: "Commit gate fixture".to_owned(),
            trigger: "durable".to_owned(),
            original_request: "Create one exact recoverable task commit.".to_owned(),
            branch: Some("main".to_owned()),
            markdown_path: projection_relative().to_owned(),
            git_readiness: GitReadiness::detected(
                "Present",
                "Clean",
                Some("main".to_owned()),
                Some(git_text(root, &["rev-parse", "HEAD"])),
                "Clean: git status --short returned empty",
                git_flow_enabled,
            ),
            standards: Vec::new(),
            verification_plan: vec![VerificationCheck::new(
                check_id("GLOBAL-V1"),
                vec!["git".to_owned(), "--version".to_owned()],
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
            vec!["test".to_owned(), "create-commit-plan".to_owned()],
        )
        .expect("commit plan should persist");
    let mut task = Task::new(task_id(), "Commit the exact feature", Vec::new());
    task.add_acceptance_criterion(AcceptanceCriterion::new(
        criterion_id(),
        "The planned feature is implemented",
    ))
    .expect("criterion should be added");
    task.add_verification_check(VerificationCheck::new(
        check_id(CHECK_ID),
        vec!["git".to_owned(), "--version".to_owned()],
        ".",
        0,
        true,
    ))
    .expect("task check should be added");
    task.add_file_map_entry(FileMapEntry::new(
        TASK_PATH,
        FileChange::Create,
        "Own the feature implementation",
        task_id(),
    ))
    .expect("file responsibility should be added");
    if with_commit_gate {
        task.set_commit_gate(CommitGate::new(
            true,
            COMMIT_MESSAGE,
            vec![TASK_PATH.to_owned()],
        ))
        .expect("commit gate should be added");
    }
    mutate(&store, 1, 2, vec!["tasks"], move |plan| {
        plan.add_task(task, timestamp(1))
    });
    mutate(&store, 2, 3, vec!["tasks.T1.status"], |plan| {
        plan.mark_task_ready(&task_id(), timestamp(2))
    });
    mutate(&store, 3, 4, vec!["status"], |plan| {
        plan.finalize(timestamp(3))
    });
    mutate(&store, 4, 5, vec!["approvals"], |plan| {
        plan.record_approval(Approval::plan(
            "user",
            "chat:commit-plan-approval",
            timestamp(4),
            if git_flow_enabled {
                GitFlowConsent::Approved
            } else {
                GitFlowConsent::Disabled
            },
        ))
    });
    plan = store
        .load_plan(&plan_id)
        .expect("approved commit plan should load");
    write_projection(
        &root.join(projection_relative()),
        &render_plan(&plan).expect("commit plan should render"),
        None,
    )
    .expect("commit plan projection should publish");
    plan_id
}

fn prepare_task(root: &Path, plan_id: &PlanId, state: FixtureState) {
    let execution = ExecutionService::discover(root).expect("execution service should discover");
    let started = execution
        .start_task(mutation(5, 10, "start"), task_id())
        .expect("task should start");
    if matches!(state, FixtureState::Done) {
        fs::write(root.join(TASK_PATH), "pub fn feature() -> u8 { 2 }\n")
            .expect("task file should be written before verification");
        let checked = execution
            .run_check(
                &mutation(started.revision, 11, "check"),
                &check_id(CHECK_ID),
            )
            .expect("task check should run");
        assert!(checked.is_success());
        let completion =
            CompletionService::discover(root).expect("completion service should discover");
        let criterion = completion
            .pass_criterion(
                mutation(checked.plan().revision, 12, "criterion"),
                criterion_id(),
                checked.evidence().id().clone(),
            )
            .expect("criterion should bind to command evidence");
        completion
            .complete_task(mutation(criterion.revision, 13, "complete"), task_id())
            .expect("task should complete");
    } else {
        fs::write(root.join(TASK_PATH), "pub fn feature() -> u8 { 2 }\n")
            .expect("in-progress task file should be written");
    }
    GitBindingService::discover(root)
        .expect("binding service should discover")
        .bind_current(plan_id.clone())
        .expect("plan should bind to the current worktree");
}

fn mutate<F>(
    store: &PlanStore,
    expected_revision: u64,
    sequence: u64,
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
                request_id(sequence),
                "codex",
                vec!["test".to_owned(), sequence.to_string()],
                changed_fields.into_iter().map(str::to_owned).collect(),
            )
            .expect("mutation request should be valid"),
            mutation,
        )
        .expect("plan mutation should persist");
}

fn mutation(expected_revision: u64, sequence: u64, action: &str) -> PlanMutationRequest {
    PlanMutationRequest {
        plan_id: plan_id(),
        expected_revision,
        request_id: request_id(sequence),
        actor: "codex".to_owned(),
        command: vec!["test".to_owned(), action.to_owned()],
        updated_at: timestamp(u8::try_from(sequence).unwrap_or(59).min(59)),
    }
}

fn assert_preflight_refusal(repository: &TestRepository, category: ErrorCategory) {
    let before_head = git_text(repository.root(), &["rev-parse", "HEAD"]);
    let before_index = fs::read(repository.root().join(".git/index"))
        .expect("Git index should be readable before refusal");
    let error = GitCommitService::discover(repository.root())
        .expect("commit service should discover")
        .commit(repository.plan_id(), &task_id())
        .expect_err("unsafe preflight should refuse");
    assert_eq!(error.category(), category, "{error}");
    assert_eq!(
        git_text(repository.root(), &["rev-parse", "HEAD"]),
        before_head
    );
    assert_eq!(
        fs::read(repository.root().join(".git/index"))
            .expect("Git index should be readable after refusal"),
        before_index
    );
    assert!(
        !repository
            .journal()
            .intent_path(repository.plan_id(), &task_id())
            .exists()
    );
}

fn assert_manual_refusal(repository: &TestRepository, sequence: u64, category: ErrorCategory) {
    let plan = load_plan(repository);
    let revision = plan.revision();
    let commit = git_text(repository.root(), &["rev-parse", "HEAD"]);
    let error = GitCommitService::discover(repository.root())
        .expect("manual commit service should discover")
        .record_manual(
            mutation(revision, sequence, "record-manual-refusal"),
            &task_id(),
            &commit,
            "chat:manual-commit-approved",
        )
        .expect_err("invalid manual commit should be rejected");
    assert_eq!(error.category(), category, "{error}");
    assert_eq!(load_plan(repository).revision(), revision);
    assert_eq!(
        load_plan(repository)
            .task(&task_id())
            .and_then(Task::commit_gate)
            .map(CommitGate::status),
        Some(CommitStatus::Pending)
    );
}

fn load_plan(repository: &TestRepository) -> Plan {
    PlanStore::new(repository.root())
        .load_plan(repository.plan_id())
        .expect("fixture plan should load")
}

fn journal_bytes(repository: &TestRepository) -> [Vec<u8>; 3] {
    let journal = repository.journal();
    [
        fs::read(journal.intent_path(repository.plan_id(), &task_id()))
            .expect("commit intent should be readable"),
        fs::read(journal.staged_path(repository.plan_id(), &task_id()))
            .expect("staged phase should be readable"),
        fs::read(journal.completion_path(repository.plan_id(), &task_id()))
            .expect("completion phase should be readable"),
    ]
}

fn install_pre_commit_hook(root: &Path) {
    let hook = root.join(".git/hooks/pre-commit");
    fs::write(
        &hook,
        "#!/bin/sh\nprintf 'planned hook failure' >&2\nexit 1\n",
    )
    .expect("pre-commit hook should be written");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = fs::metadata(&hook)
            .expect("hook metadata should exist")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&hook, permissions).expect("hook should become executable");
    }
}

fn install_post_commit_pause_hook(root: &Path) {
    let hook = root.join(".git/hooks/post-commit");
    fs::write(
        &hook,
        "#!/bin/sh\nprintf 'commit-created' > .mino/post-commit.marker\ncount=0\nwhile test ! -f .mino/post-commit-lock-ready; do\n  count=$((count + 1))\n  if test \"$count\" -ge 300; then\n    exit 99\n  fi\n  sleep 0.1\ndone\n",
    )
    .expect("post-commit pause hook should be written");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = fs::metadata(&hook)
            .expect("hook metadata should exist")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&hook, permissions).expect("hook should become executable");
    }
}

fn install_completion_collision_hook(root: &Path) {
    let hook = root.join(".git/hooks/post-commit");
    fs::write(
        &hook,
        format!(
            "#!/bin/sh\nprintf 'occupied' > .mino/git/commits/{PLAN_ID}/{TASK_ID}/completion.json\n"
        ),
    )
    .expect("completion collision hook should be written");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = fs::metadata(&hook)
            .expect("hook metadata should exist")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&hook, permissions).expect("hook should become executable");
    }
}

fn run_commit(repository: &TestRepository) -> Output {
    run_mino(
        repository.root(),
        &[
            "git",
            "commit",
            "--plan",
            repository.plan_id().as_str(),
            "--task",
            TASK_ID,
        ],
    )
}

fn run_mino(root: &Path, command: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mino"))
        .arg("--root")
        .arg(root)
        .args(["--format", "json", "--no-input"])
        .args(command)
        .stdin(Stdio::null())
        .output()
        .expect("Mino binary should run")
}

fn json_success(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    serde_json::from_slice(&output.stdout).expect("success output should be JSON")
}

fn assert_json_error(output: &Output, exit_code: i32, code: &str) {
    assert_eq!(
        output.status.code(),
        Some(exit_code),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let value: Value =
        serde_json::from_slice(&output.stdout).expect("failure output should be JSON");
    assert_eq!(value["error"]["code"], code);
}

fn initialize_git(root: &Path) {
    git(root, &["init", "--quiet", "--initial-branch", "main"]);
    git(root, &["config", "user.name", "Mino Tests"]);
    git(
        root,
        &["config", "user.email", "mino-tests@example.invalid"],
    );
    git(
        root,
        &[
            "add",
            "--",
            ".gitignore",
            ".agents/skills/mino",
            "src/lib.rs",
        ],
    );
    git(
        root,
        &["commit", "--quiet", "-m", "test: create commit fixture"],
    );
}

fn git(root: &Path, arguments: &[&str]) {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .expect("Git should run");
    assert!(
        output.status.success(),
        "git {arguments:?} failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_text(root: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .expect("Git should run");
    assert!(
        output.status.success(),
        "git {arguments:?} failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("Git text should be UTF-8")
        .trim()
        .to_owned()
}

fn git_status(root: &Path) -> String {
    git_text(root, &["status", "--short"])
}

fn projection_relative() -> &'static str {
    "docs/plan/2026-07-26-commit-gate.md"
}

fn plan_id() -> PlanId {
    PlanId::parse(PLAN_ID).expect("plan ID should be valid")
}

fn task_id() -> TaskId {
    TaskId::parse(TASK_ID).expect("task ID should be valid")
}

fn criterion_id() -> CriterionId {
    CriterionId::parse(CRITERION_ID).expect("criterion ID should be valid")
}

fn check_id(value: &str) -> CheckId {
    CheckId::parse(value).expect("check ID should be valid")
}

fn request_id(sequence: u64) -> RequestId {
    RequestId::parse(format!("63000000-0000-0000-0000-{sequence:012x}"))
        .expect("request ID should be valid")
}

fn timestamp(minute: u8) -> Timestamp {
    Timestamp::parse(format!("2026-07-26T07:{minute:02}:00Z")).expect("timestamp should be valid")
}
