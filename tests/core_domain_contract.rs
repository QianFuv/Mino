//! Contract tests for the versioned Mino domain aggregate.

use mino::domain::{
    AcceptanceCriterion, AmendmentPatch, CheckId, CheckRunOutcome, CheckpointKind, CommitGate,
    CriterionId, DeviationClassification, DeviationStatus, DomainError, DomainErrorKind, Event,
    Evidence, EvidenceId, GitFlowConsent, MaterialReviewDisposition, Plan, PlanId, PlanStatus,
    RequestId, ReviewClassification, ReviewItem, Task, TaskId, TaskStatus, Timestamp,
    VerificationCheck, WorkspaceFingerprint, WorkspaceGitEntry,
};
use mino::store::{canonical_json_bytes, sha256_digest};
use schemars::schema_for;
use serde_json::{Value, json};

fn timestamp(minute: u8) -> Timestamp {
    Timestamp::parse(format!("2026-07-25T11:{minute:02}:00Z"))
        .expect("test timestamp should be valid")
}

#[test]
fn capture_blocked_outcome_has_a_stable_wire_value() {
    let serialized = serde_json::to_value(CheckRunOutcome::CaptureBlocked)
        .expect("capture-blocked outcome should serialize");
    assert_eq!(serialized, json!("capture_blocked"));
    assert_eq!(
        serde_json::from_value::<CheckRunOutcome>(serialized)
            .expect("capture-blocked outcome should deserialize"),
        CheckRunOutcome::CaptureBlocked
    );
}

fn task_id(value: &str) -> TaskId {
    TaskId::parse(value).expect("test task identifier should be valid")
}

fn check_id(value: &str) -> CheckId {
    CheckId::parse(value).expect("test check identifier should be valid")
}

fn criterion_id(value: &str) -> CriterionId {
    CriterionId::parse(value).expect("test criterion identifier should be valid")
}

fn evidence_id(value: &str) -> EvidenceId {
    EvidenceId::parse(value).expect("test evidence identifier should be valid")
}

fn plan_id() -> PlanId {
    PlanId::parse("2026-07-25-domain-contract").expect("test plan identifier should be valid")
}

fn configured_task(id: TaskId, title: &str, depends_on: Vec<TaskId>) -> Task {
    let criterion = criterion_id(&format!("{id}-A1"));
    let check = check_id(&format!("{id}-V1"));
    let mut task = Task::new(id, title, depends_on);
    task.add_acceptance_criterion(AcceptanceCriterion::new(
        criterion,
        format!("{title} satisfies its observable behavior"),
    ))
    .expect("criterion should be added");
    task.add_verification_check(VerificationCheck::new(
        check,
        vec!["cargo".to_owned(), "test".to_owned()],
        ".",
        0,
        true,
    ))
    .expect("check should be added");
    task.set_commit_gate(CommitGate::new(
        true,
        "test(core): verify domain task",
        vec!["src/domain/**".to_owned()],
    ))
    .expect("commit gate should be added");
    task
}

fn add_global_verification(plan: &mut Plan, minute: u8) {
    plan.add_global_verification(
        VerificationCheck::new(
            check_id("GLOBAL-V1"),
            vec!["cargo".to_owned(), "test".to_owned()],
            ".",
            0,
            true,
        ),
        timestamp(minute),
    )
    .expect("global verification should be added");
}

fn satisfy_task(plan: &mut Plan, task_id: &TaskId, first_evidence: u16, minute: u8) {
    plan.record_task_criterion_pass(
        task_id,
        &criterion_id(&format!("{task_id}-A1")),
        evidence_id(&format!("E{first_evidence:04}")),
        timestamp(minute),
    )
    .expect("criterion evidence should be recorded");
    plan.record_task_check_pass(
        task_id,
        &check_id(&format!("{task_id}-V1")),
        evidence_id(&format!("E{:04}", first_evidence + 1)),
        timestamp(minute.saturating_add(1)),
    )
    .expect("check evidence should be recorded");
}

fn satisfy_global(plan: &mut Plan, evidence: u16, minute: u8) {
    plan.record_global_check_pass(
        &check_id("GLOBAL-V1"),
        evidence_id(&format!("E{evidence:04}")),
        timestamp(minute),
    )
    .expect("global evidence should be recorded");
}

fn set_final_outcome(plan: &mut Plan, minute: u8) {
    plan.set_final_outcome(
        "Verified protocol execution completed".to_owned(),
        "N/A".to_owned(),
        Vec::new(),
        timestamp(minute),
    )
    .expect("Final Outcome should be recorded");
}

fn satisfy_commit(plan: &mut Plan, task_id: &TaskId, evidence: u16, minute: u8) {
    let commit_digit = char::from_digit(u32::from(evidence % 16), 16)
        .expect("commit fixture digit should be hexadecimal");
    plan.record_task_commit(
        task_id,
        &commit_digit.to_string().repeat(40),
        vec![format!("src/domain/{task_id}.rs")],
        evidence_id(&format!("E{evidence:04}")),
        timestamp(minute),
    )
    .expect("task commit should be recorded");
}

