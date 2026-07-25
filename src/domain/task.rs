//! Task, criterion, verification, file-map, and commit-gate entities.

use std::collections::BTreeSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{
    CheckId, CheckStatus, CommitStatus, CriterionId, CriterionStatus, DomainError, DomainErrorKind,
    DraftTaskInput, EvidenceId, TaskId, TaskStatus,
};

/// The planned change kind for a file-map entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub enum FileChange {
    /// Create a file that does not exist.
    Create,
    /// Modify an existing file.
    Modify,
    /// Delete an existing file.
    Delete,
    /// Add or change test-only content.
    Test,
    /// Record a path reference without a file mutation.
    #[serde(rename = "N/A")]
    NotApplicable,
}

/// A path and responsibility assigned to a task.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FileMapEntry {
    pub(crate) path: String,
    pub(crate) change: FileChange,
    pub(crate) reason: String,
    pub(crate) task_id: TaskId,
}

impl FileMapEntry {
    /// Creates one task-owned file responsibility.
    #[must_use]
    pub fn new(
        path: impl Into<String>,
        change: FileChange,
        reason: impl Into<String>,
        task_id: TaskId,
    ) -> Self {
        Self {
            path: path.into(),
            change,
            reason: reason.into(),
            task_id,
        }
    }

    /// Returns the project-relative path or narrow glob.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Returns the planned change kind.
    #[must_use]
    pub const fn change(&self) -> FileChange {
        self.change
    }

    /// Returns the task responsibility for this path.
    #[must_use]
    pub fn reason(&self) -> &str {
        &self.reason
    }

    /// Returns the task that owns this file responsibility.
    #[must_use]
    pub const fn task_id(&self) -> &TaskId {
        &self.task_id
    }
}

/// An observable condition that must be proven before task completion.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AcceptanceCriterion {
    pub(crate) id: CriterionId,
    pub(crate) description: String,
    pub(crate) status: CriterionStatus,
    pub(crate) evidence_refs: Vec<EvidenceId>,
}

impl AcceptanceCriterion {
    /// Creates a pending acceptance criterion without evidence.
    #[must_use]
    pub fn new(id: CriterionId, description: impl Into<String>) -> Self {
        Self {
            id,
            description: description.into(),
            status: CriterionStatus::Pending,
            evidence_refs: Vec::new(),
        }
    }

    /// Returns the stable criterion identifier.
    #[must_use]
    pub const fn id(&self) -> &CriterionId {
        &self.id
    }

    /// Returns the current criterion status.
    #[must_use]
    pub const fn status(&self) -> CriterionStatus {
        self.status
    }

    /// Returns the observable acceptance description.
    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }

    /// Returns evidence currently bound to the criterion.
    #[must_use]
    pub fn evidence_refs(&self) -> &[EvidenceId] {
        &self.evidence_refs
    }

    fn record_pass(&mut self, evidence_id: EvidenceId) -> Result<(), DomainError> {
        if self.evidence_refs.contains(&evidence_id) {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                format!(
                    "Criterion {} already references evidence {evidence_id}",
                    self.id
                ),
            ));
        }
        self.status = CriterionStatus::Passed;
        self.evidence_refs.push(evidence_id);
        Ok(())
    }

    fn reset_for_rework(&mut self) {
        self.status = CriterionStatus::Pending;
    }
}

/// A deterministic command and expected result used for verification.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VerificationCheck {
    pub(crate) id: CheckId,
    pub(crate) command: Vec<String>,
    pub(crate) cwd: String,
    pub(crate) expected_exit_code: i32,
    pub(crate) required: bool,
    pub(crate) status: CheckStatus,
    pub(crate) evidence_refs: Vec<EvidenceId>,
}

