//! Plan-scoped commit preflight, staging, recovery, evidence, and gate recording.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::application::completion::{validate_task_deviations, validate_task_evidence};
use crate::application::git_binding::map_git_error;
use crate::application::plan::{PlanMutationRequest, PlanService, derived_request_id};
use crate::domain::{
    CommitStatus, Evidence, EvidenceId, EvidenceType, GitFlowConsent, Plan, PlanId, PlanStatus,
    RequestId, Task, TaskId, TaskStatus, Timestamp,
};
use crate::evidence::{
    AddEvidenceRequest, EvidenceError, EvidenceErrorKind, EvidenceRequestContext, EvidenceSource,
    EvidenceStore,
};
use crate::git::{
    ActiveBindingStatus, ActiveBindingStore, CommitFileSnapshot, GitAdapter, GitBranchJournalStore,
    GitCommitCompletion, GitCommitCompletionInput, GitCommitIntent, GitCommitJournal,
    GitCommitJournalStore, GitCommitObject, GitFacts, GitStagedCommit, capture_commit_snapshots,
    ensure_no_clean_filters, inspect_commit, run_task_commit, stage_commit_paths,
    task_commit_entries, validate_commit_snapshot_scope, verify_commit_snapshots, write_index_tree,
};
use crate::runner::Redactor;
use crate::{ErrorCategory, MinoError};

/// Complete read-only commit eligibility and exact-path snapshot.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GitCommitPreflight {
    /// Target plan.
    pub plan_id: PlanId,
    /// Current plan revision.
    pub plan_revision: u64,
    /// Done task whose gate is eligible.
    pub task_id: TaskId,
    /// Exact planned commit message.
    pub message: String,
    /// Exact full parent commit.
    pub parent_head: String,
    /// Exact checked-out branch.
    pub branch: String,
    /// Sorted exact task paths.
    pub files: Vec<String>,
    /// Bounded pre-staging file fingerprints.
    pub snapshots: Vec<CommitFileSnapshot>,
    /// Live Git facts used by the policy decision.
    pub facts: GitFacts,
}

/// Completed, reconciled, or replayed task-commit operation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GitTaskCommitReport {
    /// Immutable pre-index intent.
    pub intent: GitCommitIntent,
    /// Immutable staged tree.
    pub staged: GitStagedCommit,
    /// Immutable terminal journal result.
    pub completion: GitCommitCompletion,
    /// Current plan revision containing the committed gate.
    pub plan_revision: u64,
    /// Current Git facts after completion or replay.
    pub facts: GitFacts,
    /// Whether the terminal journal was replayed without mutation.
    pub replayed: bool,
    /// Whether an already-created commit was reconciled after interruption.
    pub reconciled: bool,
}

/// Application boundary for recoverable task-level Git commits.
#[derive(Clone, Debug)]
pub struct GitCommitService {
    root: PathBuf,
    plans: PlanService,
    evidence: EvidenceStore,
}

impl GitCommitService {
    /// Discovers an initialized project and creates its commit service.
    ///
    /// # Errors
    ///
    /// Returns an environment-unavailable error when project discovery fails.
    pub fn discover(start: &Path) -> Result<Self, MinoError> {
        let plans = PlanService::discover(start)?;
        let root = plans.root().to_path_buf();
        Ok(Self {
            evidence: EvidenceStore::new(&root),
            root,
            plans,
        })
    }

    /// Returns exact commit eligibility without modifying Git or Mino state.
    ///
    /// # Errors
    ///
    /// Returns a typed error for any approval, lifecycle, evidence, binding,
    /// index, branch, parent, File Map, scope, filter, or content-policy gate.
    pub fn preflight(
        &self,
        plan_id: &PlanId,
        task_id: &TaskId,
    ) -> Result<GitCommitPreflight, MinoError> {
        let plan = self.plans.load_verified(plan_id)?;
        self.preflight_plan(&plan, task_id)
    }

    /// Creates or exactly recovers one approved task-level Git commit.
    ///
    /// # Errors
    ///
    /// Returns a typed error for failed preflight, unsafe staged state, Git
    /// failure, journal drift, evidence failure, or plan publication failure.
    pub fn commit(
        &self,
        plan_id: &PlanId,
        task_id: &TaskId,
    ) -> Result<GitTaskCommitReport, MinoError> {
        let store = GitCommitJournalStore::new(&self.root);
        match store
            .load(plan_id, task_id)
            .map_err(|error| map_git_error(&error))?
        {
            Some(journal) if journal.completion.is_some() => {
                return self.replay_completed(journal);
            }
            Some(_) => {}
            None => {
                self.preflight(plan_id, task_id)?;
            }
        }
        let _lock = store.lock().map_err(|error| map_git_error(&error))?;
        match store
            .load(plan_id, task_id)
            .map_err(|error| map_git_error(&error))?
        {
            Some(journal) if journal.completion.is_some() => self.replay_completed(journal),
            Some(journal) => self.execute_journal(&store, journal),
            None => self.prepare_and_execute(&store, plan_id, task_id),
        }
    }