fn satisfy_commit_skip(plan: &mut Plan, task_id: &TaskId, evidence: u16, minute: u8) {
    plan.skip_task_commit(
        task_id,
        evidence_id(&format!("E{evidence:04}")),
        timestamp(minute),
    )
    .expect("approved commit skip should be recorded");
}

fn approved_two_task_plan() -> (Plan, TaskId, TaskId) {
    let first_id = task_id("T1");
    let second_id = task_id("T2");
    let mut plan = Plan::new(
        plan_id(),
        "Implement a deterministic protocol.",
        timestamp(0),
    );
    plan.add_task(
        configured_task(first_id.clone(), "Define the model", Vec::new()),
        timestamp(1),
    )
    .expect("first task should be added");
    plan.add_task(
        configured_task(
            second_id.clone(),
            "Verify transitions",
            vec![first_id.clone()],
        ),
        timestamp(2),
    )
    .expect("second task should be added");
    add_global_verification(&mut plan, 3);
    plan.mark_task_ready(&first_id, timestamp(4))
        .expect("first task should become ready");
    plan.mark_task_ready(&second_id, timestamp(5))
        .expect("second task should become ready");
    plan.finalize(timestamp(6))
        .expect("complete draft should finalize");
    plan.record_approval(mino::domain::Approval::plan(
        "user",
        "chat:approval",
        timestamp(7),
        GitFlowConsent::Approved,
    ))
    .expect("ready plan should accept approval");
    (plan, first_id, second_id)
}

fn populate_authored_fixture(serialized: &mut Value) {
    serialized["summary"] = Value::from("Define a strict typed protocol foundation");
    serialized["metadata"] = json!({
        "name": "Protocol domain",
        "priority": "P1",
        "plan_type": "Feature",
        "area": "core",
        "owner": "codex",
        "created_at": "2026-07-25T11:00:00Z",
        "updated_at": "2026-07-25T11:07:00Z",
        "branch": "main",
        "markdown_path": "docs/plan/domain.md"
    });
    serialized["context"] = json!([{
        "reference": "AGENTS.md",
        "fact": "Rust checks are mandatory",
        "implication": "Each task declares deterministic checks"
    }]);
    serialized["scope"] = json!({
        "goal": "Define the protocol aggregate",
        "deliverables": ["Versioned schema", "Lifecycle model"],
        "in_scope": ["Plan and task state"],
        "out_of_scope": ["Persistence adapters"]
    });
    serialized["decisions"] = json!([{
        "item": "Serialization",
        "type": "Decision",
        "decision": "Use strict JSON",
        "reason": "Reject ambiguous state",
        "status": "Accepted"
    }]);
    serialized["approach"] = json!({
        "summary": "Model invariants in typed aggregates",
        "file_map": [{
            "path": "src/domain/plan.rs",
            "change": "Modify",
            "reason": "Own plan transitions",
            "task_id": "T1"
        }]
    });
    serialized["interfaces"] = Value::from("JSON plan protocol and Rust domain API");
    serialized["edge_cases"] = json!([{
        "case": "A task completes without evidence",
        "expected_behavior": "Reject the transition",
        "covered_by": ["T1-A1", "T1-V1"]
    }]);
    serialized["standards"] = json!([{
        "package_id": "rust-core",
        "version": "1.0.0",
        "digest": "sha256:standard",
        "source": "repository"
    }]);
    serialized["git_readiness"] = json!({
        "repository": "Present",
        "working_tree": "Clean",
        "branch": "main",
        "base_commit": "61c9a67",
        "base_status": "Clean",
        "git_flow_enabled": true,
        "git_flow_consent": "Approved",
        "approved_at": "2026-07-25T11:07:00Z"
    });
}

fn populate_execution_fixture(serialized: &mut Value) {
    serialized["tasks"][0]["steps"] = json!(["Define the aggregate", "Verify invariants"]);
    serialized["tasks"][0]["file_map"] = json!([{
        "path": "src/domain/task.rs",
        "change": "Modify",
        "reason": "Own task transitions",
        "task_id": "T1"
    }]);
    serialized["review_items"] = json!([{
        "id": "REV-1",
        "reviewer": "reviewer",
        "feedback": "Evidence gates are explicit",
        "classification": "Accepted",
        "action": "Record acceptance",
        "linked_task": "T1",
        "status": "Resolved",
        "recorded_at": "2026-07-25T11:07:00Z"
    }]);
    serialized["follow_ups"] = json!(["Add persistence adapters later"]);
    serialized["lineage"] = json!({
        "parent_plan_id": "2026-07-24-parent-plan",
        "forked_from_revision": 4,
        "fork_reason": "Isolate the protocol foundation",
        "source_state_hash": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "forked_at": "2026-07-25T11:00:00Z"
    });
    serialized["final_outcome"] = json!({
        "summary": "Protocol model ready for execution",
        "remaining_risk": "Persistence is deferred",
        "follow_up_tasks": ["Implement storage"]
    });
    serialized["extensions"] = json!({"vendor.example/trace": {"enabled": true}});
}

fn full_field_plan_value(plan: &Plan) -> Value {
    let mut serialized = serde_json::to_value(plan).expect("plan should serialize");
    populate_authored_fixture(&mut serialized);
    populate_execution_fixture(&mut serialized);
    serialized
}

