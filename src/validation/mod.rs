//! Fixed-order schema, semantic, graph, and policy validation with canonical remediation.

mod finding;
mod graph;
mod policy;
mod schema;
mod semantic;

use std::path::Path;

use serde_json::Value;

use crate::domain::Plan;
use crate::store::sha256_digest;
use crate::{ErrorCategory, MinoError, NextAction};

use crate::application::AGENT_EXECUTOR_IDENTITY;

pub use finding::{
    VALIDATION_KIND, ValidationFinding, ValidationLayer, ValidationReport, ValidationSeverity,
};

/// Runs all validation layers in fixed order and derives canonical next actions.
///
/// # Errors
///
/// Returns an environment or standards error when repository facts required by
/// policy validation cannot be loaded.
pub fn validate_plan(root: &Path, plan: &Plan) -> Result<ValidationReport, MinoError> {
    let mut findings = Vec::new();
    schema::validate(plan, &mut findings);
    semantic::validate(plan, &mut findings);
    graph::validate(plan, &mut findings);
    policy::validate(root, plan, &mut findings)?;
    let next_actions = derive_next_actions(plan, &findings);
    Ok(ValidationReport::new(plan, findings, next_actions))
}

/// Converts a blocking validation report into the stable exit-2 failure envelope.
#[must_use]
pub fn validation_failure(report: &ValidationReport) -> MinoError {
    let details = serde_json::to_value(report).unwrap_or_else(|error| {
        Value::String(format!("Failed to serialize validation details: {error}"))
    });
    MinoError::new(
        ErrorCategory::IncompleteOrValidation,
        format!(
            "Plan {} revision {} has {} blocking validation finding(s)",
            report.plan_id,
            report.revision,
            report
                .findings
                .iter()
                .filter(|finding| finding.blocking)
                .count()
        ),
    )
    .with_remediation(report.missing_locations(), report.next_actions.clone())
    .with_details(details)
}

pub(crate) fn contains_placeholder(value: &str) -> bool {
    let normalized = value.trim().to_ascii_lowercase();
    matches!(
        normalized.as_str(),
        "n/a" | "na" | "tbd" | "todo" | "???" | "placeholder"
    ) || normalized.contains("{{")
        || normalized.contains("}}")
        || normalized.contains("<todo>")
        || normalized.contains("<tbd>")
}

fn derive_next_actions(plan: &Plan, findings: &[ValidationFinding]) -> Vec<NextAction> {
    if findings.iter().all(|finding| !finding.blocking)
        && plan.status() == crate::domain::PlanStatus::Draft
    {
        return vec![NextAction {
            id: "plan.finalize".to_owned(),
            argv: vec![
                "mino".to_owned(),
                "plan".to_owned(),
                "finalize".to_owned(),
                "--plan".to_owned(),
                plan.id().to_string(),
                "--expect-revision".to_owned(),
                plan.revision().to_string(),
                "--request-id".to_owned(),
                derived_request_id(plan, "plan.finalize"),
                "--actor".to_owned(),
                AGENT_EXECUTOR_IDENTITY.to_owned(),
                "--format".to_owned(),
                "json".to_owned(),
                "--no-input".to_owned(),
            ],
        }];
    }
    if findings.iter().all(|finding| !finding.blocking) {
        return Vec::new();
    }
    if let Some(action) = conflict_next_action(plan, findings) {
        return vec![action];
    }
    let mut actions = Vec::new();
    if findings.iter().any(requires_standards_reconciliation) {
        actions.push(standards_apply_action(plan));
    }
    if plan.status() == crate::domain::PlanStatus::Draft
        && findings.iter().any(requires_authored_remediation)
    {
        actions.push(plan_apply_action(plan));
    }
    actions
}

fn requires_standards_reconciliation(finding: &ValidationFinding) -> bool {
    finding.blocking
        && ((finding.id.starts_with("POLICY-STANDARD-")
            && !finding.id.starts_with("POLICY-STANDARD-CONFLICT-"))
            || finding.id == "POLICY-TOOL-UNAVAILABLE")
}

