//! Repository, Git, standards, verification, commit, and approval policy checks.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::domain::{Plan, PlanStatus, StandardSelection, VerificationCheck};
use crate::standards::{
    EmbeddedCatalog, ResolvedCheck, ResolvedCheckStatus, StandardsRecommendation, SystemToolProbe,
    apply_recommendation, recommend_for_paths, recommend_initial,
};
use crate::{MinoError, project};

use super::{ValidationFinding, ValidationLayer};

const CONVENTIONAL_COMMIT_TYPES: &[&str] = &[
    "build", "chore", "ci", "docs", "feat", "fix", "perf", "refactor", "revert", "style", "test",
];

pub(crate) fn validate(
    root: &Path,
    plan: &Plan,
    findings: &mut Vec<ValidationFinding>,
) -> Result<(), MinoError> {
    validate_git(plan, findings);
    validate_commit_gates(plan, findings);
    validate_approval(plan, findings);
    validate_standards(root, plan, findings)
}

fn validate_git(plan: &Plan, findings: &mut Vec<ValidationFinding>) {
    let git = plan.git_readiness();
    let repository_decided = matches!(git.repository(), "Present" | "Missing");
    let tree_decided = matches!(git.working_tree(), "Clean" | "Dirty" | "Not Applicable");
    if !repository_decided || !tree_decided || git.base_status().eq_ignore_ascii_case("unknown") {
        findings.push(ValidationFinding::error(
            "POLICY-GIT-READINESS-UNDECIDED",
            ValidationLayer::Policy,
            "git_readiness",
            "Git repository and working-tree readiness must be explicitly decided",
        ));
    }
    if git.git_flow_enabled() && (git.repository() != "Present" || git.working_tree() != "Clean") {
        findings.push(ValidationFinding::error(
            "POLICY-GIT-FLOW-BASELINE-INVALID",
            ValidationLayer::Policy,
            "git_readiness.git_flow_enabled",
            "Git Flow requires a present repository and clean working tree",
        ));
    }
}

fn validate_commit_gates(plan: &Plan, findings: &mut Vec<ValidationFinding>) {
    for task in plan.tasks() {
        let location = format!("tasks.{}.commit_gate", task.id());
        let Some(gate) = task.commit_gate() else {
            if plan.git_readiness().git_flow_enabled() {
                findings.push(ValidationFinding::error(
                    "POLICY-COMMIT-GATE-MISSING",
                    ValidationLayer::Policy,
                    location,
                    format!("Git Flow task {} requires a commit gate", task.id()),
                ));
            }
            continue;
        };
        if gate.is_required() && !is_conventional_commit(gate.planned_message()) {
            findings.push(ValidationFinding::error(
                "POLICY-COMMIT-MESSAGE-INVALID",
                ValidationLayer::Policy,
                format!("{location}.planned_message"),
                format!(
                    "Task {} planned commit message is not valid Conventional Commit syntax",
                    task.id()
                ),
            ));
        }
        if gate.is_required() && gate.scope().is_empty() {
            findings.push(ValidationFinding::error(
                "POLICY-COMMIT-SCOPE-MISSING",
                ValidationLayer::Policy,
                format!("{location}.scope"),
                format!("Task {} requires a non-empty commit scope", task.id()),
            ));
            continue;
        }
        for scope in gate.scope() {
            if !task
                .file_map()
                .iter()
                .any(|entry| scope_covers(scope, entry.path()))
            {
                findings.push(ValidationFinding::error(
                    "POLICY-COMMIT-SCOPE-OUTSIDE-FILE-MAP",
                    ValidationLayer::Policy,
                    format!("{location}.scope"),
                    format!(
                        "Commit scope {scope} does not cover a file owned by task {}",
                        task.id()
                    ),
                ));
            }
        }
        if gate.is_required() {
            for entry in task.file_map() {
                if !gate
                    .scope()
                    .iter()
                    .any(|scope| scope_covers(scope, entry.path()))
                {
                    findings.push(ValidationFinding::error(
                        "POLICY-FILE-MAP-OUTSIDE-COMMIT-SCOPE",
                        ValidationLayer::Policy,
                        format!("tasks.{}.file_map.{}", task.id(), entry.path()),
                        format!("Task file {} is outside its commit scope", entry.path()),
                    ));
                }
            }
        }
    }
}

fn validate_approval(plan: &Plan, findings: &mut Vec<ValidationFinding>) {
    if matches!(
        plan.status(),
        PlanStatus::InProgress | PlanStatus::Review | PlanStatus::Done
    ) && plan.approvals().is_empty()
    {
        findings.push(ValidationFinding::error(
            "POLICY-EXECUTION-APPROVAL-MISSING",
            ValidationLayer::Policy,
            "approvals",
            "A plan must record explicit approval before execution",
        ));
    }
}

fn validate_standards(
    root: &Path,
    plan: &Plan,
    findings: &mut Vec<ValidationFinding>,
) -> Result<(), MinoError> {
    let scan = project::scan(root)?;
    let catalog = EmbeddedCatalog::load()?;
    let paths = plan
        .approach()
        .file_map()
        .iter()
        .map(|entry| PathBuf::from(entry.path()))
        .collect::<Vec<_>>();
    let recommendation = if paths.is_empty() {
        recommend_initial(&catalog, &scan)?
    } else {
        recommend_for_paths(&catalog, &scan, &paths)?
    };
    let expected = apply_recommendation(root, &catalog, &recommendation, &SystemToolProbe)?;
    let selected = validate_selected_standards(&catalog, plan, findings);
    validate_required_standards(&recommendation, &selected, findings);
    validate_required_checks(plan, expected.checks, findings);
    Ok(())
}