fn assert_invalid_transition(result: Result<(), DomainError>) {
    let error = result.expect_err("transition should be rejected");
    assert_eq!(error.kind(), DomainErrorKind::InvalidTransition);
}

#[test]
fn schema_and_round_trip_are_strict_and_deterministic() {
    let (plan, _, _) = approved_two_task_plan();
    let serialized = full_field_plan_value(&plan);
    let round_trip: Plan =
        serde_json::from_value(serialized.clone()).expect("plan should deserialize");

    assert_eq!(
        serde_json::to_value(round_trip).expect("round trip should serialize"),
        serialized
    );
    assert_eq!(
        serde_json::to_value(Plan::schema()).expect("schema should serialize"),
        serde_json::to_value(Plan::schema()).expect("schema should serialize deterministically")
    );

    let schema = serde_json::to_value(Plan::schema()).expect("schema should serialize");
    assert_eq!(
        schema["$schema"],
        "https://json-schema.org/draft/2020-12/schema"
    );
    assert_eq!(schema["additionalProperties"], false);
    assert_eq!(schema["$defs"]["Task"]["additionalProperties"], false);
    assert_eq!(schema["$defs"]["SchemaVersion"]["const"], 1);
    assert_eq!(schema["properties"]["revision"]["minimum"], 1);
    assert_eq!(
        schema["$defs"]["ProtocolVersion"]["properties"]["version"]["const"],
        "2026-05-11"
    );
    assert_eq!(
        schema["$defs"]["ProtocolVersion"]["properties"]["revision"]["const"],
        "review-rework-git-flow-v1"
    );
    assert_eq!(schema["$defs"]["Timestamp"]["format"], "date-time");
    assert!(
        schema["$defs"]["PlanId"]["pattern"]
            .as_str()
            .is_some_and(|pattern| pattern.contains("02-29"))
    );
    assert_eq!(schema["$defs"]["EvidenceId"]["pattern"], "^E0*[1-9][0-9]*$");

    let mut unknown_field = serialized.clone();
    unknown_field["unknown_field"] = Value::Bool(true);
    assert!(serde_json::from_value::<Plan>(unknown_field).is_err());

    let mut unsupported_schema = serialized.clone();
    unsupported_schema["schema_version"] = Value::from(99);
    let schema_error =
        serde_json::from_value::<Plan>(unsupported_schema).expect_err("version should fail");
    assert!(
        schema_error
            .to_string()
            .contains("Unsupported schema version")
    );

    let mut unsupported_protocol = serialized;
    unsupported_protocol["protocol_version"]["revision"] = Value::from("future-revision");
    let protocol_error =
        serde_json::from_value::<Plan>(unsupported_protocol).expect_err("revision should fail");
    assert!(
        protocol_error
            .to_string()
            .contains("Unsupported protocol version/revision")
    );
}

#[test]
fn legacy_material_review_projection_loads_without_synthetic_history_or_links() {
    let payload = json!({
        "id": "REV-1",
        "reviewer": "reviewer",
        "feedback": "Change the public contract",
        "classification": "Material Change",
        "action": "Pause for a protected material amendment",
        "linked_task": null,
        "status": "Blocked",
        "recorded_at": "2026-07-25T11:10:00Z",
        "disposition": "Accept Change",
        "disposition_actor": "user",
        "disposition_reference": "chat:accept-change",
        "disposition_reason": "The request belongs to this objective",
        "disposed_at": "2026-07-25T11:11:00Z"
    });
    let item: ReviewItem =
        serde_json::from_value(payload.clone()).expect("legacy review item should load");
    assert_eq!(
        item.disposition(),
        Some(MaterialReviewDisposition::AcceptChange)
    );
    assert!(item.linked_changes().is_empty());
    assert!(item.material_decisions().is_empty());
    assert_eq!(
        serde_json::to_value(item).expect("legacy review item should serialize"),
        payload
    );
}

#[test]
fn plan_lifecycle_requires_approval_and_preserves_legal_order() {
    let first_id = task_id("T1");
    let mut plan = Plan::new(plan_id(), "Implement the lifecycle.", timestamp(0));
    plan.add_task(
        configured_task(first_id.clone(), "Execute the task", Vec::new()),
        timestamp(1),
    )
    .expect("task should be added");
    add_global_verification(&mut plan, 2);
    plan.mark_task_ready(&first_id, timestamp(3))
        .expect("task should become ready");
    plan.finalize(timestamp(4)).expect("draft should finalize");

    let approval_error = plan
        .start_task(&first_id, timestamp(5))
        .expect_err("execution should require approval");
    assert_eq!(approval_error.kind(), DomainErrorKind::ApprovalRequired);

    plan.record_approval(mino::domain::Approval::plan(
        "user",
        "chat:approval",
        timestamp(6),
        GitFlowConsent::Approved,
    ))
    .expect("approval should be recorded");
    plan.start_task(&first_id, timestamp(7))
        .expect("first task should start");
    assert_eq!(plan.status(), PlanStatus::InProgress);
    assert_eq!(
        plan.task(&first_id).map(mino::domain::Task::status),
        Some(TaskStatus::InProgress)
    );

    satisfy_task(&mut plan, &first_id, 1, 8);
    plan.complete_task(&first_id, timestamp(10))
        .expect("active task should complete");
    satisfy_commit(&mut plan, &first_id, 3, 11);
    satisfy_global(&mut plan, 4, 12);
    set_final_outcome(&mut plan, 13);
    plan.finish_execution(timestamp(13))
        .expect("completed plan should enter review");
    assert_eq!(plan.status(), PlanStatus::Review);
    plan.accept_review(
        "reviewer".to_owned(),
        "chat:acceptance".to_owned(),
        timestamp(14),
    )
    .expect("reviewed plan should complete");
    assert_eq!(plan.status(), PlanStatus::Done);
    plan.validate_invariants()
        .expect("completed plan should satisfy invariants");
}