    fn prepare_and_execute(
        &self,
        store: &GitCommitJournalStore,
        plan_id: &PlanId,
        task_id: &TaskId,
    ) -> Result<GitTaskCommitReport, MinoError> {
        let preflight = self.preflight(plan_id, task_id)?;
        let intent = GitCommitIntent::new(
            preflight.plan_id,
            preflight.plan_revision,
            preflight.task_id,
            required_path(
                preflight.facts.common_dir.as_deref(),
                "Git common directory",
            )?,
            required_path(preflight.facts.worktree.as_deref(), "Git worktree")?,
            preflight.branch,
            preflight.parent_head,
            preflight.message,
            preflight.snapshots,
            Timestamp::now_utc(),
        )
        .map_err(|error| map_git_error(&error))?;
        let intent = store
            .prepare(intent)
            .map_err(|error| map_git_error(&error))?;
        self.execute_journal(
            store,
            GitCommitJournal {
                intent,
                staged: None,
                completion: None,
            },
        )
    }

    fn execute_journal(
        &self,
        store: &GitCommitJournalStore,
        journal: GitCommitJournal,
    ) -> Result<GitTaskCommitReport, MinoError> {
        let plan = self.plans.load_verified(&journal.intent.plan_id)?;
        let task = validate_prepared_plan(&plan, &journal.intent)?;
        let evidence = self
            .evidence
            .list(plan.id())
            .map_err(|error| map_evidence_error(&error))?;
        validate_task_evidence(&plan, task, &evidence)?;
        validate_task_deviations(&plan, task)?;
        validate_commit_snapshot_scope(task, &journal.intent.files)
            .map_err(|error| map_git_error(&error))?;
        let facts = self.inspect_git()?;
        validate_intent_identity(&self.root, &plan, &journal.intent, &facts)?;
        if facts.head.as_deref() != Some(journal.intent.parent_head.as_str()) {
            let staged = journal.staged.ok_or_else(|| {
                MinoError::new(
                    ErrorCategory::DriftDetected,
                    "Git HEAD advanced before a staged commit phase was recorded",
                )
            })?;
            return self.reconcile_created(store, journal.intent, staged, &facts, true);
        }
        let staged = match journal.staged {
            Some(staged) => {
                self.verify_staged_state(&journal.intent, &staged, &facts)?;
                staged
            }
            None => self.stage_intent(store, &journal.intent, &facts)?,
        };
        self.run_and_reconcile(store, journal.intent, staged)
    }

    fn stage_intent(
        &self,
        store: &GitCommitJournalStore,
        intent: &GitCommitIntent,
        before: &GitFacts,
    ) -> Result<GitStagedCommit, MinoError> {
        let paths = intent.paths();
        let already_staged = before.staged_paths == paths && before.unstaged_paths.is_empty();
        if !already_staged {
            if !source_status_matches(before, intent) {
                return Err(MinoError::new(
                    ErrorCategory::DriftDetected,
                    "Prepared task files no longer match the unstaged source state",
                ));
            }
            ensure_no_clean_filters(&self.root, &paths).map_err(|error| map_git_error(&error))?;
            let result = match stage_commit_paths(&self.root, &paths) {
                Ok(result) => result,
                Err(error) => {
                    return Err(self.blocked_failure(
                        intent,
                        format!("Git staging could not be observed safely: {error}"),
                    ));
                }
            };
            if !result.success {
                return Err(self.blocked_failure(
                    intent,
                    format!(
                        "Git staging failed with exit {:?}: {}",
                        result.exit_code, result.stderr
                    ),
                ));
            }
        }
        self.record_staged_state(store, intent, &paths)
            .map_err(|error| {
                self.blocked_failure(
                    intent,
                    format!("Git staging could not be verified or journaled: {error}"),
                )
            })
    }

