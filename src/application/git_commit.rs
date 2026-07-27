//! Plan-scoped commit preflight, staging, recovery, evidence, and gate recording.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::application::completion::{
    FreshnessScope, reconcile_stale_checks, validate_task_deviations, validate_task_evidence,
};
use crate::application::git_binding::map_git_error;
use crate::application::plan::{
    PlanMutationRequest, PlanOperationReport, PlanService, derived_request_id,
};
use crate::domain::{
    CheckStatus, CommitStatus, CriterionStatus, Evidence, EvidenceId, EvidenceType, FileChange,
    GitFlowConsent, Plan, PlanId, PlanStatus, RequestId, Task, TaskId, TaskStatus, Timestamp,
    VerificationCheck, WorkspaceRepositoryMode,
};
use crate::evidence::{
    AddEvidenceRequest, EvidenceError, EvidenceErrorKind, EvidenceRequestContext, EvidenceSource,
    EvidenceStore,
};
use crate::git::{
    ActiveBindingStatus, ActiveBindingStore, CommitFileSnapshot, GitAdapter, GitBranchJournalStore,
    GitCommitCompletion, GitCommitCompletionInput, GitCommitIntent, GitCommitJournal,
    GitCommitJournalStore, GitCommitObject, GitFacts, GitStagedCommit, capture_commit_snapshots,
    ensure_no_clean_filters, inspect_commit, matches_file_map_path, run_task_commit,
    stage_commit_paths, task_commit_entries, validate_commit_snapshot_scope,
    verify_commit_snapshots, write_index_tree,
};
use crate::runner::Redactor;
use crate::workspace::{
    WorkspaceDeltaEntry, WorkspaceDeltaKind, recapture_workspace_fingerprint, workspace_delta,
};
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

/// A caller-created commit verified and attached to one required task gate.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GitManualCommitReport {
    /// Verified immutable commit object.
    pub commit: GitCommitObject,
    /// Immutable commit evidence attached to the gate.
    pub evidence: Evidence,
    /// Plan mutation that recorded the terminal gate.
    pub plan: PlanOperationReport,
    /// Live Git identity proving the commit is current branch HEAD.
    pub facts: GitFacts,
}