fn requires_authored_remediation(finding: &ValidationFinding) -> bool {
    finding.blocking
        && !requires_standards_reconciliation(finding)
        && finding.id != "POLICY-SCAN-INCOMPLETE"
}

fn standards_apply_action(plan: &Plan) -> NextAction {
    NextAction {
        id: "standards.apply".to_owned(),
        argv: vec![
            "mino".to_owned(),
            "standards".to_owned(),
            "apply".to_owned(),
            "--recommended".to_owned(),
            "--seed-verification".to_owned(),
            "--plan".to_owned(),
            plan.id().to_string(),
            "--expect-revision".to_owned(),
            plan.revision().to_string(),
            "--request-id".to_owned(),
            derived_request_id(plan, "standards.apply"),
            "--actor".to_owned(),
            AGENT_EXECUTOR_IDENTITY.to_owned(),
            "--format".to_owned(),
            "json".to_owned(),
            "--no-input".to_owned(),
        ],
    }
}

fn plan_apply_action(plan: &Plan) -> NextAction {
    NextAction {
        id: "plan.apply".to_owned(),
        argv: vec![
            "mino".to_owned(),
            "plan".to_owned(),
            "apply".to_owned(),
            "--plan".to_owned(),
            plan.id().to_string(),
            "--file".to_owned(),
            "draft.yaml".to_owned(),
            "--expect-revision".to_owned(),
            plan.revision().to_string(),
            "--request-id".to_owned(),
            derived_request_id(plan, "plan.apply.validation"),
            "--actor".to_owned(),
            AGENT_EXECUTOR_IDENTITY.to_owned(),
            "--format".to_owned(),
            "json".to_owned(),
            "--no-input".to_owned(),
        ],
    }
}

fn conflict_next_action(plan: &Plan, findings: &[ValidationFinding]) -> Option<NextAction> {
    let has_stale_conflict = findings.iter().any(|finding| {
        matches!(
            finding.id.as_str(),
            "POLICY-STANDARD-CONFLICT-UNTRACKED" | "POLICY-STANDARD-CONFLICT-STALE"
        )
    });
    let has_unresolved_conflict = findings
        .iter()
        .any(|finding| finding.id == "POLICY-STANDARD-CONFLICT-UNRESOLVED");
    if has_stale_conflict {
        Some(NextAction {
            id: "standards.conflict.refresh".to_owned(),
            argv: vec![
                "mino".to_owned(),
                "standards".to_owned(),
                "conflict".to_owned(),
                "refresh".to_owned(),
                "--plan".to_owned(),
                plan.id().to_string(),
                "--expect-revision".to_owned(),
                plan.revision().to_string(),
                "--request-id".to_owned(),
                derived_request_id(plan, "standards.conflict.refresh"),
                "--actor".to_owned(),
                AGENT_EXECUTOR_IDENTITY.to_owned(),
                "--format".to_owned(),
                "json".to_owned(),
                "--no-input".to_owned(),
            ],
        })
    } else if has_unresolved_conflict {
        Some(NextAction {
            id: "standards.conflict.list".to_owned(),
            argv: vec![
                "mino".to_owned(),
                "standards".to_owned(),
                "conflict".to_owned(),
                "list".to_owned(),
                "--plan".to_owned(),
                plan.id().to_string(),
                "--format".to_owned(),
                "json".to_owned(),
                "--no-input".to_owned(),
            ],
        })
    } else {
        None
    }
}

fn derived_request_id(plan: &Plan, action: &str) -> String {
    let digest = sha256_digest(format!("{}:{}:{action}", plan.id(), plan.revision()).as_bytes());
    let value = &digest["sha256:".len().."sha256:".len() + 32];
    format!(
        "{}-{}-{}-{}-{}",
        &value[0..8],
        &value[8..12],
        &value[12..16],
        &value[16..20],
        &value[20..32]
    )
}