    fn record_staged_state(
        &self,
        store: &GitCommitJournalStore,
        intent: &GitCommitIntent,
        paths: &[String],
    ) -> Result<GitStagedCommit, MinoError> {
        let after = self.inspect_git()?;
        if after.head.as_deref() != Some(intent.parent_head.as_str())
            || after.branch.as_deref() != Some(intent.branch.as_str())
            || after.staged_paths != paths
            || !after.unstaged_paths.is_empty()
        {
            return Err(MinoError::new(
                ErrorCategory::DriftDetected,
                "Git staging ended in an unexpected index or worktree state",
            ));
        }
        verify_commit_snapshots(&self.root, &intent.files)
            .map_err(|error| map_git_error(&error))?;
        let tree = write_index_tree(&self.root).map_err(|error| map_git_error(&error))?;
        let staged = GitStagedCommit::new(intent, tree, Timestamp::now_utc())
            .map_err(|error| map_git_error(&error))?;
        store
            .record_staged(intent, staged)
            .map_err(|error| map_git_error(&error))
    }

    fn verify_staged_state(
        &self,
        intent: &GitCommitIntent,
        staged: &GitStagedCommit,
        facts: &GitFacts,
    ) -> Result<(), MinoError> {
        if facts.head.as_deref() != Some(intent.parent_head.as_str())
            || facts.branch.as_deref() != Some(intent.branch.as_str())
            || facts.staged_paths != staged.files
            || !facts.unstaged_paths.is_empty()
        {
            return Err(MinoError::new(
                ErrorCategory::DriftDetected,
                "Current index or worktree does not match the recorded staged commit",
            ));
        }
        verify_commit_snapshots(&self.root, &intent.files)
            .map_err(|error| map_git_error(&error))?;
        let tree = write_index_tree(&self.root).map_err(|error| map_git_error(&error))?;
        if tree == staged.tree {
            Ok(())
        } else {
            Err(MinoError::new(
                ErrorCategory::DriftDetected,
                "Current index tree differs from the recorded staged tree",
            ))
        }
    }

    fn run_and_reconcile(
        &self,
        store: &GitCommitJournalStore,
        intent: GitCommitIntent,
        staged: GitStagedCommit,
    ) -> Result<GitTaskCommitReport, MinoError> {
        let result = run_task_commit(&self.root, &intent.message);
        let facts = match self.inspect_git() {
            Ok(facts) => facts,
            Err(error) => {
                let run_error = result.as_ref().err().map_or_else(String::new, |error| {
                    format!("; commit runner also failed: {error}")
                });
                return Err(self.blocked_failure(
                    &intent,
                    format!(
                        "Git state could not be inspected after commit attempt: {error}{run_error}"
                    ),
                ));
            }
        };
        if facts.head.as_deref() != Some(intent.parent_head.as_str()) {
            let reconciled = match &result {
                Ok(result) => !result.success,
                Err(_) => true,
            };
            return self.reconcile_created(store, intent, staged, &facts, reconciled);
        }
        match result {
            Ok(result) if result.success => Err(self.blocked_failure(
                &intent,
                "Git reported commit success without advancing HEAD".to_owned(),
            )),
            Ok(result) => Err(self.blocked_failure(
                &intent,
                format!(
                    "Git commit failed with exit {:?}: {}",
                    result.exit_code, result.stderr
                ),
            )),
            Err(error) => Err(self.blocked_failure(
                &intent,
                format!("Git commit could not be observed safely: {error}"),
            )),
        }
    }

    fn reconcile_created(
        &self,
        store: &GitCommitJournalStore,
        intent: GitCommitIntent,
        staged: GitStagedCommit,
        facts: &GitFacts,
        reconciled: bool,
    ) -> Result<GitTaskCommitReport, MinoError> {
        if facts.branch.as_deref() != Some(intent.branch.as_str())
            || normalized_path(facts.common_dir.as_deref()).as_deref()
                != Some(intent.common_dir.as_str())
            || normalized_path(facts.worktree.as_deref()).as_deref()
                != Some(intent.worktree.as_str())
        {
            return Err(MinoError::new(
                ErrorCategory::DriftDetected,
                "Created commit is no longer on the prepared branch and worktree identity",
            ));
        }
        if !facts.staged_paths.is_empty() || !facts.unstaged_paths.is_empty() {
            return Err(MinoError::new(
                ErrorCategory::DriftDetected,
                "Created commit is exact, but the index or worktree contains additional changes",
            ));
        }
        let commit_id = facts.head.as_deref().ok_or_else(|| {
            MinoError::new(ErrorCategory::DriftDetected, "Created commit has no HEAD")
        })?;
        let commit =
            inspect_commit(&self.root, commit_id).map_err(|error| map_git_error(&error))?;
        validate_commit_object(&intent, &staged, &commit)?;
        self.finish_commit(store, intent, staged, commit, facts.clone(), reconciled)
    }

