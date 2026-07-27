//! Protected amendment classification, invalidation, rollback, and review contracts.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use mino::domain::{
    AcceptanceCriterion, AmendmentClassification, AmendmentPatch, AmendmentStatus, Approval,
    CheckId, CheckStatus, CheckpointKind, CommitGate, CriterionId, CriterionStatus, EvidenceId,
    FileChange, FileMapEntry, GitFlowConsent, GitReadiness, MaterialReviewDisposition, Plan,
    PlanDraftSeed, PlanId, PlanStatus, ReviewClassification, ReviewStatus, StandardSelection, Task,
    TaskId, TaskStatus, Timestamp, VerificationCheck,
};
use mino::project::initialize;
use mino::store::{canonical_json_bytes, sha256_digest};
use serde_json::json;

static NEXT_TEMP_DIRECTORY: AtomicU64 = AtomicU64::new(1);

struct TestProject {
    path: PathBuf,
}

impl TestProject {
    fn new() -> Self {
        let sequence = NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "mino-plan-amendment-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("temporary project should be created");
        fs::write(
            path.join("Cargo.toml"),
            "[package]\nname = \"amendment-fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .expect("fixture manifest should be written");
        fs::create_dir(path.join("src")).expect("source directory should be created");
        fs::write(path.join("src/lib.rs"), "pub fn fixture() -> u8 { 1 }\n")
            .expect("fixture source should be written");
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
                .is_some_and(|name| name.to_string_lossy().starts_with("mino-plan-amendment-"))
        {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

fn timestamp(minute: u8) -> Timestamp {
    Timestamp::parse(format!("2026-07-26T10:{minute:02}:00Z")).expect("timestamp should parse")
}

fn plan_id(label: &str) -> PlanId {
    PlanId::parse(format!("2026-07-26-{label}")).expect("plan ID should parse")
}

fn task_id(value: &str) -> TaskId {
    TaskId::parse(value).expect("task ID should parse")
}

fn check_id(value: &str) -> CheckId {
    CheckId::parse(value).expect("check ID should parse")
}

fn criterion_id(value: &str) -> CriterionId {
    CriterionId::parse(value).expect("criterion ID should parse")
}

fn evidence_id(number: u16) -> EvidenceId {
    EvidenceId::parse(format!("E{number:04}")).expect("evidence ID should parse")
}

fn state_hash(plan: &Plan) -> String {
    sha256_digest(&canonical_json_bytes(plan).expect("plan should canonicalize"))
}

fn patch(value: &serde_json::Value) -> AmendmentPatch {
    serde_json::from_value(json!({ "operations": value })).expect("patch should parse")
}

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/drafts")
        .join(name)
}

fn run_mino(arguments: &[String]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mino"))
        .args(arguments)
        .stdin(Stdio::null())
        .output()
        .expect("Mino binary should run")
}

fn base_arguments(project: &TestProject) -> Vec<String> {
    vec![
        "--root".to_owned(),
        project.path().to_string_lossy().into_owned(),
        "--format".to_owned(),
        "json".to_owned(),
        "--no-input".to_owned(),
    ]
}

fn request_id(number: u64) -> String {
    format!("71000000-0000-0000-0000-{number:012}")
}

fn parse_success(output: &Output) -> serde_json::Value {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty(), "JSON stderr should be empty");
    serde_json::from_slice(&output.stdout).expect("stdout should be JSON")
}

fn mutation_arguments(
    project: &TestProject,
    command: &[&str],
    plan_id: &str,
    expected_revision: u64,
    request_number: u64,
) -> Vec<String> {
    let mut arguments = base_arguments(project);
    arguments.extend(command.iter().map(|part| (*part).to_owned()));
    arguments.extend([
        "--plan".to_owned(),
        plan_id.to_owned(),
        "--expect-revision".to_owned(),
        expected_revision.to_string(),
        "--request-id".to_owned(),
        request_id(request_number),
        "--actor".to_owned(),
        "codex".to_owned(),
    ]);
    arguments
}

fn create_approved_cli_plan(project: &TestProject) -> (String, u64) {
    let request_file = project.path().join("request.md");
    fs::write(
        &request_file,
        "Exercise the protected amendment protocol.\n",
    )
    .expect("request should be written");
    let mut create = base_arguments(project);
    create.extend([
        "plan".to_owned(),
        "create".to_owned(),
        "--name".to_owned(),
        "amendment-cli".to_owned(),
        "--trigger".to_owned(),
        "durable".to_owned(),
        "--request-file".to_owned(),
        request_file.to_string_lossy().into_owned(),
        "--request-id".to_owned(),
        request_id(1),
        "--actor".to_owned(),
        "codex".to_owned(),
    ]);
    let created = parse_success(&run_mino(&create));
    let plan_id = created["plan_id"]
        .as_str()
        .expect("create should return plan ID")
        .to_owned();
    let mut apply = mutation_arguments(project, &["plan", "apply"], &plan_id, 1, 2);
    apply.extend([
        "--file".to_owned(),
        fixture_path("complete.yaml").to_string_lossy().into_owned(),
    ]);
    let applied = parse_success(&run_mino(&apply));
    let finalize = mutation_arguments(
        project,
        &["plan", "finalize"],
        &plan_id,
        applied["revision"].as_u64().expect("revision should exist"),
        3,
    );
    let finalized = parse_success(&run_mino(&finalize));
    let mut approve = mutation_arguments(
        project,
        &["plan", "approve"],
        &plan_id,
        finalized["revision"]
            .as_u64()
            .expect("revision should exist"),
        4,
    );
    approve.extend([
        "--approval-ref".to_owned(),
        "chat:cli-plan-approved".to_owned(),
        "--git-flow-consent".to_owned(),
        "disabled".to_owned(),
    ]);
    let approved = parse_success(&run_mino(&approve));
    (
        plan_id,
        approved["revision"]
            .as_u64()
            .expect("revision should exist"),
    )
}

fn configured_task(id: &str, dependency: Option<&str>) -> Task {
    let id_value = task_id(id);
    let mut task = Task::new(
        id_value.clone(),
        format!("Implement {id}"),
        dependency.into_iter().map(task_id).collect(),
    );
    task.add_step(format!("Implement the {id} behavior"))
        .expect("step should be added");
    task.add_file_map_entry(FileMapEntry::new(
        format!("src/{id}.rs"),
        FileChange::Modify,
        format!("Own {id}"),
        id_value.clone(),
    ))
    .expect("file should be added");
    task.add_acceptance_criterion(AcceptanceCriterion::new(
        criterion_id(&format!("{id}-A1")),
        format!("{id} behavior is observable"),
    ))
    .expect("criterion should be added");
    task.add_verification_check(VerificationCheck::new(
        check_id(&format!("{id}-V1")),
        vec!["cargo".to_owned(), "test".to_owned()],
        ".",
        0,
        true,
    ))
    .expect("check should be added");
    task.set_commit_gate(CommitGate::new(
        true,
        format!("feat({}): implement behavior", id.to_ascii_lowercase()),
        vec![format!("src/{id}.rs")],
    ))
    .expect("commit gate should be added");
    task
}

fn approved_plan(label: &str, two_tasks: bool) -> Plan {
    approved_plan_with_standards(label, two_tasks, Vec::new())
}

fn approved_plan_with_standards(
    label: &str,
    two_tasks: bool,
    standards: Vec<StandardSelection>,
) -> Plan {
    let mut plan = Plan::from_draft_seed(
        PlanDraftSeed {
            id: plan_id(label),
            name: "Protected amendment".to_owned(),
            trigger: "durable".to_owned(),
            original_request: "Implement an approved behavior.".to_owned(),
            branch: Some("main".to_owned()),
            markdown_path: format!("docs/plan/2026-07-26-{label}.md"),
            git_readiness: GitReadiness::detected(
                "Present",
                "Clean",
                Some("main".to_owned()),
                Some("1111111".to_owned()),
                "Clean",
                true,
            ),
            standards,
            verification_plan: vec![VerificationCheck::new(
                check_id("GLOBAL-V1"),
                vec!["cargo".to_owned(), "test".to_owned()],
                ".",
                0,
                true,
            )],
        },
        timestamp(0),
    );
    plan.add_task(configured_task("T1", None), timestamp(1))
        .expect("first task should be added");
    if two_tasks {
        plan.add_task(configured_task("T2", Some("T1")), timestamp(2))
            .expect("second task should be added");
    }
    for (index, task) in plan.task_order().to_vec().iter().enumerate() {
        let minute = 3 + u8::try_from(index).expect("fixture task count should fit u8");
        plan.mark_task_ready(task, timestamp(minute))
            .expect("task should become ready");
    }
    plan.finalize(timestamp(6)).expect("plan should finalize");
    plan.record_approval(Approval::plan(
        "user",
        "chat:plan-approved",
        timestamp(7),
        GitFlowConsent::Approved,
    ))
    .expect("plan should be approved");
    plan
}

fn embedded_standard(package_id: &str) -> StandardSelection {
    StandardSelection::new(
        package_id,
        "1.0.0",
        format!("sha256:{}", "1".repeat(64)),
        "embedded",
    )
}

fn start_and_satisfy_first_task(plan: &mut Plan) {
    let task = task_id("T1");
    plan.start_task(&task, timestamp(8))
        .expect("task should start");
    plan.record_checkpoint(
        &task,
        CheckpointKind::Implementation,
        "Implemented the approved behavior",
        "codex",
        timestamp(9),
    )
    .expect("checkpoint should be recorded");
    plan.record_task_criterion_pass(&task, &criterion_id("T1-A1"), evidence_id(1), timestamp(10))
        .expect("criterion should pass");
    plan.record_task_check_pass(&task, &check_id("T1-V1"), evidence_id(2), timestamp(11))
        .expect("check should pass");
}

#[test]
fn minor_amendment_invalidates_only_the_replaced_check_and_blocks_bypass() {
    let mut plan = approved_plan("minor-amendment", false);
    start_and_satisfy_first_task(&mut plan);
    let base_revision = plan.revision();
    let change_id = plan
        .propose_amendment(
            "Correct the exact verification executable".to_owned(),
            patch(&json!([{
                "operation": "replace-task-verification",
                "task_id": "T1",
                "check_id": "T1-V1",
                "command": ["cargo", "test", "--lib"],
                "cwd": ".",
                "expected_exit_code": 0,
                "required": true
            }])),
            Some(AmendmentClassification::Minor),
            state_hash(&plan),
            "codex".to_owned(),
            timestamp(12),
        )
        .expect("Minor proposal should succeed");
    assert_eq!(change_id, "C1");
    assert_eq!(plan.revision(), base_revision + 1);
    assert_eq!(plan.status(), PlanStatus::InProgress);
    assert!(plan.has_pending_amendment());
    assert!(
        plan.record_checkpoint(
            &task_id("T1"),
            CheckpointKind::Verification,
            "Attempted bypass",
            "codex",
            timestamp(13),
        )
        .is_err()
    );

    plan.apply_amendment("C1", timestamp(14))
        .expect("Minor proposal should apply without approval");
    let task = plan.task(&task_id("T1")).expect("task should exist");
    assert_eq!(task.status(), TaskStatus::InProgress);
    assert_eq!(
        task.acceptance_criteria()[0].status(),
        CriterionStatus::Passed
    );
    assert_eq!(task.verification_checks()[0].status(), CheckStatus::Pending);
    assert_eq!(
        task.verification_checks()[0].command(),
        ["cargo", "test", "--lib"]
    );
    assert!(!plan.is_evidence_stale(&evidence_id(1)));
    assert!(plan.is_evidence_stale(&evidence_id(2)));
    assert!(plan.has_plan_approval());
    assert_eq!(
        plan.amendment("C1").expect("change should exist").status(),
        AmendmentStatus::Applied
    );
}

#[test]
fn minor_file_and_note_allowlist_expands_only_the_task_local_contract() {
    let mut plan = approved_plan("minor-file", false);
    plan.propose_amendment(
        "Add the required fixture and implementation rationale".to_owned(),
        patch(&json!([
            {
                "operation": "add-task-file",
                "kind": "Test Fixture",
                "task_id": "T1",
                "path": "tests/fixtures/T1.json",
                "change": "Test",
                "reason": "Provide the existing task fixture"
            },
            {
                "operation": "add-implementation-note",
                "task_id": "T1",
                "note": "The fixture exercises the already approved behavior."
            }
        ])),
        None,
        state_hash(&plan),
        "codex".to_owned(),
        timestamp(8),
    )
    .expect("allowlisted proposal should succeed");
    plan.apply_amendment("C1", timestamp(9))
        .expect("allowlisted proposal should apply");

    let task = plan.task(&task_id("T1")).expect("task should exist");
    assert!(
        task.file_map()
            .iter()
            .any(|entry| entry.path() == "tests/fixtures/T1.json")
    );
    assert_eq!(
        task.implementation_notes(),
        ["The fixture exercises the already approved behavior."]
    );
    assert!(
        task.commit_gate()
            .expect("gate should exist")
            .scope()
            .contains(&"tests/fixtures/T1.json".to_owned())
    );
    assert!(!plan.has_plan_approval());
    assert_eq!(
        plan.git_readiness().git_flow_consent(),
        GitFlowConsent::Pending
    );
}

#[test]
fn add_task_file_for_missing_language_is_material_and_invalidates_approval() {
    let mut uncovered = approved_plan_with_standards(
        "uncovered-language",
        false,
        vec![embedded_standard("common"), embedded_standard("rust")],
    );
    let python_file = patch(&json!([{
        "operation": "add-task-file",
        "kind": "Test Fixture",
        "task_id": "T1",
        "path": "tests/fixtures/**/*.py",
        "change": "Test",
        "reason": "Exercise the approved behavior in a Python fixture"
    }]));
    let before = canonical_json_bytes(&uncovered).expect("plan should canonicalize");
    assert!(
        uncovered
            .propose_amendment(
                "Add the cross-language fixture".to_owned(),
                python_file.clone(),
                Some(AmendmentClassification::Minor),
                state_hash(&uncovered),
                "codex".to_owned(),
                timestamp(8),
            )
            .is_err()
    );
    assert_eq!(
        canonical_json_bytes(&uncovered).expect("plan should canonicalize"),
        before
    );

    uncovered
        .propose_amendment(
            "Add the cross-language fixture".to_owned(),
            python_file,
            None,
            state_hash(&uncovered),
            "codex".to_owned(),
            timestamp(9),
        )
        .expect("uncovered language should be classified as Material");
    let amendment = uncovered.amendment("C1").expect("amendment should exist");
    assert_eq!(
        amendment.minimum_classification(),
        AmendmentClassification::Material
    );
    assert_eq!(amendment.status(), AmendmentStatus::ApprovalRequired);
    uncovered
        .approve_amendment(
            "C1",
            "user".to_owned(),
            "chat:approve-python-fixture".to_owned(),
            timestamp(10),
        )
        .expect("Material amendment should be approved");
    uncovered
        .apply_amendment("C1", timestamp(11))
        .expect("Material amendment should apply");
    assert_eq!(uncovered.status(), PlanStatus::Ready);
    assert!(!uncovered.has_plan_approval());
    assert!(
        uncovered
            .task(&task_id("T1"))
            .expect("task should exist")
            .file_map()
            .iter()
            .any(|entry| entry.path() == "tests/fixtures/**/*.py")
    );
}

#[test]
fn add_task_file_for_selected_language_remains_minor() {
    let mut covered = approved_plan_with_standards(
        "covered-language",
        false,
        vec![embedded_standard("common"), embedded_standard("rust")],
    );
    covered
        .propose_amendment(
            "Add another Rust support file".to_owned(),
            patch(&json!([{
                "operation": "add-task-file",
                "kind": "Support File",
                "task_id": "T1",
                "path": "src/support.rs",
                "change": "Create",
                "reason": "Support the already covered Rust implementation"
            }])),
            Some(AmendmentClassification::Minor),
            state_hash(&covered),
            "codex".to_owned(),
            timestamp(8),
        )
        .expect("covered language should remain Minor");
    assert_eq!(
        covered
            .amendment("C1")
            .expect("amendment should exist")
            .minimum_classification(),
        AmendmentClassification::Minor
    );
}

#[test]
fn verification_gate_weakening_is_contextually_material() {
    let mut plan = approved_plan("verification-gate", false);
    let weakening = patch(&json!([{
        "operation": "replace-task-verification",
        "task_id": "T1",
        "check_id": "T1-V1",
        "command": ["cargo", "test"],
        "cwd": ".",
        "expected_exit_code": 0,
        "required": false
    }]));
    let before = canonical_json_bytes(&plan).expect("plan should canonicalize");
    assert!(
        plan.propose_amendment(
            "Remove a required verification gate".to_owned(),
            weakening.clone(),
            Some(AmendmentClassification::Minor),
            state_hash(&plan),
            "codex".to_owned(),
            timestamp(8),
        )
        .is_err()
    );
    assert_eq!(
        canonical_json_bytes(&plan).expect("plan should canonicalize"),
        before
    );
    plan.propose_amendment(
        "Remove a required verification gate".to_owned(),
        weakening,
        None,
        state_hash(&plan),
        "codex".to_owned(),
        timestamp(9),
    )
    .expect("contextual classifier should raise the proposal");
    let amendment = plan.amendment("C1").expect("change should exist");
    assert_eq!(
        amendment.minimum_classification(),
        AmendmentClassification::Material
    );
    assert_eq!(amendment.status(), AmendmentStatus::ApprovalRequired);
}

#[test]
fn proposer_can_withdraw_unapproved_amendment_without_applying_it() {
    let mut plan = approved_plan("withdraw-amendment", false);
    let original = plan
        .task(&task_id("T1"))
        .expect("task should exist")
        .verification_checks()[0]
        .command()
        .to_vec();
    plan.propose_amendment(
        "Correct the verification command".to_owned(),
        patch(&json!([{
            "operation": "replace-task-verification",
            "task_id": "T1",
            "check_id": "T1-V1",
            "command": ["cargo", "test", "--lib"],
            "cwd": ".",
            "expected_exit_code": 0,
            "required": true
        }])),
        None,
        state_hash(&plan),
        "codex".to_owned(),
        timestamp(8),
    )
    .expect("proposal should succeed");
    let before_invalid = canonical_json_bytes(&plan).expect("plan should canonicalize");
    assert!(
        plan.withdraw_amendment(
            "C1",
            "other".to_owned(),
            "Not the proposer".to_owned(),
            timestamp(9),
        )
        .is_err()
    );
    assert_eq!(canonical_json_bytes(&plan).unwrap(), before_invalid);

    plan.withdraw_amendment(
        "C1",
        "codex".to_owned(),
        "The replacement was incorrect".to_owned(),
        timestamp(10),
    )
    .expect("proposer should withdraw");
    let amendment = plan.amendment("C1").expect("change should exist");
    assert_eq!(amendment.status(), AmendmentStatus::Withdrawn);
    assert_eq!(amendment.disposition_actor(), Some("codex"));
    assert_eq!(
        amendment.disposition_reason(),
        Some("The replacement was incorrect")
    );
    assert!(amendment.disposition_reference().is_none());
    assert!(!plan.has_pending_amendment());
    assert_eq!(
        plan.task(&task_id("T1"))
            .expect("task should exist")
            .verification_checks()[0]
            .command(),
        original
    );
}

#[test]
fn material_rejection_restores_execution_without_staling_evidence() {
    let mut plan = approved_plan("reject-amendment", false);
    start_and_satisfy_first_task(&mut plan);
    plan.propose_amendment(
        "Change the promised behavior".to_owned(),
        patch(&json!([{
            "operation": "replace-summary",
            "summary": "Rejected behavior"
        }])),
        None,
        state_hash(&plan),
        "codex".to_owned(),
        timestamp(12),
    )
    .expect("Material proposal should succeed");
    assert_eq!(plan.status(), PlanStatus::Blocked);
    assert_eq!(
        plan.task(&task_id("T1")).unwrap().status(),
        TaskStatus::Blocked
    );
    let before_invalid = canonical_json_bytes(&plan).expect("plan should canonicalize");
    assert!(
        plan.reject_amendment(
            "C1",
            "user".to_owned(),
            String::new(),
            "Reject the proposal".to_owned(),
            timestamp(13),
        )
        .is_err()
    );
    assert_eq!(canonical_json_bytes(&plan).unwrap(), before_invalid);

    plan.reject_amendment(
        "C1",
        "user".to_owned(),
        "chat:material-rejected".to_owned(),
        "Keep the approved behavior".to_owned(),
        timestamp(14),
    )
    .expect("Material proposal should reject");
    let amendment = plan.amendment("C1").expect("change should exist");
    assert_eq!(amendment.status(), AmendmentStatus::Rejected);
    assert_eq!(amendment.disposition_actor(), Some("user"));
    assert_eq!(
        amendment.disposition_reference(),
        Some("chat:material-rejected")
    );
    assert_eq!(plan.status(), PlanStatus::InProgress);
    assert_eq!(
        plan.task(&task_id("T1")).unwrap().status(),
        TaskStatus::InProgress
    );
    assert!(!plan.is_evidence_stale(&evidence_id(1)));
    assert!(!plan.is_evidence_stale(&evidence_id(2)));
    assert!(plan.has_plan_approval());
    assert_ne!(plan.summary(), "Rejected behavior");
    plan.record_checkpoint(
        &task_id("T1"),
        CheckpointKind::Verification,
        "Continue after rejecting the amendment",
        "codex",
        timestamp(15),
    )
    .expect("terminal amendment must not block later evidence-bearing mutations");
}

#[test]
fn original_approver_can_cancel_approved_material_amendment() {
    let mut plan = approved_plan("cancel-amendment", false);
    plan.propose_amendment(
        "Change the promised behavior".to_owned(),
        patch(&json!([{
            "operation": "replace-summary",
            "summary": "Cancelled behavior"
        }])),
        None,
        state_hash(&plan),
        "codex".to_owned(),
        timestamp(8),
    )
    .expect("Material proposal should succeed");
    plan.approve_amendment(
        "C1",
        "user".to_owned(),
        "chat:material-approved".to_owned(),
        timestamp(9),
    )
    .expect("Material proposal should approve");
    let before_invalid = canonical_json_bytes(&plan).expect("plan should canonicalize");
    assert!(
        plan.cancel_amendment(
            "C1",
            "other".to_owned(),
            "chat:cancelled".to_owned(),
            "Wrong actor".to_owned(),
            timestamp(10),
        )
        .is_err()
    );
    assert_eq!(canonical_json_bytes(&plan).unwrap(), before_invalid);

    plan.cancel_amendment(
        "C1",
        "user".to_owned(),
        "chat:material-cancelled".to_owned(),
        "Approval was rescinded".to_owned(),
        timestamp(11),
    )
    .expect("original approver should cancel");
    let amendment = plan.amendment("C1").expect("change should exist");
    assert_eq!(amendment.status(), AmendmentStatus::Cancelled);
    assert_eq!(amendment.disposition_actor(), Some("user"));
    assert_eq!(
        amendment.disposition_reference(),
        Some("chat:material-cancelled")
    );
    assert_eq!(plan.status(), PlanStatus::Ready);
    assert!(plan.has_plan_approval());
    assert_ne!(plan.summary(), "Cancelled behavior");
    assert!(!plan.has_pending_amendment());
}

#[test]
fn material_amendment_cannot_be_lowered_or_applied_without_approval() {
    let mut plan = approved_plan("material-amendment", false);
    start_and_satisfy_first_task(&mut plan);
    let material_patch = patch(&json!([{
        "operation": "replace-summary",
        "summary": "Deliver the revised user-visible behavior."
    }]));
    let before = canonical_json_bytes(&plan).expect("plan should canonicalize");
    assert!(
        plan.propose_amendment(
            "Change the promised behavior".to_owned(),
            material_patch.clone(),
            Some(AmendmentClassification::Minor),
            state_hash(&plan),
            "codex".to_owned(),
            timestamp(12),
        )
        .is_err()
    );
    assert_eq!(
        canonical_json_bytes(&plan).expect("plan should canonicalize"),
        before
    );

    plan.propose_amendment(
        "Change the promised behavior".to_owned(),
        material_patch,
        None,
        state_hash(&plan),
        "codex".to_owned(),
        timestamp(13),
    )
    .expect("Material proposal should succeed");
    assert_eq!(plan.status(), PlanStatus::Blocked);
    assert_eq!(
        plan.amendment("C1").expect("change should exist").status(),
        AmendmentStatus::ApprovalRequired
    );
    assert!(plan.apply_amendment("C1", timestamp(14)).is_err());

    plan.approve_amendment(
        "C1",
        "user".to_owned(),
        "chat:material-approved".to_owned(),
        timestamp(15),
    )
    .expect("Material approval should record");
    plan.apply_amendment("C1", timestamp(16))
        .expect("approved Material proposal should apply");
    assert_eq!(plan.status(), PlanStatus::Ready);
    assert_eq!(plan.summary(), "Deliver the revised user-visible behavior.");
    assert!(!plan.has_plan_approval());
    assert!(
        plan.execution_state()
            .expect("state should load")
            .is_empty()
    );
    assert_eq!(
        plan.task(&task_id("T1"))
            .expect("task should exist")
            .status(),
        TaskStatus::Ready
    );
    assert!(plan.is_evidence_stale(&evidence_id(1)));
    assert!(plan.is_evidence_stale(&evidence_id(2)));
}

#[test]
#[allow(clippy::too_many_lines)]
fn material_patch_adds_a_complete_migration_execution_graph() {
    let mut plan = approved_plan("material-graph-add", false);
    start_and_satisfy_first_task(&mut plan);
    let graph_patch = patch(&json!([
        {
            "operation": "add-task",
            "task": {
                "id": "T2",
                "title": "Create the compatibility migration",
                "depends_on": ["T1"],
                "steps": ["Create the migration"],
                "files": [{
                    "path": "migrations/v2.rs",
                    "change": "Create",
                    "reason": "Own the approved migration"
                }],
                "acceptance_criteria": [{
                    "id": "T2-A1",
                    "description": "Existing callers can migrate"
                }],
                "verification": [{
                    "id": "T2-V1",
                    "command": ["cargo", "test", "migration"],
                    "cwd": ".",
                    "expected_exit_code": 0,
                    "required": true
                }],
                "commit_gate": {
                    "required": true,
                    "planned_message": "feat(migration): add compatibility path",
                    "scope": ["migrations/v2.rs"]
                }
            }
        },
        {
            "operation": "update-task-definition",
            "task_id": "T2",
            "title": "Implement the approved compatibility migration",
            "steps": ["Create the migration", "Document the compatibility boundary"]
        },
        {
            "operation": "replace-task-dependencies",
            "task_id": "T2",
            "depends_on": ["T1"]
        },
        {
            "operation": "update-criterion",
            "task_id": "T2",
            "criterion_id": "T2-A1",
            "description": "Existing callers retain a documented migration path"
        },
        {
            "operation": "add-criterion",
            "task_id": "T2",
            "criterion": {
                "id": "T2-A2",
                "description": "The public compatibility promise is verified"
            }
        },
        {
            "operation": "update-task-verification",
            "task_id": "T2",
            "check_id": "T2-V1",
            "verification": {
                "id": "T2-V1",
                "command": ["cargo", "test", "migration", "--all-features"],
                "cwd": ".",
                "expected_exit_code": 0,
                "required": true
            }
        },
        {
            "operation": "add-task-verification",
            "task_id": "T2",
            "verification": {
                "id": "T2-V2",
                "command": ["cargo", "test", "compatibility"],
                "cwd": ".",
                "expected_exit_code": 0,
                "required": true
            }
        },
        {
            "operation": "add-global-verification",
            "verification": {
                "id": "GLOBAL-INTEGRATION",
                "command": ["cargo", "test", "--all-features"],
                "cwd": ".",
                "expected_exit_code": 0,
                "required": true
            }
        },
        {
            "operation": "replace-commit-gate",
            "task_id": "T2",
            "commit_gate": {
                "required": true,
                "planned_message": "feat(migration): implement compatibility path",
                "scope": ["migrations/v2.rs"]
            }
        },
        {
            "operation": "replace-task-order",
            "task_order": ["T1", "T2"]
        }
    ]));
    let before = state_hash(&plan);
    plan.propose_amendment(
        "Add the approved migration graph".to_owned(),
        graph_patch,
        None,
        before,
        "codex".to_owned(),
        timestamp(12),
    )
    .expect("complete Material graph should propose");
    let amendment = plan.amendment("C1").expect("proposal should exist");
    assert_eq!(
        amendment.minimum_classification(),
        AmendmentClassification::Material
    );
    assert_eq!(
        amendment.impact().affected_tasks(),
        [task_id("T1"), task_id("T2")]
    );
    assert_eq!(
        amendment.impact().affected_checks(),
        [
            check_id("GLOBAL-INTEGRATION"),
            check_id("GLOBAL-V1"),
            check_id("T1-V1"),
            check_id("T2-V1"),
            check_id("T2-V2"),
        ]
    );
    assert_eq!(
        amendment.impact().stale_evidence(),
        [evidence_id(1), evidence_id(2)]
    );
    plan.approve_amendment(
        "C1",
        "user".to_owned(),
        "chat:material-graph-approved".to_owned(),
        timestamp(13),
    )
    .expect("Material graph should approve");
    plan.apply_amendment("C1", timestamp(14))
        .expect("Material graph should apply atomically");

    assert_eq!(plan.status(), PlanStatus::Ready);
    assert!(!plan.has_plan_approval());
    assert_eq!(plan.task_order(), [task_id("T1"), task_id("T2")]);
    let migration = plan
        .task(&task_id("T2"))
        .expect("migration task should exist");
    assert_eq!(migration.status(), TaskStatus::Ready);
    assert_eq!(
        migration.title(),
        "Implement the approved compatibility migration"
    );
    assert_eq!(migration.acceptance_criteria().len(), 2);
    assert_eq!(migration.verification_checks().len(), 2);
    assert_eq!(
        migration
            .commit_gate()
            .expect("migration gate should exist")
            .planned_message(),
        "feat(migration): implement compatibility path"
    );
    assert!(
        plan.global_verification()
            .iter()
            .any(|check| check.id() == &check_id("GLOBAL-INTEGRATION"))
    );
    assert!(plan.is_evidence_stale(&evidence_id(1)));
    assert!(plan.is_evidence_stale(&evidence_id(2)));
}

#[test]
#[allow(clippy::too_many_lines)]
fn material_patch_removes_graph_nodes_without_reusing_task_ids() {
    let mut plan = approved_plan("material-graph-remove", true);
    let graph_patch = patch(&json!([
        {
            "operation": "add-criterion",
            "task_id": "T1",
            "criterion": {
                "id": "T1-A2",
                "description": "The replacement criterion remains observable"
            }
        },
        {
            "operation": "remove-criterion",
            "task_id": "T1",
            "criterion_id": "T1-A1"
        },
        {
            "operation": "add-task-verification",
            "task_id": "T1",
            "verification": {
                "id": "T1-V2",
                "command": ["cargo", "test", "replacement"],
                "cwd": ".",
                "expected_exit_code": 0,
                "required": true
            }
        },
        {
            "operation": "remove-task-verification",
            "task_id": "T1",
            "check_id": "T1-V1"
        },
        {
            "operation": "add-global-verification",
            "verification": {
                "id": "GLOBAL-V2",
                "command": ["cargo", "test", "replacement-global"],
                "cwd": ".",
                "expected_exit_code": 0,
                "required": true
            }
        },
        {
            "operation": "update-global-verification",
            "check_id": "GLOBAL-V2",
            "verification": {
                "id": "GLOBAL-V2",
                "command": ["cargo", "test", "replacement-global", "--all-features"],
                "cwd": ".",
                "expected_exit_code": 0,
                "required": true
            }
        },
        {
            "operation": "remove-global-verification",
            "check_id": "GLOBAL-V1"
        },
        {
            "operation": "remove-commit-gate",
            "task_id": "T1"
        },
        {
            "operation": "remove-task",
            "task_id": "T2"
        }
    ]));
    plan.propose_amendment(
        "Replace and remove obsolete graph nodes".to_owned(),
        graph_patch,
        None,
        state_hash(&plan),
        "codex".to_owned(),
        timestamp(8),
    )
    .expect("graph removal should propose");
    plan.approve_amendment(
        "C1",
        "user".to_owned(),
        "chat:graph-removal-approved".to_owned(),
        timestamp(9),
    )
    .expect("graph removal should approve");
    plan.apply_amendment("C1", timestamp(10))
        .expect("valid graph removal should apply");

    assert_eq!(plan.task_order(), [task_id("T1")]);
    assert!(plan.task(&task_id("T2")).is_none());
    let task = plan
        .task(&task_id("T1"))
        .expect("retained task should exist");
    assert_eq!(task.acceptance_criteria()[0].id(), &criterion_id("T1-A2"));
    assert_eq!(task.verification_checks()[0].id(), &check_id("T1-V2"));
    assert!(task.commit_gate().is_none());
    assert_eq!(plan.global_verification()[0].id(), &check_id("GLOBAL-V2"));
    assert_eq!(
        plan.global_verification()[0].command(),
        ["cargo", "test", "replacement-global", "--all-features"]
    );
    assert!(
        plan.approach()
            .file_map()
            .iter()
            .all(|entry| entry.task_id() != &task_id("T2"))
    );
    assert_eq!(
        plan.next_task_id().expect("next task ID should allocate"),
        task_id("T3")
    );
}

#[test]
fn invalid_material_graph_apply_is_atomic_and_strictly_typed() {
    let mut plan = approved_plan("material-graph-atomic", false);
    let invalid = patch(&json!([{
        "operation": "remove-criterion",
        "task_id": "T1",
        "criterion_id": "T1-A1"
    }]));
    assert!(
        plan.propose_amendment(
            "Remove the only criterion".to_owned(),
            invalid.clone(),
            Some(AmendmentClassification::Minor),
            state_hash(&plan),
            "codex".to_owned(),
            timestamp(8),
        )
        .is_err()
    );
    plan.propose_amendment(
        "Remove the only criterion".to_owned(),
        invalid,
        None,
        state_hash(&plan),
        "codex".to_owned(),
        timestamp(9),
    )
    .expect("structurally typed graph proposal should record");
    plan.approve_amendment(
        "C1",
        "user".to_owned(),
        "chat:invalid-graph-approved".to_owned(),
        timestamp(10),
    )
    .expect("invalid final graph may still receive approval");
    let before_apply = canonical_json_bytes(&plan).expect("plan should canonicalize");
    assert!(plan.apply_amendment("C1", timestamp(11)).is_err());
    assert_eq!(
        canonical_json_bytes(&plan).expect("plan should canonicalize"),
        before_apply
    );

    assert!(
        serde_json::from_value::<AmendmentPatch>(json!({
            "operations": [{
                "operation": "add-global-verification",
                "verification": {
                    "id": "GLOBAL-V2",
                    "command": ["cargo", "test"],
                    "cwd": ".",
                    "required": true,
                    "unexpected": true
                }
            }]
        }))
        .is_err()
    );
}

#[test]
fn material_graph_rejects_duplicate_checks_and_dependency_cycles_atomically() {
    let mut duplicate = approved_plan("material-duplicate-check", false);
    duplicate
        .propose_amendment(
            "Introduce a duplicate check".to_owned(),
            patch(&json!([{
                "operation": "add-global-verification",
                "verification": {
                    "id": "T1-V1",
                    "command": ["cargo", "test"],
                    "cwd": ".",
                    "expected_exit_code": 0,
                    "required": true
                }
            }])),
            None,
            state_hash(&duplicate),
            "codex".to_owned(),
            timestamp(8),
        )
        .expect("typed duplicate proposal should record before final graph validation");
    duplicate
        .approve_amendment(
            "C1",
            "user".to_owned(),
            "chat:duplicate-check-approved".to_owned(),
            timestamp(9),
        )
        .expect("duplicate proposal should approve");
    let duplicate_before = canonical_json_bytes(&duplicate).expect("plan should canonicalize");
    assert!(duplicate.apply_amendment("C1", timestamp(10)).is_err());
    assert_eq!(canonical_json_bytes(&duplicate).unwrap(), duplicate_before);

    let mut cycle = approved_plan("material-dependency-cycle", true);
    cycle
        .propose_amendment(
            "Introduce a dependency cycle".to_owned(),
            patch(&json!([{
                "operation": "replace-task-dependencies",
                "task_id": "T1",
                "depends_on": ["T2"]
            }])),
            None,
            state_hash(&cycle),
            "codex".to_owned(),
            timestamp(8),
        )
        .expect("typed dependency proposal should record");
    cycle
        .approve_amendment(
            "C1",
            "user".to_owned(),
            "chat:dependency-cycle-approved".to_owned(),
            timestamp(9),
        )
        .expect("dependency proposal should approve");
    let cycle_before = canonical_json_bytes(&cycle).expect("plan should canonicalize");
    assert!(cycle.apply_amendment("C1", timestamp(10)).is_err());
    assert_eq!(canonical_json_bytes(&cycle).unwrap(), cycle_before);
}

#[test]
#[allow(clippy::too_many_lines)]
fn material_apply_is_atomic_and_supersedes_review_only_after_success() {
    let mut plan = approved_plan("material-rollback", true);
    let invalid_order = patch(&json!([{
        "operation": "replace-task-order",
        "task_order": ["T2", "T1"]
    }]));
    plan.propose_amendment(
        "Reverse the core task order".to_owned(),
        invalid_order,
        None,
        state_hash(&plan),
        "codex".to_owned(),
        timestamp(8),
    )
    .expect("proposal classification should succeed");
    plan.approve_amendment(
        "C1",
        "user".to_owned(),
        "chat:order-approved".to_owned(),
        timestamp(9),
    )
    .expect("approval should record");
    let before_apply = canonical_json_bytes(&plan).expect("plan should canonicalize");
    assert!(plan.apply_amendment("C1", timestamp(10)).is_err());
    assert_eq!(
        canonical_json_bytes(&plan).expect("plan should canonicalize"),
        before_apply
    );

    let mut reviewed = approved_plan("material-review", false);
    start_and_satisfy_first_task(&mut reviewed);
    reviewed
        .complete_task(&task_id("T1"), timestamp(12))
        .expect("task should complete");
    reviewed
        .record_task_commit(
            &task_id("T1"),
            &"a".repeat(40),
            vec!["src/T1.rs".to_owned()],
            evidence_id(3),
            timestamp(13),
        )
        .expect("commit should record");
    reviewed
        .record_global_check_pass(&check_id("GLOBAL-V1"), evidence_id(4), timestamp(14))
        .expect("global check should pass");
    reviewed
        .set_final_outcome(
            "Approved implementation is verified".to_owned(),
            "N/A".to_owned(),
            Vec::new(),
            timestamp(15),
        )
        .expect("Final Outcome should record");
    reviewed
        .finish_execution(timestamp(15))
        .expect("plan should enter Review");
    let review_id = reviewed
        .record_review(
            "user".to_owned(),
            "The public contract must change".to_owned(),
            ReviewClassification::MaterialChange,
            None,
            timestamp(16),
        )
        .expect("material review should record");
    assert!(
        reviewed
            .propose_amendment(
                "Apply the requested contract change".to_owned(),
                patch(&json!([{
                    "operation": "replace-interfaces",
                    "interfaces": "Revised public interface contract"
                }])),
                Some(AmendmentClassification::Material),
                state_hash(&reviewed),
                "codex".to_owned(),
                timestamp(17),
            )
            .is_err()
    );
    reviewed
        .dispose_material_review(
            &review_id,
            MaterialReviewDisposition::AcceptChange,
            "user".to_owned(),
            "chat:accept-material-change".to_owned(),
            "The requested contract change belongs in this plan".to_owned(),
            timestamp(17),
        )
        .expect("accept-change disposition should record");
    reviewed
        .propose_amendment(
            "Apply the requested contract change".to_owned(),
            patch(&json!([{
                "operation": "replace-interfaces",
                "interfaces": "Revised public interface contract"
            }])),
            Some(AmendmentClassification::Material),
            state_hash(&reviewed),
            "codex".to_owned(),
            timestamp(17),
        )
        .expect("review-owned proposal should record");
    reviewed
        .approve_amendment(
            "C1",
            "user".to_owned(),
            "chat:review-change-approved".to_owned(),
            timestamp(18),
        )
        .expect("review amendment should be approved");
    reviewed
        .apply_amendment("C1", timestamp(19))
        .expect("review amendment should apply");
    let review = reviewed.review_item("REV-1").expect("review should exist");
    assert_eq!(review.status(), ReviewStatus::Resolved);
    assert_eq!(review.superseded_by_change(), Some("C1"));
    assert_eq!(reviewed.status(), PlanStatus::Ready);
    assert!(!reviewed.final_outcome().is_complete());
    assert!(reviewed.is_evidence_stale(&evidence_id(3)));
    assert!(reviewed.is_evidence_stale(&evidence_id(4)));
}

#[test]
fn cli_amendment_is_strict_revision_checked_and_replayable() {
    let project = TestProject::new();
    let (plan_id, revision) = create_approved_cli_plan(&project);
    let patch_file = project.path().join("minor-amendment.yaml");
    fs::write(
        &patch_file,
        "operations:\n  - operation: add-implementation-note\n    task_id: T1\n    note: Preserve the approved behavior while documenting the implementation.\n",
    )
    .expect("amendment patch should be written");
    let mut propose = mutation_arguments(
        &project,
        &["plan", "amend", "propose"],
        &plan_id,
        revision,
        5,
    );
    propose.extend([
        "--reason".to_owned(),
        "Record the task-local implementation rationale".to_owned(),
        "--patch-file".to_owned(),
        patch_file.to_string_lossy().into_owned(),
        "--classification".to_owned(),
        "minor".to_owned(),
    ]);
    let proposed = parse_success(&run_mino(&propose));
    assert_eq!(proposed["assigned_id"], "C1");
    assert_eq!(proposed["replayed"], false);
    assert_eq!(proposed["next_actions"][0]["id"], "plan.amend.apply");

    let replayed = parse_success(&run_mino(&propose));
    assert_eq!(replayed["replayed"], true);
    assert_eq!(replayed["revision"], proposed["revision"]);

    let apply = mutation_arguments(
        &project,
        &["plan", "amend", "apply"],
        &plan_id,
        proposed["revision"]
            .as_u64()
            .expect("proposal revision should exist"),
        6,
    );
    let mut apply = apply;
    apply.extend(["--change".to_owned(), "C1".to_owned()]);
    let applied = parse_success(&run_mino(&apply));
    assert_eq!(applied["status"], "Ready");

    let mut show = base_arguments(&project);
    show.extend([
        "plan".to_owned(),
        "show".to_owned(),
        "--plan".to_owned(),
        plan_id.clone(),
    ]);
    let current = parse_success(&run_mino(&show));
    assert_eq!(current["amendments"][0]["status"], "Applied");
    assert_eq!(
        current["tasks"][0]["implementation_notes"][0],
        "Preserve the approved behavior while documenting the implementation."
    );
    assert_eq!(current["approvals"], json!([]));

    let invalid_patch = project.path().join("invalid-amendment.yaml");
    fs::write(
        &invalid_patch,
        "operations:\n  - operation: add-implementation-note\n    task_id: T1\n    note: Invalid extra field.\n    status: Done\n",
    )
    .expect("invalid amendment patch should be written");
    let mut invalid = mutation_arguments(
        &project,
        &["plan", "amend", "propose"],
        &plan_id,
        applied["revision"]
            .as_u64()
            .expect("apply revision should exist"),
        7,
    );
    invalid.extend([
        "--reason".to_owned(),
        "Attempt an untyped state injection".to_owned(),
        "--patch-file".to_owned(),
        invalid_patch.to_string_lossy().into_owned(),
    ]);
    let rejected = run_mino(&invalid);
    assert_eq!(rejected.status.code(), Some(2));
    let after_rejection = parse_success(&run_mino(&show));
    assert_eq!(after_rejection["revision"], applied["revision"]);
    assert_eq!(
        after_rejection["amendments"].as_array().map(Vec::len),
        Some(1)
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn cli_terminal_dispositions_are_revision_checked_and_replayable() {
    let project = TestProject::new();
    let (plan_id, revision) = create_approved_cli_plan(&project);
    let minor_patch = project.path().join("withdraw-amendment.yaml");
    fs::write(
        &minor_patch,
        "operations:\n  - operation: add-implementation-note\n    task_id: T1\n    note: This proposal will be withdrawn.\n",
    )
    .expect("Minor patch should be written");
    let mut minor_proposal_command = mutation_arguments(
        &project,
        &["plan", "amend", "propose"],
        &plan_id,
        revision,
        20,
    );
    minor_proposal_command.extend([
        "--reason".to_owned(),
        "Propose a task-local note".to_owned(),
        "--patch-file".to_owned(),
        minor_patch.to_string_lossy().into_owned(),
    ]);
    let minor_proposal = parse_success(&run_mino(&minor_proposal_command));
    let mut agent_context = base_arguments(&project);
    agent_context.extend(["agent".to_owned(), "context".to_owned()]);
    let minor_guidance = parse_success(&run_mino(&agent_context));
    assert!(
        minor_guidance["allowed_actions"]
            .as_array()
            .is_some_and(|actions| actions.contains(&json!("plan.amend.withdraw")))
    );
    let mut withdraw = mutation_arguments(
        &project,
        &["plan", "amend", "withdraw"],
        &plan_id,
        minor_proposal["revision"].as_u64().unwrap(),
        21,
    );
    withdraw.extend([
        "--change".to_owned(),
        "C1".to_owned(),
        "--reason".to_owned(),
        "The note is unnecessary".to_owned(),
    ]);
    let withdrawn = parse_success(&run_mino(&withdraw));
    assert_eq!(withdrawn["replayed"], false);
    let replay = parse_success(&run_mino(&withdraw));
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["revision"], withdrawn["revision"]);

    let material_patch = project.path().join("terminal-material-amendment.yaml");
    fs::write(
        &material_patch,
        "operations:\n  - operation: replace-summary\n    summary: This terminal proposal must not apply.\n",
    )
    .expect("Material patch should be written");
    let mut rejected_proposal_command = mutation_arguments(
        &project,
        &["plan", "amend", "propose"],
        &plan_id,
        withdrawn["revision"].as_u64().unwrap(),
        22,
    );
    rejected_proposal_command.extend([
        "--reason".to_owned(),
        "Propose a rejected behavior".to_owned(),
        "--patch-file".to_owned(),
        material_patch.to_string_lossy().into_owned(),
    ]);
    let rejected_proposal = parse_success(&run_mino(&rejected_proposal_command));
    let rejection_guidance = parse_success(&run_mino(&agent_context));
    assert!(
        rejection_guidance["allowed_actions"]
            .as_array()
            .is_some_and(|actions| actions.contains(&json!("plan.amend.reject")))
    );
    let mut reject = mutation_arguments(
        &project,
        &["plan", "amend", "reject"],
        &plan_id,
        rejected_proposal["revision"].as_u64().unwrap(),
        23,
    );
    reject.extend([
        "--change".to_owned(),
        "C2".to_owned(),
        "--decision-ref".to_owned(),
        "chat:rejected".to_owned(),
        "--reason".to_owned(),
        "Keep the approved summary".to_owned(),
    ]);
    let rejected = parse_success(&run_mino(&reject));

    let mut cancelled_proposal_command = mutation_arguments(
        &project,
        &["plan", "amend", "propose"],
        &plan_id,
        rejected["revision"].as_u64().unwrap(),
        24,
    );
    cancelled_proposal_command.extend([
        "--reason".to_owned(),
        "Propose a cancelled behavior".to_owned(),
        "--patch-file".to_owned(),
        material_patch.to_string_lossy().into_owned(),
    ]);
    let cancelled_proposal = parse_success(&run_mino(&cancelled_proposal_command));
    let mut approve = mutation_arguments(
        &project,
        &["plan", "amend", "approve"],
        &plan_id,
        cancelled_proposal["revision"].as_u64().unwrap(),
        25,
    );
    approve.extend([
        "--change".to_owned(),
        "C3".to_owned(),
        "--approval-ref".to_owned(),
        "chat:approved-before-cancel".to_owned(),
    ]);
    let approved = parse_success(&run_mino(&approve));
    let cancellation_guidance = parse_success(&run_mino(&agent_context));
    assert!(
        cancellation_guidance["allowed_actions"]
            .as_array()
            .is_some_and(|actions| actions.contains(&json!("plan.amend.cancel")))
    );
    let mut cancel = mutation_arguments(
        &project,
        &["plan", "amend", "cancel"],
        &plan_id,
        approved["revision"].as_u64().unwrap(),
        26,
    );
    cancel.extend([
        "--change".to_owned(),
        "C3".to_owned(),
        "--decision-ref".to_owned(),
        "chat:cancelled".to_owned(),
        "--reason".to_owned(),
        "Rescind the approved proposal".to_owned(),
    ]);
    parse_success(&run_mino(&cancel));

    let mut show = base_arguments(&project);
    show.extend([
        "plan".to_owned(),
        "show".to_owned(),
        "--plan".to_owned(),
        plan_id.clone(),
    ]);
    let current = parse_success(&run_mino(&show));
    assert_eq!(current["amendments"][0]["status"], "Withdrawn");
    assert_eq!(current["amendments"][1]["status"], "Rejected");
    assert_eq!(current["amendments"][2]["status"], "Cancelled");
    assert_eq!(
        current["amendments"][2]["disposition_reference"],
        "chat:cancelled"
    );
    assert_ne!(current["summary"], "This terminal proposal must not apply.");
    let projection = fs::read_to_string(
        project
            .path()
            .join("docs/plan")
            .join(format!("{plan_id}.md")),
    )
    .expect("projection should be readable");
    assert!(projection.contains("chat:cancelled"));
    assert!(projection.contains("Rescind the approved proposal"));
}