impl VerificationCheck {
    /// Creates a pending deterministic verification check.
    #[must_use]
    pub fn new(
        id: CheckId,
        command: Vec<String>,
        cwd: impl Into<String>,
        expected_exit_code: i32,
        required: bool,
    ) -> Self {
        Self {
            id,
            command,
            cwd: cwd.into(),
            expected_exit_code,
            required,
            status: CheckStatus::Pending,
            evidence_refs: Vec::new(),
        }
    }

    /// Returns the stable check identifier.
    #[must_use]
    pub const fn id(&self) -> &CheckId {
        &self.id
    }

    /// Returns the current check status.
    #[must_use]
    pub const fn status(&self) -> CheckStatus {
        self.status
    }

    /// Returns whether this check is required.
    #[must_use]
    pub const fn is_required(&self) -> bool {
        self.required
    }

    /// Returns the exact executable and argument vector.
    #[must_use]
    pub fn command(&self) -> &[String] {
        &self.command
    }

    /// Returns the project-relative working directory.
    #[must_use]
    pub fn cwd(&self) -> &str {
        &self.cwd
    }

    /// Returns the expected process exit code.
    #[must_use]
    pub const fn expected_exit_code(&self) -> i32 {
        self.expected_exit_code
    }

    /// Returns evidence currently bound to the check.
    #[must_use]
    pub fn evidence_refs(&self) -> &[EvidenceId] {
        &self.evidence_refs
    }

    pub(crate) fn record_pass(&mut self, evidence_id: EvidenceId) -> Result<(), DomainError> {
        if self.evidence_refs.contains(&evidence_id) {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                format!(
                    "Check {} already references evidence {evidence_id}",
                    self.id
                ),
            ));
        }
        self.status = CheckStatus::Passed;
        self.evidence_refs.push(evidence_id);
        Ok(())
    }

    pub(crate) fn begin_run(&mut self) -> Result<(), DomainError> {
        if matches!(self.status, CheckStatus::Running | CheckStatus::Blocked) {
            return Err(DomainError::new(
                DomainErrorKind::InvalidTransition,
                format!("Check {} cannot start from {:?}", self.id, self.status),
            ));
        }
        self.status = CheckStatus::Running;
        Ok(())
    }

    pub(crate) fn record_run(
        &mut self,
        evidence_id: EvidenceId,
        passed: bool,
    ) -> Result<(), DomainError> {
        if self.status != CheckStatus::Running {
            return Err(DomainError::new(
                DomainErrorKind::InvalidTransition,
                format!("Check {} is not Running", self.id),
            ));
        }
        if self.evidence_refs.contains(&evidence_id) {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                format!(
                    "Check {} already references evidence {evidence_id}",
                    self.id
                ),
            ));
        }
        self.status = if passed {
            CheckStatus::Passed
        } else {
            CheckStatus::Failed
        };
        self.evidence_refs.push(evidence_id);
        Ok(())
    }

    pub(crate) fn reset_for_rework(&mut self) {
        self.status = CheckStatus::Pending;
    }
}

/// The exact task-level commit policy declared by an approved plan.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommitGate {
    pub(crate) required: bool,
    pub(crate) status: CommitStatus,
    pub(crate) planned_message: String,
    pub(crate) scope: Vec<String>,
    pub(crate) actual_commit: Option<String>,
    pub(crate) committed_files: Vec<String>,
    pub(crate) evidence_refs: Vec<EvidenceId>,
}

impl CommitGate {
    /// Creates a pending task-level commit gate.
    #[must_use]
    pub fn new(required: bool, planned_message: impl Into<String>, scope: Vec<String>) -> Self {
        Self {
            required,
            status: if required {
                CommitStatus::Pending
            } else {
                CommitStatus::NotRequired
            },
            planned_message: planned_message.into(),
            scope,
            actual_commit: None,
            committed_files: Vec::new(),
            evidence_refs: Vec::new(),
        }
    }

    /// Returns whether the task requires a Git commit.
    #[must_use]
    pub const fn is_required(&self) -> bool {
        self.required
    }