    fn finish_commit(
        &self,
        store: &GitCommitJournalStore,
        intent: GitCommitIntent,
        staged: GitStagedCommit,
        commit: GitCommitObject,
        facts: GitFacts,
        reconciled: bool,
    ) -> Result<GitTaskCommitReport, MinoError> {
        let (evidence_id, plan_revision) = self.record_commit_gate(&intent, &commit)?;
        let completion = GitCommitCompletion::new(
            &intent,
            &staged,
            GitCommitCompletionInput {
                commit: commit.commit,
                parent: commit.parent,
                tree: commit.tree,
                message: commit.message,
                files: commit.files,
                evidence_id,
                recorded_plan_revision: plan_revision,
                completed_at: Timestamp::now_utc(),
            },
        )
        .map_err(|error| map_git_error(&error))?;
        let completion = store
            .complete(&intent, &staged, completion)
            .map_err(|error| map_git_error(&error))?;
        Ok(GitTaskCommitReport {
            intent,
            staged,
            completion,
            plan_revision,
            facts,
            replayed: false,
            reconciled,
        })
    }

    fn record_commit_gate(
        &self,
        intent: &GitCommitIntent,
        commit: &GitCommitObject,
    ) -> Result<(EvidenceId, u64), MinoError> {
        let evidence_id = self.commit_evidence(intent, commit)?;
        let current = self.plans.load_verified(&intent.plan_id)?;
        let gate = current
            .task(&intent.task_id)
            .and_then(Task::commit_gate)
            .ok_or_else(|| {
                MinoError::new(
                    ErrorCategory::DriftDetected,
                    "Committed task no longer has a commit gate",
                )
            })?;
        if gate.status() == CommitStatus::Committed {
            if gate.actual_commit() == Some(commit.commit.as_str())
                && gate.committed_files() == commit.files
                && gate.evidence_refs().contains(&evidence_id)
            {
                return Ok((evidence_id, current.revision()));
            }
            return Err(MinoError::new(
                ErrorCategory::DriftDetected,
                "Recorded task commit gate conflicts with the created commit",
            ));
        }
        let request_id = RequestId::parse(derived_request_id(
            &current,
            &format!("git.commit.record.{}.{}", intent.task_id, commit.commit),
        ))
        .expect("derived request identifiers are valid");
        let task_id = intent.task_id.clone();
        let commit_id = commit.commit.clone();
        let files = commit.files.clone();
        let evidence_for_mutation = evidence_id.clone();
        let report = self.plans.commit_semantic(
            PlanMutationRequest {
                plan_id: current.id().clone(),
                expected_revision: current.revision(),
                request_id,
                actor: "mino".to_owned(),
                command: commit_command(current.id(), &intent.task_id),
                updated_at: Timestamp::now_utc(),
            },
            vec![
                format!("tasks.{task_id}.commit_gate.status"),
                format!("tasks.{task_id}.commit_gate.actual_commit"),
                format!("tasks.{task_id}.commit_gate.committed_files"),
                format!("tasks.{task_id}.commit_gate.evidence_refs"),
            ],
            |_| Ok(None),
            move |plan, at| {
                plan.record_task_commit(
                    &task_id,
                    &commit_id,
                    files.clone(),
                    evidence_for_mutation.clone(),
                    at,
                )
            },
        )?;
        Ok((evidence_id, report.revision))
    }

