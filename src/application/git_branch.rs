//! Approval-gated branch proposal, prepared execution, and recovery orchestration.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::application::git_binding::map_git_error;
use crate::application::plan::PlanService;
use crate::domain::{Plan, PlanId, PlanStatus, Timestamp};
use crate::git::{
    ActiveBindingResolution, ActiveBindingStore, GitAdapter, GitBranchCompletion, GitBranchIntent,
    GitBranchJournal, GitBranchJournalStore, GitFacts, create_and_switch_branch,
    local_branch_target, proposed_branch_name, validate_branch_name,
};
use crate::{ErrorCategory, MinoError};

use super::git_readiness::{GitReadinessRequirement, require_current_git_readiness};

/// Stable reason that a proposed branch cannot currently be created.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GitBranchBlocker {
    /// The plan is already terminal.
    PlanDone,
    /// The plan was not captured from a Git repository.
    PlanRepositoryUnavailable,
    /// The current path is not a Git repository.
    RepositoryUnavailable,
    /// The repository has no worktree.
    WorktreeUnavailable,
    /// The plan did not enable clean-baseline Git Flow.
    GitFlowDisabled,
    /// Staged, unstaged, unmerged, or untracked changes exist.
    WorktreeDirty,
    /// No committed HEAD exists from which to create the branch.
    HeadUnavailable,
    /// The current branch or detached mode differs from the plan snapshot.
    SourceBranchMismatch,
    /// The current HEAD differs from the plan base commit.
    BaseHeadMismatch,
    /// The proposed local branch already exists.
    BranchExists,
}

/// Deterministic read-only branch proposal and current eligibility report.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GitBranchProposal {
    /// Plan for which the branch is proposed.
    pub plan_id: PlanId,
    /// Current verified plan revision.
    pub plan_revision: u64,
    /// Deterministically derived local branch name.
    pub branch_name: String,
    /// Current repository and worktree facts.
    pub facts: GitFacts,
    /// Existing target of the proposed local branch, when present.
    pub existing_branch_head: Option<String>,
    /// Ordered policy blockers derived from the plan and live repository.
    pub blockers: Vec<GitBranchBlocker>,
    /// Whether branch creation is currently eligible for explicit approval.
    pub can_create: bool,
}

/// Completed or replayed branch-creation operation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GitBranchCreateReport {
    /// Immutable approval-bound prepared intent.
    pub intent: GitBranchIntent,
    /// Immutable terminal result.
    pub completion: GitBranchCompletion,
    /// Current repository facts after completion or replay.
    pub facts: GitFacts,
    /// Current relationship between the active binding and worktree.
    pub active_binding: ActiveBindingResolution,
    /// Whether an already completed operation was returned without Git mutation.
    pub replayed: bool,
    /// Whether a prepared operation was reconciled after Git had already switched.
    pub reconciled: bool,
}

/// Application boundary for deterministic branch proposals and approved creation.
#[derive(Clone, Debug)]
pub struct GitBranchService {
    root: PathBuf,
}

impl GitBranchService {
    /// Discovers an initialized project and creates its branch service.
    ///
    /// # Errors
    ///
    /// Returns an environment-unavailable error when project discovery fails.
    pub fn discover(start: &Path) -> Result<Self, MinoError> {
        let project = crate::project::discover(start)?;
        Ok(Self {
            root: project.path().to_path_buf(),
        })
    }

    /// Returns a deterministic proposal without writing Git or Mino state.
    ///
    /// # Errors
    ///
    /// Returns a typed error for a missing/drifted plan, unavailable Git, or
    /// invalid derived branch-name output.
    pub fn propose(&self, plan_id: &PlanId) -> Result<GitBranchProposal, MinoError> {
        let plan = self.load_plan(plan_id)?;
        let facts = self.inspect_git()?;
        self.build_proposal(&plan, facts)
    }