#[test]
fn only_the_first_dependency_complete_task_can_run() {
    let (mut plan, first_id, second_id) = approved_two_task_plan();

    let order_error = plan
        .start_task(&second_id, timestamp(7))
        .expect_err("second task must not skip first");
    assert_eq!(order_error.kind(), DomainErrorKind::TaskOrderViolation);

    plan.start_task(&first_id, timestamp(8))
        .expect("first task should start");
    let active_error = plan
        .start_task(&second_id, timestamp(9))
        .expect_err("two tasks must not run together");
    assert_eq!(active_error.kind(), DomainErrorKind::ActiveTaskExists);

    satisfy_task(&mut plan, &first_id, 1, 10);
    plan.complete_task(&first_id, timestamp(12))
        .expect("first task should complete");
    satisfy_commit(&mut plan, &first_id, 3, 13);
    plan.start_task(&second_id, timestamp(14))
        .expect("dependency-complete second task should start");
    satisfy_task(&mut plan, &second_id, 4, 15);
    plan.complete_task(&second_id, timestamp(17))
        .expect("second task should complete");
    satisfy_commit(&mut plan, &second_id, 6, 18);
    satisfy_global(&mut plan, 7, 19);
    set_final_outcome(&mut plan, 20);
    plan.finish_execution(timestamp(20))
        .expect("plan should enter review");
    plan.validate_invariants()
        .expect("ordered lifecycle should satisfy invariants");
}

#[test]
fn approved_commit_skips_satisfy_task_order_finish_and_acceptance() {
    let (mut plan, first_id, second_id) = approved_two_task_plan();
    plan.start_task(&first_id, timestamp(8))
        .expect("first task should start");
    satisfy_task(&mut plan, &first_id, 1, 9);
    plan.complete_task(&first_id, timestamp(11))
        .expect("first task should complete");
    satisfy_commit_skip(&mut plan, &first_id, 3, 12);

    plan.start_task(&second_id, timestamp(13))
        .expect("skipped first gate should permit the second task");
    satisfy_task(&mut plan, &second_id, 4, 14);
    plan.complete_task(&second_id, timestamp(16))
        .expect("second task should complete");
    satisfy_commit_skip(&mut plan, &second_id, 6, 17);
    satisfy_global(&mut plan, 7, 18);
    set_final_outcome(&mut plan, 19);

    plan.finish_execution(timestamp(19))
        .expect("skipped required gates should permit review");
    plan.accept_review(
        "reviewer".to_owned(),
        "chat:skips-accepted".to_owned(),
        timestamp(20),
    )
    .expect("skipped required gates should permit acceptance");
    assert_eq!(plan.status(), PlanStatus::Done);
    plan.validate_invariants()
        .expect("skipped commit lifecycle should remain valid");
}

#[test]
fn final_verification_failure_has_an_explicit_rework_exit() {
    let (mut plan, first_id, second_id) = approved_two_task_plan();
    let premature = plan
        .rework_failed_global_verification(&first_id, "No global failure exists", timestamp(8))
        .expect_err("rework must not start before final verification fails");
    assert_eq!(premature.kind(), DomainErrorKind::InvalidTransition);

    plan.start_task(&first_id, timestamp(9))
        .expect("first task should start");
    satisfy_task(&mut plan, &first_id, 1, 10);
    plan.complete_task(&first_id, timestamp(12))
        .expect("first task should complete");
    satisfy_commit(&mut plan, &first_id, 3, 13);
    plan.start_task(&second_id, timestamp(14))
        .expect("second task should start");
    satisfy_task(&mut plan, &second_id, 4, 15);
    plan.complete_task(&second_id, timestamp(17))
        .expect("second task should complete");
    satisfy_commit(&mut plan, &second_id, 6, 18);
    plan.begin_check_run(&check_id("GLOBAL-V1"), timestamp(19))
        .expect("global check should start");
    plan.record_check_run(
        &check_id("GLOBAL-V1"),
        evidence_id("E0007"),
        false,
        timestamp(20),
    )
    .expect("global failure should persist");

    plan.rework_failed_global_verification(
        &first_id,
        "Global verification exposed T1 behavior",
        timestamp(21),
    )
    .expect("failed global verification should reopen T1");
    assert_eq!(
        plan.task(&first_id)
            .expect("first task should exist")
            .status(),
        TaskStatus::Ready
    );
    assert_eq!(
        plan.global_verification()[0].status(),
        mino::domain::CheckStatus::Pending
    );

    plan.start_task(&first_id, timestamp(22))
        .expect("reopened task should start");
    satisfy_task(&mut plan, &first_id, 8, 23);
    plan.complete_task(&first_id, timestamp(25))
        .expect("reopened task should complete with fresh evidence");
    satisfy_commit_skip(&mut plan, &first_id, 10, 26);
    satisfy_global(&mut plan, 11, 27);
    set_final_outcome(&mut plan, 28);
    plan.finish_execution(timestamp(28))
        .expect("fresh global verification should restore Review");
    assert_eq!(plan.status(), PlanStatus::Review);
}

