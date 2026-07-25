//! Semantic completeness checks for authored plan meaning.

use crate::domain::Plan;

use super::{ValidationFinding, ValidationLayer};

pub(crate) fn validate(plan: &Plan, findings: &mut Vec<ValidationFinding>) {
    require_text(
        findings,
        "SEMANTIC-SUMMARY-MISSING",
        "summary",
        plan.summary(),
    );
    require_text(
        findings,
        "SEMANTIC-GOAL-MISSING",
        "scope.goal",
        plan.scope().goal(),
    );
    require_list(
        findings,
        "SEMANTIC-DELIVERABLES-MISSING",
        "scope.deliverables",
        plan.scope().deliverables(),
    );
    require_list(
        findings,
        "SEMANTIC-IN-SCOPE-MISSING",
        "scope.in_scope",
        plan.scope().in_scope(),
    );
    require_list(
        findings,
        "SEMANTIC-OUT-OF-SCOPE-MISSING",
        "scope.out_of_scope",
        plan.scope().out_of_scope(),
    );
    require_text(
        findings,
        "SEMANTIC-APPROACH-MISSING",
        "approach",
        plan.approach().summary(),
    );
    require_text(
        findings,
        "SEMANTIC-INTERFACES-MISSING",
        "interfaces",
        plan.interfaces(),
    );
    validate_decisions(plan, findings);
    if plan.tasks().is_empty() {
        findings.push(ValidationFinding::error(
            "SEMANTIC-TASKS-MISSING",
            ValidationLayer::Semantic,
            "tasks",
            "A plan requires at least one implementation task",
        ));
    }
    for task in plan.tasks() {
        if task.acceptance_criteria().is_empty() {
            findings.push(ValidationFinding::error(
                "SEMANTIC-TASK-CRITERIA-MISSING",
                ValidationLayer::Semantic,
                format!("tasks.{}.acceptance_criteria", task.id()),
                format!(
                    "Task {} requires at least one acceptance criterion",
                    task.id()
                ),
            ));
        }
        if task.verification_checks().is_empty() {
            findings.push(ValidationFinding::error(
                "SEMANTIC-TASK-VERIFICATION-MISSING",
                ValidationLayer::Semantic,
                format!("tasks.{}.verification", task.id()),
                format!(
                    "Task {} requires at least one verification command",
                    task.id()
                ),
            ));
        }
    }
    if plan.global_verification().is_empty() {
        findings.push(ValidationFinding::error(
            "SEMANTIC-GLOBAL-VERIFICATION-MISSING",
            ValidationLayer::Semantic,
            "verification_plan",
            "A plan requires at least one global verification command",
        ));
    }
}

fn validate_decisions(plan: &Plan, findings: &mut Vec<ValidationFinding>) {
    for (index, decision) in plan.decisions().iter().enumerate() {
        let normalized_kind = decision
            .kind()
            .split(|character: char| character.is_whitespace() || matches!(character, '-' | '_'))
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
            .to_ascii_lowercase();
        if matches!(normalized_kind.as_str(), "question" | "open question")
            && !matches!(
                decision.status().trim().to_ascii_lowercase().as_str(),
                "answered" | "accepted" | "closed" | "resolved"
            )
        {
            findings.push(ValidationFinding::error(
                "SEMANTIC-BLOCKING-QUESTION-OPEN",
                ValidationLayer::Semantic,
                format!("decisions.{index}.status"),
                format!("Question {} remains unresolved", decision.item()),
            ));
        }
    }
}

fn require_text(findings: &mut Vec<ValidationFinding>, id: &str, location: &str, value: &str) {
    if value.trim().is_empty() {
        findings.push(ValidationFinding::error(
            id,
            ValidationLayer::Semantic,
            location,
            format!("Required authored field {location} is empty"),
        ));
    }
}

fn require_list(
    findings: &mut Vec<ValidationFinding>,
    id: &str,
    location: &str,
    values: &[String],
) {
    if values.is_empty() || values.iter().any(|value| value.trim().is_empty()) {
        findings.push(ValidationFinding::error(
            id,
            ValidationLayer::Semantic,
            location,
            format!("Required authored list {location} is empty or contains an empty item"),
        ));
    }
}
