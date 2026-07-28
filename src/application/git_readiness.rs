//! Live Git readiness capture, comparison, refresh, and protected-transition gates.

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::application::git_binding::map_git_error;
use crate::application::plan::{
    PlanMutationRequest, PlanOperationReport, PlanService, derived_request_id,
};
use crate::domain::{
    CleanupConsentStatus, CommitStatus, GitReadiness, GitReadinessObservation, GitReadinessState,
    GitRepositoryMode, GitSetupDecision, Plan, PrePlanCleanupItem, Timestamp,
};
use crate::git::{
    GitAdapter, GitAvailability, GitBranchJournalStore, GitFacts, GitStatusKind, inspect_commit,
    matches_file_map_path,
};
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

/// Authored content for one proposed pre-plan cleanup commit.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrePlanCleanupItemInput {
    /// Single logical change represented by this cleanup commit.
    pub logical_change: String,
    /// Exact repository-relative files assigned to this cleanup commit.
    pub files: Vec<String>,
    /// Exact single-line Conventional Commit message.
    pub planned_commit_message: String,
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
        let is_fresh = mutation_is_fresh(&current, &request)?;
        let prior_state = current
            .git_readiness_state()
            .map_err(|error| domain_state_error(&error))?;
        if is_fresh
            && prior_state.as_ref().is_some_and(|state| {
                state.cleanup().items().iter().any(|item| {
                    item.consent_status() == CleanupConsentStatus::Approved
                        && item.actual_commit().is_none()
                })
            })
        {
            return Err(decision_message(
                "Approved cleanup items must be recorded before Git readiness can be refreshed",
            ));
        }
        let mut captured = if is_fresh {
            capture_git_readiness(self.plans.root(), request.updated_at.clone())?
        } else {
            CapturedGitReadiness {
                state: required_state(&current)?,
                summary: current.git_readiness().clone(),
                branch: current.metadata().branch().map(str::to_owned),
            }
        };
        if is_fresh && let Some(prior) = prior_state {
            captured.state = prior
                .refreshed_from(&captured.state)
                .map_err(|error| domain_state_error(&error))?;
        }
        captured
            .summary
            .set_git_flow_enabled(captured.state.git_flow_allowed());
        let state = captured.state;
        let summary = captured.summary;
        let branch = captured.branch;
        let block_reason = readiness_block_reason(&current, &state);
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
            move |plan, at| {
                plan.refresh_git_readiness(
                    &state,
                    summary.clone(),
                    branch.clone(),
                    block_reason.clone(),
                    at,
                )
            },
        )
    }

    /// Records one explicit setup decision without running Git initialization.
    ///
    /// # Errors
    ///
    /// Returns a typed error for stale readiness, invalid decision metadata,
    /// unsupported lifecycle state, or persistence failure.
    pub fn decide_setup(
        &self,
        request: PlanMutationRequest,
        decision: GitSetupDecision,
        decision_reference: String,
    ) -> Result<PlanOperationReport, MinoError> {
        require_approval_reference(&decision_reference)?;
        let current = self.plans.load_verified(&request.plan_id)?;
        let is_fresh = mutation_is_fresh(&current, &request)?;
        let mut state = required_state(&current)?;
        if is_fresh {
            require_current_git_readiness(
                self.plans.root(),
                &current,
                GitReadinessRequirement::CleanBaseline,
            )?;
            state
                .decide_setup(
                    decision,
                    request.actor.clone(),
                    decision_reference,
                    request.updated_at.clone(),
                )
                .map_err(|error| decision_error(&error))?;
        }
        self.commit_decision_state(request, &current, state)
    }

    /// Records an exact, complete proposal for all observed dirty paths.
    ///
    /// # Errors
    ///
    /// Returns a typed error for stale live facts, unsafe or incomplete file
    /// coverage, invalid commit messages, or persistence failure.
    pub fn propose_cleanup(
        &self,
        request: PlanMutationRequest,
        inputs: Vec<PrePlanCleanupItemInput>,
    ) -> Result<PlanOperationReport, MinoError> {
        let current = self.plans.load_verified(&request.plan_id)?;
        let is_fresh = mutation_is_fresh(&current, &request)?;
        let mut state = required_state(&current)?;
        if is_fresh {
            require_current_git_readiness(
                self.plans.root(),
                &current,
                GitReadinessRequirement::CleanBaseline,
            )?;
            let items = inputs
                .into_iter()
                .enumerate()
                .map(|(index, input)| {
                    PrePlanCleanupItem::new(
                        format!("C{}", index + 1),
                        input.logical_change,
                        input.files,
                        input.planned_commit_message,
                    )
                    .map_err(|error| decision_error(&error))
                })
                .collect::<Result<Vec<_>, _>>()?;
            state
                .propose_cleanup(items)
                .map_err(|error| decision_error(&error))?;
        }
        self.commit_decision_state(request, &current, state)
    }

    /// Approves one exact cleanup item in the current proposal.
    ///
    /// # Errors
    ///
    /// Returns a typed error for stale live facts, missing approval metadata,
    /// an unknown or already-approved item, or persistence failure.
    pub fn approve_cleanup_item(
        &self,
        request: PlanMutationRequest,
        item_id: &str,
        approval_reference: String,
    ) -> Result<PlanOperationReport, MinoError> {
        require_approval_reference(&approval_reference)?;
        let current = self.plans.load_verified(&request.plan_id)?;
        let is_fresh = mutation_is_fresh(&current, &request)?;
        let mut state = required_state(&current)?;
        if is_fresh {
            require_current_git_readiness(
                self.plans.root(),
                &current,
                GitReadinessRequirement::CleanBaseline,
            )?;
            state
                .approve_cleanup_item(
                    item_id,
                    request.actor.clone(),
                    approval_reference,
                    request.updated_at.clone(),
                )
                .map_err(|error| decision_error(&error))?;
        }
        self.commit_decision_state(request, &current, state)
    }

    /// Records an explicit decision to continue without pre-plan cleanup.
    ///
    /// # Errors
    ///
    /// Returns a typed error for stale live facts, missing decision metadata,
    /// prior item approval, unsupported lifecycle state, or persistence failure.
    pub fn decline_cleanup(
        &self,
        request: PlanMutationRequest,
        decision_reference: String,
    ) -> Result<PlanOperationReport, MinoError> {
        require_approval_reference(&decision_reference)?;
        let current = self.plans.load_verified(&request.plan_id)?;
        let is_fresh = mutation_is_fresh(&current, &request)?;
        let mut state = required_state(&current)?;
        if is_fresh {
            require_current_git_readiness(
                self.plans.root(),
                &current,
                GitReadinessRequirement::CleanBaseline,
            )?;
            state
                .decline_cleanup(
                    request.actor.clone(),
                    decision_reference,
                    request.updated_at.clone(),
                )
                .map_err(|error| decision_error(&error))?;
        }
        self.commit_decision_state(request, &current, state)
    }

    /// Verifies and records one already-created cleanup commit without mutating Git.
    ///
    /// # Errors
    ///
    /// Returns a typed error unless the commit is current HEAD, follows proposal
    /// order, and has the exact approved parent, message, and files.
    pub fn record_cleanup_commit(
        &self,
        request: PlanMutationRequest,
        item_id: &str,
        commit_id: &str,
    ) -> Result<PlanOperationReport, MinoError> {
        let current = self.plans.load_verified(&request.plan_id)?;
        let is_fresh = mutation_is_fresh(&current, &request)?;
        let mut state = required_state(&current)?;
        if is_fresh {
            require_current_git_readiness(
                self.plans.root(),
                &current,
                GitReadinessRequirement::IdentityOnly,
            )?;
            validate_cleanup_item_order(&state, item_id)?;
            let commit = inspect_commit(self.plans.root(), commit_id)
                .map_err(|error| map_git_error(&error))?;
            validate_cleanup_commit(self.plans.root(), &state, item_id, &commit)?;
            state
                .record_cleanup_commit(item_id, &commit.commit, request.updated_at.clone())
                .map_err(|error| decision_error(&error))?;
        }
        self.commit_decision_state(request, &current, state)
    }

    fn commit_decision_state(
        &self,
        request: PlanMutationRequest,
        current: &Plan,
        state: GitReadinessState,
    ) -> Result<PlanOperationReport, MinoError> {
        let block_reason = readiness_block_reason(current, &state);
        self.plans.commit_semantic(
            request,
            vec![
                "extensions.git_readiness_state".to_owned(),
                "git_readiness.git_flow_enabled".to_owned(),
                "status".to_owned(),
                "resume_status".to_owned(),
                "blocker".to_owned(),
                "approvals".to_owned(),
                "extensions.workspace.plan_baseline".to_owned(),
            ],
            |_| Ok(None),
            move |plan, at| plan.update_git_readiness_state(&state, block_reason.clone(), at),
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
    let (observed_paths, blockers) = cleanup_live_facts(facts);
    let state = GitReadinessState::captured(observation, observed_paths, blockers)
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
    Ok(CapturedGitReadiness {
        summary: GitReadiness::detected(
            "Present",
            working_tree,
            facts.branch.clone(),
            facts.head.clone(),
            base_status,
            state.git_flow_allowed(),
        ),
        state,
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
        ) || plan.is_blocked_for_git_readiness()
        {
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

fn cleanup_live_facts(facts: &GitFacts) -> (Vec<String>, Vec<String>) {
    if !facts.is_worktree || facts.is_clean {
        return (Vec::new(), Vec::new());
    }
    let mut paths = facts
        .status
        .iter()
        .flat_map(|entry| {
            std::iter::once(entry.path.clone()).chain(entry.original_path.iter().cloned())
        })
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    let mut blockers = facts
        .status
        .iter()
        .flat_map(|entry| {
            let unmerged =
                (entry.kind == GitStatusKind::Unmerged).then(|| format!("unmerged:{}", entry.path));
            let submodule = entry
                .is_submodule()
                .then(|| format!("submodule:{}", entry.path));
            [unmerged, submodule].into_iter().flatten()
        })
        .collect::<Vec<_>>();
    blockers.sort();
    blockers.dedup();
    (paths, blockers)
}

fn mutation_is_fresh(plan: &Plan, request: &PlanMutationRequest) -> Result<bool, MinoError> {
    if plan.revision() == request.expected_revision {
        return Ok(true);
    }
    if request
        .expected_revision
        .checked_add(1)
        .is_some_and(|revision| revision == plan.revision())
    {
        return Ok(false);
    }
    Err(MinoError::new(
        ErrorCategory::RevisionConflict,
        format!(
            "Plan {} is revision {}, not expected revision {}",
            plan.id(),
            plan.revision(),
            request.expected_revision
        ),
    ))
}

pub(crate) fn readiness_block_reason(plan: &Plan, state: &GitReadinessState) -> Option<String> {
    if state.setup().decision() == GitSetupDecision::BlockedUntilManualSetup
        && state.observation().repository_mode() == GitRepositoryMode::NotRepository
    {
        return Some("manual Git setup has not completed".to_owned());
    }
    if !state.cleanup().blockers().is_empty() {
        return Some(format!(
            "cleanup cannot be separated safely: {}",
            state.cleanup().blockers().join(", ")
        ));
    }
    let overlap = state.cleanup().observed_paths().iter().find(|path| {
        plan.tasks().iter().any(|task| {
            task.file_map()
                .iter()
                .any(|entry| matches_file_map_path(entry.path(), path))
        })
    });
    overlap.map(|path| format!("pre-plan path overlaps the task File Map: {path}"))
}

fn validate_cleanup_commit(
    root: &Path,
    state: &GitReadinessState,
    item_id: &str,
    commit: &crate::git::GitCommitObject,
) -> Result<(), MinoError> {
    let items = state.cleanup().items();
    let index = validate_cleanup_item_order(state, item_id)?;
    let expected_parent = if index == 0 {
        state.observation().head()
    } else {
        items[index - 1].actual_commit()
    }
    .ok_or_else(|| decision_message("Cleanup commit order has no verified parent"))?;
    let item = &items[index];
    let facts = GitAdapter::new(root)
        .inspect()
        .map_err(|error| map_git_error(&error))?;
    let matches = facts.head.as_deref() == Some(commit.commit.as_str())
        && commit.parent == expected_parent
        && commit.message == item.planned_commit_message()
        && commit.files == item.files();
    if matches {
        Ok(())
    } else {
        Err(MinoError::new(
            ErrorCategory::PolicyViolation,
            "Cleanup commit does not match the approved current-HEAD item",
        )
        .with_details(json!({
            "cleanup_item": item_id,
            "expected_parent": expected_parent,
            "expected_message": item.planned_commit_message(),
            "expected_files": item.files(),
        })))
    }
}

fn validate_cleanup_item_order(
    state: &GitReadinessState,
    item_id: &str,
) -> Result<usize, MinoError> {
    let items = state.cleanup().items();
    let index = items
        .iter()
        .position(|item| item.id() == item_id)
        .ok_or_else(|| decision_message(format!("Cleanup item {item_id} does not exist")))?;
    let next_index = items
        .iter()
        .position(|item| item.actual_commit().is_none())
        .ok_or_else(|| decision_message("Every cleanup item is already recorded"))?;
    if index != next_index {
        return Err(decision_message(format!(
            "Cleanup commits must be recorded in order; {} is next",
            items[next_index].id()
        )));
    }
    Ok(index)
}

fn require_approval_reference(reference: &str) -> Result<(), MinoError> {
    if reference.trim().is_empty() {
        Err(MinoError::new(
            ErrorCategory::ApprovalRequired,
            "Git readiness decision requires a non-empty approval reference",
        ))
    } else {
        Ok(())
    }
}

fn decision_message(message: impl Into<String>) -> MinoError {
    MinoError::new(ErrorCategory::IncompleteOrValidation, message)
}

fn decision_error(error: &crate::domain::DomainError) -> MinoError {
    decision_message(error.to_string())
}

fn domain_state_error(error: &crate::domain::DomainError) -> MinoError {
    MinoError::new(ErrorCategory::DriftDetected, error.to_string())
}