/// An explicitly approved exception that satisfies one required task gate.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GitCommitSkipReport {
    /// Immutable accepted-exception evidence attached to the gate.
    pub evidence: Evidence,
    /// Plan mutation that recorded the skipped gate.
    pub plan: PlanOperationReport,
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

    /// Verifies and records a caller-created commit without mutating Git.
    ///
    /// # Errors
    ///
    /// Returns a typed error unless the commit is current branch HEAD, has the
    /// legal parent, exact planned message and scope, and contains the bytes
    /// covered by the task's current verification evidence.
    pub fn record_manual(
        &self,
        request: PlanMutationRequest,
        task_id: &TaskId,
        commit_id: &str,
        approval_reference: &str,
    ) -> Result<GitManualCommitReport, MinoError> {
        require_approval_reference(approval_reference)?;
        let current = self.plans.load_verified(&request.plan_id)?;
        let is_replay = mutation_is_replay(&current, &request)?;
        let task = validate_manual_commit_gate(&current, task_id, is_replay)?;
        let facts = self.inspect_git()?;
        validate_live_identity(&self.root, &current, &facts)?;
        let commit =
            inspect_commit(&self.root, commit_id).map_err(|error| map_git_error(&error))?;
        validate_manual_commit_identity(&current, task, &commit, &facts)?;
        let evidence = if is_replay {
            terminal_gate_evidence(&self.evidence, &current, task, EvidenceType::Commit)?
        } else {
            let evidence = self
                .evidence
                .list(current.id())
                .map_err(|error| map_evidence_error(&error))?;
            validate_manual_task_evidence(&self.root, &current, task, &evidence, &commit)?;
            validate_manual_commit_scope(&self.root, &current, task, &commit)?;
            self.add_manual_commit_evidence(&request, task_id, &commit, approval_reference)?
        };
        let commit_for_mutation = commit.clone();
        let task_for_mutation = task_id.clone();
        let evidence_for_mutation = evidence.id().clone();
        let plan = self.plans.commit_semantic(
            request,
            commit_changed_fields(task_id),
            |_| Ok(None),
            move |plan, at| {
                plan.record_task_commit(
                    &task_for_mutation,
                    &commit_for_mutation.commit,
                    commit_for_mutation.files.clone(),
                    evidence_for_mutation.clone(),
                    at,
                )
            },
        )?;
        Ok(GitManualCommitReport {
            commit,
            evidence,
            plan,
            facts,
        })
    }

    /// Records an explicitly approved exception for one required commit gate.
    ///
    /// # Errors
    ///
    /// Returns a typed error for stale verification, an illegal gate state,
    /// missing approval metadata, or a revision/request conflict.
    pub fn skip_gate(
        &self,
        request: PlanMutationRequest,
        task_id: &TaskId,
        approval_reference: &str,
        reason: &str,
    ) -> Result<GitCommitSkipReport, MinoError> {
        require_approval_reference(approval_reference)?;
        if reason.trim().is_empty() {
            return Err(MinoError::new(
                ErrorCategory::IncompleteOrValidation,
                "Commit-gate skip requires a non-empty reason",
            ));
        }
        let current = self.plans.load_verified(&request.plan_id)?;
        let is_replay = mutation_is_replay(&current, &request)?;
        let task = validate_skip_gate(&current, task_id, is_replay)?;
        let evidence = if is_replay {
            terminal_gate_evidence(
                &self.evidence,
                &current,
                task,
                EvidenceType::AcceptedException,
            )?
        } else {
            let existing = self
                .evidence
                .list(current.id())
                .map_err(|error| map_evidence_error(&error))?;
            if let Some((report, stale)) = reconcile_stale_checks(
                &self.plans,
                &self.root,
                &current,
                &existing,
                FreshnessScope::Task(task.id()),
                &request.actor,
                &request.updated_at,
            )? {
                return Err(stale_commit_error(&report, &stale));
            }
            validate_task_evidence(&self.root, &current, task, &existing)?;
            validate_task_deviations(&current, task)?;
            self.add_skip_evidence(&request, task_id, approval_reference, reason)?
        };
        let task_for_mutation = task_id.clone();
        let evidence_for_mutation = evidence.id().clone();
        let plan = self.plans.commit_semantic(
            request,
            vec![
                format!("tasks.{task_id}.commit_gate.status"),
                format!("tasks.{task_id}.commit_gate.evidence_refs"),
            ],
            |_| Ok(None),
            move |plan, at| {
                plan.skip_task_commit(&task_for_mutation, evidence_for_mutation.clone(), at)
            },
        )?;
        Ok(GitCommitSkipReport { evidence, plan })
    }

    fn add_manual_commit_evidence(
        &self,
        request: &PlanMutationRequest,
        task_id: &TaskId,
        commit: &GitCommitObject,
        approval_reference: &str,
    ) -> Result<Evidence, MinoError> {
        self.add_gate_evidence(
            request,
            task_id,
            EvidenceType::Commit,
            commit.commit.clone(),
            format!(
                "Manual task commit {} approved by {} with message {}",
                commit.commit, approval_reference, commit.message
            ),
        )
    }

    fn add_skip_evidence(
        &self,
        request: &PlanMutationRequest,
        task_id: &TaskId,
        approval_reference: &str,
        reason: &str,
    ) -> Result<Evidence, MinoError> {
        self.add_gate_evidence(
            request,
            task_id,
            EvidenceType::AcceptedException,
            approval_reference.to_owned(),
            format!("Approved commit-gate skip: {}", reason.trim()),
        )
    }

    fn add_gate_evidence(
        &self,
        request: &PlanMutationRequest,
        task_id: &TaskId,
        kind: EvidenceType,
        reference: String,
        description: String,
    ) -> Result<Evidence, MinoError> {
        let context = EvidenceRequestContext::new(
            request.plan_id.clone(),
            request.expected_revision,
            request.request_id.clone(),
            request.actor.clone(),
            request.command.clone(),
            request.updated_at.clone(),
        )
        .map_err(|error| map_evidence_error(&error))?;
        let evidence = AddEvidenceRequest::new(
            context,
            kind,
            EvidenceSource::Reference(reference),
            description,
        )
        .map_err(|error| map_evidence_error(&error))?
        .with_task(task_id.clone());
        self.evidence
            .add(&evidence, &Redactor::default())
            .map(|report| report.evidence().clone())
            .map_err(|error| map_evidence_error(&error))
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
        if let Some((report, stale)) = reconcile_stale_checks(
            &self.plans,
            &self.root,
            plan,
            &evidence,
            FreshnessScope::Task(task.id()),
            "mino",
            &Timestamp::now_utc(),
        )? {
            return Err(stale_commit_error(&report, &stale));
        }
        validate_task_evidence(&self.root, plan, task, &evidence)?;
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
    if !plan.has_plan_approval()
        || !plan.git_readiness().git_flow_enabled()
        || plan.git_readiness().git_flow_consent() != GitFlowConsent::Approved
    {
        return Err(MinoError::new(
            ErrorCategory::ApprovalRequired,
            "Task commit requires current plan approval and Approved Git Flow consent",
        ));
    }
    let mut allowed = vec![CommitStatus::Pending];
    if allow_blocked_gate {
        allowed.extend([CommitStatus::Blocked, CommitStatus::Committed]);
    }
    validate_task_commit_gate(plan, task_id, &allowed)
}