#[test]
fn blocked_execution_resumes_and_review_rework_reopens_a_task() {
    let first_id = task_id("T1");
    let mut plan = Plan::new(plan_id(), "Exercise block and rework.", timestamp(0));
    plan.add_task(
        configured_task(first_id.clone(), "Run once", Vec::new()),
        timestamp(1),
    )
    .expect("task should be added");
    add_global_verification(&mut plan, 2);
    plan.mark_task_ready(&first_id, timestamp(3))
        .expect("task should become ready");
    plan.finalize(timestamp(4)).expect("plan should finalize");
    plan.record_approval(mino::domain::Approval::plan(
        "user",
        "chat:approval",
        timestamp(5),
        GitFlowConsent::Approved,
    ))
    .expect("approval should be recorded");
    plan.start_task(&first_id, timestamp(6))
        .expect("task should start");

    plan.block("dependency unavailable", timestamp(7))
        .expect("active plan should block");
    assert_eq!(plan.status(), PlanStatus::Blocked);
    assert_eq!(
        plan.task(&first_id).map(mino::domain::Task::status),
        Some(TaskStatus::Blocked)
    );

    plan.resume(timestamp(8))
        .expect("blocked plan should resume");
    assert_eq!(plan.status(), PlanStatus::InProgress);
    assert_eq!(
        plan.task(&first_id).map(mino::domain::Task::status),
        Some(TaskStatus::InProgress)
    );

    satisfy_task(&mut plan, &first_id, 1, 9);
    plan.complete_task(&first_id, timestamp(11))
        .expect("resumed task should complete");
    satisfy_commit(&mut plan, &first_id, 3, 12);
    satisfy_global(&mut plan, 4, 13);
    set_final_outcome(&mut plan, 14);
    plan.finish_execution(timestamp(14))
        .expect("plan should enter review");
    let review_id = plan
        .record_review(
            "reviewer".to_owned(),
            "Re-run acceptance evidence".to_owned(),
            ReviewClassification::AcceptanceDefect,
            Some(first_id.clone()),
            timestamp(15),
        )
        .expect("acceptance defect should be recorded");
    plan.begin_review_rework(&review_id, None, timestamp(16))
        .expect("review should reopen the task");
    assert_eq!(plan.status(), PlanStatus::InProgress);
    assert_eq!(
        plan.task(&first_id).map(mino::domain::Task::status),
        Some(TaskStatus::Ready)
    );

    plan.start_task(&first_id, timestamp(17))
        .expect("rework task should start");
    let reused_evidence_error = plan
        .record_task_criterion_pass(
            &first_id,
            &criterion_id("T1-A1"),
            evidence_id("E0001"),
            timestamp(18),
        )
        .expect_err("rework must not reuse prior evidence");
    assert_eq!(
        reused_evidence_error.kind(),
        DomainErrorKind::InvariantViolation
    );
    let stale_evidence_error = plan
        .complete_task(&first_id, timestamp(18))
        .expect_err("rework should require fresh passing state");
    assert_eq!(
        stale_evidence_error.kind(),
        DomainErrorKind::InvariantViolation
    );
    satisfy_task(&mut plan, &first_id, 5, 19);
    plan.complete_task(&first_id, timestamp(21))
        .expect("rework task should complete");
    let global_error = plan
        .finish_execution(timestamp(22))
        .expect_err("rework should require global verification again");
    assert_eq!(global_error.kind(), DomainErrorKind::InvariantViolation);
    satisfy_global(&mut plan, 7, 23);
    set_final_outcome(&mut plan, 24);
    plan.finish_execution(timestamp(24))
        .expect("reworked plan should return to review");
    plan.resolve_review(&review_id, timestamp(25))
        .expect("completed rework should resolve its review item");
    plan.accept_review(
        "reviewer".to_owned(),
        "chat:rework-accepted".to_owned(),
        timestamp(26),
    )
    .expect("reworked plan should be accepted");
    plan.validate_invariants()
        .expect("reworked plan should satisfy invariants");
}