    fn commit_evidence(
        &self,
        intent: &GitCommitIntent,
        commit: &GitCommitObject,
    ) -> Result<EvidenceId, MinoError> {
        let existing = self
            .evidence
            .list(&intent.plan_id)
            .map_err(|error| map_evidence_error(&error))?;
        let superseded = existing
            .iter()
            .filter_map(Evidence::supersedes)
            .collect::<BTreeSet<_>>();
        let matches = existing
            .iter()
            .filter(|evidence| {
                evidence.kind() == EvidenceType::Commit
                    && evidence.task_id() == Some(&intent.task_id)
                    && evidence.artifact_path() == Some(commit.commit.as_str())
                    && !superseded.contains(evidence.id())
            })
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [evidence] => return Ok(evidence.id().clone()),
            [] => {}
            _ => {
                return Err(MinoError::new(
                    ErrorCategory::DriftDetected,
                    "Multiple current Commit evidence records identify the same task commit",
                ));
            }
        }
        let plan = self.plans.load_verified(&intent.plan_id)?;
        let request_id = RequestId::parse(derived_request_id(
            &plan,
            &format!("git.commit.evidence.{}.{}", intent.task_id, commit.commit),
        ))
        .expect("derived request identifiers are valid");
        let context = EvidenceRequestContext::new(
            plan.id().clone(),
            plan.revision(),
            request_id,
            "mino",
            commit_command(plan.id(), &intent.task_id),
            Timestamp::now_utc(),
        )
        .map_err(|error| map_evidence_error(&error))?;
        let request = AddEvidenceRequest::new(
            context,
            EvidenceType::Commit,
            EvidenceSource::Reference(commit.commit.clone()),
            format!(
                "Task {} committed {} path(s) with message {}",
                intent.task_id,
                commit.files.len(),
                intent.message
            ),
        )
        .map_err(|error| map_evidence_error(&error))?
        .with_task(intent.task_id.clone());
        self.evidence
            .add(&request, &Redactor::default())
            .map(|report| report.evidence().id().clone())
            .map_err(|error| map_evidence_error(&error))
    }

    fn replay_completed(
        &self,
        journal: GitCommitJournal,
    ) -> Result<GitTaskCommitReport, MinoError> {
        let staged = journal.staged.ok_or_else(|| {
            MinoError::new(
                ErrorCategory::DriftDetected,
                "Completed commit journal has no staged phase",
            )
        })?;
        let completion = journal.completion.ok_or_else(|| {
            MinoError::new(
                ErrorCategory::DriftDetected,
                "Completed commit journal has no terminal phase",
            )
        })?;
        let plan = self.plans.load_verified(&journal.intent.plan_id)?;
        let gate = plan
            .task(&journal.intent.task_id)
            .and_then(Task::commit_gate)
            .ok_or_else(|| {
                MinoError::new(
                    ErrorCategory::DriftDetected,
                    "Committed task gate is missing",
                )
            })?;
        if gate.status() != CommitStatus::Committed
            || gate.actual_commit() != Some(completion.commit.as_str())
            || gate.committed_files() != completion.files
            || !gate.evidence_refs().contains(&completion.evidence_id)
        {
            return Err(MinoError::new(
                ErrorCategory::DriftDetected,
                "Completed commit journal conflicts with current plan state",
            ));
        }
        let commit = inspect_commit(&self.root, &completion.commit)
            .map_err(|error| map_git_error(&error))?;
        validate_commit_object(&journal.intent, &staged, &commit)?;
        Ok(GitTaskCommitReport {
            intent: journal.intent,
            staged,
            completion,
            plan_revision: plan.revision(),
            facts: self.inspect_git()?,
            replayed: true,
            reconciled: false,
        })
    }

    fn blocked_failure(&self, intent: &GitCommitIntent, reason: String) -> MinoError {
        let current = match self.plans.load_verified(&intent.plan_id) {
            Ok(plan) => plan,
            Err(error) => {
                return MinoError::new(
                    ErrorCategory::EnvironmentUnavailable,
                    format!("{reason}; failed to load the plan for blocking: {error}"),
                );
            }
        };
        if current.status() == PlanStatus::Blocked {
            return MinoError::new(ErrorCategory::EnvironmentUnavailable, reason);
        }
        let request_id = RequestId::parse(derived_request_id(
            &current,
            &format!("git.commit.block.{}", intent.task_id),
        ))
        .expect("derived request identifiers are valid");
        let task_id = intent.task_id.clone();
        let block_reason = reason.clone();
        let result = self.plans.commit_semantic(
            PlanMutationRequest {
                plan_id: current.id().clone(),
                expected_revision: current.revision(),
                request_id,
                actor: "mino".to_owned(),
                command: commit_command(current.id(), &intent.task_id),
                updated_at: Timestamp::now_utc(),
            },
            vec![
                "status".to_owned(),
                "resume_status".to_owned(),
                "blocker".to_owned(),
                format!("tasks.{task_id}.commit_gate.status"),
            ],
            |_| Ok(None),
            move |plan, at| plan.block_task_commit(&task_id, block_reason.clone(), at),
        );
        match result {
            Ok(report) => MinoError::new(
                ErrorCategory::EnvironmentUnavailable,
                format!(
                    "{reason}; plan {} is Blocked at revision {} and the Git/index state was preserved",
                    intent.plan_id, report.revision
                ),
            ),
            Err(error) => MinoError::new(
                ErrorCategory::EnvironmentUnavailable,
                format!("{reason}; additionally failed to block the plan safely: {error}"),
            ),
        }
    }

    fn preflight_plan(
        &self,
        plan: &Plan,
        task_id: &TaskId,
    ) -> Result<GitCommitPreflight, MinoError> {
        let task = validate_new_commit_gate(plan, task_id)?;
        let evidence = self
            .evidence
            .list(plan.id())
            .map_err(|error| map_evidence_error(&error))?;
        validate_task_evidence(plan, task, &evidence)?;
        validate_task_deviations(plan, task)?;
        let facts = self.inspect_git()?;
        validate_live_identity(&self.root, plan, &facts)?;
        let parent_head = expected_parent(plan, task_id, facts.head.as_deref())?;
        let entries =
            task_commit_entries(plan, task, &facts).map_err(|error| map_git_error(&error))?;
        let files = entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect::<Vec<_>>();
        ensure_no_clean_filters(&self.root, &files).map_err(|error| map_git_error(&error))?;
        let snapshots = capture_commit_snapshots(&self.root, &entries)
            .map_err(|error| map_git_error(&error))?;
        let gate = task.commit_gate().expect("validated gate exists");
        Ok(GitCommitPreflight {
            plan_id: plan.id().clone(),
            plan_revision: plan.revision(),
            task_id: task.id().clone(),
            message: gate.planned_message().to_owned(),
            parent_head,
            branch: facts.branch.clone().expect("validated branch exists"),
            files,
            snapshots,
            facts,
        })
    }

    fn inspect_git(&self) -> Result<GitFacts, MinoError> {
        GitAdapter::new(&self.root)
            .inspect()
            .map_err(|error| map_git_error(&error))
    }
}