fn validate_manual_commit_gate<'a>(
    plan: &'a Plan,
    task_id: &TaskId,
    is_replay: bool,
) -> Result<&'a Task, MinoError> {
    let mut allowed = vec![CommitStatus::Pending, CommitStatus::Blocked];
    if is_replay {
        allowed.push(CommitStatus::Committed);
    }
    validate_task_commit_gate(plan, task_id, &allowed)
}

fn validate_skip_gate<'a>(
    plan: &'a Plan,
    task_id: &TaskId,
    is_replay: bool,
) -> Result<&'a Task, MinoError> {
    let mut allowed = vec![CommitStatus::Pending, CommitStatus::Blocked];
    if is_replay {
        allowed.push(CommitStatus::Skipped);
    }
    validate_task_commit_gate(plan, task_id, &allowed)
}

fn validate_task_commit_gate<'a>(
    plan: &'a Plan,
    task_id: &TaskId,
    allowed_statuses: &[CommitStatus],
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
    if !plan.has_plan_approval() {
        return Err(MinoError::new(
            ErrorCategory::ApprovalRequired,
            "Task commit requires current plan approval",
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
    if !allowed_statuses.contains(&gate.status()) {
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

fn mutation_is_replay(plan: &Plan, request: &PlanMutationRequest) -> Result<bool, MinoError> {
    let replay_revision = request.expected_revision.checked_add(1).ok_or_else(|| {
        MinoError::new(
            ErrorCategory::RevisionConflict,
            "Expected revision overflowed",
        )
    })?;
    if plan.revision() == request.expected_revision {
        Ok(false)
    } else if plan.revision() == replay_revision {
        Ok(true)
    } else {
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
}

fn require_approval_reference(reference: &str) -> Result<(), MinoError> {
    if reference.trim().is_empty() {
        Err(MinoError::new(
            ErrorCategory::ApprovalRequired,
            "Commit-gate mutation requires a non-empty approval reference",
        ))
    } else {
        Ok(())
    }
}

fn commit_changed_fields(task_id: &TaskId) -> Vec<String> {
    vec![
        format!("tasks.{task_id}.commit_gate.status"),
        format!("tasks.{task_id}.commit_gate.actual_commit"),
        format!("tasks.{task_id}.commit_gate.committed_files"),
        format!("tasks.{task_id}.commit_gate.evidence_refs"),
    ]
}

fn terminal_gate_evidence(
    store: &EvidenceStore,
    plan: &Plan,
    task: &Task,
    kind: EvidenceType,
) -> Result<Evidence, MinoError> {
    let gate = task.commit_gate().ok_or_else(|| {
        MinoError::new(
            ErrorCategory::DriftDetected,
            format!("Task {} has no commit gate", task.id()),
        )
    })?;
    let evidence_id = gate.evidence_refs().first().ok_or_else(|| {
        MinoError::new(
            ErrorCategory::DriftDetected,
            format!("Task {} terminal commit gate has no evidence", task.id()),
        )
    })?;
    store
        .list(plan.id())
        .map_err(|error| map_evidence_error(&error))?
        .into_iter()
        .find(|evidence| {
            evidence.id() == evidence_id
                && evidence.kind() == kind
                && evidence.task_id() == Some(task.id())
        })
        .ok_or_else(|| {
            MinoError::new(
                ErrorCategory::DriftDetected,
                format!(
                    "Task {} terminal commit-gate evidence is missing",
                    task.id()
                ),
            )
        })
}

fn validate_manual_commit_identity(
    plan: &Plan,
    task: &Task,
    commit: &GitCommitObject,
    facts: &GitFacts,
) -> Result<(), MinoError> {
    if facts.head.as_deref() != Some(commit.commit.as_str()) {
        return Err(MinoError::new(
            ErrorCategory::DriftDetected,
            "Manual commit must be the current branch HEAD",
        ));
    }
    if !facts.staged_paths.is_empty()
        || facts
            .unstaged_paths
            .iter()
            .any(|path| commit.files.contains(path))
    {
        return Err(MinoError::new(
            ErrorCategory::DriftDetected,
            "Manual commit paths must match the current index and worktree",
        ));
    }
    let gate = task.commit_gate().expect("validated commit gate exists");
    if commit.message != gate.planned_message() {
        return Err(MinoError::new(
            ErrorCategory::PolicyViolation,
            "Manual commit message differs from the exact planned message",
        ));
    }
    let expected = expected_parent(plan, task.id(), Some(&commit.parent))?;
    if expected != commit.parent {
        return Err(MinoError::new(
            ErrorCategory::DriftDetected,
            "Manual commit parent differs from the legal task parent",
        ));
    }
    Ok(())
}

fn validate_manual_commit_scope(
    root: &Path,
    plan: &Plan,
    task: &Task,
    commit: &GitCommitObject,
) -> Result<(), MinoError> {
    let workspace = plan
        .workspace_state()
        .map_err(|error| MinoError::new(ErrorCategory::DriftDetected, error.to_string()))?;
    let baseline = workspace.task_baseline(task.id()).ok_or_else(|| {
        MinoError::new(
            ErrorCategory::IncompleteOrValidation,
            format!(
                "Task {} has no recorded workspace start baseline",
                task.id()
            ),
        )
    })?;
    let delta = workspace_delta(root, plan, baseline)?;
    let delta_paths = delta
        .entries()
        .iter()
        .map(|entry| entry.path().to_owned())
        .collect::<Vec<_>>();
    if delta_paths != commit.files {
        return Err(MinoError::new(
            ErrorCategory::PolicyViolation,
            "Manual commit paths do not equal the task-local workspace delta",
        ));
    }
    let gate = task.commit_gate().expect("validated commit gate exists");
    let outside = delta.entries().iter().find(|entry| {
        !task.file_map().iter().any(|file| {
            matches_file_map_path(file.path(), entry.path())
                && manual_change_is_compatible(file.change(), entry)
        }) || !gate
            .scope()
            .iter()
            .any(|scope| matches_file_map_path(scope, entry.path()))
    });
    if let Some(entry) = outside {
        Err(MinoError::new(
            ErrorCategory::PolicyViolation,
            format!(
                "Manual commit path {} is outside File Map or Commit Scope",
                entry.path()
            ),
        ))
    } else {
        Ok(())
    }
}

fn manual_change_is_compatible(change: FileChange, entry: &WorkspaceDeltaEntry) -> bool {
    match change {
        FileChange::Create => entry.kind() == WorkspaceDeltaKind::Created,
        FileChange::Modify => entry.kind() == WorkspaceDeltaKind::Modified,
        FileChange::Delete => entry.kind() == WorkspaceDeltaKind::Deleted,
        FileChange::Test => true,
        FileChange::NotApplicable => false,
    }
}

fn validate_manual_task_evidence(
    root: &Path,
    plan: &Plan,
    task: &Task,
    evidence: &[Evidence],
    commit: &GitCommitObject,
) -> Result<(), MinoError> {
    let superseded = evidence
        .iter()
        .filter_map(Evidence::supersedes)
        .collect::<BTreeSet<_>>();
    for check in task
        .verification_checks()
        .iter()
        .filter(|check| check.is_required())
    {
        validate_manual_check(root, plan, task, check, evidence, &superseded, commit)?;
    }
    for criterion in task.acceptance_criteria() {
        let evidence_id = criterion.evidence_refs().last().ok_or_else(|| {
            MinoError::new(
                ErrorCategory::IncompleteOrValidation,
                format!("Criterion {} has no evidence", criterion.id()),
            )
        })?;
        let record = manual_evidence_by_id(plan, task, evidence, &superseded, evidence_id)?;
        match criterion.status() {
            CriterionStatus::AcceptedException
                if record.kind() == EvidenceType::AcceptedException
                    && record.criterion_id() == Some(criterion.id()) => {}
            CriterionStatus::Passed if record.kind() == EvidenceType::Command => {
                let check_id = record.check_id().ok_or_else(|| {
                    MinoError::new(
                        ErrorCategory::IncompleteOrValidation,
                        format!("Criterion {} command evidence has no check", criterion.id()),
                    )
                })?;
                let check = task
                    .verification_checks()
                    .iter()
                    .find(|check| check.id() == check_id)
                    .ok_or_else(|| {
                        MinoError::new(
                            ErrorCategory::IncompleteOrValidation,
                            format!("Criterion {} references a foreign check", criterion.id()),
                        )
                    })?;
                validate_manual_check(root, plan, task, check, evidence, &superseded, commit)?;
            }
            _ => {
                return Err(MinoError::new(
                    ErrorCategory::IncompleteOrValidation,
                    format!("Criterion {} evidence is incomplete", criterion.id()),
                ));
            }
        }
    }
    Ok(())
}

fn validate_manual_check(
    root: &Path,
    plan: &Plan,
    task: &Task,
    check: &VerificationCheck,
    evidence: &[Evidence],
    superseded: &BTreeSet<&EvidenceId>,
    commit: &GitCommitObject,
) -> Result<(), MinoError> {
    if check.status() != CheckStatus::Passed {
        return Err(MinoError::new(
            ErrorCategory::IncompleteOrValidation,
            format!("Check {} has not passed", check.id()),
        ));
    }
    let evidence_id = check.evidence_refs().last().ok_or_else(|| {
        MinoError::new(
            ErrorCategory::IncompleteOrValidation,
            format!("Check {} has no evidence", check.id()),
        )
    })?;
    let record = manual_evidence_by_id(plan, task, evidence, superseded, evidence_id)?;
    if record.kind() != EvidenceType::Command
        || record.check_id() != Some(check.id())
        || record.exit_code() != Some(check.expected_exit_code())
    {
        return Err(MinoError::new(
            ErrorCategory::IncompleteOrValidation,
            format!("Check {} evidence is incompatible", check.id()),
        ));
    }
    let fingerprint = record.workspace_fingerprint().ok_or_else(|| {
        MinoError::new(
            ErrorCategory::IncompleteOrValidation,
            format!("Check {} evidence has no workspace fingerprint", check.id()),
        )
    })?;
    let current = recapture_workspace_fingerprint(root, plan, fingerprint)?;
    let mismatch = if fingerprint.repository_mode() != WorkspaceRepositoryMode::Git
        || current.repository_mode() != WorkspaceRepositoryMode::Git
    {
        Some("repository mode")
    } else if fingerprint.head() != Some(commit.parent.as_str()) {
        Some("verified parent")
    } else if current.head() != Some(commit.commit.as_str()) {
        Some("current HEAD")
    } else if current.file_snapshots() != fingerprint.file_snapshots() {
        Some("verified file snapshots")
    } else {
        None
    };
    if let Some(mismatch) = mismatch {
        return Err(MinoError::new(
            ErrorCategory::IncompleteOrValidation,
            format!(
                "Check {} did not verify the manual commit {mismatch}",
                check.id()
            ),
        ));
    }
    Ok(())
}

fn manual_evidence_by_id<'a>(
    plan: &Plan,
    task: &Task,
    evidence: &'a [Evidence],
    superseded: &BTreeSet<&EvidenceId>,
    evidence_id: &EvidenceId,
) -> Result<&'a Evidence, MinoError> {
    let record = evidence
        .iter()
        .find(|record| record.id() == evidence_id)
        .ok_or_else(|| {
            MinoError::new(
                ErrorCategory::IncompleteOrValidation,
                format!("Evidence {evidence_id} is missing"),
            )
        })?;
    if superseded.contains(evidence_id)
        || plan.is_evidence_stale(evidence_id)
        || record.plan_id() != plan.id()
        || record.task_id() != Some(task.id())
        || record
            .captured_revision()
            .is_none_or(|revision| revision > plan.revision())
    {
        Err(MinoError::new(
            ErrorCategory::IncompleteOrValidation,
            format!("Evidence {evidence_id} is stale or incompatible"),
        ))
    } else {
        Ok(record)
    }
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
                candidate_gate.is_required() && !candidate_gate.is_satisfied()
            })
        })
    });
    if let Some(candidate) = prior_incomplete {
        return Err(MinoError::new(
            ErrorCategory::PolicyViolation,
            format!("Task {candidate} must be committed before task {task_id}"),
        ));
    }
    if !gate.is_satisfied() {
        let first_pending = plan.task_order()[position..]
            .iter()
            .filter_map(|candidate| plan.task(candidate))
            .find(|candidate| {
                candidate.commit_gate().is_some_and(|candidate_gate| {
                    candidate_gate.is_required() && !candidate_gate.is_satisfied()
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
    let workspace = plan
        .workspace_state()
        .map_err(|error| MinoError::new(ErrorCategory::DriftDetected, error.to_string()))?;
    if let Some(baseline) = workspace.task_baseline(task_id)
        && baseline.repository_mode() == WorkspaceRepositoryMode::Git
        && let Some(task_start_head) = baseline.head()
    {
        if task_start_head.eq_ignore_ascii_case(live_head) {
            return Ok(live_head.to_ascii_lowercase());
        }
        return Err(MinoError::new(
            ErrorCategory::DriftDetected,
            "Current HEAD does not match the task-start workspace baseline",
        ));
    }
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
    let mut prior_commit = None;
    for gate in plan.task_order()[..position]
        .iter()
        .filter_map(|prior_id| plan.task(prior_id))
        .filter_map(Task::commit_gate)
        .filter(|gate| gate.is_required())
    {
        match gate.status() {
            CommitStatus::Committed => {
                prior_commit = Some(gate.actual_commit().map(str::to_owned).ok_or_else(|| {
                    MinoError::new(
                        ErrorCategory::DriftDetected,
                        "Committed prior task has no actual commit",
                    )
                })?);
            }
            CommitStatus::Skipped => {}
            _ => {
                return Err(MinoError::new(
                    ErrorCategory::PolicyViolation,
                    "A prior required task commit is incomplete",
                ));
            }
        }
    }
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

fn stale_commit_error(
    report: &crate::application::plan::PlanOperationReport,
    stale: &[crate::domain::CheckId],
) -> MinoError {
    MinoError::new(
        ErrorCategory::IncompleteOrValidation,
        format!(
            "Task checks {} became Stale at plan revision {}; rerun verification before commit",
            stale
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", "),
            report.revision
        ),
    )
}
