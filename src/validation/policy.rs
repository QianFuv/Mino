//! Repository, Git, standards, verification, commit, and approval policy checks.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::domain::{GitRepositoryMode, Plan, PlanStatus, StandardSelection, VerificationCheck};
use crate::standards::{
    EmbeddedCatalog, ResolvedCheck, ResolvedCheckStatus, StandardConflictStatus,
    StandardsRecommendation, SystemToolProbe, apply_recommendation, assess_standard_conflicts,
    detect_standard_conflicts, recommend_for_paths, recommend_initial,
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
    validate_project_scan(plan, findings)?;
    validate_git(plan, findings)?;
    validate_commit_gates(plan, findings);
    validate_approval(plan, findings);
    validate_standards(root, plan, findings)
}

fn validate_project_scan(
    plan: &Plan,
    findings: &mut Vec<ValidationFinding>,
) -> Result<(), MinoError> {
    if plan.scan_is_incomplete().map_err(|error| {
        MinoError::new(
            crate::ErrorCategory::DriftDetected,
            format!("Project scan state is malformed: {error}"),
        )
    })? {
        findings.push(ValidationFinding::error(
            "POLICY-SCAN-INCOMPLETE",
            ValidationLayer::Policy,
            "extensions.project_scan.acceptance",
            "Project discovery was truncated and requires explicit acceptance of the exact scan digest",
        ));
    }
    Ok(())
}

fn validate_git(plan: &Plan, findings: &mut Vec<ValidationFinding>) -> Result<(), MinoError> {
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
    let state = plan.git_readiness_state().map_err(|error| {
        MinoError::new(
            crate::ErrorCategory::DriftDetected,
            format!("Git readiness state is malformed: {error}"),
        )
    })?;
    let Some(state) = state else {
        findings.push(ValidationFinding::error(
            "POLICY-GIT-READINESS-REFRESH-REQUIRED",
            ValidationLayer::Policy,
            "extensions.git_readiness_state",
            "Live Git readiness has not been captured for this plan revision",
        ));
        return Ok(());
    };
    let observation = state.observation();
    let summary_matches = match observation.repository_mode() {
        GitRepositoryMode::NotRepository => {
            git.repository() == "Missing"
                && git.working_tree() == "Not Applicable"
                && git.branch().is_none()
                && git.base_commit().is_none()
                && !git.git_flow_enabled()
        }
        GitRepositoryMode::Worktree => {
            git.repository() == "Present"
                && git.working_tree()
                    == if observation.is_clean() {
                        "Clean"
                    } else {
                        "Dirty"
                    }
                && git.branch() == observation.branch()
                && git.base_commit() == observation.head()
                && (!git.git_flow_enabled()
                    || (observation.is_clean()
                        && observation.branch().is_some()
                        && observation.head().is_some()))
        }
        GitRepositoryMode::Bare => {
            git.repository() == "Present"
                && git.working_tree() == "Not Applicable"
                && git.branch().is_none()
                && git.base_commit().is_none()
                && !git.git_flow_enabled()
        }
    };
    if !summary_matches {
        findings.push(ValidationFinding::error(
            "POLICY-GIT-READINESS-SUMMARY-MISMATCH",
            ValidationLayer::Policy,
            "git_readiness",
            "Git readiness summary does not match its typed live observation",
        ));
    }
    Ok(())
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
    validate_standards_conflicts(root, plan, findings)?;
    Ok(())
}

fn validate_standards_conflicts(
    root: &Path,
    plan: &Plan,
    findings: &mut Vec<ValidationFinding>,
) -> Result<(), MinoError> {
    let detected = detect_standard_conflicts(root, plan)?;
    let state = plan.standards_conflict_state().map_err(|error| {
        crate::MinoError::new(crate::ErrorCategory::PolicyViolation, error.to_string())
    })?;
    let assessment = assess_standard_conflicts(&detected, &state);
    for conflict in assessment.conflicts {
        let (id, action) = match conflict.status {
            StandardConflictStatus::Untracked => ("POLICY-STANDARD-CONFLICT-UNTRACKED", "refresh"),
            StandardConflictStatus::Unresolved => ("POLICY-STANDARD-CONFLICT-UNRESOLVED", "decide"),
            StandardConflictStatus::Stale => ("POLICY-STANDARD-CONFLICT-STALE", "refresh"),
            StandardConflictStatus::Resolved => continue,
        };
        let candidates = conflict
            .conflict
            .candidates()
            .iter()
            .map(|candidate| {
                format!(
                    "{} [precedence {}, {:?}, {}] = {}",
                    candidate.id(),
                    candidate.precedence(),
                    candidate.source_kind(),
                    candidate.source(),
                    bounded_value(candidate.value())
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        findings.push(ValidationFinding::error(
            id,
            ValidationLayer::Policy,
            format!("standards.conflicts.{}", conflict.conflict.id()),
            format!(
                "Rule {} has conflicting candidates: {candidates}. Explicitly {action} this conflict; no candidate is merged automatically",
                conflict.conflict.rule_id()
            ),
        ));
    }
    for conflict_id in assessment.stale_conflict_ids {
        findings.push(ValidationFinding::error(
            "POLICY-STANDARD-CONFLICT-STALE",
            ValidationLayer::Policy,
            format!("standards.conflicts.{conflict_id}"),
            format!(
                "Persisted standards conflict {conflict_id} no longer matches live sources; refresh is required"
            ),
        ));
    }
    Ok(())
}

fn bounded_value(value: &str) -> String {
    const MAX_CHARACTERS: usize = 160;
    let mut characters = value.chars();
    let mut bounded = characters.by_ref().take(MAX_CHARACTERS).collect::<String>();
    if characters.next().is_some() {
        bounded.push('…');
    }
    bounded
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