#[test]
fn completion_gates_and_deserialization_reject_forged_state() {
    let first_id = task_id("T1");
    let mut plan = Plan::new(plan_id(), "Reject forged state.", timestamp(0));
    plan.add_task(
        configured_task(first_id.clone(), "Guard completion", Vec::new()),
        timestamp(1),
    )
    .expect("task should be added");
    add_global_verification(&mut plan, 2);
    plan.mark_task_ready(&first_id, timestamp(3))
        .expect("task should become ready");
    plan.finalize(timestamp(4)).expect("plan should finalize");
    plan.record_approval(mino::domain::Approval::plan(
        "user",
        "chat:approval",
        timestamp(5),
        GitFlowConsent::Approved,
    ))
    .expect("approval should be recorded");
    plan.start_task(&first_id, timestamp(6))
        .expect("task should start");

    let completion_error = plan
        .complete_task(&first_id, timestamp(7))
        .expect_err("task evidence is mandatory");
    assert_eq!(completion_error.kind(), DomainErrorKind::InvariantViolation);
    satisfy_task(&mut plan, &first_id, 1, 8);
    plan.complete_task(&first_id, timestamp(10))
        .expect("evidenced task should complete");
    let global_error = plan
        .finish_execution(timestamp(11))
        .expect_err("global evidence is mandatory");
    assert_eq!(global_error.kind(), DomainErrorKind::InvariantViolation);

    let mut forged_plan =
        serde_json::to_value(Plan::new(plan_id(), "Forged completion.", timestamp(0)))
            .expect("draft should serialize");
    forged_plan["status"] = Value::from("Done");
    assert!(serde_json::from_value::<Plan>(forged_plan).is_err());

    let mut forged_commit_gate = serde_json::to_value(
        plan.task(&first_id)
            .expect("completed task should remain available"),
    )
    .expect("completed task should serialize");
    forged_commit_gate["commit_gate"]["status"] = Value::from("Committed");
    assert!(serde_json::from_value::<Task>(forged_commit_gate).is_err());

    let mut forged_task =
        serde_json::to_value(configured_task(first_id, "Forged task", Vec::new()))
            .expect("task should serialize");
    forged_task["status"] = Value::from("Done");
    forged_task["acceptance_criteria"][0]["status"] = Value::from("Passed");
    forged_task["verification_checks"][0]["status"] = Value::from("Passed");
    assert!(serde_json::from_value::<Task>(forged_task).is_err());
}

#[test]
fn legacy_deviation_checkpoint_materializes_and_can_be_rejected() {
    let (mut plan, first_id, _) = approved_two_task_plan();
    plan.start_task(&first_id, timestamp(8))
        .expect("first task should start");
    plan.record_checkpoint(
        &first_id,
        CheckpointKind::Deviation,
        "Legacy execution departed from the plan",
        "codex",
        timestamp(9),
    )
    .expect("legacy-compatible deviation checkpoint should record");
    let mut serialized = serde_json::to_value(plan).expect("plan should serialize");
    serialized["extensions"]["execution"]
        .as_object_mut()
        .expect("execution extension should be an object")
        .remove("deviations");
    let mut legacy: Plan = serde_json::from_value(serialized).expect("legacy plan should load");
    let execution = legacy.execution_state().expect("execution should decode");
    assert_eq!(execution.deviations().len(), 1);
    assert_eq!(execution.deviations()[0].id(), "D1");
    assert_eq!(
        execution.deviations()[0].classification(),
        DeviationClassification::Unclassified
    );
    assert_eq!(
        execution.deviations()[0].legacy_checkpoint_sequence(),
        Some(1)
    );
    assert!(execution.deviations()[0].affected_paths().is_empty());

    legacy
        .reject_deviation(
            "D1",
            "user".to_owned(),
            "chat:legacy-deviation-rejected".to_owned(),
            "The departure is not accepted".to_owned(),
            timestamp(10),
        )
        .expect("legacy deviation should be dispositionable");
    assert_eq!(
        legacy
            .execution_state()
            .expect("execution should decode")
            .deviations()[0]
            .status(),
        DeviationStatus::Rejected
    );
}

#[test]
fn applied_amendment_can_supersede_an_open_deviation() {
    let (mut plan, first_id, _) = approved_two_task_plan();
    plan.start_task(&first_id, timestamp(8))
        .expect("first task should start");
    assert!(
        plan.record_deviation(
            &first_id,
            DeviationClassification::Minor,
            "An unsafe support path was proposed".to_owned(),
            vec!["../support/generated.txt".to_owned()],
            "codex".to_owned(),
            timestamp(9),
        )
        .is_err()
    );
    let deviation_id = plan
        .record_deviation(
            &first_id,
            DeviationClassification::Minor,
            "An implementation note was missing".to_owned(),
            vec!["support/generated.txt".to_owned()],
            "codex".to_owned(),
            timestamp(9),
        )
        .expect("deviation should record");
    assert_eq!(deviation_id, "D1");
    assert!(
        plan.supersede_deviation(
            "D1",
            "codex".to_owned(),
            "C1".to_owned(),
            "No applied amendment exists".to_owned(),
            timestamp(10),
        )
        .is_err()
    );
    let patch: AmendmentPatch = serde_json::from_value(json!({
        "operations": [{
            "operation": "replace-summary",
            "summary": "The approved plan now includes the deviation."
        }]
    }))
    .expect("amendment patch should parse");
    plan.propose_amendment(
        "Include the deviation in the approved plan".to_owned(),
        patch,
        None,
        format!("sha256:{}", "a".repeat(64)),
        "codex".to_owned(),
        timestamp(11),
    )
    .expect("Material amendment should propose");
    assert!(
        plan.supersede_deviation(
            "D1",
            "codex".to_owned(),
            "C1".to_owned(),
            "The amendment is not applied".to_owned(),
            timestamp(12),
        )
        .is_err()
    );
    plan.approve_amendment(
        "C1",
        "user".to_owned(),
        "chat:deviation-amendment-approved".to_owned(),
        timestamp(13),
    )
    .expect("Material amendment should approve");
    plan.apply_amendment("C1", timestamp(14))
        .expect("Material amendment should apply");
    plan.supersede_deviation(
        "D1",
        "codex".to_owned(),
        "C1".to_owned(),
        "The applied amendment updates the approved task record".to_owned(),
        timestamp(15),
    )
    .expect("applied amendment should supersede deviation");
    let execution = plan.execution_state().expect("execution should decode");
    let deviation = execution.deviation("D1").expect("deviation should exist");
    assert_eq!(deviation.status(), DeviationStatus::Superseded);
    assert_eq!(deviation.affected_paths(), ["support/generated.txt"]);
    assert_eq!(deviation.amendment_id(), Some("C1"));
}

