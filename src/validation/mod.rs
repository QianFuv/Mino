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
    if findings.iter().all(|finding| !finding.blocking) {
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
                "--format".to_owned(),
                "json".to_owned(),
                "--no-input".to_owned(),
            ],
        }];
    }
    let mut actions = Vec::new();
    if findings.iter().any(|finding| {
        finding.id.starts_with("POLICY-STANDARD") || finding.id == "POLICY-TOOL-UNAVAILABLE"
    }) {
        let mut argv = vec![
            "mino".to_owned(),
            "standards".to_owned(),
            "recommend".to_owned(),
        ];
        for entry in plan.approach().file_map() {
            argv.extend(["--path".to_owned(), entry.path().to_owned()]);
        }
        argv.extend([
            "--format".to_owned(),
            "json".to_owned(),
            "--no-input".to_owned(),
        ]);
        actions.push(NextAction {
            id: "standards.recommend".to_owned(),
            argv,
        });
    }
    actions.push(NextAction {
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
            "--format".to_owned(),
            "json".to_owned(),
            "--no-input".to_owned(),
        ],
    });
    actions
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