    /// Returns the current commit-gate status.
    #[must_use]
    pub const fn status(&self) -> CommitStatus {
        self.status
    }

    /// Returns the exact planned Conventional Commit message.
    #[must_use]
    pub fn planned_message(&self) -> &str {
        &self.planned_message
    }

    /// Returns paths or globs allowed in the task commit.
    #[must_use]
    pub fn scope(&self) -> &[String] {
        &self.scope
    }
}

/// An ordered implementation unit inside a plan.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Task {
    id: TaskId,
    title: String,
    status: TaskStatus,
    resume_status: Option<TaskStatus>,
    depends_on: Vec<TaskId>,
    steps: Vec<String>,
    file_map: Vec<FileMapEntry>,
    acceptance_criteria: Vec<AcceptanceCriterion>,
    verification_checks: Vec<VerificationCheck>,
    commit_gate: Option<CommitGate>,
    evidence_refs: Vec<EvidenceId>,
    blocker: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UncheckedTask {
    id: TaskId,
    title: String,
    status: TaskStatus,
    resume_status: Option<TaskStatus>,
    depends_on: Vec<TaskId>,
    steps: Vec<String>,
    file_map: Vec<FileMapEntry>,
    acceptance_criteria: Vec<AcceptanceCriterion>,
    verification_checks: Vec<VerificationCheck>,
    commit_gate: Option<CommitGate>,
    evidence_refs: Vec<EvidenceId>,
    blocker: Option<String>,
}

impl TryFrom<UncheckedTask> for Task {
    type Error = DomainError;

    fn try_from(unchecked: UncheckedTask) -> Result<Self, Self::Error> {
        let task = Self {
            id: unchecked.id,
            title: unchecked.title,
            status: unchecked.status,
            resume_status: unchecked.resume_status,
            depends_on: unchecked.depends_on,
            steps: unchecked.steps,
            file_map: unchecked.file_map,
            acceptance_criteria: unchecked.acceptance_criteria,
            verification_checks: unchecked.verification_checks,
            commit_gate: unchecked.commit_gate,
            evidence_refs: unchecked.evidence_refs,
            blocker: unchecked.blocker,
        };
        task.validate_invariants()?;
        Ok(task)
    }
}

impl<'de> Deserialize<'de> for Task {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let unchecked = UncheckedTask::deserialize(deserializer)?;
        Self::try_from(unchecked).map_err(serde::de::Error::custom)
    }
}

impl Task {
    /// Creates a Draft task with an explicit dependency list.
    #[must_use]
    pub fn new(id: TaskId, title: impl Into<String>, depends_on: Vec<TaskId>) -> Self {
        Self {
            id,
            title: title.into(),
            status: TaskStatus::Draft,
            resume_status: None,
            depends_on,
            steps: Vec::new(),
            file_map: Vec::new(),
            acceptance_criteria: Vec::new(),
            verification_checks: Vec::new(),
            commit_gate: None,
            evidence_refs: Vec::new(),
            blocker: None,
        }
    }