#[test]
fn transition_matrix_rejects_commands_outside_their_legal_states() {
    let first_id = task_id("T1");
    let second_id = task_id("T2");
    let mut draft = Plan::new(plan_id(), "Exercise illegal transitions.", timestamp(0));
    assert_invalid_transition(draft.start_task(&first_id, timestamp(1)));
    assert_invalid_transition(draft.complete_task(&first_id, timestamp(1)));
    assert_invalid_transition(draft.finish_execution(timestamp(1)));
    assert_invalid_transition(draft.begin_review_rework("REV-1", None, timestamp(1)));
    assert_invalid_transition(draft.accept_review(
        "reviewer".to_owned(),
        "chat:accept".to_owned(),
        timestamp(1),
    ));
    assert_invalid_transition(draft.block("not executable", timestamp(1)));
    assert_invalid_transition(draft.resume(timestamp(1)));

    let (mut plan, _, _) = approved_two_task_plan();
    assert_invalid_transition(plan.finalize(timestamp(8)));
    assert_invalid_transition(plan.complete_task(&first_id, timestamp(8)));
    assert_invalid_transition(plan.finish_execution(timestamp(8)));
    assert_invalid_transition(plan.begin_review_rework("REV-1", None, timestamp(8)));
    assert_invalid_transition(plan.accept_review(
        "reviewer".to_owned(),
        "chat:accept".to_owned(),
        timestamp(8),
    ));
    assert_invalid_transition(plan.resume(timestamp(8)));

    plan.start_task(&first_id, timestamp(8))
        .expect("first task should start");
    assert_invalid_transition(plan.finalize(timestamp(9)));
    assert_invalid_transition(plan.begin_review_rework("REV-1", None, timestamp(9)));
    assert_invalid_transition(plan.accept_review(
        "reviewer".to_owned(),
        "chat:accept".to_owned(),
        timestamp(9),
    ));
    assert_invalid_transition(plan.resume(timestamp(9)));
    satisfy_task(&mut plan, &first_id, 1, 10);
    plan.complete_task(&first_id, timestamp(12))
        .expect("first task should complete");
    satisfy_commit(&mut plan, &first_id, 3, 13);
    plan.start_task(&second_id, timestamp(14))
        .expect("second task should start");
    satisfy_task(&mut plan, &second_id, 4, 15);
    plan.complete_task(&second_id, timestamp(17))
        .expect("second task should complete");
    satisfy_commit(&mut plan, &second_id, 6, 18);
    satisfy_global(&mut plan, 7, 19);
    set_final_outcome(&mut plan, 20);
    plan.finish_execution(timestamp(20))
        .expect("plan should enter review");

    assert_invalid_transition(plan.start_task(&first_id, timestamp(21)));
    assert_invalid_transition(plan.complete_task(&first_id, timestamp(21)));
    assert_invalid_transition(plan.finish_execution(timestamp(21)));
    assert_invalid_transition(plan.block("too late", timestamp(21)));
    assert_invalid_transition(plan.resume(timestamp(21)));
    plan.accept_review(
        "reviewer".to_owned(),
        "chat:accept".to_owned(),
        timestamp(22),
    )
    .expect("review should be accepted");

    assert_invalid_transition(plan.start_task(&first_id, timestamp(23)));
    assert_invalid_transition(plan.complete_task(&first_id, timestamp(23)));
    assert_invalid_transition(plan.finish_execution(timestamp(23)));
    assert_invalid_transition(plan.begin_review_rework("REV-1", None, timestamp(23)));
    assert_invalid_transition(plan.accept_review(
        "reviewer".to_owned(),
        "chat:accept".to_owned(),
        timestamp(23),
    ));
    assert_invalid_transition(plan.block("complete", timestamp(23)));
    assert_invalid_transition(plan.resume(timestamp(23)));
}