    /// Creates the deterministic branch after an explicit approval reference.
    ///
    /// A prepared immutable intent makes retries recover a branch that Git
    /// created before active-binding or result publication completed.
    ///
    /// # Errors
    ///
    /// Returns approval-required, policy, revision, drift, or environment
    /// errors without bypassing current plan/repository facts.
    pub fn create(
        &self,
        plan_id: &PlanId,
        approval_reference: &str,
        explicit_branch: Option<&str>,
    ) -> Result<GitBranchCreateReport, MinoError> {
        validate_approval_reference(approval_reference)?;
        let plan = self.load_plan(plan_id)?;
        let branch_name = proposed_branch_name(plan_id);
        self.validate_explicit_branch(explicit_branch, &branch_name)?;
        let store = GitBranchJournalStore::new(&self.root);
        match store.load(plan_id).map_err(|error| map_git_error(&error))? {
            Some(journal) if journal.completion.is_some() => {
                return self.replay_completed(&plan, journal, approval_reference);
            }
            Some(_) => {}
            None => {
                require_current_git_readiness(
                    &self.root,
                    &plan,
                    GitReadinessRequirement::CleanBaseline,
                )?;
                require_eligible(&self.propose(plan_id)?)?;
            }
        }
        let _lock = store.lock().map_err(|error| map_git_error(&error))?;
        match store.load(plan_id).map_err(|error| map_git_error(&error))? {
            Some(journal) if journal.completion.is_some() => {
                self.replay_completed(&plan, journal, approval_reference)
            }
            Some(journal) => self.resume_prepared(&plan, &store, journal, approval_reference),
            None => self.prepare_and_execute(&plan, &store, approval_reference),
        }
    }

    fn prepare_and_execute(
        &self,
        plan: &Plan,
        store: &GitBranchJournalStore,
        approval_reference: &str,
    ) -> Result<GitBranchCreateReport, MinoError> {
        let facts = self.inspect_git()?;
        let proposal = self.build_proposal(plan, facts.clone())?;
        require_eligible(&proposal)?;
        let intent = GitBranchIntent::new(
            plan.id().clone(),
            plan.revision(),
            required_path(facts.common_dir.as_deref(), "Git common directory")?,
            required_path(facts.worktree.as_deref(), "Git worktree")?,
            facts.branch.clone(),
            facts.head.clone().ok_or_else(|| {
                MinoError::new(
                    ErrorCategory::PolicyViolation,
                    "Branch creation requires a committed HEAD",
                )
            })?,
            proposal.branch_name,
            approval_reference.to_owned(),
            Timestamp::now_utc(),
        )
        .map_err(|error| map_git_error(&error))?;
        let intent = store
            .prepare(intent)
            .map_err(|error| map_git_error(&error))?;
        self.execute_prepared(plan, store, intent)
    }

    fn resume_prepared(
        &self,
        plan: &Plan,
        store: &GitBranchJournalStore,
        journal: GitBranchJournal,
        approval_reference: &str,
    ) -> Result<GitBranchCreateReport, MinoError> {
        validate_intent_request(plan, &journal.intent, approval_reference)?;
        self.execute_prepared(plan, store, journal.intent)
    }

    fn execute_prepared(
        &self,
        plan: &Plan,
        store: &GitBranchJournalStore,
        intent: GitBranchIntent,
    ) -> Result<GitBranchCreateReport, MinoError> {
        let before = self.inspect_git()?;
        let branch_target = local_branch_target(&self.root, &intent.branch_name)
            .map_err(|error| map_git_error(&error))?;
        if is_target_state(&before, &intent, branch_target.as_deref()) {
            return self.finish_prepared(plan, store, intent, before, true);
        }
        if !is_source_state(&before, &intent, branch_target.as_deref()) {
            return Err(MinoError::new(
                ErrorCategory::DriftDetected,
                "Prepared branch operation no longer matches its source or target Git state",
            ));
        }
        let command = create_and_switch_branch(&self.root, &intent.branch_name, &intent.base_head)
            .map_err(|error| map_git_error(&error))?;
        let after = self.inspect_git()?;
        let after_target = local_branch_target(&self.root, &intent.branch_name)
            .map_err(|error| map_git_error(&error))?;
        if is_target_state(&after, &intent, after_target.as_deref()) {
            return self.finish_prepared(plan, store, intent, after, false);
        }
        if !command.success && is_source_state(&after, &intent, after_target.as_deref()) {
            return Err(MinoError::new(
                ErrorCategory::EnvironmentUnavailable,
                format!(
                    "Git branch creation failed with exit {:?}: {}",
                    command.exit_code, command.stderr
                ),
            ));
        }
        Err(MinoError::new(
            ErrorCategory::DriftDetected,
            "Git branch creation ended in an unexpected repository state; the prepared intent was preserved",
        ))
    }

    fn finish_prepared(
        &self,
        plan: &Plan,
        store: &GitBranchJournalStore,
        intent: GitBranchIntent,
        facts: GitFacts,
        reconciled: bool,
    ) -> Result<GitBranchCreateReport, MinoError> {
        let bindings = ActiveBindingStore::new(&self.root);
        bindings
            .resolve(&facts)
            .map_err(|error| map_git_error(&error))?;
        bindings
            .bind(
                &facts,
                plan.id().clone(),
                plan.revision(),
                Timestamp::now_utc(),
            )
            .map_err(|error| map_git_error(&error))?;
        let completion = GitBranchCompletion::new(
            &intent,
            facts.head.clone().ok_or_else(|| {
                MinoError::new(ErrorCategory::DriftDetected, "Created branch has no HEAD")
            })?,
            Timestamp::now_utc(),
        )
        .map_err(|error| map_git_error(&error))?;
        let completion = store
            .complete(&intent, completion)
            .map_err(|error| map_git_error(&error))?;
        let active_binding = bindings
            .resolve(&facts)
            .map_err(|error| map_git_error(&error))?;
        Ok(GitBranchCreateReport {
            intent,
            completion,
            facts,
            active_binding,
            replayed: false,
            reconciled,
        })
    }