    /// Builds a Draft task from strict authored input and deterministic identifiers.
    ///
    /// # Errors
    ///
    /// Returns an invariant error when a supplied task or criterion identifier
    /// differs from the deterministic next identifier, or authored content is invalid.
    pub fn from_draft(expected_id: &TaskId, input: DraftTaskInput) -> Result<Self, DomainError> {
        if input.id.as_ref().is_some_and(|id| id != expected_id) {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                format!("Expected task ID {expected_id}"),
            ));
        }
        let mut task = Self::new(expected_id.clone(), input.title, input.depends_on);
        for step in input.steps {
            task.add_step(step)?;
        }
        for file in input.files {
            task.add_file_map_entry(FileMapEntry::new(
                file.path,
                file.change,
                file.reason,
                expected_id.clone(),
            ))?;
        }
        for (index, criterion) in input.acceptance_criteria.into_iter().enumerate() {
            let number = index.checked_add(1).ok_or_else(|| {
                DomainError::new(
                    DomainErrorKind::InvariantViolation,
                    "Acceptance criterion count overflowed",
                )
            })?;
            let expected = CriterionId::parse(format!("{expected_id}-A{number}"))?;
            if criterion.id.as_ref().is_some_and(|id| id != &expected) {
                return Err(DomainError::new(
                    DomainErrorKind::InvariantViolation,
                    format!("Expected criterion ID {expected}"),
                ));
            }
            task.add_acceptance_criterion(AcceptanceCriterion::new(
                expected,
                criterion.description,
            ))?;
        }
        for verification in input.verification {
            task.add_verification_check(verification.into_check())?;
        }
        if let Some(commit_gate) = input.commit_gate {
            task.set_commit_gate(CommitGate::new(
                commit_gate.required,
                commit_gate.planned_message,
                commit_gate.scope,
            ))?;
        }
        task.validate_invariants()?;
        Ok(task)
    }

    /// Returns the stable task identifier.
    #[must_use]
    pub const fn id(&self) -> &TaskId {
        &self.id
    }

    /// Returns the task title.
    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Returns the current task status.
    #[must_use]
    pub const fn status(&self) -> TaskStatus {
        self.status
    }

    /// Returns the declared dependencies.
    #[must_use]
    pub fn dependencies(&self) -> &[TaskId] {
        &self.depends_on
    }

    /// Returns ordered authored implementation steps.
    #[must_use]
    pub fn steps(&self) -> &[String] {
        &self.steps
    }

    /// Returns file responsibilities owned by the task.
    #[must_use]
    pub fn file_map(&self) -> &[FileMapEntry] {
        &self.file_map
    }

    /// Returns the acceptance criteria.
    #[must_use]
    pub fn acceptance_criteria(&self) -> &[AcceptanceCriterion] {
        &self.acceptance_criteria
    }

    /// Returns the verification checks.
    #[must_use]
    pub fn verification_checks(&self) -> &[VerificationCheck] {
        &self.verification_checks
    }

    /// Returns the optional task-level Git commit gate.
    #[must_use]
    pub const fn commit_gate(&self) -> Option<&CommitGate> {
        self.commit_gate.as_ref()
    }

    /// Adds one non-empty implementation step while the task is Draft.
    ///
    /// # Errors
    ///
    /// Returns an error when the task is not Draft or the step is empty.
    pub fn add_step(&mut self, step: impl Into<String>) -> Result<(), DomainError> {
        if self.status != TaskStatus::Draft {
            return Err(self.invalid_transition("add an implementation step"));
        }
        let step = step.into();
        if step.trim().is_empty() {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                format!("Task {} cannot contain an empty step", self.id),
            ));
        }
        self.steps.push(step);
        Ok(())
    }

    /// Adds one file responsibility while the task is Draft.
    ///
    /// # Errors
    ///
    /// Returns an error when the task is not Draft, the entry is incomplete,
    /// belongs to another task, or duplicates an existing path.
    pub fn add_file_map_entry(&mut self, entry: FileMapEntry) -> Result<(), DomainError> {
        if self.status != TaskStatus::Draft {
            return Err(self.invalid_transition("add a file responsibility"));
        }
        if entry.task_id != self.id
            || entry.path.trim().is_empty()
            || entry.reason.trim().is_empty()
        {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                format!(
                    "Task {} received an incomplete or foreign file entry",
                    self.id
                ),
            ));
        }
        if self
            .file_map
            .iter()
            .any(|current| current.path == entry.path)
        {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                format!("Task {} already owns file path {}", self.id, entry.path),
            ));
        }
        self.file_map.push(entry);
        Ok(())
    }

    /// Adds a unique acceptance criterion while the task is Draft.
    ///
    /// # Errors
    ///
    /// Returns an error when the task is not Draft or the identifier is duplicated.
    pub fn add_acceptance_criterion(
        &mut self,
        criterion: AcceptanceCriterion,
    ) -> Result<(), DomainError> {
        if self.status != TaskStatus::Draft {
            return Err(self.invalid_transition("add an acceptance criterion"));
        }
        if criterion.description.trim().is_empty() {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                format!("Task {} cannot contain an empty criterion", self.id),
            ));
        }
        if self
            .acceptance_criteria
            .iter()
            .any(|current| current.id == criterion.id)
        {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                format!("Task {} already has criterion {}", self.id, criterion.id),
            ));
        }
        self.acceptance_criteria.push(criterion);
        Ok(())
    }

    /// Adds a unique verification check while the task is Draft.
    ///
    /// # Errors
    ///
    /// Returns an error when the task is not Draft or the identifier is duplicated.
    pub fn add_verification_check(&mut self, check: VerificationCheck) -> Result<(), DomainError> {
        if self.status != TaskStatus::Draft {
            return Err(self.invalid_transition("add a verification check"));
        }
        if check.command.is_empty()
            || check.command.iter().any(|part| part.trim().is_empty())
            || check.cwd.trim().is_empty()
        {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                format!("Task {} contains an incomplete verification check", self.id),
            ));
        }
        if self
            .verification_checks
            .iter()
            .any(|current| current.id == check.id)
        {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                format!("Task {} already has check {}", self.id, check.id),
            ));
        }
        self.verification_checks.push(check);
        Ok(())
    }

    /// Sets the task-level commit gate while the task is Draft.
    ///
    /// # Errors
    ///
    /// Returns an error when the task is not Draft or already has a gate.
    pub fn set_commit_gate(&mut self, commit_gate: CommitGate) -> Result<(), DomainError> {
        if self.status != TaskStatus::Draft {
            return Err(self.invalid_transition("set a commit gate"));
        }
        if self.commit_gate.is_some() {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                format!("Task {} already has a commit gate", self.id),
            ));
        }
        self.commit_gate = Some(commit_gate);
        Ok(())
    }

    pub(crate) fn mark_ready(&mut self) -> Result<(), DomainError> {
        if self.status != TaskStatus::Draft {
            return Err(self.invalid_transition("mark ready"));
        }
        if self
            .depends_on
            .iter()
            .any(|dependency| dependency == &self.id)
        {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                format!("Task {} depends on itself", self.id),
            ));
        }
        if self.depends_on.iter().collect::<BTreeSet<_>>().len() != self.depends_on.len() {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                format!("Task {} contains duplicate dependencies", self.id),
            ));
        }
        self.validate_execution_definition()?;
        self.status = TaskStatus::Ready;
        Ok(())
    }

    pub(crate) fn start(&mut self) -> Result<(), DomainError> {
        if self.status != TaskStatus::Ready {
            return Err(self.invalid_transition("start"));
        }
        self.status = TaskStatus::InProgress;
        Ok(())
    }

    pub(crate) fn complete(&mut self) -> Result<(), DomainError> {
        if self.status != TaskStatus::InProgress {
            return Err(self.invalid_transition("complete"));
        }
        if !self.completion_evidence_is_satisfied() {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                format!(
                    "Task {} cannot complete while criteria or required checks are incomplete",
                    self.id
                ),
            ));
        }
        self.status = TaskStatus::Done;
        Ok(())
    }

    pub(crate) fn block(&mut self, reason: impl Into<String>) -> Result<(), DomainError> {
        if self.status != TaskStatus::InProgress {
            return Err(self.invalid_transition("block"));
        }
        self.resume_status = Some(self.status);
        self.status = TaskStatus::Blocked;
        self.blocker = Some(reason.into());
        Ok(())
    }

    pub(crate) fn resume(&mut self) -> Result<(), DomainError> {
        if self.status != TaskStatus::Blocked {
            return Err(self.invalid_transition("resume"));
        }
        let resume_status = self.resume_status.take().ok_or_else(|| {
            DomainError::new(
                DomainErrorKind::InvariantViolation,
                format!("Blocked task {} has no resume status", self.id),
            )
        })?;
        if resume_status != TaskStatus::InProgress {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                format!("Task {} has an invalid resume status", self.id),
            ));
        }
        self.status = resume_status;
        self.blocker = None;
        Ok(())
    }

    pub(crate) fn reopen_for_rework(&mut self) -> Result<(), DomainError> {
        if self.status != TaskStatus::Done {
            return Err(self.invalid_transition("reopen for rework"));
        }
        for criterion in &mut self.acceptance_criteria {
            criterion.reset_for_rework();
        }
        for check in &mut self.verification_checks {
            check.reset_for_rework();
        }
        self.status = TaskStatus::Ready;
        Ok(())
    }

    pub(crate) fn record_criterion_pass(
        &mut self,
        criterion_id: &CriterionId,
        evidence_id: EvidenceId,
    ) -> Result<(), DomainError> {
        if self.status != TaskStatus::InProgress {
            return Err(self.invalid_transition("record criterion evidence"));
        }
        let criterion = self
            .acceptance_criteria
            .iter_mut()
            .find(|criterion| &criterion.id == criterion_id)
            .ok_or_else(|| {
                DomainError::new(
                    DomainErrorKind::InvariantViolation,
                    format!("Task {} has no criterion {criterion_id}", self.id),
                )
            })?;
        criterion.record_pass(evidence_id)
    }

    pub(crate) fn record_check_pass(
        &mut self,
        check_id: &CheckId,
        evidence_id: EvidenceId,
    ) -> Result<(), DomainError> {
        if self.status != TaskStatus::InProgress {
            return Err(self.invalid_transition("record check evidence"));
        }
        let check = self
            .verification_checks
            .iter_mut()
            .find(|check| &check.id == check_id)
            .ok_or_else(|| {
                DomainError::new(
                    DomainErrorKind::InvariantViolation,
                    format!("Task {} has no check {check_id}", self.id),
                )
            })?;
        check.record_pass(evidence_id)
    }

    pub(crate) fn begin_check_run(&mut self, check_id: &CheckId) -> Result<(), DomainError> {
        if self.status != TaskStatus::InProgress {
            return Err(self.invalid_transition("run a verification check"));
        }
        let check = self
            .verification_checks
            .iter_mut()
            .find(|check| &check.id == check_id)
            .ok_or_else(|| {
                DomainError::new(
                    DomainErrorKind::InvariantViolation,
                    format!("Task {} has no check {check_id}", self.id),
                )
            })?;
        check.begin_run()
    }

    pub(crate) fn record_check_run(
        &mut self,
        check_id: &CheckId,
        evidence_id: EvidenceId,
        passed: bool,
    ) -> Result<(), DomainError> {
        if self.status != TaskStatus::InProgress {
            return Err(self.invalid_transition("record a verification check run"));
        }
        let check = self
            .verification_checks
            .iter_mut()
            .find(|check| &check.id == check_id)
            .ok_or_else(|| {
                DomainError::new(
                    DomainErrorKind::InvariantViolation,
                    format!("Task {} has no check {check_id}", self.id),
                )
            })?;
        check.record_run(evidence_id, passed)
    }

    pub(crate) fn validate_invariants(&self) -> Result<(), DomainError> {
        if self.title.trim().is_empty() {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                format!("Task {} has an empty title", self.id),
            ));
        }
        if self.status != TaskStatus::Draft {
            if self
                .depends_on
                .iter()
                .any(|dependency| dependency == &self.id)
            {
                return Err(DomainError::new(
                    DomainErrorKind::InvariantViolation,
                    format!("Task {} depends on itself", self.id),
                ));
            }
            if self.depends_on.iter().collect::<BTreeSet<_>>().len() != self.depends_on.len() {
                return Err(DomainError::new(
                    DomainErrorKind::InvariantViolation,
                    format!("Task {} contains duplicate dependencies", self.id),
                ));
            }
        }
        if self.steps.iter().any(|step| step.trim().is_empty()) {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                format!("Task {} contains an empty step", self.id),
            ));
        }
        let file_paths = self
            .file_map
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<BTreeSet<_>>();
        if file_paths.len() != self.file_map.len()
            || self.file_map.iter().any(|entry| {
                entry.task_id != self.id
                    || entry.path.trim().is_empty()
                    || entry.reason.trim().is_empty()
            })
        {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                format!("Task {} contains an invalid file map", self.id),
            ));
        }
        let criterion_ids = self
            .acceptance_criteria
            .iter()
            .map(|criterion| &criterion.id)
            .collect::<BTreeSet<_>>();
        if criterion_ids.len() != self.acceptance_criteria.len() {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                format!("Task {} has duplicate criterion identifiers", self.id),
            ));
        }

        let check_ids = self
            .verification_checks
            .iter()
            .map(|check| &check.id)
            .collect::<BTreeSet<_>>();
        if check_ids.len() != self.verification_checks.len() {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                format!("Task {} has duplicate check identifiers", self.id),
            ));
        }
        if self.status != TaskStatus::Draft {
            self.validate_execution_definition()?;
        }
        self.validate_running_checks()?;
        if self.status == TaskStatus::Blocked {
            if self.resume_status != Some(TaskStatus::InProgress)
                || self.blocker.as_deref().is_none_or(str::is_empty)
            {
                return Err(DomainError::new(
                    DomainErrorKind::InvariantViolation,
                    format!(
                        "Blocked task {} requires an In Progress resume state and reason",
                        self.id
                    ),
                ));
            }
        } else if self.resume_status.is_some() || self.blocker.is_some() {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                format!(
                    "Only a Blocked task {} may retain resume state or a blocker",
                    self.id
                ),
            ));
        }
        self.validate_completion_state()?;
        Ok(())
    }

    fn validate_completion_state(&self) -> Result<(), DomainError> {
        if self.status == TaskStatus::Done && !self.completion_evidence_is_satisfied() {
            Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                format!("Done task {} lacks complete evidence", self.id),
            ))
        } else {
            Ok(())
        }
    }

    fn validate_running_checks(&self) -> Result<(), DomainError> {
        if self.status != TaskStatus::InProgress
            && self
                .verification_checks
                .iter()
                .any(|check| check.status == CheckStatus::Running)
        {
            Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                format!(
                    "Only an In Progress task may contain a Running check: {}",
                    self.id
                ),
            ))
        } else {
            Ok(())
        }
    }

    fn validate_execution_definition(&self) -> Result<(), DomainError> {
        if self.acceptance_criteria.is_empty() || self.verification_checks.is_empty() {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                format!(
                    "Task {} requires at least one criterion and verification check",
                    self.id
                ),
            ));
        }
        if self
            .verification_checks
            .iter()
            .any(|check| check.command.is_empty() || check.cwd.trim().is_empty())
        {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                format!("Task {} contains an incomplete verification check", self.id),
            ));
        }
        Ok(())
    }

    fn completion_evidence_is_satisfied(&self) -> bool {
        !self.acceptance_criteria.is_empty()
            && !self.verification_checks.is_empty()
            && self.acceptance_criteria.iter().all(|criterion| {
                matches!(
                    criterion.status,
                    CriterionStatus::Passed | CriterionStatus::AcceptedException
                ) && !criterion.evidence_refs.is_empty()
            })
            && self.verification_checks.iter().all(|check| {
                !check.required
                    || (matches!(check.status, CheckStatus::Passed)
                        && !check.evidence_refs.is_empty())
            })
    }

    fn invalid_transition(&self, action: &'static str) -> DomainError {
        DomainError::new(
            DomainErrorKind::InvalidTransition,
            format!(
                "Task {} cannot {action} from status {:?}",
                self.id, self.status
            ),
        )
    }
}
