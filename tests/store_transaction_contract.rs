//! Contract tests for recoverable revisioned plan storage.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

#[cfg(unix)]
use std::os::unix::fs::symlink;
#[cfg(windows)]
use std::os::windows::fs::symlink_dir;

use mino::domain::{
    AcceptanceCriterion, CheckId, CommitGate, CriterionId, Plan, PlanId, RequestId, Task, TaskId,
    Timestamp, VerificationCheck,
};
use mino::store::{
    CommitOptions, FailurePoint, LockOptions, MutationRequest, PlanStore, StoreErrorKind,
    sha256_digest,
};

static NEXT_TEMP_DIRECTORY: AtomicU64 = AtomicU64::new(1);

struct TestProject {
    path: PathBuf,
}

impl TestProject {
    fn new(label: &str) -> Self {
        let sequence = NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "mino-store-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("temporary project should be created");
        Self { path }
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
                .is_some_and(|name| name.to_string_lossy().starts_with("mino-store-"))
        {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

fn timestamp(minute: u8) -> Timestamp {
    Timestamp::parse(format!("2026-07-25T12:{minute:02}:00Z"))
        .expect("test timestamp should be valid")
}

fn plan_id() -> PlanId {
    PlanId::parse("2026-07-25-store-contract").expect("test plan ID should be valid")
}

fn request_id(sequence: u64) -> RequestId {
    RequestId::parse(format!("00000000-0000-0000-0000-{sequence:012x}"))
        .expect("test request ID should be valid")
}

fn task_id() -> TaskId {
    TaskId::parse("T1").expect("test task ID should be valid")
}

fn configured_task() -> Task {
    let id = task_id();
    let mut task = Task::new(id, "Persist one task", Vec::new());
    task.add_acceptance_criterion(AcceptanceCriterion::new(
        CriterionId::parse("T1-A1").expect("criterion ID should be valid"),
        "The task is durably stored",
    ))
    .expect("criterion should be added");
    task.add_verification_check(VerificationCheck::new(
        CheckId::parse("T1-V1").expect("check ID should be valid"),
        vec!["cargo".to_owned(), "test".to_owned()],
        ".",
        0,
        true,
    ))
    .expect("check should be added");
    task.set_commit_gate(CommitGate::new(
        true,
        "feat(store): persist task",
        vec!["src/store/**".to_owned()],
    ))
    .expect("commit gate should be set");
    task
}

fn command(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| (*part).to_owned()).collect()
}

fn mutation_request(
    sequence: u64,
    command_parts: &[&str],
    changed_fields: &[&str],
) -> MutationRequest {
    MutationRequest::new(
        1,
        request_id(sequence),
        "codex",
        command(command_parts),
        changed_fields
            .iter()
            .map(|field| (*field).to_owned())
            .collect(),
    )
    .expect("mutation request should be valid")
}

fn create_plan(store: &PlanStore) -> mino::store::CommitReceipt {
    store
        .create_plan(
            &Plan::new(plan_id(), "Verify recoverable storage.", timestamp(0)),
            request_id(1),
            "codex",
            command(&["mino", "plan", "create"]),
        )
        .expect("plan should be created")
}

#[cfg(any(unix, windows))]
#[test]
fn plan_store_rejects_a_symlinked_plans_directory() {
    let project = TestProject::new("symlink-plans");
    let external = TestProject::new("symlink-plans-external");
    fs::create_dir(project.path().join(".mino")).expect("Mino directory should be created");
    let sentinel = external.path().join("sentinel.txt");
    fs::write(&sentinel, b"outside\n").expect("outside sentinel should be written");
    #[cfg(unix)]
    let symlink_result = symlink(external.path(), project.path().join(".mino/plans"));
    #[cfg(windows)]
    let symlink_result = symlink_dir(external.path(), project.path().join(".mino/plans"));
    if symlink_result
        .as_ref()
        .is_err_and(|error| error.kind() == std::io::ErrorKind::PermissionDenied)
    {
        return;
    }
    symlink_result.expect("plans symlink should be created");

    let error = PlanStore::new(project.path())
        .create_plan(
            &Plan::new(plan_id(), "Reject escaped storage.", timestamp(0)),
            request_id(1),
            "codex",
            command(&["mino", "plan", "create"]),
        )
        .expect_err("symlinked plans directory must be rejected");
    assert_eq!(error.kind(), StoreErrorKind::CorruptState);
    assert_eq!(
        fs::read(&sentinel).expect("outside sentinel should remain readable"),
        b"outside\n"
    );
    assert_eq!(
        fs::read_dir(external.path())
            .expect("outside directory should remain readable")
            .count(),
        1
    );
}

fn commit_task(
    store: &PlanStore,
    request: RequestId,
    options: CommitOptions,
) -> Result<mino::store::CommitReceipt, mino::store::StoreError> {
    store.commit_with_options(
        &plan_id(),
        MutationRequest::new(
            1,
            request,
            "codex",
            command(&["mino", "plan", "task", "add", "T1"]),
            vec!["tasks".to_owned(), "task_order".to_owned()],
        )
        .expect("mutation request should be valid"),
        options,
        |plan| plan.add_task(configured_task(), timestamp(1)),
    )
}

fn replay_task(store: &PlanStore, request: RequestId) -> mino::store::CommitReceipt {
    store
        .commit(
            &plan_id(),
            MutationRequest::new(
                1,
                request,
                "codex",
                command(&["mino", "plan", "task", "add", "T1"]),
                vec!["tasks".to_owned(), "task_order".to_owned()],
            )
            .expect("mutation request should be valid"),
            |_| panic!("idempotent replay must not invoke the mutation"),
        )
        .expect("request should replay its original result")
}

#[test]
fn successful_commits_bind_state_events_snapshots_and_replay() {
    let project = TestProject::new("success");
    let store = PlanStore::new(project.path());
    let creation = create_plan(&store);
    assert_eq!(creation.revision(), 1);
    let creation_replay = store
        .create_plan(
            &Plan::new(plan_id(), "Verify recoverable storage.", timestamp(0)),
            request_id(1),
            "codex",
            command(&["mino", "plan", "create"]),
        )
        .expect("creation request should replay");
    assert!(creation_replay.is_replay());

    let multi_revision = store
        .commit(
            &plan_id(),
            mutation_request(
                4,
                &["mino", "plan", "invalid-multi-revision"],
                &["tasks", "verification_plan"],
            ),
            |plan| {
                plan.add_task(configured_task(), timestamp(1))?;
                plan.add_global_verification(
                    VerificationCheck::new(
                        CheckId::parse("GLOBAL-V1").expect("check ID should be valid"),
                        vec!["cargo".to_owned(), "test".to_owned()],
                        ".",
                        0,
                        true,
                    ),
                    timestamp(2),
                )
            },
        )
        .expect_err("one request must not advance multiple revisions");
    assert_eq!(multi_revision.kind(), StoreErrorKind::InvalidMutation);
    let initial_audit = store.audit(&plan_id()).expect("initial store should audit");
    assert_eq!(initial_audit.revision(), 1);
    assert_eq!(initial_audit.event_count(), 1);
    let request = request_id(2);
    let receipt = commit_task(&store, request.clone(), CommitOptions::default())
        .expect("task mutation should commit");

    assert_eq!(receipt.revision(), 2);
    assert_eq!(receipt.event_sequence(), 2);
    assert_eq!(receipt.state_hash(), receipt.snapshot_digest());
    assert!(!receipt.is_replay());
    let current_bytes =
        fs::read(store.paths().current_plan(&plan_id())).expect("current plan should be readable");
    let snapshot_bytes =
        fs::read(store.paths().snapshot(&plan_id(), 2)).expect("snapshot should be readable");
    assert_eq!(current_bytes, snapshot_bytes);
    assert_eq!(sha256_digest(&current_bytes), receipt.state_hash());

    let audit = store.audit(&plan_id()).expect("store should audit cleanly");
    assert_eq!(audit.revision(), 2);
    assert_eq!(audit.event_count(), 2);
    assert_eq!(audit.snapshot_count(), 2);
    assert_eq!(audit.state_hash(), receipt.state_hash());
    let replay = replay_task(&store, request.clone());
    assert!(replay.is_replay());
    assert_eq!(replay.state_hash(), receipt.state_hash());

    let conflict = store
        .commit(
            &plan_id(),
            MutationRequest::new(
                1,
                request,
                "codex",
                command(&["mino", "different", "command"]),
                vec!["tasks".to_owned()],
            )
            .expect("mutation request should be valid"),
            |_| panic!("conflicting replay must not invoke the mutation"),
        )
        .expect_err("a request ID must not identify two commands");
    assert_eq!(conflict.kind(), StoreErrorKind::RequestConflict);
    let stale = store
        .commit(
            &plan_id(),
            mutation_request(3, &["mino", "plan", "noop"], &["metadata"]),
            |_| panic!("stale mutation must not be invoked"),
        )
        .expect_err("stale revision should fail");
    assert_eq!(stale.kind(), StoreErrorKind::StaleRevision);
    assert_eq!(
        store.audit(&plan_id()).expect("audit should remain clean"),
        audit
    );
}

#[test]
fn every_publication_boundary_recovers_to_one_complete_revision() {
    let failure_points = [
        FailurePoint::BeforeJournal,
        FailurePoint::AfterJournal,
        FailurePoint::BeforeSnapshot,
        FailurePoint::AfterSnapshot,
        FailurePoint::BeforeEvent,
        FailurePoint::AfterEvent,
        FailurePoint::BeforePlanBackup,
        FailurePoint::AfterPlanBackup,
        FailurePoint::BeforePlanPublish,
        FailurePoint::AfterPlanPublish,
    ];
    for (index, failure_point) in failure_points.into_iter().enumerate() {
        let project = TestProject::new(&format!("failure-{index}"));
        let store = PlanStore::new(project.path());
        create_plan(&store);
        let request = request_id(u64::try_from(index).expect("index should fit") + 10);
        let failure = commit_task(
            &store,
            request.clone(),
            CommitOptions::fail_at(failure_point),
        )
        .expect_err("configured boundary should interrupt");
        assert_eq!(failure.kind(), StoreErrorKind::InjectedFailure);

        let recovery = store.recover(&plan_id()).expect("journal should recover");
        let should_roll_forward = failure_point != FailurePoint::BeforeJournal;
        assert_eq!(recovery.was_recovered(), should_roll_forward);
        assert_eq!(recovery.revision(), if should_roll_forward { 2 } else { 1 });
        let initial_audit = store
            .audit(&plan_id())
            .expect("recovered store should audit");
        assert_eq!(
            initial_audit.event_count(),
            if should_roll_forward { 2 } else { 1 }
        );
        assert_eq!(
            initial_audit.snapshot_count(),
            if should_roll_forward { 2 } else { 1 }
        );
        assert!(
            !store
                .paths()
                .plan_directory(&plan_id())
                .join("transaction")
                .exists()
        );
        let retry = if should_roll_forward {
            replay_task(&store, request)
        } else {
            commit_task(&store, request, CommitOptions::default())
                .expect("pre-journal interruption should retry normally")
        };
        assert_eq!(retry.is_replay(), should_roll_forward);
        assert_eq!(
            store
                .load_plan(&plan_id())
                .expect("plan should load")
                .tasks()
                .len(),
            1
        );
        assert_eq!(
            store.events(&plan_id()).expect("events should load").len(),
            2
        );
    }
}

#[test]
fn recovery_discards_a_partial_event_tail_before_replaying_the_journal_event() {
    let project = TestProject::new("partial-event");
    let store = PlanStore::new(project.path());
    create_plan(&store);
    let failure = commit_task(
        &store,
        request_id(20),
        CommitOptions::fail_at(FailurePoint::AfterSnapshot),
    )
    .expect_err("commit should stop before its event");
    assert_eq!(failure.kind(), StoreErrorKind::InjectedFailure);
    let mut event_log = OpenOptions::new()
        .append(true)
        .open(store.paths().event_log(&plan_id()))
        .expect("event log should open");
    event_log
        .write_all(b"{\"partial\"")
        .expect("partial tail should be injected");
    event_log
        .sync_all()
        .expect("partial tail should be durable");

    let recovery = store
        .recover(&plan_id())
        .expect("partial tail should recover");
    assert!(recovery.was_recovered());
    assert_eq!(
        store.events(&plan_id()).expect("events should load").len(),
        2
    );
    assert_eq!(
        store
            .audit(&plan_id())
            .expect("audit should pass")
            .revision(),
        2
    );
}

#[test]
fn bounded_lock_contention_fails_without_mutating_state() {
    let project = TestProject::new("lock");
    let lock_options = LockOptions::new(Duration::from_millis(75), Duration::from_millis(5))
        .expect("lock options should be valid");
    let store = PlanStore::with_lock_options(project.path(), lock_options);
    create_plan(&store);
    let first_store = store.clone();
    let (entered_sender, entered_receiver) = mpsc::channel();
    let (release_sender, release_receiver) = mpsc::channel();
    let first_commit = thread::spawn(move || {
        first_store.commit(
            &plan_id(),
            mutation_request(
                30,
                &["mino", "plan", "task", "add", "T1"],
                &["tasks", "task_order"],
            ),
            |plan| {
                entered_sender.send(()).expect("entry signal should send");
                release_receiver
                    .recv_timeout(Duration::from_secs(2))
                    .expect("release signal should arrive");
                plan.add_task(configured_task(), timestamp(1))
            },
        )
    });
    entered_receiver
        .recv_timeout(Duration::from_secs(2))
        .expect("first commit should hold the lock");
    let contended = store
        .commit(
            &plan_id(),
            mutation_request(31, &["mino", "plan", "noop"], &["metadata"]),
            |_| panic!("contended mutation must not run"),
        )
        .expect_err("second commit should reach the bounded timeout");
    assert_eq!(contended.kind(), StoreErrorKind::LockTimeout);
    release_sender
        .send(())
        .expect("first commit should release");
    let first_receipt = first_commit
        .join()
        .expect("first commit thread should not panic")
        .expect("first commit should succeed");
    assert_eq!(first_receipt.revision(), 2);
    let audit = store
        .audit(&plan_id())
        .expect("store should remain consistent");
    assert_eq!(audit.event_count(), 2);
    assert_eq!(audit.snapshot_count(), 2);
}

#[test]
fn audit_rejects_tampered_immutable_snapshots() {
    let project = TestProject::new("tamper");
    let store = PlanStore::new(project.path());
    create_plan(&store);
    fs::write(store.paths().snapshot(&plan_id(), 1), b"tampered\n")
        .expect("snapshot tampering should be injected");
    let error = store
        .audit(&plan_id())
        .expect_err("tampered immutable history must fail audit");
    assert_eq!(error.kind(), StoreErrorKind::CorruptState);
}
