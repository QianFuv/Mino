//! Live Git readiness capture, comparison, refresh, and protected-transition gates.

use std::path::Path;

use serde_json::json;

use crate::application::git_binding::map_git_error;
use crate::application::plan::{
    PlanMutationRequest, PlanOperationReport, PlanService, derived_request_id,
};
use crate::domain::{
    CommitStatus, GitReadiness, GitReadinessObservation, GitReadinessState, GitRepositoryMode,
    Plan, Timestamp,
};
use crate::git::{GitAdapter, GitAvailability, GitBranchJournalStore, GitFacts};
use crate::store::{canonical_json_bytes, sha256_digest};
use crate::{ErrorCategory, MinoError, NextAction};

/// Git facts that must remain stable for one protected transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GitReadinessRequirement {
    /// Identity, HEAD, status, and a Git Flow clean baseline must remain current.
    CleanBaseline,
    /// Repository and branch identity must remain current; task dirt and HEAD are checked elsewhere.
    IdentityOnly,
}

#[derive(Clone, Debug)]
pub(crate) struct CapturedGitReadiness {
    pub(crate) state: GitReadinessState,
    pub(crate) summary: GitReadiness,
    pub(crate) branch: Option<String>,
}

/// Application boundary for explicit revisioned Git readiness refresh.
#[derive(Clone, Debug)]
pub struct GitReadinessService {
    plans: PlanService,
}

impl GitReadinessService {
    /// Discovers an initialized project and creates its readiness service.
    ///
    /// # Errors
    ///
    /// Returns an environment error when the project cannot be discovered.
    pub fn discover(start: &Path) -> Result<Self, MinoError> {
        Ok(Self {
            plans: PlanService::discover(start)?,
        })
    }

    /// Captures and commits current Git readiness as one plan revision.
    ///
    /// Ready-plan refresh invalidates the prior plan approval and workspace
    /// baseline. Exact mutation retries replay without re-observing Git.
    ///
    /// # Errors
    ///
    /// Returns a typed error for stale revisions, unsupported lifecycle state,
    /// unavailable or invalid Git facts, projection drift, or storage failure.
    pub fn refresh(&self, request: PlanMutationRequest) -> Result<PlanOperationReport, MinoError> {
        let current = self.plans.load_verified(&request.plan_id)?;
        let target_revision = request.expected_revision.checked_add(1).ok_or_else(|| {
            MinoError::new(
                ErrorCategory::RevisionConflict,
                "Expected revision overflowed",
            )
        })?;
        let captured = if current.revision() == request.expected_revision {
            capture_git_readiness(self.plans.root(), request.updated_at.clone())?
        } else if current.revision() == target_revision {
            CapturedGitReadiness {
                state: required_state(&current)?,
                summary: current.git_readiness().clone(),
                branch: current.metadata().branch().map(str::to_owned),
            }
        } else {
            return Err(MinoError::new(
                ErrorCategory::RevisionConflict,
                format!(
                    "Plan {} is revision {}, not expected revision {}",
                    current.id(),
                    current.revision(),
                    request.expected_revision
                ),
            ));
        };
        let state = captured.state;
        let summary = captured.summary;
        let branch = captured.branch;
        self.plans.commit_semantic(
            request,
            vec![
                "extensions.git_readiness_state".to_owned(),
                "git_readiness".to_owned(),
                "metadata.branch".to_owned(),
                "approvals".to_owned(),
                "extensions.workspace.plan_baseline".to_owned(),
            ],
            |_| Ok(None),
            move |plan, at| plan.refresh_git_readiness(&state, summary.clone(), branch.clone(), at),
        )
    }
}

pub(crate) fn detect_initial_git_readiness(
    root: &Path,
    observed_at: Timestamp,
) -> Result<(GitReadiness, Option<String>, GitReadinessState), MinoError> {
    let captured = capture_git_readiness(root, observed_at)?;
    Ok((captured.summary, captured.branch, captured.state))
}

pub(crate) fn capture_git_readiness(
    root: &Path,
    observed_at: Timestamp,
) -> Result<CapturedGitReadiness, MinoError> {
    match GitAdapter::new(root)
        .inspect_availability()
        .map_err(|error| map_git_error(&error))?
    {
        GitAvailability::NotRepository => capture_non_repository(observed_at),
        GitAvailability::Available(facts) => capture_available(&facts, observed_at),
    }
}

