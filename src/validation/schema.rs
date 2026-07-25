//! Schema-layer checks for authored strings, identifiers, uniqueness, and paths.

use std::collections::BTreeSet;
use std::path::{Component, Path};

use crate::domain::{Plan, Task};

use super::{ValidationFinding, ValidationLayer, contains_placeholder};

pub(crate) fn validate(plan: &Plan, findings: &mut Vec<ValidationFinding>) {
    let known_references = known_references(plan);
    check_text(findings, "metadata.name", plan.metadata().name());
    check_text(findings, "metadata.priority", plan.metadata().priority());
    check_text(findings, "metadata.plan_type", plan.metadata().plan_type());
    check_text(findings, "metadata.area", plan.metadata().area());
    check_text(findings, "metadata.owner", plan.metadata().owner());
    check_text(findings, "summary", plan.summary());
    check_text(findings, "scope.goal", plan.scope().goal());
    check_values(findings, "scope.deliverables", plan.scope().deliverables());
    check_values(findings, "scope.in_scope", plan.scope().in_scope());
    check_values(findings, "scope.out_of_scope", plan.scope().out_of_scope());
    check_text(findings, "approach", plan.approach().summary());
    check_text(findings, "interfaces", plan.interfaces());
    for (index, context) in plan.context().iter().enumerate() {
        check_text(
            findings,
            &format!("context.{index}.reference"),
            context.reference(),
        );
        check_text(findings, &format!("context.{index}.fact"), context.fact());
        check_text(
            findings,
            &format!("context.{index}.implication"),
            context.implication(),
        );
    }
    for (index, decision) in plan.decisions().iter().enumerate() {
        for (field, value) in [
            ("item", decision.item()),
            ("type", decision.kind()),
            ("decision", decision.value()),
            ("reason", decision.reason()),
            ("status", decision.status()),
        ] {
            check_text(findings, &format!("decisions.{index}.{field}"), value);
        }
    }
    for (index, edge_case) in plan.edge_cases().iter().enumerate() {
        check_text(
            findings,
            &format!("edge_cases.{index}.case"),
            edge_case.case(),
        );
        check_text(
            findings,
            &format!("edge_cases.{index}.expected_behavior"),
            edge_case.expected_behavior(),
        );
        check_values(
            findings,
            &format!("edge_cases.{index}.covered_by"),
            edge_case.covered_by(),
        );
        for (reference_index, reference) in edge_case.covered_by().iter().enumerate() {
            if !known_references.contains(reference.as_str()) {
                findings.push(ValidationFinding::error(
                    "SCHEMA-REFERENCE-UNKNOWN",
                    ValidationLayer::Schema,
                    format!("edge_cases.{index}.covered_by.{reference_index}"),
                    format!("Coverage reference {reference} does not name a criterion or check"),
                ));
            }
        }
    }
    let mut check_ids = BTreeSet::new();
    for check in plan.global_verification() {
        let location = format!("verification_plan.{}", check.id());
        check_check_id(findings, &mut check_ids, &location, check.id().as_str());
        check_path(findings, &format!("{location}.cwd"), check.cwd(), true);
    }
    for task in plan.tasks() {
        validate_task(task, findings, &mut check_ids);
    }
    for (index, entry) in plan.approach().file_map().iter().enumerate() {
        check_path(
            findings,
            &format!("approach.file_map.{index}.path"),
            entry.path(),
            false,
        );
        check_text(
            findings,
            &format!("approach.file_map.{}.reason", entry.task_id()),
            entry.reason(),
        );
    }
    for (index, standard) in plan.standards().iter().enumerate() {
        for (field, value) in [
            ("package_id", standard.package_id()),
            ("version", standard.version()),
            ("digest", standard.digest()),
            ("source", standard.source()),
        ] {
            check_text(findings, &format!("standards.{index}.{field}"), value);
        }
    }
}

