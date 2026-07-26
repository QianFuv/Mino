//! Branch-name validation, hook-disabled creation, and immutable intent journals.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use fs4::{FileExt, TryLockError};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::domain::{PlanId, Timestamp};
use crate::managed_fs::{ManagedFsError, ManagedFsErrorKind, ManagedPath, ProjectFs};

use super::command::{run_mutating, run_read_only};
use super::{GitError, GitErrorKind};

const BRANCH_JOURNAL_VERSION: u32 = 1;
const BRANCH_LOCK_TIMEOUT: Duration = Duration::from_secs(2);
const BRANCH_LOCK_RETRY: Duration = Duration::from_millis(10);
const MAX_BRANCH_JOURNAL_BYTES: u64 = 1024 * 1024;
static NEXT_BRANCH_FILE: AtomicU64 = AtomicU64::new(1);

/// Immutable declaration prepared before an approved branch mutation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitBranchIntent {
    version: u32,
    /// Plan authorized for the branch operation.
    pub plan_id: PlanId,
    /// Plan revision observed before the operation.
    pub plan_revision: u64,
    /// Canonical common Git directory.
    pub common_dir: String,
    /// Canonical worktree root.
    pub worktree: String,
    /// Source branch, absent when the approved base is detached.
    pub source_branch: Option<String>,
    /// Exact full commit from which the branch must be created.
    pub base_head: String,
    /// Deterministically proposed local branch name.
    pub branch_name: String,
    /// Auditable external approval reference.
    pub approval_reference: String,
    /// Time at which the immutable intent was prepared.
    pub prepared_at: Timestamp,
}

impl GitBranchIntent {
    /// Creates one validated prepared branch intent.
    ///
    /// # Errors
    ///
    /// Returns an invalid-output error when an identity, revision, commit, or
    /// approval field is incomplete or inconsistent.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        plan_id: PlanId,
        plan_revision: u64,
        common_dir: String,
        worktree: String,
        source_branch: Option<String>,
        base_head: String,
        branch_name: String,
        approval_reference: String,
        prepared_at: Timestamp,
    ) -> Result<Self, GitError> {
        let intent = Self {
            version: BRANCH_JOURNAL_VERSION,
            plan_id,
            plan_revision,
            common_dir,
            worktree,
            source_branch,
            base_head,
            branch_name,
            approval_reference,
            prepared_at,
        };
        validate_intent(&intent)?;
        Ok(intent)
    }
}

/// Immutable result published after branch creation and active binding succeed.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitBranchCompletion {
    version: u32,
    /// Plan whose branch operation completed.
    pub plan_id: PlanId,
    /// Created local branch name.
    pub branch_name: String,
    /// Exact commit at branch creation.
    pub head: String,
    /// Completion timestamp.
    pub completed_at: Timestamp,
}

impl GitBranchCompletion {
    /// Creates a terminal branch-operation record bound to one intent.
    ///
    /// # Errors
    ///
    /// Returns an invalid-output error when the result does not identify the
    /// prepared plan, branch, and base commit.
    pub fn new(
        intent: &GitBranchIntent,
        head: String,
        completed_at: Timestamp,
    ) -> Result<Self, GitError> {
        let completion = Self {
            version: BRANCH_JOURNAL_VERSION,
            plan_id: intent.plan_id.clone(),
            branch_name: intent.branch_name.clone(),
            head,
            completed_at,
        };
        validate_completion(intent, &completion)?;
        Ok(completion)
    }
}

/// Current immutable branch-operation journal for one plan.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GitBranchJournal {
    /// Prepared intent when the operation began.
    pub intent: GitBranchIntent,
    /// Terminal result when binding and publication completed.
    pub completion: Option<GitBranchCompletion>,
}

/// Outcome returned by the single allowed branch mutation command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitBranchCommandResult {
    /// Whether Git returned a successful status.
    pub success: bool,
    /// Process exit code when the operating system supplied one.
    pub exit_code: Option<i32>,
    /// Diagnostic stderr decoded lossily for a refusal report.
    pub stderr: String,
}