pub(crate) fn readiness_mismatches(
    root: &Path,
    plan: &Plan,
    requirement: GitReadinessRequirement,
) -> Result<Vec<String>, MinoError> {
    let Some(expected_state) = plan
        .git_readiness_state()
        .map_err(|error| domain_state_error(&error))?
    else {
        return Ok(vec!["extensions.git_readiness_state".to_owned()]);
    };
    let live = capture_git_readiness(root, Timestamp::now_utc())?;
    compare_readiness(root, plan, &expected_state, &live.state, requirement)
}

pub(crate) fn require_current_git_readiness(
    root: &Path,
    plan: &Plan,
    requirement: GitReadinessRequirement,
) -> Result<(), MinoError> {
    let mismatches = readiness_mismatches(root, plan, requirement)?;
    if mismatches.is_empty() {
        Ok(())
    } else {
        Err(readiness_refresh_error(plan, &mismatches))
    }
}

pub(crate) fn refresh_action(plan: &Plan) -> NextAction {
    NextAction {
        id: "git.readiness.refresh".to_owned(),
        argv: vec![
            "mino".to_owned(),
            "git".to_owned(),
            "readiness".to_owned(),
            "refresh".to_owned(),
            "--plan".to_owned(),
            plan.id().to_string(),
            "--expect-revision".to_owned(),
            plan.revision().to_string(),
            "--request-id".to_owned(),
            derived_request_id(plan, "git.readiness.refresh"),
            "--actor".to_owned(),
            super::AGENT_EXECUTOR_IDENTITY.to_owned(),
            "--format".to_owned(),
            "json".to_owned(),
            "--no-input".to_owned(),
        ],
    }
}

fn capture_non_repository(observed_at: Timestamp) -> Result<CapturedGitReadiness, MinoError> {
    let observation = GitReadinessObservation::new(
        GitRepositoryMode::NotRepository,
        None,
        None,
        None,
        None,
        empty_status_digest(),
        false,
        observed_at,
    )
    .map_err(|error| domain_state_error(&error))?;
    Ok(CapturedGitReadiness {
        state: GitReadinessState::new(observation).map_err(|error| domain_state_error(&error))?,
        summary: GitReadiness::detected(
            "Missing",
            "Not Applicable",
            None,
            None,
            "No Git repository",
            false,
        ),
        branch: None,
    })
}

fn capture_available(
    facts: &GitFacts,
    observed_at: Timestamp,
) -> Result<CapturedGitReadiness, MinoError> {
    let repository_mode = if facts.is_worktree {
        GitRepositoryMode::Worktree
    } else {
        GitRepositoryMode::Bare
    };
    let worktree = normalized_optional_path(facts.worktree.as_deref())?;
    let common_dir = normalized_optional_path(facts.common_dir.as_deref())?;
    let status_digest = sha256_digest(&canonical_json_bytes(&facts.status).map_err(|error| {
        MinoError::new(
            ErrorCategory::DriftDetected,
            format!("Failed to encode Git status identity: {error}"),
        )
    })?);
    let observation = GitReadinessObservation::new(
        repository_mode,
        worktree,
        common_dir,
        facts.branch.clone(),
        facts.head.clone(),
        status_digest,
        facts.is_clean,
        observed_at,
    )
    .map_err(|error| domain_state_error(&error))?;
    let working_tree = if facts.is_worktree {
        if facts.is_clean { "Clean" } else { "Dirty" }
    } else {
        "Not Applicable"
    };
    let base_status = if facts.is_worktree {
        if facts.is_clean {
            "Clean: Git status contains no entries"
        } else {
            "Dirty: Git status contains changes"
        }
    } else {
        "Not Applicable: bare Git repository"
    };
    let git_flow_enabled =
        facts.is_worktree && facts.is_clean && facts.branch.is_some() && facts.head.is_some();
    Ok(CapturedGitReadiness {
        state: GitReadinessState::new(observation).map_err(|error| domain_state_error(&error))?,
        summary: GitReadiness::detected(
            "Present",
            working_tree,
            facts.branch.clone(),
            facts.head.clone(),
            base_status,
            git_flow_enabled,
        ),
        branch: facts.branch.clone(),
    })
}

