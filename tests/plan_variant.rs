//! Contracts for historical plan forks, semantic diffs, and non-destructive archive state.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use mino::ErrorCategory;
use mino::application::plan::{
    DraftMutation, DraftMutationRequest, PlanMutationRequest, PlanService,
};
use mino::application::plan_variant::{ForkPlanRequest, PlanVariantService};
use mino::diff::{DiffCategory, diff_plans};
use mino::domain::{
    Approval, CheckId, CheckStatus, CheckpointKind, CommitStatus, CriterionId, CriterionStatus,
    DraftCommitGateInput, DraftCriterionInput, DraftFileInput, DraftMetadataInput, DraftPlanInput,
    DraftScopeInput, DraftTaskInput, DraftVerificationInput, EvidenceId, FileChange,
    GitFlowConsent, GitReadiness, Plan, PlanDraftSeed, PlanId, PlanStatus, RequestId,
    ReviewClassification, StandardSelection, TaskId, TaskStatus, Timestamp, VerificationCheck,
};
use mino::git::{ActiveBindingStore, GitAdapter};
use mino::project::initialize;
use mino::render::{render_plan, write_projection};
use mino::standards::EmbeddedCatalog;
use mino::store::{
    MutationRequest, PlanStore, StoreErrorKind, canonical_json_bytes, sha256_digest,
};
use serde_json::Value;

static NEXT_TEMP_DIRECTORY: AtomicU64 = AtomicU64::new(1);

struct TestProject {
    path: PathBuf,
}