#[test]
fn identifiers_timestamps_evidence_and_events_are_strict() {
    assert!(PlanId::parse("UPPERCASE").is_err());
    assert!(PlanId::parse("2026-13-25-invalid-month").is_err());
    assert!(PlanId::parse("domain-contract").is_err());
    assert!(TaskId::parse("T0").is_err());
    assert!(TaskId::parse("X1").is_err());
    assert!(CheckId::parse("lowercase-check").is_err());
    assert!(CheckId::parse("NOVARIANT").is_err());
    assert!(CheckId::parse("RUST-FMT").is_ok());
    assert!(CriterionId::parse("T1-A0").is_err());
    assert!(EvidenceId::parse("E0000").is_err());
    assert!(EvidenceId::parse("E0001").is_ok());
    assert!(RequestId::parse("123E4567-E89B-12D3-A456-426614174000").is_err());
    assert!(RequestId::parse("123e4567-e89b-12d3-a456-426614174000").is_ok());
    assert!(Timestamp::parse("2026-07-25").is_err());
    assert_eq!(
        Timestamp::parse("2026-07-25T19:00:00+08:00")
            .expect("offset timestamp should normalize")
            .as_str(),
        "2026-07-25T11:00:00Z"
    );

    let evidence_json = json!({
        "id": "E0001",
        "plan_id": "2026-07-25-domain-contract",
        "task_id": "T1",
        "criterion_id": "T1-A1",
        "check_id": "T1-V1",
        "type": "command",
        "command": ["cargo", "test"],
        "cwd": ".",
        "exit_code": 0,
        "duration_milliseconds": 125,
        "output_summary": "tests passed",
        "output_digest": "sha256:output",
        "artifact_path": null,
        "artifact_digest": null,
        "actor": "codex",
        "captured_at": "2026-07-25T11:00:00Z",
        "redactions": [],
        "supersedes": null
    });
    let evidence: Evidence =
        serde_json::from_value(evidence_json.clone()).expect("evidence should deserialize");
    assert_eq!(
        serde_json::to_value(evidence).expect("evidence should serialize"),
        evidence_json
    );

    let event_json = json!({
        "sequence": 1,
        "timestamp": "2026-07-25T11:00:00Z",
        "actor": "codex",
        "command": ["mino", "plan", "finalize"],
        "request_id": "123e4567-e89b-12d3-a456-426614174000",
        "revision_before": 1,
        "revision_after": 2,
        "changed_fields": ["status"],
        "result": "Succeeded",
        "state_hash": "sha256:state",
        "snapshot_digest": "sha256:snapshot"
    });
    let event: Event =
        serde_json::from_value(event_json.clone()).expect("event should deserialize");
    assert_eq!(
        serde_json::to_value(event).expect("event should serialize"),
        event_json
    );

    let mut invalid_evidence = evidence_json;
    invalid_evidence["unexpected"] = Value::Bool(true);
    assert!(serde_json::from_value::<Evidence>(invalid_evidence).is_err());
    assert!(serde_json::to_value(schema_for!(Evidence)).is_ok());
    assert!(serde_json::to_value(schema_for!(Event)).is_ok());
}

#[test]
fn legacy_workspace_fingerprint_loads_without_granting_git_blob_identity() {
    let normalized = WorkspaceGitEntry::new(&"A".repeat(40), "100644")
        .expect("valid Git entry should normalize");
    assert_eq!(normalized.blob_oid(), "a".repeat(40));
    assert!(WorkspaceGitEntry::new(&"d".repeat(64), "100755").is_ok());
    assert!(WorkspaceGitEntry::new(&"g".repeat(40), "100644").is_err());
    assert!(WorkspaceGitEntry::new(&"a".repeat(40), "100664").is_err());

    let mut payload = json!({
        "repository_mode": "git",
        "head": "a".repeat(40),
        "index_tree": format!("sha256:{}", "b".repeat(64)),
        "status_entries": [],
        "scope": {
            "kind": "task",
            "task_id": "T1",
            "patterns": ["src/feature.rs"]
        },
        "file_snapshots": [{
            "path": "src/feature.rs",
            "kind": "regular",
            "length": 1,
            "executable": false,
            "sha256": format!("sha256:{}", "c".repeat(64))
        }]
    });
    let digest = fingerprint_payload_digest(&payload);
    payload["fingerprint_digest"] = Value::String(digest);
    let legacy: WorkspaceFingerprint =
        serde_json::from_value(payload.clone()).expect("legacy fingerprint should remain readable");
    assert!(!legacy.has_complete_git_entries());
    assert!(
        serde_json::to_value(&legacy)
            .expect("legacy fingerprint should serialize")
            ["file_snapshots"][0]
            .get("expected_git_entry")
            .is_none()
    );

    let mut malformed = payload;
    malformed
        .as_object_mut()
        .expect("fingerprint should be an object")
        .remove("fingerprint_digest");
    malformed["file_snapshots"][0]["expected_git_entry"] = json!({
        "blob_oid": "g".repeat(40),
        "mode": "100644"
    });
    let malformed_digest = fingerprint_payload_digest(&malformed);
    malformed["fingerprint_digest"] = Value::String(malformed_digest);
    assert!(serde_json::from_value::<WorkspaceFingerprint>(malformed).is_err());
}

fn fingerprint_payload_digest(payload: &Value) -> String {
    let mut bytes =
        canonical_json_bytes(payload).expect("workspace fingerprint payload should canonicalize");
    assert_eq!(bytes.pop(), Some(b'\n'));
    sha256_digest(&bytes)
}