fn compare_readiness(
    root: &Path,
    plan: &Plan,
    expected_state: &GitReadinessState,
    live_state: &GitReadinessState,
    requirement: GitReadinessRequirement,
) -> Result<Vec<String>, MinoError> {
    let expected = expected_state.observation();
    let live = live_state.observation();
    let mut mismatches = Vec::new();
    compare_field(
        &mut mismatches,
        "repository_mode",
        expected.repository_mode() == live.repository_mode(),
    );
    compare_field(
        &mut mismatches,
        "worktree",
        expected.worktree() == live.worktree(),
    );
    compare_field(
        &mut mismatches,
        "common_dir",
        expected.common_dir() == live.common_dir(),
    );
    let branch_matches =
        expected.branch() == live.branch() || authorized_branch(root, plan, live.branch())?;
    compare_field(&mut mismatches, "branch", branch_matches);
    if requirement == GitReadinessRequirement::CleanBaseline {
        compare_field(
            &mut mismatches,
            "head",
            expected_head(plan, expected) == live.head(),
        );
        compare_field(
            &mut mismatches,
            "status_digest",
            expected.status_digest() == live.status_digest(),
        );
        compare_field(
            &mut mismatches,
            "is_clean",
            expected.is_clean() == live.is_clean(),
        );
        if plan.git_readiness().git_flow_enabled() {
            compare_field(
                &mut mismatches,
                "git_flow_worktree",
                live.repository_mode() == GitRepositoryMode::Worktree,
            );
            compare_field(&mut mismatches, "git_flow_clean", live.is_clean());
            compare_field(&mut mismatches, "git_flow_head", live.head().is_some());
        }
    }
    mismatches.sort();
    mismatches.dedup();
    Ok(mismatches)
}

fn authorized_branch(
    root: &Path,
    plan: &Plan,
    live_branch: Option<&str>,
) -> Result<bool, MinoError> {
    let Some(live_branch) = live_branch else {
        return Ok(false);
    };
    let journal = GitBranchJournalStore::new(root)
        .load(plan.id())
        .map_err(|error| map_git_error(&error))?;
    Ok(journal.is_some_and(|journal| {
        journal.completion.is_some() && journal.intent.branch_name == live_branch
    }))
}

fn expected_head<'a>(plan: &'a Plan, observation: &'a GitReadinessObservation) -> Option<&'a str> {
    plan.task_order()
        .iter()
        .filter_map(|task_id| plan.task(task_id))
        .filter_map(|task| task.commit_gate())
        .filter(|gate| gate.status() == CommitStatus::Committed)
        .filter_map(|gate| gate.actual_commit())
        .next_back()
        .or_else(|| observation.head())
}

fn compare_field(mismatches: &mut Vec<String>, field: &str, matches: bool) {
    if !matches {
        mismatches.push(field.to_owned());
    }
}

fn readiness_refresh_error(plan: &Plan, mismatches: &[String]) -> MinoError {
    MinoError::new(
        ErrorCategory::DriftDetected,
        format!(
            "Git readiness for plan {} must be refreshed before this action",
            plan.id()
        ),
    )
    .with_remediation(
        mismatches.to_owned(),
        if matches!(
            plan.status(),
            crate::domain::PlanStatus::Draft | crate::domain::PlanStatus::Ready
        ) {
            vec![refresh_action(plan)]
        } else {
            Vec::new()
        },
    )
    .with_details(json!({ "readiness_mismatches": mismatches }))
}

fn required_state(plan: &Plan) -> Result<GitReadinessState, MinoError> {
    plan.git_readiness_state()
        .map_err(|error| domain_state_error(&error))?
        .ok_or_else(|| {
            readiness_refresh_error(plan, &["extensions.git_readiness_state".to_owned()])
        })
}

fn normalized_optional_path(path: Option<&Path>) -> Result<Option<String>, MinoError> {
    path.map(normalized_path).transpose()
}

fn normalized_path(path: &Path) -> Result<String, MinoError> {
    path.to_str()
        .map(|value| value.replace('\\', "/"))
        .ok_or_else(|| {
            MinoError::new(
                ErrorCategory::DriftDetected,
                "Git identity path is not valid UTF-8",
            )
        })
}

fn empty_status_digest() -> String {
    sha256_digest(b"[]")
}

fn domain_state_error(error: &crate::domain::DomainError) -> MinoError {
    MinoError::new(ErrorCategory::DriftDetected, error.to_string())
}