/// Store for immutable prepared and completed branch-operation records.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitBranchJournalStore {
    project_root: PathBuf,
}

impl GitBranchJournalStore {
    /// Creates a journal store under one initialized project.
    #[must_use]
    pub fn new(project_root: impl Into<PathBuf>) -> Self {
        Self {
            project_root: project_root.into(),
        }
    }

    /// Returns the prepared intent path for one plan.
    #[must_use]
    pub fn intent_path(&self, plan_id: &PlanId) -> PathBuf {
        self.operation_directory(plan_id).join("intent.json")
    }

    /// Returns the terminal result path for one plan.
    #[must_use]
    pub fn completion_path(&self, plan_id: &PlanId) -> PathBuf {
        self.operation_directory(plan_id).join("completion.json")
    }

    /// Loads one journal without creating files or directories.
    ///
    /// # Errors
    ///
    /// Returns an invalid-output or environment error for malformed,
    /// unsupported, oversized, or unreadable journal state.
    pub fn load(&self, plan_id: &PlanId) -> Result<Option<GitBranchJournal>, GitError> {
        let filesystem = self.filesystem()?;
        let intent_path = Self::intent_managed(plan_id);
        let completion_path = Self::completion_managed(plan_id);
        if !filesystem.exists(&intent_path).map_err(managed_git_error)? {
            if filesystem
                .exists(&completion_path)
                .map_err(managed_git_error)?
            {
                return Err(invalid(
                    "Branch completion exists without a prepared intent",
                ));
            }
            return Ok(None);
        }
        let intent: GitBranchIntent = read_json(&filesystem, &intent_path)?;
        validate_intent(&intent)?;
        if &intent.plan_id != plan_id {
            return Err(invalid("Branch intent path and plan identity disagree"));
        }
        let completion = if filesystem
            .exists(&completion_path)
            .map_err(managed_git_error)?
        {
            Some(read_json(&filesystem, &completion_path)?)
        } else {
            None
        };
        if let Some(completion) = &completion {
            validate_completion(&intent, completion)?;
        }
        Ok(Some(GitBranchJournal { intent, completion }))
    }

    /// Acquires the bounded project branch-operation lock.
    ///
    /// # Errors
    ///
    /// Returns an unavailable error for an uninitialized project, unsafe
    /// directory state, lock I/O failure, or lock timeout.
    pub fn lock(&self) -> Result<GitBranchJournalLock, GitError> {
        let filesystem = self.filesystem()?;
        let mino_directory = managed_path(".mino");
        if !filesystem
            .is_directory(&mino_directory)
            .map_err(managed_git_error)?
        {
            return Err(unavailable(format!(
                "Mino state directory {} is missing",
                filesystem.display_path(&mino_directory).display()
            )));
        }
        let git_directory = mino_directory.join("git").map_err(managed_git_error)?;
        filesystem
            .ensure_directory(&git_directory)
            .map_err(managed_git_error)?;
        let lock_path = git_directory
            .join("branch.lock")
            .map_err(managed_git_error)?;
        GitBranchJournalLock::acquire(&filesystem, &lock_path)
    }

    /// Publishes a new intent or replays the semantically identical intent.
    ///
    /// # Errors
    ///
    /// Returns a policy error when a different operation already owns the
    /// plan journal, or an environment error when publication fails.
    pub fn prepare(&self, candidate: GitBranchIntent) -> Result<GitBranchIntent, GitError> {
        let filesystem = self.filesystem()?;
        validate_intent(&candidate)?;
        if let Some(existing) = self.load(&candidate.plan_id)? {
            if same_intent(&existing.intent, &candidate) {
                return Ok(existing.intent);
            }
            return Err(GitError::new(
                GitErrorKind::PolicyViolation,
                format!(
                    "Plan {} already has a different prepared branch operation",
                    candidate.plan_id
                ),
            ));
        }
        let directory = Self::operation_managed(&candidate.plan_id);
        filesystem
            .ensure_directory(&directory)
            .map_err(managed_git_error)?;
        write_new_json(
            &filesystem,
            &Self::intent_managed(&candidate.plan_id),
            &candidate,
        )?;
        Ok(candidate)
    }