fn validate_new_commit_gate<'a>(plan: &'a Plan, task_id: &TaskId) -> Result<&'a Task, MinoError> {
    validate_common_plan_gate(plan, task_id, false)
}

fn validate_prepared_plan<'a>(
    plan: &'a Plan,
    intent: &GitCommitIntent,
) -> Result<&'a Task, MinoError> {
    if plan.id() != &intent.plan_id || plan.revision() < intent.plan_revision {
        return Err(MinoError::new(
            ErrorCategory::RevisionConflict,
            "Prepared commit intent does not belong to the current plan history",
        ));
    }
    let task = validate_common_plan_gate(plan, &intent.task_id, true)?;
    let gate = task.commit_gate().expect("validated gate exists");
    if gate.planned_message() != intent.message {
        return Err(MinoError::new(
            ErrorCategory::DriftDetected,
            "Prepared commit message differs from the current task gate",
        ));
    }
    Ok(task)
}

fn validate_common_plan_gate<'a>(
    plan: &'a Plan,
    task_id: &TaskId,
    allow_blocked_gate: bool,
) -> Result<&'a Task, MinoError> {
    validate_no_pending_amendment(plan)?;
    if plan.status() != PlanStatus::InProgress {
        return Err(MinoError::new(
            ErrorCategory::PolicyViolation,
            format!(
                "Task commit requires an In Progress plan, found {:?}",
                plan.status()
            ),
        ));
    }
    if !plan.has_plan_approval()
        || !plan.git_readiness().git_flow_enabled()
        || plan.git_readiness().git_flow_consent() != GitFlowConsent::Approved
    {
        return Err(MinoError::new(
            ErrorCategory::ApprovalRequired,
            "Task commit requires current plan approval and Approved Git Flow consent",
        ));
    }
    let task = plan.task(task_id).ok_or_else(|| {
        MinoError::new(
            ErrorCategory::IncompleteOrValidation,
            format!("Task {task_id} does not exist"),
        )
    })?;
    if task.status() != TaskStatus::Done {
        return Err(MinoError::new(
            ErrorCategory::PolicyViolation,
            format!("Task {task_id} must be Done before commit"),
        ));
    }
    let gate = task
        .commit_gate()
        .filter(|gate| gate.is_required())
        .ok_or_else(|| {
            MinoError::new(
                ErrorCategory::PolicyViolation,
                format!("Task {task_id} has no required commit gate"),
            )
        })?;
    let status_allowed = gate.status() == CommitStatus::Pending
        || allow_blocked_gate
            && matches!(
                gate.status(),
                CommitStatus::Blocked | CommitStatus::Committed
            );
    if !status_allowed {
        return Err(MinoError::new(
            ErrorCategory::PolicyViolation,
            format!("Task {task_id} commit gate is {:?}", gate.status()),
        ));
    }
    validate_commit_order(plan, task_id, gate)?;
    Ok(task)
}