impl TestProject {
    fn new(label: &str) -> Self {
        let sequence = NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "mino-plan-variant-{label}-{}-{sequence}",
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
                .is_some_and(|name| name.to_string_lossy().starts_with("mino-plan-variant-"))
        {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

fn timestamp(minute: u8) -> Timestamp {
    Timestamp::parse(format!("2026-07-26T01:{minute:02}:00Z")).expect("timestamp should parse")
}

fn plan_id(value: &str) -> PlanId {
    PlanId::parse(value).expect("plan ID should parse")
}

fn request_id(number: u64) -> RequestId {
    RequestId::parse(format!("80000000-0000-0000-0000-{number:012}"))
        .expect("request ID should parse")
}

fn evidence_id(value: &str) -> EvidenceId {
    EvidenceId::parse(value).expect("evidence ID should parse")
}

fn command(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| (*part).to_owned()).collect()
}

fn run_mino(project: &TestProject, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mino"))
        .arg("--root")
        .arg(project.path())
        .args(["--format", "json", "--no-input"])
        .args(arguments)
        .stdin(Stdio::null())
        .output()
        .expect("Mino binary should run")
}

fn retain_binding_after_git_removal(project: &TestProject, plan_id: &PlanId, revision: u64) {
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
        .bind(&facts, plan_id.clone(), revision, timestamp(59))
        .expect("active binding should be written");
    fs::remove_dir_all(project.path().join(".git"))
        .expect("Git repository should be removed from the fixture");
}

fn parse_success(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "JSON success stderr should be empty"
    );
    serde_json::from_slice(&output.stdout).expect("stdout should be JSON")
}

fn parse_failure(output: &Output, exit_code: i32) -> Value {
    assert_eq!(output.status.code(), Some(exit_code));
    assert!(
        output.stderr.is_empty(),
        "JSON failure stderr should be empty"
    );
    serde_json::from_slice(&output.stdout).expect("failure stdout should be JSON")
}

fn readiness() -> GitReadiness {
    GitReadiness::detected(
        "Missing",
        "Not Applicable",
        None,
        None,
        "Not Applicable: variant contract",
        false,
    )
}

#[allow(clippy::too_many_lines)]
fn rich_source_plan() -> Plan {
    let catalog = EmbeddedCatalog::load().expect("embedded standards should load");
    let common = catalog
        .package("common")
        .expect("Common standards should exist");
    let task_id = TaskId::parse("T1").expect("task ID should parse");
    let criterion_id = CriterionId::parse("T1-A1").expect("criterion ID should parse");
    let task_check_id = CheckId::parse("T1-V1").expect("check ID should parse");
    let global_check_id = CheckId::parse("GLOBAL-V1").expect("check ID should parse");
    let mut plan = Plan::from_draft_seed(
        PlanDraftSeed {
            id: plan_id("2026-07-26-rich-source"),
            name: "Rich source".to_owned(),
            trigger: "durable".to_owned(),
            original_request: "Preserve authored values and reset trusted runtime state."
                .to_owned(),
            branch: None,
            markdown_path: "docs/plan/2026-07-26-rich-source.md".to_owned(),
            git_readiness: readiness(),
            standards: vec![StandardSelection::new(
                common.package_id(),
                common.version(),
                common.digest(),
                "embedded",
            )],
            verification_plan: vec![VerificationCheck::new(
                global_check_id.clone(),
                command(&["cargo", "test"]),
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
                area: Some("plan/variant".to_owned()),
                owner: Some("codex".to_owned()),
                ..DraftMetadataInput::default()
            }),
            summary: Some("Compare an alternative without trusting prior execution.".to_owned()),
            scope: Some(DraftScopeInput {
                goal: Some("Create one independently executable alternative.".to_owned()),
                deliverables: Some(vec!["Alternative Draft".to_owned()]),
                in_scope: Some(vec!["Authored plan state".to_owned()]),
                out_of_scope: Some(vec!["Plan merging".to_owned()]),
            }),
            approach: Some("Copy authored state from the audited source snapshot.".to_owned()),
            interfaces: Some("Retained snapshot to independent revision-one Draft.".to_owned()),
            ..DraftPlanInput::default()
        },
        timestamp(1),
    )
    .expect("authored fields should apply");
    plan.author_task(
        DraftTaskInput {
            id: Some(task_id.clone()),
            title: "Exercise trusted execution state".to_owned(),
            depends_on: Vec::new(),
            steps: vec!["Implement the selected approach".to_owned()],
            files: vec![DraftFileInput {
                path: "src/lib.rs".to_owned(),
                change: FileChange::Modify,
                reason: "Own the implementation fixture".to_owned(),
            }],
            acceptance_criteria: vec![DraftCriterionInput {
                id: Some(criterion_id.clone()),
                description: "The implementation is verified".to_owned(),
            }],
            verification: vec![DraftVerificationInput {
                id: task_check_id.clone(),
                command: command(&["cargo", "test"]),
                cwd: ".".to_owned(),
                expected_exit_code: 0,
                required: true,
            }],
            commit_gate: Some(DraftCommitGateInput {
                required: true,
                planned_message: "feat(plan): exercise variant fixture".to_owned(),
                scope: vec!["src/lib.rs".to_owned()],
            }),
        },
        timestamp(2),
    )
    .expect("task should be authored");
    plan.finalize(timestamp(3)).expect("plan should finalize");
    plan.record_approval(Approval::plan(
        "user",
        "chat:plan-approved",
        timestamp(4),
        GitFlowConsent::Approved,
    ))
    .expect("approval should record");
    plan.start_task(&task_id, timestamp(5))
        .expect("task should start");
    plan.record_checkpoint(
        &task_id,
        CheckpointKind::Implementation,
        "Implementation completed",
        "codex",
        timestamp(6),
    )
    .expect("checkpoint should record");
    plan.record_task_criterion_pass(&task_id, &criterion_id, evidence_id("E0001"), timestamp(7))
        .expect("criterion should pass");
    plan.record_task_check_pass(&task_id, &task_check_id, evidence_id("E0002"), timestamp(8))
        .expect("task check should pass");
    plan.complete_task(&task_id, timestamp(9))
        .expect("task should complete");
    plan.record_task_commit(
        &task_id,
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        vec!["src/lib.rs".to_owned()],
        evidence_id("E0003"),
        timestamp(10),
    )
    .expect("commit should record");
    plan.record_global_check_pass(&global_check_id, evidence_id("E0004"), timestamp(11))
        .expect("global check should pass");
    plan.set_final_outcome(
        "Archived alternative was fully verified".to_owned(),
        "N/A".to_owned(),
        Vec::new(),
        timestamp(12),
    )
    .expect("Final Outcome should record");
    plan.finish_execution(timestamp(12))
        .expect("plan should enter Review");
    plan.record_review(
        "reviewer".to_owned(),
        "Track a future comparison enhancement".to_owned(),
        ReviewClassification::FollowUp,
        None,
        timestamp(13),
    )
    .expect("review item should record");
    plan
}

fn stored_draft(project: &TestProject, id: &str, name: &str, number: u64) -> Plan {
    let plan_id = plan_id(id);
    let plan = Plan::from_draft_seed(
        PlanDraftSeed {
            id: plan_id.clone(),
            name: name.to_owned(),
            trigger: "durable".to_owned(),
            original_request: format!("Author {name}."),
            branch: None,
            markdown_path: format!("docs/plan/{plan_id}.md"),
            git_readiness: readiness(),
            standards: Vec::new(),
            verification_plan: Vec::new(),
        },
        timestamp(0),
    );
    PlanStore::new(project.path())
        .create_plan(
            &plan,
            request_id(number),
            "codex",
            command(&["mino", "plan", "create"]),
        )
        .expect("plan should be stored");
    write_current_projection(project.path(), &plan, None);
    plan
}

fn write_current_projection(root: &Path, plan: &Plan, prior: Option<&Plan>) {
    let rendered = render_plan(plan).expect("plan should render");
    let prior = prior.map(|plan| render_plan(plan).expect("prior plan should render"));
    let relative = plan
        .metadata()
        .markdown_path()
        .expect("managed path should exist");
    write_projection(&root.join(relative), &rendered, prior.as_ref())
        .expect("projection should write");
}

fn advance_summary(project: &TestProject, source: &Plan, number: u64) -> Plan {
    let store = PlanStore::new(project.path());
    let updated_at = timestamp(1);
    let input = DraftPlanInput {
        summary: Some("Current source summary".to_owned()),
        ..DraftPlanInput::default()
    };
    store
        .commit(
            source.id(),
            MutationRequest::new(
                source.revision(),
                request_id(number),
                "codex",
                command(&["mino", "plan", "summary", "set"]),
                vec!["summary".to_owned()],
            )
            .expect("mutation request should validate"),
            move |plan| plan.apply_draft_input(input, updated_at),
        )
        .expect("summary should commit");
    let current = store
        .load_plan(source.id())
        .expect("current source should load");
    write_current_projection(project.path(), &current, Some(source));
    current
}

fn source_bytes(store: &PlanStore, plan_id: &PlanId, revisions: &[u64]) -> Vec<Vec<u8>> {
    let mut paths = vec![
        store.paths().current_plan(plan_id),
        store.paths().event_log(plan_id),
    ];
    paths.extend(
        revisions
            .iter()
            .map(|revision| store.paths().snapshot(plan_id, *revision)),
    );
    paths
        .iter()
        .map(|path| fs::read(path).expect("source artifact should be readable"))
        .collect()
}

fn fork_request(source_plan_id: PlanId, number: u64) -> ForkPlanRequest {
    ForkPlanRequest {
        source_plan_id,
        from_revision: 1,
        name: "Alternative design".to_owned(),
        reason: "Compare a smaller implementation".to_owned(),
        request_id: request_id(number),
        actor: "codex".to_owned(),
        command: command(&["mino", "plan", "fork", "--from-revision", "1"]),
        forked_at: timestamp(20),
    }
}

#[test]
fn fork_retains_authored_state_and_resets_every_execution_trust_binding() {
    let source = rich_source_plan();
    let source_before = canonical_json_bytes(&source).expect("source should canonicalize");
    let source_hash = sha256_digest(&source_before);
    let fork = Plan::fork_from_snapshot(
        &source,
        plan_id("2026-07-26-independent-alternative"),
        "Independent alternative".to_owned(),
        "Compare a second approach".to_owned(),
        source_hash.clone(),
        readiness(),
        None,
        "docs/plan/2026-07-26-independent-alternative.md".to_owned(),
        timestamp(30),
    )
    .expect("source should fork");

    assert_eq!(
        canonical_json_bytes(&source).expect("source should remain canonical"),
        source_before
    );
    assert_eq!(fork.revision(), 1);
    assert_eq!(fork.status(), PlanStatus::Draft);
    assert_eq!(fork.original_request(), source.original_request());
    assert_eq!(fork.summary(), source.summary());
    assert_eq!(fork.standards(), source.standards());
    assert_eq!(fork.task_order(), source.task_order());
    assert!(fork.approvals().is_empty());
    assert!(fork.review_items().is_empty());
    assert!(fork.follow_ups().is_empty());
    assert!(!fork.is_archived());
    let lineage = fork.lineage().expect("fork should have lineage");
    assert_eq!(lineage.parent_plan_id(), source.id());
    assert_eq!(lineage.forked_from_revision(), source.revision());
    assert_eq!(lineage.fork_reason(), "Compare a second approach");
    assert_eq!(lineage.source_state_hash(), source_hash);

    let task = &fork.tasks()[0];
    assert_eq!(task.status(), TaskStatus::Draft);
    assert!(task.evidence_refs().is_empty());
    assert_eq!(
        task.acceptance_criteria()[0].status(),
        CriterionStatus::Pending
    );
    assert!(task.acceptance_criteria()[0].evidence_refs().is_empty());
    assert_eq!(task.verification_checks()[0].status(), CheckStatus::Pending);
    assert!(task.verification_checks()[0].evidence_refs().is_empty());
    let gate = task.commit_gate().expect("authored gate should remain");
    assert_eq!(gate.status(), CommitStatus::Pending);
    assert_eq!(
        gate.planned_message(),
        "feat(plan): exercise variant fixture"
    );
    assert_eq!(gate.scope(), ["src/lib.rs"]);
    assert_eq!(gate.actual_commit(), None);
    assert!(gate.committed_files().is_empty());
    assert!(gate.evidence_refs().is_empty());
    assert_eq!(fork.global_verification()[0].status(), CheckStatus::Pending);
    assert!(fork.global_verification()[0].evidence_refs().is_empty());
    let fork_value = serde_json::to_value(&fork).expect("fork should serialize");
    assert_eq!(fork_value["extensions"], serde_json::json!({}));
    assert_eq!(fork_value["final_outcome"]["summary"], "");
}

#[test]
fn fork_uses_the_exact_audited_revision_and_is_retry_safe_and_atomic() {
    let project = TestProject::new("fork-service");
    let source = stored_draft(
        &project,
        "2026-07-26-source-alternative",
        "Source alternative",
        100,
    );
    let current = advance_summary(&project, &source, 101);
    let store = PlanStore::new(project.path());
    let source_before = source_bytes(&store, source.id(), &[1, 2]);
    let service = PlanVariantService::discover(project.path()).expect("service should discover");
    let request = fork_request(source.id().clone(), 102);
    let first = service.fork(request.clone()).expect("fork should succeed");
    let second = service.fork(request).expect("exact retry should replay");

    assert!(!first.operation.replayed);
    assert!(second.operation.replayed);
    assert_eq!(first.operation.plan_id, second.operation.plan_id);
    let fork = store
        .load_plan(&first.operation.plan_id)
        .expect("fork should load");
    let snapshot = store
        .load_snapshot(source.id(), 1)
        .expect("source snapshot should load");
    assert_eq!(fork.summary(), snapshot.summary());
    assert_ne!(fork.summary(), current.summary());
    assert_eq!(fork.lineage().unwrap().forked_from_revision(), 1);
    assert_eq!(
        fork.lineage().unwrap().source_state_hash(),
        sha256_digest(
            &canonical_json_bytes(&snapshot).expect("source snapshot should canonicalize")
        )
    );
    assert_eq!(source_bytes(&store, source.id(), &[1, 2]), source_before);

    let target_before = source_bytes(&store, fork.id(), &[1]);
    let mut collision = fork_request(source.id().clone(), 103);
    collision.reason = "Reuse the same generated identifier".to_owned();
    let error = service
        .fork(collision)
        .expect_err("different request must report a collision");
    assert_eq!(error.category(), ErrorCategory::PolicyViolation);
    assert_eq!(source_bytes(&store, fork.id(), &[1]), target_before);
    assert_eq!(source_bytes(&store, source.id(), &[1, 2]), source_before);
}

#[test]
fn missing_or_corrupt_source_history_never_publishes_a_target() {
    let missing_project = TestProject::new("fork-missing");
    let missing_source = stored_draft(
        &missing_project,
        "2026-07-26-missing-source",
        "Missing source",
        200,
    );
    let missing_store = PlanStore::new(missing_project.path());
    let missing_before = source_bytes(&missing_store, missing_source.id(), &[1]);
    let missing_service =
        PlanVariantService::discover(missing_project.path()).expect("service should discover");
    let mut missing_request = fork_request(missing_source.id().clone(), 201);
    missing_request.from_revision = 99;
    missing_request.name = "Missing snapshot target".to_owned();
    assert!(missing_service.fork(missing_request).is_err());
    let missing_target = plan_id("2026-07-26-missing-snapshot-target");
    assert!(
        !missing_store
            .paths()
            .plan_directory(&missing_target)
            .exists()
    );
    assert_eq!(
        source_bytes(&missing_store, missing_source.id(), &[1]),
        missing_before
    );

    let corrupt_project = TestProject::new("fork-corrupt");
    let corrupt_source = stored_draft(
        &corrupt_project,
        "2026-07-26-corrupt-source",
        "Corrupt source",
        210,
    );
    let corrupt_store = PlanStore::new(corrupt_project.path());
    fs::write(
        corrupt_store.paths().snapshot(corrupt_source.id(), 1),
        b"{\"corrupt\":true}\n",
    )
    .expect("snapshot corruption should be injected");
    let corrupt_before = source_bytes(&corrupt_store, corrupt_source.id(), &[1]);
    let corrupt_service =
        PlanVariantService::discover(corrupt_project.path()).expect("service should discover");
    let mut corrupt_request = fork_request(corrupt_source.id().clone(), 211);
    corrupt_request.name = "Corrupt snapshot target".to_owned();
    let error = corrupt_service
        .fork(corrupt_request)
        .expect_err("corrupt history must fail audit");
    assert_eq!(error.category(), ErrorCategory::DriftDetected);
    let corrupt_target = plan_id("2026-07-26-corrupt-snapshot-target");
    assert!(
        !corrupt_store
            .paths()
            .plan_directory(&corrupt_target)
            .exists()
    );
    assert_eq!(
        source_bytes(&corrupt_store, corrupt_source.id(), &[1]),
        corrupt_before
    );
}

#[test]
fn semantic_diff_is_stable_directional_revision_aware_and_non_mutating() {
    let project = TestProject::new("diff-service");
    let source = stored_draft(&project, "2026-07-26-diff-source", "Diff source", 300);
    let current = advance_summary(&project, &source, 301);
    let store = PlanStore::new(project.path());
    let source_before = source_bytes(&store, source.id(), &[1, 2]);
    let service = PlanVariantService::discover(project.path()).expect("service should discover");

    let forward = service
        .diff(source.id(), Some(1), source.id(), None)
        .expect("historical-to-current diff should succeed");
    let repeated = service
        .diff(source.id(), Some(1), source.id(), Some(2))
        .expect("explicit historical diff should succeed");
    let reverse = service
        .diff(source.id(), Some(2), source.id(), Some(1))
        .expect("reverse diff should succeed");
    assert_eq!(forward, repeated);
    assert!(forward.protocol_compatible);
    assert_eq!(forward.diff_kind, "mino.plan-diff/v1");
    assert_eq!(forward.changes.len(), 1);
    assert_eq!(forward.changes[0].category, DiffCategory::Changed);
    assert_eq!(forward.changes[0].path, "summary");
    assert_eq!(forward.changes[0].before, Some(Value::from("")));
    assert_eq!(
        forward.changes[0].after,
        Some(Value::from("Current source summary"))
    );
    assert_eq!(reverse.changes.len(), 1);
    assert_eq!(reverse.changes[0].category, DiffCategory::Changed);
    assert_eq!(reverse.changes[0].path, forward.changes[0].path);
    assert_eq!(reverse.changes[0].before, forward.changes[0].after);
    assert_eq!(reverse.changes[0].after, forward.changes[0].before);
    assert!(
        forward
            .render_human()
            .contains("2026-07-26-diff-source@1 -> 2026-07-26-diff-source@2")
    );
    assert_eq!(source_bytes(&store, source.id(), &[1, 2]), source_before);
    assert_eq!(current.revision(), 2);

    let left = rich_source_plan();
    let mut value = serde_json::to_value(&left).expect("plan should serialize");
    let task = value["tasks"][0].clone();
    let mut second_task = task;
    second_task["id"] = Value::from("T2");
    second_task["title"] = Value::from("Second independent task");
    second_task["acceptance_criteria"][0]["id"] = Value::from("T2-A1");
    second_task["verification_checks"][0]["id"] = Value::from("T2-V1");
    second_task["file_map"][0]["task_id"] = Value::from("T2");
    value["tasks"]
        .as_array_mut()
        .expect("tasks should be an array")
        .push(second_task);
    value["task_order"] = serde_json::json!(["T1", "T2"]);
    let ordered: Plan = serde_json::from_value(value.clone()).expect("ordered plan should parse");
    value["task_order"] = serde_json::json!(["T2", "T1"]);
    let reordered: Plan = serde_json::from_value(value).expect("reordered plan should parse");
    let moved = diff_plans(&ordered, &reordered).expect("plans should compare");
    assert_eq!(
        moved
            .changes
            .iter()
            .filter(|change| change.category == DiffCategory::Moved)
            .map(|change| change.path.as_str())
            .collect::<Vec<_>>(),
        ["task_order.T1", "task_order.T2"]
    );
    let additions = diff_plans(&left, &ordered).expect("added task should compare");
    let removals = diff_plans(&ordered, &left).expect("removed task should compare");
    assert_eq!(additions.changes.len(), 2);
    for addition in &additions.changes {
        assert_eq!(addition.category, DiffCategory::Added);
        let removal = removals
            .changes
            .iter()
            .find(|change| change.path == addition.path)
            .expect("reverse diff should contain the same path");
        assert_eq!(removal.category, DiffCategory::Removed);
        assert_eq!(removal.before, addition.after);
        assert_eq!(removal.after, addition.before);
    }
}

#[test]
#[allow(clippy::too_many_lines)]
fn cli_exposes_the_complete_fork_diff_archive_and_active_selection_sequence() {
    let project = TestProject::new("cli-sequence");
    let source = stored_draft(&project, "2026-07-26-cli-source", "CLI source", 350);
    let source_id = source.id().to_string();
    retain_binding_after_git_removal(&project, source.id(), source.revision());
    let forked = parse_success(&run_mino(
        &project,
        &[
            "plan",
            "fork",
            "--plan",
            &source_id,
            "--from-revision",
            "1",
            "--name",
            "CLI alternative",
            "--reason",
            "Compare the CLI alternative",
            "--request-id",
            "80000000-0000-0000-0000-000000000351",
            "--actor",
            "codex",
        ],
    ));
    assert_eq!(forked["kind"], "mino.result/v1");
    assert_eq!(forked["revision"], 1);
    assert_eq!(forked["status"], "Draft");
    assert_eq!(forked["lineage"]["parent_plan_id"], source_id);
    let fork_id = forked["plan_id"]
        .as_str()
        .expect("fork should return a plan ID")
        .to_owned();

    let diff = parse_success(&run_mino(
        &project,
        &["plan", "diff", "--left", &source_id, "--right", &fork_id],
    ));
    assert_eq!(diff["kind"], "mino.result/v1");
    assert_eq!(diff["diff_kind"], "mino.plan-diff/v1");
    assert!(diff["changes"].as_array().is_some_and(|changes| {
        changes
            .iter()
            .any(|change| change["category"] == "changed" && change["path"] == "metadata.name")
    }));

    let alternatives = parse_success(&run_mino(&project, &["plan", "alternatives"]));
    assert_eq!(alternatives["selection_revision"], 1);
    assert_eq!(alternatives["selected_plan"], source_id);
    assert_eq!(alternatives["alternatives"], serde_json::json!([fork_id]));
    let comparison_context = parse_success(&run_mino(&project, &["agent", "context"]));
    assert_eq!(comparison_context["active_plan"]["id"], source_id);
    assert_eq!(
        comparison_context["plan_selection"]["selection_revision"],
        1
    );
    assert_eq!(
        comparison_context["plan_selection"]["selected_plan"],
        source_id
    );
    assert_eq!(
        comparison_context["plan_selection"]["alternatives"],
        serde_json::json!([fork_id])
    );
    assert_eq!(comparison_context["approval_required"], true);
    assert_eq!(
        comparison_context["next_actions"][0]["id"],
        "plan.alternatives"
    );

    let selected_archive = parse_failure(
        &run_mino(
            &project,
            &[
                "plan",
                "archive",
                "--plan",
                &source_id,
                "--expect-revision",
                "1",
                "--request-id",
                "80000000-0000-0000-0000-000000000352",
                "--actor",
                "codex",
                "--reason",
                "Attempt to archive the selected plan",
                "--approval-ref",
                "chat:selected-archive-refused",
            ],
        ),
        5,
    );
    assert_eq!(selected_archive["error"]["code"], "policy_violation");

    let selection_arguments = [
        "plan",
        "select",
        "--plan",
        &fork_id,
        "--expect-selection-revision",
        "1",
        "--request-id",
        "80000000-0000-0000-0000-000000000353",
        "--actor",
        "codex",
        "--approval-ref",
        "chat:cli-alternative-selected",
        "--reason",
        "Choose the CLI alternative",
    ];
    let selected = parse_success(&run_mino(&project, &selection_arguments));
    assert_eq!(selected["selection_revision"], 2);
    assert_eq!(selected["selected_plan"], fork_id);
    assert_eq!(selected["alternatives"], serde_json::json!([source_id]));
    let replayed_selection = parse_success(&run_mino(&project, &selection_arguments));
    assert_eq!(replayed_selection["replayed"], true);
    assert_eq!(replayed_selection["selection_revision"], 2);

    let archived = parse_success(&run_mino(
        &project,
        &[
            "plan",
            "archive",
            "--plan",
            &source_id,
            "--expect-revision",
            "1",
            "--request-id",
            "80000000-0000-0000-0000-000000000354",
            "--actor",
            "codex",
            "--reason",
            "Select the CLI alternative",
            "--approval-ref",
            "chat:cli-alternative-selected",
        ],
    ));
    assert_eq!(archived["revision"], 2);
    assert_eq!(archived["status"], "Draft");

    let context = parse_success(&run_mino(&project, &["agent", "context"]));
    assert_eq!(context["kind"], "mino.agent-context/v1");
    assert_eq!(context["active_plan"]["id"], fork_id);
    let shown = parse_success(&run_mino(&project, &["plan", "show", "--plan", &source_id]));
    assert_eq!(
        shown["archive"]["approval_reference"],
        "chat:cli-alternative-selected"
    );

    let refused = parse_failure(
        &run_mino(
            &project,
            &[
                "plan",
                "summary",
                "set",
                "--plan",
                &source_id,
                "--expect-revision",
                "2",
                "--request-id",
                "80000000-0000-0000-0000-000000000355",
                "--actor",
                "codex",
                "--value",
                "Attempt an archived mutation",
            ],
        ),
        5,
    );
    assert_eq!(refused["error"]["code"], "policy_violation");
}

#[test]
fn non_git_fallback_returns_none_when_retained_binding_has_no_plan_candidate() {
    let project = TestProject::new("non-git-empty");
    retain_binding_after_git_removal(&project, &plan_id("2026-07-26-missing-binding-target"), 1);
    let service = PlanService::discover(project.path()).expect("plan service should discover");
    assert!(
        service
            .active_plan()
            .expect("empty non-Git fallback should resolve")
            .is_none()
    );
}

#[test]
fn legacy_multiple_candidates_remain_visible_until_explicit_selection() {
    let project = TestProject::new("legacy-selection");
    let first = stored_draft(&project, "2026-07-26-legacy-first", "Legacy first", 380);
    let second = stored_draft(&project, "2026-07-26-legacy-second", "Legacy second", 381);
    let context = parse_success(&run_mino(&project, &["agent", "context"]));
    assert_eq!(context["active_plan"], Value::Null);
    assert_eq!(context["plan_selection"]["selection_revision"], 0);
    assert_eq!(
        context["plan_selection"]["alternatives"],
        serde_json::json!([first.id(), second.id()])
    );
    assert_eq!(context["approval_required"], true);
    assert_eq!(context["next_actions"][0]["id"], "plan.alternatives");

    let selected = parse_success(&run_mino(
        &project,
        &[
            "plan",
            "select",
            "--plan",
            second.id().as_str(),
            "--expect-selection-revision",
            "0",
            "--request-id",
            "80000000-0000-0000-0000-000000000382",
            "--actor",
            "codex",
            "--approval-ref",
            "chat:legacy-selection",
            "--reason",
            "Choose the second retained alternative",
        ],
    ));
    assert_eq!(selected["selection_revision"], 1);
    assert_eq!(selected["selected_plan"], second.id().as_str());
    let selected_context = parse_success(&run_mino(&project, &["agent", "context"]));
    assert_eq!(selected_context["active_plan"]["id"], second.id().as_str());
    assert_eq!(
        selected_context["plan_selection"]["alternatives"],
        serde_json::json!([first.id()])
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn archive_is_approval_bound_auditable_retry_safe_and_semantically_inactive() {
    let project = TestProject::new("archive-service");
    let source = stored_draft(&project, "2026-07-26-archive-source", "Archive source", 400);
    let store = PlanStore::new(project.path());
    let snapshot_one_before = fs::read(store.paths().snapshot(source.id(), 1))
        .expect("source snapshot should be readable");
    let plans = PlanService::discover(project.path()).expect("plan service should discover");
    assert_eq!(plans.active_plan().unwrap().unwrap().id(), source.id());
    let service = PlanVariantService::discover(project.path()).expect("service should discover");
    let request = PlanMutationRequest {
        plan_id: source.id().clone(),
        expected_revision: 1,
        request_id: request_id(401),
        actor: "codex".to_owned(),
        command: command(&["mino", "plan", "archive"]),
        updated_at: timestamp(40),
    };
    let first = service
        .archive(
            request.clone(),
            "Select the alternative".to_owned(),
            "chat:alternative-selected".to_owned(),
        )
        .expect("archive should record");
    let replay = service
        .archive(
            request,
            "Select the alternative".to_owned(),
            "chat:alternative-selected".to_owned(),
        )
        .expect("exact archive retry should replay");
    assert!(!first.replayed);
    assert!(replay.replayed);

    let archived = store
        .load_plan(source.id())
        .expect("archived plan should load");
    assert_eq!(archived.status(), PlanStatus::Draft);
    assert_eq!(archived.revision(), 2);
    let record = archived
        .archive_record()
        .expect("archive should be present");
    assert_eq!(record.reason(), "Select the alternative");
    assert_eq!(record.actor(), "codex");
    assert_eq!(record.approval_reference(), "chat:alternative-selected");
    assert_eq!(
        fs::read(store.paths().snapshot(source.id(), 1))
            .expect("source snapshot should remain readable"),
        snapshot_one_before
    );
    assert!(store.paths().snapshot(source.id(), 2).is_file());
    assert!(store.paths().event_log(source.id()).is_file());
    assert!(
        plans
            .active_plan()
            .expect("active plan should resolve")
            .is_none()
    );
    let projection = fs::read_to_string(
        project
            .path()
            .join(archived.metadata().markdown_path().unwrap()),
    )
    .expect("projection should be readable");
    assert!(projection.contains("## Archive"));
    assert!(projection.contains("chat:alternative-selected"));

    let diff = service
        .diff(source.id(), Some(1), source.id(), None)
        .expect("archive-only revisions should compare");
    assert!(diff.changes.is_empty());
    let mutation_error = plans
        .mutate(
            DraftMutationRequest {
                plan_id: source.id().clone(),
                expected_revision: 2,
                request_id: request_id(402),
                actor: "codex".to_owned(),
                command: command(&["mino", "plan", "summary", "set"]),
                updated_at: timestamp(41),
            },
            &DraftMutation::Summary("Archived plans are immutable".to_owned()),
        )
        .expect_err("archived plans must reject fresh mutations");
    assert_eq!(mutation_error.category(), ErrorCategory::PolicyViolation);

    let store_error = store
        .commit(
            source.id(),
            MutationRequest::new(
                2,
                request_id(403),
                "codex",
                command(&["mino", "exec", "start"]),
                vec!["status".to_owned()],
            )
            .expect("store request should validate"),
            |plan| {
                plan.apply_draft_input(
                    DraftPlanInput {
                        summary: Some("Bypass the archive boundary".to_owned()),
                        ..DraftPlanInput::default()
                    },
                    timestamp(42),
                )
            },
        )
        .expect_err("the store must reject every fresh archived-plan mutation");
    assert_eq!(store_error.kind(), StoreErrorKind::InvalidMutation);
}