    /// Publishes or replays the terminal result for one prepared intent.
    ///
    /// # Errors
    ///
    /// Returns an invalid-output error for a missing/mismatched intent or an
    /// environment error when immutable result publication fails.
    pub fn complete(
        &self,
        intent: &GitBranchIntent,
        candidate: GitBranchCompletion,
    ) -> Result<GitBranchCompletion, GitError> {
        let filesystem = self.filesystem()?;
        validate_completion(intent, &candidate)?;
        let journal = self
            .load(&intent.plan_id)?
            .ok_or_else(|| invalid("Branch completion has no prepared intent"))?;
        if !same_intent(&journal.intent, intent) {
            return Err(invalid(
                "Branch completion intent does not match stored state",
            ));
        }
        if let Some(existing) = journal.completion {
            if same_completion(&existing, &candidate) {
                return Ok(existing);
            }
            return Err(invalid(
                "Stored branch completion conflicts with observed result",
            ));
        }
        write_new_json(
            &filesystem,
            &Self::completion_managed(&intent.plan_id),
            &candidate,
        )?;
        Ok(candidate)
    }

    fn operation_directory(&self, plan_id: &PlanId) -> PathBuf {
        self.project_root
            .join(".mino/git/branches")
            .join(plan_id.as_str())
    }

    fn operation_managed(plan_id: &PlanId) -> ManagedPath {
        ManagedPath::new(PathBuf::from(".mino/git/branches").join(plan_id.as_str()))
            .expect("validated plan ID should form a managed branch journal path")
    }

    fn intent_managed(plan_id: &PlanId) -> ManagedPath {
        Self::operation_managed(plan_id)
            .join("intent.json")
            .expect("static branch intent name should form a managed path")
    }

    fn completion_managed(plan_id: &PlanId) -> ManagedPath {
        Self::operation_managed(plan_id)
            .join("completion.json")
            .expect("static branch completion name should form a managed path")
    }

    fn filesystem(&self) -> Result<ProjectFs, GitError> {
        ProjectFs::open(&self.project_root).map_err(managed_git_error)
    }
}

/// Held advisory lock for one project branch operation.
#[derive(Debug)]
pub struct GitBranchJournalLock {
    file: std::fs::File,
}

impl GitBranchJournalLock {
    fn acquire(filesystem: &ProjectFs, path: &ManagedPath) -> Result<Self, GitError> {
        let file = filesystem.open_lock_file(path).map_err(managed_git_error)?;
        let started_at = Instant::now();
        loop {
            match FileExt::try_lock(&file) {
                Ok(()) => return Ok(Self { file }),
                Err(TryLockError::WouldBlock) if started_at.elapsed() < BRANCH_LOCK_TIMEOUT => {
                    thread::sleep(BRANCH_LOCK_RETRY);
                }
                Err(TryLockError::WouldBlock) => {
                    return Err(unavailable(format!(
                        "Timed out acquiring branch-operation lock {}",
                        filesystem.display_path(path).display()
                    )));
                }
                Err(TryLockError::Error(error)) => {
                    return Err(path_error(
                        "lock branch operation",
                        &filesystem.display_path(path),
                        &error,
                    ));
                }
            }
        }
    }
}

impl Drop for GitBranchJournalLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

/// Returns the deterministic local branch proposed for one plan.
#[must_use]
pub fn proposed_branch_name(plan_id: &PlanId) -> String {
    format!("mino/{plan_id}")
}