fn validate_no_pending_amendment(plan: &Plan) -> Result<(), MinoError> {
    if let Some(amendment) = plan.pending_amendment() {
        return Err(MinoError::new(
            if amendment.classification() == crate::domain::AmendmentClassification::Material {
                ErrorCategory::ApprovalRequired
            } else {
                ErrorCategory::PolicyViolation
            },
            format!(
                "Task commit cannot continue while amendment {} awaits apply",
                amendment.id()
            ),
        ));
    }
    Ok(())
}

fn validate_commit_order(
    plan: &Plan,
    task_id: &TaskId,
    gate: &crate::domain::CommitGate,
) -> Result<(), MinoError> {
    let position = plan
        .task_order()
        .iter()
        .position(|candidate| candidate == task_id)
        .ok_or_else(|| {
            MinoError::new(
                ErrorCategory::DriftDetected,
                format!("Task {task_id} is missing from implementation order"),
            )
        })?;
    let prior_incomplete = plan.task_order()[..position].iter().find(|candidate| {
        plan.task(candidate).is_some_and(|candidate| {
            candidate.commit_gate().is_some_and(|candidate_gate| {
                candidate_gate.is_required() && candidate_gate.status() != CommitStatus::Committed
            })
        })
    });
    if let Some(candidate) = prior_incomplete {
        return Err(MinoError::new(
            ErrorCategory::PolicyViolation,
            format!("Task {candidate} must be committed before task {task_id}"),
        ));
    }
    if gate.status() != CommitStatus::Committed {
        let first_pending = plan.task_order()[position..]
            .iter()
            .filter_map(|candidate| plan.task(candidate))
            .find(|candidate| {
                candidate.commit_gate().is_some_and(|candidate_gate| {
                    candidate_gate.is_required()
                        && candidate_gate.status() != CommitStatus::Committed
                })
            });
        if first_pending.is_none_or(|candidate| candidate.id() != task_id) {
            return Err(MinoError::new(
                ErrorCategory::PolicyViolation,
                format!("Task {task_id} is not the first uncommitted required gate"),
            ));
        }
    }
    Ok(())
}

fn validate_live_identity(root: &Path, plan: &Plan, facts: &GitFacts) -> Result<(), MinoError> {
    if !facts.repository || !facts.is_worktree || facts.branch.is_none() || facts.head.is_none() {
        return Err(MinoError::new(
            ErrorCategory::PolicyViolation,
            "Task commits require a non-detached Git worktree with a committed HEAD",
        ));
    }
    let binding = ActiveBindingStore::new(root)
        .resolve(facts)
        .map_err(|error| map_git_error(&error))?;
    if binding.status != ActiveBindingStatus::Current
        || binding
            .binding
            .as_ref()
            .is_none_or(|binding| binding.plan_id != *plan.id())
    {
        return Err(MinoError::new(
            ErrorCategory::PolicyViolation,
            "Task commit requires a current same-worktree binding to the plan",
        ));
    }
    let branch = facts.branch.as_deref().expect("validated branch exists");
    if plan.git_readiness().branch() == Some(branch) {
        return Ok(());
    }
    let journal = GitBranchJournalStore::new(root)
        .load(plan.id())
        .map_err(|error| map_git_error(&error))?;
    if journal
        .is_some_and(|journal| journal.completion.is_some() && journal.intent.branch_name == branch)
    {
        Ok(())
    } else {
        Err(MinoError::new(
            ErrorCategory::PolicyViolation,
            format!(
                "Current branch {branch} is not authorized by the plan or a completed branch intent"
            ),
        ))
    }
}

fn validate_intent_identity(
    root: &Path,
    plan: &Plan,
    intent: &GitCommitIntent,
    facts: &GitFacts,
) -> Result<(), MinoError> {
    validate_live_identity(root, plan, facts)?;
    if facts.branch.as_deref() == Some(intent.branch.as_str())
        && normalized_path(facts.common_dir.as_deref()).as_deref()
            == Some(intent.common_dir.as_str())
        && normalized_path(facts.worktree.as_deref()).as_deref() == Some(intent.worktree.as_str())
    {
        Ok(())
    } else {
        Err(MinoError::new(
            ErrorCategory::DriftDetected,
            "Prepared commit worktree or branch identity is stale",
        ))
    }
}

