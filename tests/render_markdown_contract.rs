//! Contract tests for deterministic managed Markdown projections.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(unix)]
use std::os::unix::fs::symlink;
#[cfg(windows)]
use std::os::windows::fs::symlink_dir;

#[cfg(any(unix, windows))]
use mino::application::plan::{CreatePlanRequest, PlanService};
use mino::domain::{
    AcceptanceCriterion, CheckId, CommitGate, CriterionId, Plan, PlanId, RequestId, Task, TaskId,
    Timestamp, VerificationCheck,
};
use mino::render::{
    ProjectionStatus, ProjectionWriteOutcome, RenderErrorKind, check_projection, render_plan,
    write_projection,
};
use mino::store::{PlanStore, canonical_json_bytes, sha256_digest};
#[cfg(any(unix, windows))]
use mino::{ErrorCategory, project};

static NEXT_TEMP_DIRECTORY: AtomicU64 = AtomicU64::new(1);

struct TestProject {
    path: PathBuf,
}

impl TestProject {
    fn new(label: &str) -> Self {
        let sequence = NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "mino-render-{label}-{}-{sequence}",
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
                .is_some_and(|name| name.to_string_lossy().starts_with("mino-render-"))
        {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

fn timestamp(minute: u8) -> Timestamp {
    Timestamp::parse(format!("2026-07-25T13:{minute:02}:00Z"))
        .expect("test timestamp should be valid")
}

fn plan_id(value: &str) -> PlanId {
    PlanId::parse(value).expect("test plan ID should be valid")
}

fn request_id() -> RequestId {
    RequestId::parse("00000000-0000-0000-0000-000000000001")
        .expect("test request ID should be valid")
}

fn configured_task() -> Task {
    let task_id = TaskId::parse("T1").expect("task ID should be valid");
    let mut task = Task::new(task_id, "Render the plan", Vec::new());
    task.add_acceptance_criterion(AcceptanceCriterion::new(
        CriterionId::parse("T1-A1").expect("criterion ID should be valid"),
        "Projection is stable",
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
        "feat(render): add projection",
        vec!["src/render/**".to_owned()],
    ))
    .expect("commit gate should be set");
    task
}

fn command(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| (*part).to_owned()).collect()
}

#[cfg(any(unix, windows))]
fn assert_plan_create_rejects_projection_symlink(relative: &str) {
    let project_root = TestProject::new(&format!("symlink-{}", relative.replace('/', "-")));
    project::initialize(project_root.path()).expect("project should initialize");
    let external = TestProject::new("symlink-projection-external");
    let sentinel = external.path().join("sentinel.txt");
    fs::write(&sentinel, b"outside\n").expect("outside sentinel should be written");
    let link = project_root.path().join(relative);
    if let Some(parent) = link.parent() {
        fs::create_dir_all(parent).expect("projection parent should be created");
    }
    #[cfg(unix)]
    let symlink_result = symlink(external.path(), &link);
    #[cfg(windows)]
    let symlink_result = symlink_dir(external.path(), &link);
    if symlink_result
        .as_ref()
        .is_err_and(|error| error.kind() == std::io::ErrorKind::PermissionDenied)
    {
        return;
    }
    symlink_result.expect("projection symlink should be created");
    let service = PlanService::discover(project_root.path()).expect("plan service should open");

    let error = service
        .create(CreatePlanRequest {
            name: format!("Reject {relative} symlink"),
            trigger: "durable".to_owned(),
            original_request: "Keep managed projections inside the project.".to_owned(),
            request_id: request_id(),
            actor: "codex".to_owned(),
            command: command(&["mino", "plan", "create"]),
            created_at: timestamp(0),
        })
        .expect_err("projection symlink must be rejected");
    assert_eq!(error.category(), ErrorCategory::DriftDetected);
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

#[test]
fn golden_projection_is_complete_byte_stable_and_lf_only() {
    let plan: Plan = serde_json::from_str(include_str!("fixtures/render/full_plan.json"))
        .expect("full plan fixture should deserialize");
    let first = render_plan(&plan).expect("plan should render");
    let second = render_plan(&plan).expect("repeated plan should render");
    let golden = include_bytes!("fixtures/render/full_plan.md");

    assert_eq!(first, second);
    assert_eq!(first.as_bytes(), golden);
    assert!(first.as_bytes().ends_with(b"\n"));
    assert!(!first.as_bytes().contains(&b'\r'));
    assert_eq!(
        first.state_hash(),
        sha256_digest(&canonical_json_bytes(&plan).expect("plan should canonicalize"))
    );
    assert_eq!(first.projection_digest(), sha256_digest(golden));
    assert!(first.markdown().contains("Renderer \\| Contract"));
    assert!(first.markdown().contains("<br>Never overwrite"));
    assert!(first.markdown().contains("````json"));
}

#[cfg(any(unix, windows))]
#[test]
fn managed_projection_rejects_symlinked_docs_ancestors() {
    for relative in ["docs", "docs/plan"] {
        assert_plan_create_rejects_projection_symlink(relative);
    }
}

#[test]
fn guarded_updates_require_exact_prior_bytes_and_preserve_manual_edits() {
    let project = TestProject::new("guarded");
    let projection_path = project.path().join("docs/plan/render.md");
    let mut plan = Plan::new(
        plan_id("2026-07-25-guarded-render"),
        "Render safely.",
        timestamp(0),
    );
    let initial = render_plan(&plan).expect("initial plan should render");
    let missing = check_projection(&projection_path, &initial).expect("check should succeed");
    assert_eq!(missing.status(), ProjectionStatus::Missing);
    assert_eq!(missing.actual_digest(), None);
    assert_eq!(
        write_projection(&projection_path, &initial, None).expect("projection should be created"),
        ProjectionWriteOutcome::Created
    );
    assert_eq!(
        write_projection(&projection_path, &initial, None).expect("same bytes should be stable"),
        ProjectionWriteOutcome::Unchanged
    );

    plan.add_task(configured_task(), timestamp(1))
        .expect("task should be added");
    let updated = render_plan(&plan).expect("updated plan should render");
    assert_eq!(
        write_projection(&projection_path, &updated, Some(&initial))
            .expect("known prior bytes should update"),
        ProjectionWriteOutcome::Updated
    );
    assert_eq!(
        check_projection(&projection_path, &updated)
            .expect("updated projection should check")
            .status(),
        ProjectionStatus::Current
    );

    let mut file = OpenOptions::new()
        .append(true)
        .open(&projection_path)
        .expect("projection should open");
    file.write_all(b"manual edit\r\n")
        .expect("manual edit should be injected");
    file.sync_all().expect("manual edit should be durable");
    let edited = fs::read(&projection_path).expect("edited projection should be readable");
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
    .expect("global check should be added");
    let next = render_plan(&plan).expect("next plan should render");
    let drift = write_projection(&projection_path, &next, Some(&updated))
        .expect_err("manual edits must never be overwritten");
    assert_eq!(drift.kind(), RenderErrorKind::Drift);
    assert_eq!(
        fs::read(&projection_path).expect("edited projection should remain"),
        edited
    );
    assert_eq!(
        check_projection(&projection_path, &next)
            .expect("drift check should succeed")
            .status(),
        ProjectionStatus::Drifted
    );
}

#[test]
fn drift_detection_does_not_change_plan_state_or_historical_snapshots() {
    let project = TestProject::new("source-state");
    let store = PlanStore::new(project.path());
    let plan = Plan::new(
        plan_id("2026-07-25-source-state"),
        "Preserve source state.",
        timestamp(0),
    );
    store
        .create_plan(
            &plan,
            request_id(),
            "codex",
            command(&["mino", "plan", "create"]),
        )
        .expect("plan should be stored");
    let rendered = render_plan(&plan).expect("stored plan should render");
    let projection_path = project.path().join("docs/plan/source-state.md");
    write_projection(&projection_path, &rendered, None).expect("projection should be created");
    let current_path = store.paths().current_plan(plan.id());
    let snapshot_path = store.paths().snapshot(plan.id(), 1);
    let current_before = fs::read(&current_path).expect("current plan should be readable");
    let snapshot_before = fs::read(&snapshot_path).expect("snapshot should be readable");
    fs::write(&projection_path, b"manually edited\n").expect("manual edit should be injected");

    let error = write_projection(&projection_path, &rendered, None)
        .expect_err("edited projection should report drift");
    assert_eq!(error.kind(), RenderErrorKind::Drift);
    assert_eq!(
        fs::read(current_path).expect("current plan should remain readable"),
        current_before
    );
    assert_eq!(
        fs::read(snapshot_path).expect("snapshot should remain readable"),
        snapshot_before
    );
    assert_eq!(
        store
            .audit(plan.id())
            .expect("store should remain valid")
            .revision(),
        1
    );
}