fn validate_selected_standards<'a>(
    catalog: &EmbeddedCatalog,
    plan: &'a Plan,
    findings: &mut Vec<ValidationFinding>,
) -> BTreeMap<&'a str, &'a StandardSelection> {
    let mut selected = BTreeMap::new();
    for standard in plan.standards() {
        if selected.insert(standard.package_id(), standard).is_some() {
            findings.push(ValidationFinding::error(
                "POLICY-STANDARD-DUPLICATE",
                ValidationLayer::Policy,
                "standards",
                format!(
                    "Standards package {} is selected more than once",
                    standard.package_id()
                ),
            ));
        }
        match catalog.package(standard.package_id()) {
            Some(package)
                if package.version() == standard.version()
                    && package.digest() == standard.digest() => {}
            Some(_) => findings.push(ValidationFinding::error(
                "POLICY-STANDARD-PIN-MISMATCH",
                ValidationLayer::Policy,
                format!("standards.{}", standard.package_id()),
                format!(
                    "Standards package {} version or digest is not exact",
                    standard.package_id()
                ),
            )),
            None => findings.push(ValidationFinding::error(
                "POLICY-STANDARD-UNKNOWN",
                ValidationLayer::Policy,
                format!("standards.{}", standard.package_id()),
                format!(
                    "Standards package {} is not in the embedded catalog",
                    standard.package_id()
                ),
            )),
        }
    }
    selected
}

fn validate_required_standards(
    recommendation: &StandardsRecommendation,
    selected: &BTreeMap<&str, &StandardSelection>,
    findings: &mut Vec<ValidationFinding>,
) {
    for recommended in &recommendation.packages {
        match selected.get(recommended.package_id.as_str()) {
            Some(standard)
                if standard.version() == recommended.version
                    && standard.digest() == recommended.digest => {}
            _ => findings.push(ValidationFinding::error(
                "POLICY-STANDARD-REQUIRED",
                ValidationLayer::Policy,
                format!("standards.{}", recommended.package_id),
                format!(
                    "File Map requires exact standards package {}",
                    recommended.package_id
                ),
            )),
        }
    }
}

fn validate_required_checks(
    plan: &Plan,
    expected_checks: Vec<ResolvedCheck>,
    findings: &mut Vec<ValidationFinding>,
) {
    let checks = all_checks(plan);
    for expected_check in expected_checks {
        let location = format!("verification.{}", expected_check.id);
        let Some(actual) = checks.get(expected_check.id.as_str()) else {
            findings.push(ValidationFinding::error(
                "POLICY-STANDARD-CHECK-MISSING",
                ValidationLayer::Policy,
                location,
                format!("Selected standards require check {}", expected_check.id),
            ));
            continue;
        };
        let expected_cwd = expected_check.cwd.to_string_lossy().replace('\\', "/");
        if actual.command() != expected_check.argv.as_slice()
            || actual.cwd() != expected_cwd.as_str()
            || actual.is_required() != expected_check.required
        {
            findings.push(ValidationFinding::error(
                "POLICY-STANDARD-CHECK-MISMATCH",
                ValidationLayer::Policy,
                location.clone(),
                format!(
                    "Check {} differs from the project-resolved standards command",
                    expected_check.id
                ),
            ));
        }
        if expected_check.status == ResolvedCheckStatus::Unresolved {
            findings.push(ValidationFinding::error(
                "POLICY-TOOL-UNAVAILABLE",
                ValidationLayer::Policy,
                location,
                expected_check
                    .unresolved_reason
                    .unwrap_or_else(|| format!("Check {} cannot run", expected_check.id)),
            ));
        }
    }
}

fn all_checks(plan: &Plan) -> BTreeMap<&str, &VerificationCheck> {
    let mut checks = BTreeMap::new();
    for check in plan.global_verification() {
        checks.entry(check.id().as_str()).or_insert(check);
    }
    for task in plan.tasks() {
        for check in task.verification_checks() {
            checks.entry(check.id().as_str()).or_insert(check);
        }
    }
    checks
}

fn is_conventional_commit(message: &str) -> bool {
    if message.contains(['\r', '\n']) || message.ends_with('.') {
        return false;
    }
    let Some((prefix, description)) = message.split_once(": ") else {
        return false;
    };
    if description.trim().is_empty() {
        return false;
    }
    let (type_, scope) = if let Some((type_, scope)) = prefix.split_once('(') {
        let Some(scope) = scope.strip_suffix(')') else {
            return false;
        };
        (type_, Some(scope))
    } else {
        (prefix, None)
    };
    CONVENTIONAL_COMMIT_TYPES.contains(&type_)
        && scope.is_none_or(|scope| {
            !scope.is_empty()
                && scope.bytes().all(|byte| {
                    byte.is_ascii_lowercase()
                        || byte.is_ascii_digit()
                        || matches!(byte, b'-' | b'_')
                })
        })
}

fn scope_covers(scope: &str, path: &str) -> bool {
    if scope == path {
        return true;
    }
    scope.strip_suffix("/**").is_some_and(|prefix| {
        path == prefix
            || path
                .strip_prefix(prefix)
                .is_some_and(|remaining| remaining.starts_with('/'))
    })
}