    fn replay_completed(
        &self,
        plan: &Plan,
        journal: GitBranchJournal,
        approval_reference: &str,
    ) -> Result<GitBranchCreateReport, MinoError> {
        validate_completed_request(plan, &journal.intent, approval_reference)?;
        let completion = journal.completion.ok_or_else(|| {
            MinoError::new(
                ErrorCategory::DriftDetected,
                "Completed branch journal has no terminal result",
            )
        })?;
        if local_branch_target(&self.root, &journal.intent.branch_name)
            .map_err(|error| map_git_error(&error))?
            .is_none()
        {
            return Err(MinoError::new(
                ErrorCategory::DriftDetected,
                "Completed branch journal refers to a missing local branch",
            ));
        }
        let facts = self.inspect_git()?;
        let active_binding = ActiveBindingStore::new(&self.root)
            .resolve(&facts)
            .map_err(|error| map_git_error(&error))?;
        if active_binding
            .binding
            .as_ref()
            .is_none_or(|binding| binding.plan_id != journal.intent.plan_id)
        {
            return Err(MinoError::new(
                ErrorCategory::DriftDetected,
                "Completed branch operation has no matching active-plan binding",
            ));
        }
        Ok(GitBranchCreateReport {
            intent: journal.intent,
            completion,
            facts,
            active_binding,
            replayed: true,
            reconciled: false,
        })
    }

    fn build_proposal(&self, plan: &Plan, facts: GitFacts) -> Result<GitBranchProposal, MinoError> {
        let branch_name = proposed_branch_name(plan.id());
        validate_branch_name(&self.root, &branch_name).map_err(|error| map_git_error(&error))?;
        let existing_branch_head = if facts.repository && facts.is_worktree {
            local_branch_target(&self.root, &branch_name).map_err(|error| map_git_error(&error))?
        } else {
            None
        };
        let blockers = proposal_blockers(plan, &facts, existing_branch_head.is_some());
        Ok(GitBranchProposal {
            plan_id: plan.id().clone(),
            plan_revision: plan.revision(),
            branch_name,
            facts,
            existing_branch_head,
            can_create: blockers.is_empty(),
            blockers,
        })
    }

    fn validate_explicit_branch(
        &self,
        explicit_branch: Option<&str>,
        proposed_branch: &str,
    ) -> Result<(), MinoError> {
        let Some(explicit_branch) = explicit_branch else {
            return Ok(());
        };
        validate_branch_name(&self.root, explicit_branch).map_err(|error| map_git_error(&error))?;
        if explicit_branch == proposed_branch {
            Ok(())
        } else {
            Err(MinoError::new(
                ErrorCategory::PolicyViolation,
                format!(
                    "Explicit branch {explicit_branch} does not match proposed branch {proposed_branch}"
                ),
            ))
        }
    }

    fn load_plan(&self, plan_id: &PlanId) -> Result<Plan, MinoError> {
        PlanService::discover(&self.root)?.load_verified(plan_id)
    }

    fn inspect_git(&self) -> Result<GitFacts, MinoError> {
        GitAdapter::new(&self.root)
            .inspect()
            .map_err(|error| map_git_error(&error))
    }
}

fn proposal_blockers(plan: &Plan, facts: &GitFacts, branch_exists: bool) -> Vec<GitBranchBlocker> {
    let mut blockers = Vec::new();
    if plan.status() == PlanStatus::Done {
        blockers.push(GitBranchBlocker::PlanDone);
    }
    if plan.git_readiness().repository() != "Present" {
        blockers.push(GitBranchBlocker::PlanRepositoryUnavailable);
    }
    if !facts.repository {
        blockers.push(GitBranchBlocker::RepositoryUnavailable);
    }
    if !facts.is_worktree {
        blockers.push(GitBranchBlocker::WorktreeUnavailable);
    }
    if !plan.git_readiness().git_flow_enabled() {
        blockers.push(GitBranchBlocker::GitFlowDisabled);
    }
    if !facts.is_clean {
        blockers.push(GitBranchBlocker::WorktreeDirty);
    }
    if facts.head.is_none() {
        blockers.push(GitBranchBlocker::HeadUnavailable);
    }
    if facts.branch.as_deref() != plan.git_readiness().branch() {
        blockers.push(GitBranchBlocker::SourceBranchMismatch);
    }
    if !base_head_matches(plan.git_readiness().base_commit(), facts.head.as_deref()) {
        blockers.push(GitBranchBlocker::BaseHeadMismatch);
    }
    if branch_exists {
        blockers.push(GitBranchBlocker::BranchExists);
    }
    blockers
}

