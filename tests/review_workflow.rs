//! Classified review, rework-task, audit, and final-acceptance contracts.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use mino::application::agent::build_agent_context;
use mino::application::amendment::AmendmentService;
use mino::application::plan::PlanMutationRequest;
use mino::application::review::ReviewService;
use mino::domain::{
    AcceptanceCriterion, AmendmentPatch, AmendmentStatus, Approval, CheckId, CommitGate,
    CriterionId, DraftCommitGateInput, DraftCriterionInput, DraftFileInput, DraftTaskInput,
    DraftVerificationInput, EvidenceId, FileChange, FileMapEntry, GitFlowConsent, GitReadiness,
    MaterialReviewDisposition, Plan, PlanDraftSeed, PlanId, PlanStatus, RequestId,
    ReviewClassification, ReviewStatus, StandardSelection, Task, TaskId, TaskStatus, Timestamp,
    VerificationCheck,
};
use mino::project::initialize;
use mino::render::{render_plan, write_projection};
use mino::store::{MutationRequest, PlanStore, canonical_json_bytes, sha256_digest};

static NEXT_TEMP_DIRECTORY: AtomicU64 = AtomicU64::new(1);

struct TestProject {
    path: PathBuf,
}

impl TestProject {
    fn new() -> Self {
        let sequence = NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "mino-review-workflow-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("temporary project should be created");
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
                .is_some_and(|name| name.to_string_lossy().starts_with("mino-review-workflow-"))
        {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

fn timestamp(minute: u8) -> Timestamp {
    Timestamp::parse(format!("2026-07-26T08:{minute:02}:00Z")).expect("timestamp should parse")
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

fn request_id(number: u64) -> RequestId {
    RequestId::parse(format!("70000000-0000-0000-0000-{number:012}"))
        .expect("request ID should parse")
}

fn run_mino(arguments: &[String]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mino"))
        .args(arguments)
        .output()
        .expect("Mino binary should run")
}

fn parse_cli_success(output: &Output) -> serde_json::Value {
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("successful command should return JSON")
}

fn seed(label: &str) -> Plan {
    Plan::from_draft_seed(
        PlanDraftSeed {
            id: plan_id(label),
            name: "Review workflow".to_owned(),
            trigger: "durable".to_owned(),
            original_request: "Implement and review one change.".to_owned(),
            branch: Some("main".to_owned()),
            markdown_path: format!("docs/plan/2026-07-26-{label}.md"),
            git_readiness: GitReadiness::detected(
                "Present",
                "Clean",
                Some("main".to_owned()),
                Some("1111111".to_owned()),
                "Clean: git status --short returned empty",
                true,
            ),
            standards: Vec::<StandardSelection>::new(),
            verification_plan: vec![VerificationCheck::new(
                check_id("GLOBAL-V1"),
                vec!["cargo".to_owned(), "test".to_owned()],
                ".",
                0,
                true,
            )],
        },
        timestamp(0),
    )
}

fn original_task() -> Task {
    let id = task_id("T1");
    let mut task = Task::new(id.clone(), "Implement the original behavior", Vec::new());
    task.add_step("Implement the reviewed behavior")
        .expect("step should be added");
    task.add_file_map_entry(FileMapEntry::new(
        "src/lib.rs",
        FileChange::Modify,
        "Own the reviewed behavior",
        id.clone(),
    ))
    .expect("file should be added");
    task.add_acceptance_criterion(AcceptanceCriterion::new(
        criterion_id("T1-A1"),
        "The reviewed behavior is observable",
    ))
    .expect("criterion should be added");
    task.add_verification_check(VerificationCheck::new(
        check_id("TASK-V1"),
        vec!["cargo".to_owned(), "test".to_owned()],
        ".",
        0,
        true,
    ))
    .expect("check should be added");
    task.set_commit_gate(CommitGate::new(
        true,
        "feat(core): implement reviewed behavior",
        vec!["src/lib.rs".to_owned()],
    ))
    .expect("commit gate should be added");
    task
}

fn reviewed_plan(label: &str) -> Plan {
    let task = task_id("T1");
    let mut plan = seed(label);
    plan.add_task(original_task(), timestamp(1))
        .expect("task should be added");
    plan.mark_task_ready(&task, timestamp(2))
        .expect("task should become ready");
    plan.finalize(timestamp(3)).expect("plan should finalize");
    plan.record_approval(Approval::plan(
        "user",
        "chat:plan-approved",
        timestamp(4),
        GitFlowConsent::Approved,
    ))
    .expect("plan should be approved");
    plan.start_task(&task, timestamp(5))
        .expect("task should start");
    satisfy_task(&mut plan, &task, "T1-A1", "TASK-V1", 1, 6);
    plan.complete_task(&task, timestamp(8))
        .expect("task should complete");
    satisfy_commit(&mut plan, &task, "src/lib.rs", 3, 9);
    plan.record_global_check_pass(&check_id("GLOBAL-V1"), evidence_id(4), timestamp(10))
        .expect("global check should pass");
    set_final_outcome(&mut plan, 11);
    plan.finish_execution(timestamp(11))
        .expect("plan should enter Review");
    plan
}

fn satisfy_task(
    plan: &mut Plan,
    task: &TaskId,
    criterion: &str,
    check: &str,
    first_evidence: u16,
    minute: u8,
) {
    plan.record_task_criterion_pass(
        task,
        &criterion_id(criterion),
        evidence_id(first_evidence),
        timestamp(minute),
    )
    .expect("criterion should pass");
    plan.record_task_check_pass(
        task,
        &check_id(check),
        evidence_id(first_evidence + 1),
        timestamp(minute + 1),
    )
    .expect("check should pass");
}

fn satisfy_commit(plan: &mut Plan, task: &TaskId, file: &str, evidence: u16, minute: u8) {
    plan.record_task_commit(
        task,
        &format!("{evidence:040x}"),
        vec![file.to_owned()],
        evidence_id(evidence),
        timestamp(minute),
    )
    .expect("commit should be recorded");
}

fn set_final_outcome(plan: &mut Plan, minute: u8) {
    plan.set_final_outcome(
        "Reviewed implementation is verified".to_owned(),
        "N/A".to_owned(),
        Vec::new(),
        timestamp(minute),
    )
    .expect("Final Outcome should be recorded");
}

fn state_hash(plan: &Plan) -> String {
    sha256_digest(&canonical_json_bytes(plan).expect("plan should canonicalize"))
}

fn review_amendment_patch() -> AmendmentPatch {
    serde_json::from_value(serde_json::json!({
        "operations": [{
            "operation": "replace-interfaces",
            "interfaces": "Revised public interface contract"
        }]
    }))
    .expect("amendment patch should parse")
}

fn pending_review_amendment(label: &str) -> Plan {
    let mut plan = reviewed_plan(label);
    let review_id = plan
        .record_review(
            "reviewer".to_owned(),
            "Change the public contract".to_owned(),
            ReviewClassification::MaterialChange,
            None,
            timestamp(12),
        )
        .expect("Material review should record");
    plan.dispose_material_review(
        &review_id,
        MaterialReviewDisposition::AcceptChange,
        "user".to_owned(),
        "chat:accept-change".to_owned(),
        "The change belongs to the active objective".to_owned(),
        timestamp(13),
    )
    .expect("Accept Change should record");
    plan.propose_amendment(
        "Apply the requested contract change".to_owned(),
        review_amendment_patch(),
        None,
        state_hash(&plan),
        "codex".to_owned(),
        timestamp(14),
    )
    .expect("review-owned amendment should propose");
    plan
}

#[derive(Clone, Copy)]
enum TerminalAmendment {
    Reject,
    Withdraw,
    Cancel,
}

fn terminate_review_amendment(plan: &mut Plan, terminal: TerminalAmendment) -> AmendmentStatus {
    match terminal {
        TerminalAmendment::Reject => plan
            .reject_amendment(
                "C1",
                "user".to_owned(),
                "chat:reject-change".to_owned(),
                "The concrete patch is not acceptable".to_owned(),
                timestamp(15),
            )
            .expect("amendment should reject"),
        TerminalAmendment::Withdraw => plan
            .withdraw_amendment(
                "C1",
                "codex".to_owned(),
                "The proposed patch is unsuitable".to_owned(),
                timestamp(15),
            )
            .expect("amendment should withdraw"),
        TerminalAmendment::Cancel => {
            plan.approve_amendment(
                "C1",
                "user".to_owned(),
                "chat:approve-change".to_owned(),
                timestamp(15),
            )
            .expect("amendment should approve");
            plan.cancel_amendment(
                "C1",
                "user".to_owned(),
                "chat:cancel-change".to_owned(),
                "The approved patch should not be applied".to_owned(),
                timestamp(16),
            )
            .expect("amendment should cancel");
        }
    }
    plan.amendment("C1")
        .expect("amendment should exist")
        .status()
}

fn complete_rework_input(id: &str) -> DraftTaskInput {
    DraftTaskInput {
        id: Some(task_id(id)),
        title: "Correct the in-scope review defect".to_owned(),
        depends_on: vec![task_id("T1")],
        steps: vec!["Apply the smallest reviewed correction".to_owned()],
        files: vec![DraftFileInput {
            path: "src/lib.rs".to_owned(),
            change: FileChange::Modify,
            reason: "Correct the approved behavior".to_owned(),
        }],
        acceptance_criteria: vec![DraftCriterionInput {
            id: Some(criterion_id(&format!("{id}-A1"))),
            description: "The review defect is corrected".to_owned(),
        }],
        verification: vec![DraftVerificationInput {
            id: check_id(&format!("{id}-VERIFY")),
            command: vec!["cargo".to_owned(), "test".to_owned()],
            cwd: ".".to_owned(),
            expected_exit_code: 0,
            required: true,
        }],
        commit_gate: Some(DraftCommitGateInput {
            required: true,
            planned_message: "fix(review): correct approved behavior".to_owned(),
            scope: vec!["src/lib.rs".to_owned()],
        }),
    }
}

#[test]
fn every_review_classification_selects_one_typed_action() {
    let mut plan = reviewed_plan("classification-matrix");
    let original_order = plan.task_order().to_vec();

    let acceptance = plan
        .record_review(
            "reviewer".to_owned(),
            "Acceptance evidence must be rerun".to_owned(),
            ReviewClassification::AcceptanceDefect,
            Some(task_id("T1")),
            timestamp(12),
        )
        .expect("acceptance defect should record");
    let rework = plan
        .record_review(
            "reviewer".to_owned(),
            "Apply one correction inside scope".to_owned(),
            ReviewClassification::InScopeRework,
            Some(task_id("T1")),
            timestamp(13),
        )
        .expect("in-scope rework should record");
    let follow_up = plan
        .record_review(
            "reviewer".to_owned(),
            "Explore a separate optimization".to_owned(),
            ReviewClassification::FollowUp,
            None,
            timestamp(14),
        )
        .expect("follow-up should record");

    assert_eq!(acceptance, "REV-1");
    assert_eq!(rework, "REV-2");
    assert_eq!(follow_up, "REV-3");
    assert_eq!(plan.review_items()[0].linked_task(), Some(&task_id("T1")));
    assert_eq!(plan.review_items()[1].linked_task(), Some(&task_id("R1")));
    assert_eq!(plan.review_items()[2].status(), ReviewStatus::Deferred);
    assert_eq!(plan.task_order(), original_order);
    assert_eq!(
        plan.follow_ups(),
        ["Explore a separate optimization".to_owned()]
    );

    let material = plan
        .record_review(
            "reviewer".to_owned(),
            "Change the public contract".to_owned(),
            ReviewClassification::MaterialChange,
            None,
            timestamp(15),
        )
        .expect("material feedback should record");
    assert_eq!(material, "REV-4");
    assert_eq!(plan.status(), PlanStatus::Blocked);
    assert!(plan.is_blocked_for_material_review());
    assert!(plan.resume(timestamp(16)).is_err());
    let context = build_agent_context(Path::new("C:/fixture"), Some(&plan))
        .expect("material review context should build");
    assert!(context.approval_required);
    assert!(context.next_actions.is_empty());
    assert_eq!(context.allowed_actions, ["plan.show", "review.disposition"]);
    assert!(
        context
            .blocked_actions
            .iter()
            .any(|action| action.action == "exec.resume")
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn material_review_dispositions_close_every_product_decision_branch() {
    let mut accepted = reviewed_plan("material-accepted");
    let accepted_id = accepted
        .record_review(
            "reviewer".to_owned(),
            "Change the public contract".to_owned(),
            ReviewClassification::MaterialChange,
            None,
            timestamp(12),
        )
        .expect("Material review should record");
    accepted
        .dispose_material_review(
            &accepted_id,
            MaterialReviewDisposition::AcceptChange,
            "user".to_owned(),
            "chat:accept-change".to_owned(),
            "The change belongs to the active objective".to_owned(),
            timestamp(13),
        )
        .expect("accept-change should record");
    let item = accepted
        .review_item(&accepted_id)
        .expect("accepted Material item should exist");
    assert_eq!(item.status(), ReviewStatus::Blocked);
    assert_eq!(
        item.disposition(),
        Some(MaterialReviewDisposition::AcceptChange)
    );
    assert_eq!(item.disposition_actor(), Some("user"));
    assert_eq!(item.disposition_reference(), Some("chat:accept-change"));
    assert!(accepted.is_blocked_for_material_review());
    let accepted_context = build_agent_context(Path::new("C:/fixture"), Some(&accepted))
        .expect("accepted Material context should build");
    assert_eq!(
        accepted_context.allowed_actions,
        ["plan.show", "plan.amend.propose"]
    );
    assert!(!accepted_context.approval_required);
    assert!(
        accepted_context
            .blocked_actions
            .iter()
            .any(|action| { action.action == "review.disposition.revise" })
    );

    let mut declined = reviewed_plan("material-declined");
    let declined_id = declined
        .record_review(
            "reviewer".to_owned(),
            "Replace the approved interface".to_owned(),
            ReviewClassification::MaterialChange,
            None,
            timestamp(12),
        )
        .expect("Material review should record");
    declined
        .dispose_material_review(
            &declined_id,
            MaterialReviewDisposition::Decline,
            "user".to_owned(),
            "chat:decline-change".to_owned(),
            "The request is outside the approved objective".to_owned(),
            timestamp(13),
        )
        .expect("decline should resolve the request");
    assert_eq!(declined.status(), PlanStatus::Review);
    assert_eq!(
        declined
            .review_item(&declined_id)
            .expect("declined Material item should exist")
            .status(),
        ReviewStatus::Resolved
    );
    declined
        .accept_review(
            "reviewer".to_owned(),
            "chat:accept-declined-result".to_owned(),
            timestamp(14),
        )
        .expect("declined Material request should not block acceptance");

    let mut deferred = reviewed_plan("material-deferred");
    let feedback = "Evaluate the interface replacement separately";
    let deferred_id = deferred
        .record_review(
            "reviewer".to_owned(),
            feedback.to_owned(),
            ReviewClassification::MaterialChange,
            None,
            timestamp(12),
        )
        .expect("Material review should record");
    deferred
        .dispose_material_review(
            &deferred_id,
            MaterialReviewDisposition::DeferToFollowUp,
            "user".to_owned(),
            "chat:defer-change".to_owned(),
            "The request is valid but independent".to_owned(),
            timestamp(13),
        )
        .expect("defer should create sourced follow-up work");
    assert_eq!(deferred.status(), PlanStatus::Review);
    assert_eq!(deferred.follow_ups(), [feedback]);
    assert_eq!(deferred.final_outcome().follow_up_tasks(), [feedback]);
    let source = deferred
        .final_outcome()
        .follow_up_sources()
        .first()
        .expect("deferred Material request should retain its review source");
    assert_eq!(source.review_id(), deferred_id);
    assert_eq!(source.task(), feedback);
    deferred
        .accept_review(
            "reviewer".to_owned(),
            "chat:accept-deferred-result".to_owned(),
            timestamp(14),
        )
        .expect("deferred Material request should not block acceptance");
}

#[test]
fn terminated_review_amendments_allow_audited_decline_or_defer_revision() {
    let cases = [
        (
            TerminalAmendment::Reject,
            AmendmentStatus::Rejected,
            MaterialReviewDisposition::Decline,
        ),
        (
            TerminalAmendment::Withdraw,
            AmendmentStatus::Withdrawn,
            MaterialReviewDisposition::DeferToFollowUp,
        ),
        (
            TerminalAmendment::Cancel,
            AmendmentStatus::Cancelled,
            MaterialReviewDisposition::Decline,
        ),
    ];
    for (index, (terminal, expected_status, revised_disposition)) in cases.into_iter().enumerate() {
        let mut plan = pending_review_amendment(&format!("decision-revision-{index}"));
        assert_eq!(
            terminate_review_amendment(&mut plan, terminal),
            expected_status
        );
        let amendment = plan.amendment("C1").expect("amendment should exist");
        assert_eq!(amendment.source_review_id(), Some("REV-1"));
        let item = plan.review_item("REV-1").expect("review should exist");
        assert_eq!(item.linked_changes(), ["C1"]);
        assert_eq!(item.material_decisions().len(), 1);
        assert!(plan.revisable_material_review().is_some());
        let context = build_agent_context(Path::new("C:/fixture"), Some(&plan))
            .expect("terminal amendment context should build");
        assert!(context.approval_required);
        assert!(
            context
                .allowed_actions
                .contains(&"plan.amend.propose".to_owned())
        );
        assert!(
            context
                .allowed_actions
                .contains(&"review.disposition.revise".to_owned())
        );

        plan.revise_material_review(
            "REV-1",
            revised_disposition,
            "user".to_owned(),
            format!("chat:revise-decision-{index}"),
            "The terminated patch changes the product decision".to_owned(),
            timestamp(18),
        )
        .expect("eligible Material decision should revise");
        assert_eq!(plan.status(), PlanStatus::Review);
        let item = plan.review_item("REV-1").expect("review should exist");
        assert_eq!(
            item.status(),
            match revised_disposition {
                MaterialReviewDisposition::Decline => ReviewStatus::Resolved,
                MaterialReviewDisposition::DeferToFollowUp => ReviewStatus::Deferred,
                MaterialReviewDisposition::AcceptChange => unreachable!(),
            }
        );
        assert_eq!(item.material_decisions().len(), 2);
        assert_eq!(
            item.material_decisions()[0].disposition(),
            MaterialReviewDisposition::AcceptChange
        );
        assert_eq!(
            item.material_decisions()[1].disposition(),
            revised_disposition
        );
        assert_eq!(item.material_decisions()[1].terminated_change(), Some("C1"));
        let expected_follow_ups =
            usize::from(revised_disposition == MaterialReviewDisposition::DeferToFollowUp);
        assert_eq!(plan.follow_ups().len(), expected_follow_ups);
        assert_eq!(
            plan.final_outcome().follow_up_sources().len(),
            expected_follow_ups
        );
    }
}

#[test]
fn material_decision_revision_rejects_pending_applied_replacement_and_wrong_review_states() {
    let mut pending = pending_review_amendment("revision-before-terminal");
    let before = canonical_json_bytes(&pending).expect("plan should canonicalize");
    assert!(
        pending
            .revise_material_review(
                "REV-1",
                MaterialReviewDisposition::Decline,
                "user".to_owned(),
                "chat:too-early".to_owned(),
                "The amendment is not terminal".to_owned(),
                timestamp(15),
            )
            .is_err()
    );
    assert_eq!(
        canonical_json_bytes(&pending).expect("plan should canonicalize"),
        before
    );

    let mut applied = pending.clone();
    applied
        .approve_amendment(
            "C1",
            "user".to_owned(),
            "chat:approved".to_owned(),
            timestamp(15),
        )
        .expect("amendment should approve");
    applied
        .apply_amendment("C1", timestamp(16))
        .expect("amendment should apply");
    assert!(
        applied
            .revise_material_review(
                "REV-1",
                MaterialReviewDisposition::Decline,
                "user".to_owned(),
                "chat:after-apply".to_owned(),
                "Applied changes cannot revise the decision".to_owned(),
                timestamp(17),
            )
            .is_err()
    );

    terminate_review_amendment(&mut pending, TerminalAmendment::Reject);
    assert!(
        pending
            .revise_material_review(
                "REV-2",
                MaterialReviewDisposition::Decline,
                "user".to_owned(),
                "chat:wrong-review".to_owned(),
                "The review link must match".to_owned(),
                timestamp(16),
            )
            .is_err()
    );
    pending
        .propose_amendment(
            "Try a replacement patch".to_owned(),
            review_amendment_patch(),
            None,
            state_hash(&pending),
            "codex".to_owned(),
            timestamp(17),
        )
        .expect("replacement amendment should propose");
    assert_eq!(
        pending
            .amendment("C2")
            .expect("replacement should exist")
            .source_review_id(),
        Some("REV-1")
    );
    assert_eq!(
        pending
            .review_item("REV-1")
            .expect("review should exist")
            .linked_changes(),
        ["C1", "C2"]
    );
    assert!(plan_revision_is_blocked_by_replacement(&mut pending));
}

#[test]
fn legacy_unlinked_review_amendment_and_projection_fields_remain_loadable() {
    let plan = pending_review_amendment("legacy-review-amendment");
    let mut value = serde_json::to_value(plan).expect("plan should serialize");
    value["amendments"][0]
        .as_object_mut()
        .expect("amendment should be an object")
        .remove("source_review_id");
    let review = value["review_items"][0]
        .as_object_mut()
        .expect("review should be an object");
    review.remove("linked_changes");
    review.remove("material_decisions");
    let legacy: Plan = serde_json::from_value(value).expect("legacy plan should remain loadable");
    assert_eq!(
        legacy
            .review_item("REV-1")
            .expect("review should exist")
            .disposition(),
        Some(MaterialReviewDisposition::AcceptChange)
    );
    assert!(
        legacy
            .review_item("REV-1")
            .expect("review should exist")
            .material_decisions()
            .is_empty()
    );
    assert_eq!(
        legacy
            .amendment("C1")
            .expect("amendment should exist")
            .source_review_id(),
        None
    );
}

fn plan_revision_is_blocked_by_replacement(plan: &mut Plan) -> bool {
    plan.revise_material_review(
        "REV-1",
        MaterialReviewDisposition::DeferToFollowUp,
        "user".to_owned(),
        "chat:replacement-pending".to_owned(),
        "A replacement patch still owns the decision boundary".to_owned(),
        timestamp(18),
    )
    .is_err()
}

#[test]
fn reserved_rework_ids_are_never_reused_and_complete_r_tasks_reach_done() {
    let mut plan = reviewed_plan("rework-lifecycle");
    let first_review = plan
        .record_review(
            "reviewer".to_owned(),
            "Correct the first defect".to_owned(),
            ReviewClassification::InScopeRework,
            Some(task_id("T1")),
            timestamp(12),
        )
        .expect("first rework should record");
    let incomplete = DraftTaskInput {
        id: Some(task_id("R1")),
        title: "Incomplete rework".to_owned(),
        depends_on: vec![task_id("T1")],
        steps: Vec::new(),
        files: Vec::new(),
        acceptance_criteria: Vec::new(),
        verification: Vec::new(),
        commit_gate: None,
    };
    assert!(
        plan.begin_review_rework(&first_review, Some(incomplete), timestamp(13))
            .is_err()
    );
    assert_eq!(plan.status(), PlanStatus::Review);
    assert!(plan.task(&task_id("R1")).is_none());

    let second_review = plan
        .record_review(
            "reviewer".to_owned(),
            "Reserve a second correction".to_owned(),
            ReviewClassification::InScopeRework,
            Some(task_id("T1")),
            timestamp(14),
        )
        .expect("second rework should record");
    assert_eq!(
        plan.review_item(&second_review)
            .and_then(|item| item.linked_task()),
        Some(&task_id("R2"))
    );

    plan.begin_review_rework(
        &first_review,
        Some(complete_rework_input("R1")),
        timestamp(15),
    )
    .expect("complete R1 should materialize");
    assert_eq!(plan.status(), PlanStatus::InProgress);
    assert_eq!(
        plan.task(&task_id("R1")).map(Task::status),
        Some(TaskStatus::Ready)
    );
    assert_eq!(plan.task_order(), [task_id("T1"), task_id("R1")]);

    plan.start_task(&task_id("R1"), timestamp(16))
        .expect("R1 should start");
    satisfy_task(&mut plan, &task_id("R1"), "R1-A1", "R1-VERIFY", 5, 17);
    plan.complete_task(&task_id("R1"), timestamp(19))
        .expect("R1 should complete");
    satisfy_commit(&mut plan, &task_id("R1"), "src/lib.rs", 7, 20);
    plan.record_global_check_pass(&check_id("GLOBAL-V1"), evidence_id(8), timestamp(21))
        .expect("global check should rerun");
    set_final_outcome(&mut plan, 22);
    plan.finish_execution(timestamp(22))
        .expect("rework should return to Review");
    assert!(
        plan.accept_review(
            "reviewer".to_owned(),
            "chat:premature".to_owned(),
            timestamp(23),
        )
        .is_err()
    );
    plan.resolve_review(&first_review, timestamp(24))
        .expect("R1 feedback should resolve");
    assert!(
        plan.accept_review(
            "reviewer".to_owned(),
            "chat:second-feedback-open".to_owned(),
            timestamp(25),
        )
        .is_err()
    );
}

#[test]
fn acceptance_defect_reruns_evidence_before_explicit_acceptance() {
    let mut plan = reviewed_plan("acceptance-rerun");
    let review = plan
        .record_review(
            "reviewer".to_owned(),
            "Repeat the acceptance proof".to_owned(),
            ReviewClassification::AcceptanceDefect,
            Some(task_id("T1")),
            timestamp(12),
        )
        .expect("acceptance defect should record");
    plan.begin_review_rework(&review, None, timestamp(13))
        .expect("acceptance rerun should begin");
    assert_eq!(
        plan.task(&task_id("T1")).map(Task::status),
        Some(TaskStatus::Ready)
    );
    plan.start_task(&task_id("T1"), timestamp(14))
        .expect("original task should restart");
    satisfy_task(&mut plan, &task_id("T1"), "T1-A1", "TASK-V1", 5, 15);
    plan.complete_task(&task_id("T1"), timestamp(17))
        .expect("evidence-only rerun should complete");
    plan.record_global_check_pass(&check_id("GLOBAL-V1"), evidence_id(7), timestamp(18))
        .expect("global check should rerun");
    set_final_outcome(&mut plan, 19);
    plan.finish_execution(timestamp(19))
        .expect("rerun should return to Review");
    plan.resolve_review(&review, timestamp(20))
        .expect("rerun should resolve feedback");
    plan.accept_review(
        "reviewer".to_owned(),
        "chat:final-acceptance".to_owned(),
        timestamp(21),
    )
    .expect("resolved plan should reach Done");
    assert_eq!(plan.status(), PlanStatus::Done);
    let acceptance = plan
        .review_items()
        .last()
        .expect("acceptance should record");
    assert_eq!(acceptance.classification(), ReviewClassification::Accepted);
    assert_eq!(
        acceptance.approval_reference(),
        Some("chat:final-acceptance")
    );
    let projection = render_plan(&plan).expect("accepted plan should render");
    assert!(projection.markdown().contains("chat:final-acceptance"));
}

#[test]
fn application_review_record_is_revision_checked_idempotent_and_audited() {
    let project = TestProject::new();
    let store = PlanStore::new(project.path());
    let initial = seed("application-audit");
    let id = initial.id().clone();
    store
        .create_plan(
            &initial,
            request_id(1),
            "codex",
            vec!["test".to_owned(), "create".to_owned()],
        )
        .expect("plan should persist");
    persist_reviewed_plan(&store, &id);
    let plan = store.load_plan(&id).expect("reviewed plan should load");
    write_projection(
        &project.path().join(format!("docs/plan/{id}.md")),
        &render_plan(&plan).expect("plan should render"),
        None,
    )
    .expect("projection should publish");

    let service = ReviewService::discover(project.path()).expect("service should discover");
    let request = PlanMutationRequest {
        plan_id: id.clone(),
        expected_revision: plan.revision(),
        request_id: request_id(50),
        actor: "reviewer".to_owned(),
        command: vec!["mino".to_owned(), "review".to_owned(), "record".to_owned()],
        updated_at: timestamp(30),
    };
    let recorded = service
        .record(
            request.clone(),
            ReviewClassification::FollowUp,
            "Track a separate optimization".to_owned(),
            None,
        )
        .expect("follow-up should persist");
    assert_eq!(recorded.assigned_id.as_deref(), Some("REV-1"));
    let advanced = service
        .record(
            PlanMutationRequest {
                plan_id: id.clone(),
                expected_revision: recorded.revision,
                request_id: request_id(51),
                actor: "reviewer".to_owned(),
                command: vec![
                    "mino".to_owned(),
                    "review".to_owned(),
                    "record".to_owned(),
                    "second".to_owned(),
                ],
                updated_at: timestamp(31),
            },
            ReviewClassification::FollowUp,
            "Track another separate optimization".to_owned(),
            None,
        )
        .expect("second follow-up should advance the plan");
    let replayed = service
        .record(
            request,
            ReviewClassification::FollowUp,
            "Track a separate optimization".to_owned(),
            None,
        )
        .expect("late exact retry should replay");
    assert!(replayed.replayed);
    assert_eq!(replayed.revision, advanced.revision);

    let plan = store.load_plan(&id).expect("updated plan should load");
    assert_eq!(
        plan.follow_ups(),
        [
            "Track a separate optimization",
            "Track another separate optimization"
        ]
    );
    let events = store.events(&id).expect("events should load");
    let event = serde_json::to_value(events.last().expect("review event should exist"))
        .expect("event should serialize");
    assert_eq!(
        event["changed_fields"],
        serde_json::json!(["review_items", "follow_ups"])
    );
    assert_eq!(event["result"], "Succeeded");
    assert_eq!(
        store.audit(&id).expect("store should audit").revision(),
        plan.revision()
    );
}

#[test]
fn application_material_defer_is_retry_safe_and_preserves_review_source() {
    let project = TestProject::new();
    let store = PlanStore::new(project.path());
    let initial = seed("application-material-disposition");
    let id = initial.id().clone();
    store
        .create_plan(
            &initial,
            request_id(1),
            "codex",
            vec!["test".to_owned(), "create".to_owned()],
        )
        .expect("plan should persist");
    persist_reviewed_plan(&store, &id);
    let plan = store.load_plan(&id).expect("reviewed plan should load");
    write_projection(
        &project.path().join(format!("docs/plan/{id}.md")),
        &render_plan(&plan).expect("plan should render"),
        None,
    )
    .expect("projection should publish");

    let service = ReviewService::discover(project.path()).expect("service should discover");
    let recorded = service
        .record(
            PlanMutationRequest {
                plan_id: id.clone(),
                expected_revision: plan.revision(),
                request_id: request_id(60),
                actor: "reviewer".to_owned(),
                command: vec!["mino".to_owned(), "review".to_owned(), "record".to_owned()],
                updated_at: timestamp(30),
            },
            ReviewClassification::MaterialChange,
            "Evaluate a separate public API".to_owned(),
            None,
        )
        .expect("Material review should persist");
    let request = PlanMutationRequest {
        plan_id: id.clone(),
        expected_revision: recorded.revision,
        request_id: request_id(61),
        actor: "user".to_owned(),
        command: vec![
            "mino".to_owned(),
            "review".to_owned(),
            "disposition".to_owned(),
        ],
        updated_at: timestamp(31),
    };
    let disposed = service
        .disposition(
            request.clone(),
            "REV-1".to_owned(),
            MaterialReviewDisposition::DeferToFollowUp,
            "chat:defer-public-api".to_owned(),
            "The API belongs to a separate objective".to_owned(),
        )
        .expect("Material defer should persist");
    let replayed = service
        .disposition(
            request,
            "REV-1".to_owned(),
            MaterialReviewDisposition::DeferToFollowUp,
            "chat:defer-public-api".to_owned(),
            "The API belongs to a separate objective".to_owned(),
        )
        .expect("exact retry should replay");
    assert!(replayed.replayed);
    assert_eq!(replayed.revision, disposed.revision);

    let plan = store.load_plan(&id).expect("disposed plan should load");
    assert_eq!(plan.status(), PlanStatus::Review);
    let source = plan
        .final_outcome()
        .follow_up_sources()
        .first()
        .expect("deferred request should retain a source");
    assert_eq!(source.review_id(), "REV-1");
    assert_eq!(source.task(), "Evaluate a separate public API");
    let events = store.events(&id).expect("events should load");
    let event = serde_json::to_value(events.last().expect("disposition event should exist"))
        .expect("event should serialize");
    assert_eq!(
        event["changed_fields"],
        serde_json::json!([
            "review_items",
            "status",
            "resume_status",
            "blocker",
            "follow_ups",
            "final_outcome"
        ])
    );
}

fn initial_disposition_arguments(
    project: &TestProject,
    id: &PlanId,
    expected_revision: u64,
) -> Vec<String> {
    vec![
        "--root".to_owned(),
        project.path().to_string_lossy().into_owned(),
        "--format".to_owned(),
        "json".to_owned(),
        "--no-input".to_owned(),
        "review".to_owned(),
        "disposition".to_owned(),
        "--plan".to_owned(),
        id.to_string(),
        "--expect-revision".to_owned(),
        expected_revision.to_string(),
        "--request-id".to_owned(),
        request_id(71).to_string(),
        "--actor".to_owned(),
        "user".to_owned(),
        "--review".to_owned(),
        "REV-1".to_owned(),
        "--decision".to_owned(),
        "accept-change".to_owned(),
        "--decision-ref".to_owned(),
        "chat:accept-change".to_owned(),
        "--reason".to_owned(),
        "The request belongs to this objective".to_owned(),
    ]
}

fn persist_accepted_material_review(project: &TestProject) -> (PlanStore, PlanId, u64) {
    let store = PlanStore::new(project.path());
    let initial = seed("application-material-revision");
    let id = initial.id().clone();
    store
        .create_plan(
            &initial,
            request_id(1),
            "codex",
            vec!["test".to_owned(), "create".to_owned()],
        )
        .expect("plan should persist");
    persist_reviewed_plan(&store, &id);
    let plan = store.load_plan(&id).expect("reviewed plan should load");
    write_projection(
        &project.path().join(format!("docs/plan/{id}.md")),
        &render_plan(&plan).expect("plan should render"),
        None,
    )
    .expect("projection should publish");
    let reviews = ReviewService::discover(project.path()).expect("review service should discover");
    let recorded = reviews
        .record(
            PlanMutationRequest {
                plan_id: id.clone(),
                expected_revision: plan.revision(),
                request_id: request_id(70),
                actor: "reviewer".to_owned(),
                command: vec!["mino".to_owned(), "review".to_owned(), "record".to_owned()],
                updated_at: timestamp(30),
            },
            ReviewClassification::MaterialChange,
            "Change the public contract".to_owned(),
            None,
        )
        .expect("Material review should persist");
    let output = run_mino(&initial_disposition_arguments(
        project,
        &id,
        recorded.revision,
    ));
    let accepted = parse_cli_success(&output);
    (
        store,
        id,
        accepted["revision"]
            .as_u64()
            .expect("accepted revision should exist"),
    )
}

fn reject_persisted_review_amendment(
    project: &TestProject,
    id: &PlanId,
    expected_revision: u64,
) -> u64 {
    let amendments =
        AmendmentService::discover(project.path()).expect("amendment service should discover");
    let proposed = amendments
        .propose(
            PlanMutationRequest {
                plan_id: id.clone(),
                expected_revision,
                request_id: request_id(72),
                actor: "codex".to_owned(),
                command: vec!["mino".to_owned(), "plan".to_owned(), "amend".to_owned()],
                updated_at: Timestamp::now_utc(),
            },
            "Apply the requested contract change".to_owned(),
            review_amendment_patch(),
            None,
        )
        .expect("review amendment should persist");
    let rejected = amendments
        .reject(
            PlanMutationRequest {
                plan_id: id.clone(),
                expected_revision: proposed.revision,
                request_id: request_id(73),
                actor: "user".to_owned(),
                command: vec!["mino".to_owned(), "plan".to_owned(), "amend".to_owned()],
                updated_at: Timestamp::now_utc(),
            },
            "C1".to_owned(),
            "chat:reject-patch".to_owned(),
            "The concrete patch is unsuitable".to_owned(),
        )
        .expect("review amendment should reject");
    rejected.revision
}

fn decision_revision_arguments(
    project: &TestProject,
    id: &PlanId,
    expected_revision: u64,
) -> Vec<String> {
    vec![
        "--root".to_owned(),
        project.path().to_string_lossy().into_owned(),
        "--format".to_owned(),
        "json".to_owned(),
        "--no-input".to_owned(),
        "review".to_owned(),
        "disposition".to_owned(),
        "revise".to_owned(),
        "--plan".to_owned(),
        id.to_string(),
        "--expect-revision".to_owned(),
        expected_revision.to_string(),
        "--request-id".to_owned(),
        request_id(74).to_string(),
        "--actor".to_owned(),
        "user".to_owned(),
        "--review".to_owned(),
        "REV-1".to_owned(),
        "--decision".to_owned(),
        "defer-to-follow-up".to_owned(),
        "--decision-ref".to_owned(),
        "chat:revise-to-follow-up".to_owned(),
        "--reason".to_owned(),
        "The rejected patch belongs in follow-up".to_owned(),
    ]
}

#[test]
fn cli_material_decision_revision_is_retry_safe_and_preserves_history() {
    let project = TestProject::new();
    let (store, id, accepted_revision) = persist_accepted_material_review(&project);
    let rejected_revision = reject_persisted_review_amendment(&project, &id, accepted_revision);
    let arguments = decision_revision_arguments(&project, &id, rejected_revision);
    let first = run_mino(&arguments);
    assert!(
        first.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    let replay = run_mino(&arguments);
    assert!(
        replay.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&replay.stderr)
    );
    let replay: serde_json::Value =
        serde_json::from_slice(&replay.stdout).expect("replay should return JSON");
    assert_eq!(replay["replayed"], true);

    let plan = store.load_plan(&id).expect("revised plan should load");
    let item = plan.review_item("REV-1").expect("review should exist");
    assert_eq!(item.material_decisions().len(), 2);
    assert_eq!(item.material_decisions()[1].terminated_change(), Some("C1"));
    assert_eq!(plan.follow_ups(), ["Change the public contract"]);
    assert_eq!(plan.final_outcome().follow_up_sources().len(), 1);
    let events = store.events(&id).expect("events should load");
    let event = serde_json::to_value(events.last().expect("revision event should exist"))
        .expect("event should serialize");
    assert_eq!(
        event["changed_fields"],
        serde_json::json!([
            "review_items",
            "status",
            "resume_status",
            "blocker",
            "follow_ups",
            "final_outcome"
        ])
    );
}

fn persist_reviewed_plan(store: &PlanStore, id: &PlanId) {
    persist(store, id, 1, 2, vec!["tasks"], |plan| {
        plan.add_task(original_task(), timestamp(1))
    });
    persist(store, id, 2, 3, vec!["tasks.T1.status"], |plan| {
        plan.mark_task_ready(&task_id("T1"), timestamp(2))
    });
    persist(store, id, 3, 4, vec!["status"], |plan| {
        plan.finalize(timestamp(3))
    });
    persist(store, id, 4, 5, vec!["approvals"], |plan| {
        plan.record_approval(Approval::plan(
            "user",
            "chat:approved",
            timestamp(4),
            GitFlowConsent::Approved,
        ))
    });
    persist(store, id, 5, 6, vec!["tasks.T1.status"], |plan| {
        plan.start_task(&task_id("T1"), timestamp(5))
    });
    persist(store, id, 6, 7, vec!["criteria"], |plan| {
        plan.record_task_criterion_pass(
            &task_id("T1"),
            &criterion_id("T1-A1"),
            evidence_id(1),
            timestamp(6),
        )
    });
    persist(store, id, 7, 8, vec!["checks"], |plan| {
        plan.record_task_check_pass(
            &task_id("T1"),
            &check_id("TASK-V1"),
            evidence_id(2),
            timestamp(7),
        )
    });
    persist(store, id, 8, 9, vec!["tasks.T1.status"], |plan| {
        plan.complete_task(&task_id("T1"), timestamp(8))
    });
    persist(store, id, 9, 10, vec!["commit_gate"], |plan| {
        plan.record_task_commit(
            &task_id("T1"),
            &"1".repeat(40),
            vec!["src/lib.rs".to_owned()],
            evidence_id(3),
            timestamp(9),
        )
    });
    persist(store, id, 10, 11, vec!["verification_plan"], |plan| {
        plan.record_global_check_pass(&check_id("GLOBAL-V1"), evidence_id(4), timestamp(10))
    });
    persist(store, id, 11, 12, vec!["final_outcome"], |plan| {
        plan.set_final_outcome(
            "Reviewed implementation is verified".to_owned(),
            "N/A".to_owned(),
            Vec::new(),
            timestamp(11),
        )
    });
    persist(store, id, 12, 13, vec!["status"], |plan| {
        plan.finish_execution(timestamp(11))
    });
}

fn persist<F>(
    store: &PlanStore,
    id: &PlanId,
    expected_revision: u64,
    request_number: u64,
    changed_fields: Vec<&str>,
    mutation: F,
) where
    F: FnOnce(&mut Plan) -> Result<(), mino::domain::DomainError>,
{
    store
        .commit(
            id,
            MutationRequest::new(
                expected_revision,
                request_id(request_number),
                "codex",
                vec!["test".to_owned(), request_number.to_string()],
                changed_fields.into_iter().map(str::to_owned).collect(),
            )
            .expect("mutation request should build"),
            mutation,
        )
        .expect("mutation should persist");
}