fn validate_task(
    task: &Task,
    findings: &mut Vec<ValidationFinding>,
    check_ids: &mut BTreeSet<String>,
) {
    let task_location = format!("tasks.{}", task.id());
    check_text(findings, &format!("{task_location}.title"), task.title());
    check_values(findings, &format!("{task_location}.steps"), task.steps());
    for (index, entry) in task.file_map().iter().enumerate() {
        check_path(
            findings,
            &format!("{task_location}.file_map.{index}.path"),
            entry.path(),
            false,
        );
        check_text(
            findings,
            &format!("{task_location}.file_map.reason"),
            entry.reason(),
        );
    }
    let criterion_prefix = format!("{}-A", task.id());
    for criterion in task.acceptance_criteria() {
        if !criterion.id().as_str().starts_with(&criterion_prefix) {
            findings.push(ValidationFinding::error(
                "SCHEMA-CRITERION-ID-OWNER",
                ValidationLayer::Schema,
                format!("{task_location}.acceptance_criteria.{}", criterion.id()),
                format!(
                    "Criterion {} does not belong to task {}",
                    criterion.id(),
                    task.id()
                ),
            ));
        }
        check_text(
            findings,
            &format!("{task_location}.acceptance_criteria.{}", criterion.id()),
            criterion.description(),
        );
    }
    for check in task.verification_checks() {
        check_check_id(
            findings,
            check_ids,
            &format!("{task_location}.verification"),
            check.id().as_str(),
        );
        check_path(
            findings,
            &format!("{task_location}.verification.{}.cwd", check.id()),
            check.cwd(),
            true,
        );
    }
    if let Some(commit_gate) = task.commit_gate() {
        check_text(
            findings,
            &format!("{task_location}.commit_gate.planned_message"),
            commit_gate.planned_message(),
        );
        for (index, scope) in commit_gate.scope().iter().enumerate() {
            check_path(
                findings,
                &format!("{task_location}.commit_gate.scope.{index}"),
                scope,
                false,
            );
        }
    }
}

fn check_check_id(
    findings: &mut Vec<ValidationFinding>,
    check_ids: &mut BTreeSet<String>,
    location: &str,
    check_id: &str,
) {
    if !check_ids.insert(check_id.to_owned()) {
        findings.push(ValidationFinding::error(
            "SCHEMA-CHECK-ID-DUPLICATE",
            ValidationLayer::Schema,
            format!("{location}.{check_id}"),
            format!("Verification check ID {check_id} appears more than once"),
        ));
    }
}

fn check_values(findings: &mut Vec<ValidationFinding>, location: &str, values: &[String]) {
    for (index, value) in values.iter().enumerate() {
        check_text(findings, &format!("{location}.{index}"), value);
    }
}

fn check_text(findings: &mut Vec<ValidationFinding>, location: &str, value: &str) {
    if contains_placeholder(value) {
        findings.push(ValidationFinding::error(
            "SCHEMA-PLACEHOLDER-UNRESOLVED",
            ValidationLayer::Schema,
            location,
            "Authored text contains an unresolved placeholder",
        ));
    }
}

fn check_path(
    findings: &mut Vec<ValidationFinding>,
    location: &str,
    value: &str,
    allows_current_directory: bool,
) {
    let path = Path::new(value);
    let is_current_directory = value == ".";
    if value.trim().is_empty()
        || value.contains('\\')
        || path.is_absolute()
        || (is_current_directory && !allows_current_directory)
        || (!is_current_directory
            && path
                .components()
                .any(|component| !matches!(component, Component::Normal(_))))
    {
        findings.push(ValidationFinding::error(
            "SCHEMA-PATH-INVALID",
            ValidationLayer::Schema,
            location,
            format!("Path {value:?} must be a safe project-relative protocol path"),
        ));
    }
}

fn known_references(plan: &Plan) -> BTreeSet<&str> {
    let mut references = plan
        .global_verification()
        .iter()
        .map(|check| check.id().as_str())
        .collect::<BTreeSet<_>>();
    for task in plan.tasks() {
        references.extend(
            task.acceptance_criteria()
                .iter()
                .map(|criterion| criterion.id().as_str()),
        );
        references.extend(
            task.verification_checks()
                .iter()
                .map(|check| check.id().as_str()),
        );
    }
    references
}