fn base_head_matches(expected: Option<&str>, actual: Option<&str>) -> bool {
    let (Some(expected), Some(actual)) = (expected, actual) else {
        return false;
    };
    expected.len() >= 4
        && expected.len() <= actual.len()
        && expected.bytes().all(|byte| byte.is_ascii_hexdigit())
        && actual
            .get(..expected.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(expected))
}

fn require_eligible(proposal: &GitBranchProposal) -> Result<(), MinoError> {
    if proposal.can_create {
        Ok(())
    } else {
        Err(MinoError::new(
            ErrorCategory::PolicyViolation,
            format!(
                "Git branch creation is blocked: {}",
                proposal
                    .blockers
                    .iter()
                    .map(|blocker| format!("{blocker:?}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ))
    }
}

fn validate_approval_reference(value: &str) -> Result<(), MinoError> {
    if value.trim().is_empty() || value.len() > 4096 || value.chars().any(char::is_control) {
        Err(MinoError::new(
            ErrorCategory::ApprovalRequired,
            "Git branch creation requires a valid non-empty approval reference",
        ))
    } else {
        Ok(())
    }
}

fn validate_intent_request(
    plan: &Plan,
    intent: &GitBranchIntent,
    approval_reference: &str,
) -> Result<(), MinoError> {
    if intent.plan_id != *plan.id()
        || plan.revision() < intent.plan_revision
        || intent.approval_reference != approval_reference
    {
        Err(MinoError::new(
            ErrorCategory::RevisionConflict,
            "Prepared branch operation does not match the current plan revision or approval reference",
        ))
    } else if plan.status() == PlanStatus::Done {
        Err(MinoError::new(
            ErrorCategory::PolicyViolation,
            "A Done plan cannot resume branch creation",
        ))
    } else if plan.git_readiness().repository() != "Present"
        || !plan.git_readiness().git_flow_enabled()
        || plan.git_readiness().branch() != intent.source_branch.as_deref()
        || !base_head_matches(
            plan.git_readiness().base_commit(),
            Some(intent.base_head.as_str()),
        )
    {
        Err(MinoError::new(
            ErrorCategory::DriftDetected,
            "Prepared branch operation no longer matches the plan Git-readiness snapshot",
        ))
    } else {
        Ok(())
    }
}

fn validate_completed_request(
    plan: &Plan,
    intent: &GitBranchIntent,
    approval_reference: &str,
) -> Result<(), MinoError> {
    if intent.plan_id != *plan.id()
        || plan.revision() < intent.plan_revision
        || intent.approval_reference != approval_reference
    {
        Err(MinoError::new(
            ErrorCategory::RevisionConflict,
            "Completed branch operation does not match the current plan or approval reference",
        ))
    } else {
        Ok(())
    }
}

fn is_source_state(facts: &GitFacts, intent: &GitBranchIntent, target: Option<&str>) -> bool {
    matching_identity(facts, intent)
        && facts.branch == intent.source_branch
        && facts.head.as_deref() == Some(intent.base_head.as_str())
        && facts.is_clean
        && target.is_none()
}

fn is_target_state(facts: &GitFacts, intent: &GitBranchIntent, target: Option<&str>) -> bool {
    matching_identity(facts, intent)
        && facts.branch.as_deref() == Some(intent.branch_name.as_str())
        && facts.head.as_deref() == Some(intent.base_head.as_str())
        && facts.is_clean
        && target == Some(intent.base_head.as_str())
}

fn matching_identity(facts: &GitFacts, intent: &GitBranchIntent) -> bool {
    normalized_path(facts.common_dir.as_deref()).as_deref() == Some(intent.common_dir.as_str())
        && normalized_path(facts.worktree.as_deref()).as_deref() == Some(intent.worktree.as_str())
}

fn required_path(path: Option<&Path>, label: &str) -> Result<String, MinoError> {
    normalized_path(path).ok_or_else(|| {
        MinoError::new(
            ErrorCategory::DriftDetected,
            format!("{label} is missing or not valid UTF-8"),
        )
    })
}

fn normalized_path(path: Option<&Path>) -> Option<String> {
    path.and_then(Path::to_str)
        .map(|value| value.replace('\\', "/"))
}