/// Validates one local branch name with Git's own branch rules.
///
/// # Errors
///
/// Returns a policy error when Git rejects the name and an unavailable error
/// when the validation command cannot run.
pub fn validate_branch_name(root: &Path, branch_name: &str) -> Result<(), GitError> {
    if branch_name.is_empty()
        || branch_name.len() > 256
        || branch_name.chars().any(char::is_control)
    {
        return Err(policy("Proposed Git branch name is invalid"));
    }
    let output = run_read_only(root, ["check-ref-format", "--branch", branch_name])?;
    if !output.success {
        return Err(policy(format!(
            "Git rejected proposed branch name {branch_name}"
        )));
    }
    let returned = std::str::from_utf8(&output.stdout)
        .map_err(|_| invalid("Git branch-name validation returned non-UTF-8 output"))?
        .trim_end_matches(['\r', '\n']);
    if returned == branch_name {
        Ok(())
    } else {
        Err(invalid(
            "Git branch-name validation changed the proposed name",
        ))
    }
}

/// Returns the full target object ID of an exact local branch, when it exists.
///
/// # Errors
///
/// Returns an unavailable or invalid-output error for unexpected Git results.
pub fn local_branch_target(root: &Path, branch_name: &str) -> Result<Option<String>, GitError> {
    validate_branch_name(root, branch_name)?;
    let reference = format!("refs/heads/{branch_name}");
    let revision = format!("{reference}^{{commit}}");
    let output = run_read_only(
        root,
        [
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            &revision,
        ],
    )?;
    if !output.success {
        return if output.exit_code == Some(1) {
            Ok(None)
        } else {
            Err(unavailable(format!(
                "Git could not inspect local branch {branch_name}: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )))
        };
    }
    let target = std::str::from_utf8(&output.stdout)
        .map_err(|_| invalid("Git branch target is not valid UTF-8"))?
        .trim_end_matches(['\r', '\n']);
    if is_object_id(target) {
        Ok(Some(target.to_owned()))
    } else {
        Err(invalid("Git branch target is not a full object ID"))
    }
}

/// Creates and switches to one validated local branch at an exact commit.
///
/// Repository hooks are disabled for this operation. The caller must perform
/// all policy checks and post-command reconciliation.
///
/// # Errors
///
/// Returns an unavailable error when Git cannot be started or an invalid-output
/// error when its bounded output contract is violated.
pub fn create_and_switch_branch(
    root: &Path,
    branch_name: &str,
    base_head: &str,
) -> Result<GitBranchCommandResult, GitError> {
    validate_branch_name(root, branch_name)?;
    if !is_object_id(base_head) {
        return Err(invalid("Branch base must be a full Git object ID"));
    }
    let output = run_mutating(
        root,
        [
            "-c",
            "core.hooksPath=/dev/null",
            "switch",
            "--quiet",
            "--no-guess",
            "--no-track",
            "-c",
            branch_name,
            base_head,
        ],
    )?;
    Ok(GitBranchCommandResult {
        success: output.success,
        exit_code: output.exit_code,
        stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
    })
}

fn validate_intent(intent: &GitBranchIntent) -> Result<(), GitError> {
    if intent.version != BRANCH_JOURNAL_VERSION
        || intent.plan_revision == 0
        || intent.common_dir.is_empty()
        || intent.worktree.is_empty()
        || intent.source_branch.as_deref().is_some_and(str::is_empty)
        || !is_object_id(&intent.base_head)
        || intent.branch_name != proposed_branch_name(&intent.plan_id)
        || !valid_approval_reference(&intent.approval_reference)
    {
        Err(invalid(
            "Prepared branch intent is incomplete or inconsistent",
        ))
    } else {
        Ok(())
    }
}

fn validate_completion(
    intent: &GitBranchIntent,
    completion: &GitBranchCompletion,
) -> Result<(), GitError> {
    if completion.version != BRANCH_JOURNAL_VERSION
        || completion.plan_id != intent.plan_id
        || completion.branch_name != intent.branch_name
        || completion.head != intent.base_head
        || !is_object_id(&completion.head)
    {
        Err(invalid(
            "Branch completion does not match its prepared intent",
        ))
    } else {
        Ok(())
    }
}

fn same_intent(left: &GitBranchIntent, right: &GitBranchIntent) -> bool {
    left.version == right.version
        && left.plan_id == right.plan_id
        && left.plan_revision == right.plan_revision
        && left.common_dir == right.common_dir
        && left.worktree == right.worktree
        && left.source_branch == right.source_branch
        && left.base_head == right.base_head
        && left.branch_name == right.branch_name
        && left.approval_reference == right.approval_reference
}

fn same_completion(left: &GitBranchCompletion, right: &GitBranchCompletion) -> bool {
    left.version == right.version
        && left.plan_id == right.plan_id
        && left.branch_name == right.branch_name
        && left.head == right.head
}

fn valid_approval_reference(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 4096 && !value.chars().any(char::is_control)
}

fn read_json<T: DeserializeOwned>(
    filesystem: &ProjectFs,
    path: &ManagedPath,
) -> Result<T, GitError> {
    let bytes = filesystem
        .read_bounded(path, MAX_BRANCH_JOURNAL_BYTES)
        .map_err(managed_git_error)?;
    if bytes.is_empty() {
        return Err(invalid(format!(
            "Branch journal {} has an invalid size",
            filesystem.display_path(path).display()
        )));
    }
    serde_json::from_slice(&bytes).map_err(|error| {
        invalid(format!(
            "Failed to parse branch journal {}: {error}",
            filesystem.display_path(path).display()
        ))
    })
}

fn write_new_json(
    filesystem: &ProjectFs,
    path: &ManagedPath,
    value: &impl Serialize,
) -> Result<(), GitError> {
    let mut bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| invalid(format!("Failed to encode branch journal: {error}")))?;
    bytes.push(b'\n');
    let parent = path
        .parent()
        .ok_or_else(|| invalid("Branch journal path has no parent directory"))?;
    let sequence = NEXT_BRANCH_FILE.fetch_add(1, Ordering::Relaxed);
    let temporary = parent
        .join(format!(
            ".mino-branch-{}-{sequence}.tmp",
            std::process::id()
        ))
        .map_err(managed_git_error)?;
    let mut file = filesystem
        .create_new_file(&temporary)
        .map_err(managed_git_error)?;
    if let Err(error) = file.write_all(&bytes).and_then(|()| file.sync_all()) {
        let _ = filesystem.remove_file_if_exists(&temporary);
        return Err(path_error(
            "write",
            &filesystem.display_path(&temporary),
            &error,
        ));
    }
    drop(file);
    if let Err(error) = filesystem.rename(&temporary, path) {
        let _ = filesystem.remove_file_if_exists(&temporary);
        return Err(managed_git_error(error));
    }
    filesystem.sync_parent(path).map_err(managed_git_error)
}

fn is_object_id(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn invalid(message: impl Into<String>) -> GitError {
    GitError::new(GitErrorKind::InvalidOutput, message)
}

fn policy(message: impl Into<String>) -> GitError {
    GitError::new(GitErrorKind::PolicyViolation, message)
}

fn unavailable(message: impl Into<String>) -> GitError {
    GitError::new(GitErrorKind::Unavailable, message)
}

fn path_error(action: &str, path: &Path, error: &std::io::Error) -> GitError {
    unavailable(format!("Failed to {action} {}: {error}", path.display()))
}

fn managed_path(path: &str) -> ManagedPath {
    ManagedPath::new(path).expect("static Git journal path should be valid")
}

fn managed_git_error(error: ManagedFsError) -> GitError {
    let kind = match error.kind() {
        ManagedFsErrorKind::InvalidPath | ManagedFsErrorKind::UnsafeComponent => {
            GitErrorKind::InvalidOutput
        }
        ManagedFsErrorKind::Io => GitErrorKind::Unavailable,
    };
    GitError::new(kind, error.into_message())
}