fn expected_parent(
    plan: &Plan,
    task_id: &TaskId,
    live_head: Option<&str>,
) -> Result<String, MinoError> {
    let live_head = live_head.ok_or_else(|| {
        MinoError::new(
            ErrorCategory::PolicyViolation,
            "Task commit requires a current HEAD",
        )
    })?;
    let position = plan
        .task_order()
        .iter()
        .position(|candidate| candidate == task_id)
        .ok_or_else(|| {
            MinoError::new(
                ErrorCategory::DriftDetected,
                format!("Task {task_id} is missing from implementation order"),
            )
        })?;
    let prior_commit = plan.task_order()[..position]
        .iter()
        .filter_map(|prior_id| plan.task(prior_id))
        .filter_map(Task::commit_gate)
        .filter(|gate| gate.is_required())
        .map(|gate| {
            if gate.status() == CommitStatus::Committed {
                gate.actual_commit().map(str::to_owned).ok_or_else(|| {
                    MinoError::new(
                        ErrorCategory::DriftDetected,
                        "Committed prior task has no actual commit",
                    )
                })
            } else {
                Err(MinoError::new(
                    ErrorCategory::PolicyViolation,
                    "A prior required task commit is incomplete",
                ))
            }
        })
        .collect::<Result<Vec<_>, _>>()?
        .pop();
    if let Some(prior_commit) = prior_commit {
        if prior_commit.eq_ignore_ascii_case(live_head) {
            return Ok(live_head.to_ascii_lowercase());
        }
        return Err(MinoError::new(
            ErrorCategory::DriftDetected,
            "Current HEAD does not equal the preceding task commit",
        ));
    }
    let base = plan.git_readiness().base_commit().ok_or_else(|| {
        MinoError::new(
            ErrorCategory::PolicyViolation,
            "Task commit requires a captured Git base commit",
        )
    })?;
    if base.len() >= 4
        && base.len() <= live_head.len()
        && base.bytes().all(|byte| byte.is_ascii_hexdigit())
        && live_head
            .get(..base.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(base))
    {
        Ok(live_head.to_ascii_lowercase())
    } else {
        Err(MinoError::new(
            ErrorCategory::DriftDetected,
            "Current HEAD does not match the approved plan base commit",
        ))
    }
}

fn source_status_matches(facts: &GitFacts, intent: &GitCommitIntent) -> bool {
    let paths = intent.paths();
    facts.head.as_deref() == Some(intent.parent_head.as_str())
        && facts.branch.as_deref() == Some(intent.branch.as_str())
        && facts.staged_paths.is_empty()
        && facts.unstaged_paths == paths
        && facts.status.len() == intent.files.len()
        && facts
            .status
            .iter()
            .zip(&intent.files)
            .all(|(entry, snapshot)| {
                entry.path == snapshot.path
                    && entry.index_status == snapshot.index_status
                    && entry.worktree_status == snapshot.worktree_status
            })
        && verify_commit_snapshots(Path::new(&intent.worktree), &intent.files).is_ok()
}

fn validate_commit_object(
    intent: &GitCommitIntent,
    staged: &GitStagedCommit,
    commit: &GitCommitObject,
) -> Result<(), MinoError> {
    if commit.parent == intent.parent_head
        && commit.tree == staged.tree
        && commit.message == intent.message
        && commit.files == staged.files
    {
        Ok(())
    } else {
        Err(MinoError::new(
            ErrorCategory::DriftDetected,
            "Created commit does not match the prepared parent, tree, message, and files",
        ))
    }
}

fn commit_command(plan_id: &PlanId, task_id: &TaskId) -> Vec<String> {
    vec![
        "mino".to_owned(),
        "git".to_owned(),
        "commit".to_owned(),
        "--plan".to_owned(),
        plan_id.to_string(),
        "--task".to_owned(),
        task_id.to_string(),
        "--format".to_owned(),
        "json".to_owned(),
        "--no-input".to_owned(),
    ]
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

fn map_evidence_error(error: &EvidenceError) -> MinoError {
    let category = match error.kind() {
        EvidenceErrorKind::InvalidRequest
        | EvidenceErrorKind::PlanNotFound
        | EvidenceErrorKind::EvidenceNotFound => ErrorCategory::IncompleteOrValidation,
        EvidenceErrorKind::RevisionConflict | EvidenceErrorKind::RequestConflict => {
            ErrorCategory::RevisionConflict
        }
        EvidenceErrorKind::CorruptStore => ErrorCategory::DriftDetected,
        EvidenceErrorKind::Io
        | EvidenceErrorKind::Serialization
        | EvidenceErrorKind::LockTimeout => ErrorCategory::EnvironmentUnavailable,
    };
    MinoError::new(category, error.message())
}
