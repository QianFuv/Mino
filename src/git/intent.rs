//! Immutable prepared, staged, and completed task-commit journals.

#[cfg(unix)]
use std::fs::File;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use fs4::{FileExt, TryLockError};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::domain::{EvidenceId, PlanId, TaskId, Timestamp};

use super::{GitError, GitErrorKind};

const COMMIT_JOURNAL_VERSION: u32 = 1;
const COMMIT_LOCK_TIMEOUT: Duration = Duration::from_secs(2);
const COMMIT_LOCK_RETRY: Duration = Duration::from_millis(10);
const MAX_COMMIT_JOURNAL_BYTES: u64 = 4 * 1024 * 1024;
static NEXT_COMMIT_FILE: AtomicU64 = AtomicU64::new(1);

/// Supported filesystem state captured before staging one task path.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommitFileSnapshotKind {
    /// A regular file whose bytes are hashed.
    File,
    /// A path deleted from the working tree.
    Deleted,
}

/// Immutable pre-staging fingerprint for one exact task path.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommitFileSnapshot {
    /// Normalized repository-relative path.
    pub path: String,
    /// File or deletion classification.
    pub kind: CommitFileSnapshotKind,
    /// SHA-256 of regular-file bytes or the stable deletion marker.
    pub digest: String,
    /// Regular-file byte length, or zero for deletion.
    pub length: u64,
    /// Whether the Unix executable mode was observed.
    pub executable: bool,
    /// Porcelain index status observed before Mino staging.
    pub index_status: char,
    /// Porcelain worktree status observed before Mino staging.
    pub worktree_status: char,
}

/// Immutable declaration published before Mino changes the Git index.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitCommitIntent {
    version: u32,
    /// Plan authorizing the task commit.
    pub plan_id: PlanId,
    /// Plan revision observed during preflight.
    pub plan_revision: u64,
    /// Done task whose gate authorizes the commit.
    pub task_id: TaskId,
    /// Canonical shared Git directory.
    pub common_dir: String,
    /// Canonical worktree root.
    pub worktree: String,
    /// Exact checked-out branch.
    pub branch: String,
    /// Exact full parent commit required by the gate.
    pub parent_head: String,
    /// Exact planned one-line Conventional Commit message.
    pub message: String,
    /// Sorted exact path fingerprints captured before staging.
    pub files: Vec<CommitFileSnapshot>,
    /// Time at which the intent was prepared.
    pub prepared_at: Timestamp,
}

impl GitCommitIntent {
    /// Creates one validated task-commit intent.
    ///
    /// # Errors
    ///
    /// Returns an invalid-output error when any identity, message, parent, or
    /// file snapshot is incomplete or inconsistent.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        plan_id: PlanId,
        plan_revision: u64,
        task_id: TaskId,
        common_dir: String,
        worktree: String,
        branch: String,
        parent_head: String,
        message: String,
        files: Vec<CommitFileSnapshot>,
        prepared_at: Timestamp,
    ) -> Result<Self, GitError> {
        let intent = Self {
            version: COMMIT_JOURNAL_VERSION,
            plan_id,
            plan_revision,
            task_id,
            common_dir,
            worktree,
            branch,
            parent_head,
            message,
            files,
            prepared_at,
        };
        validate_intent(&intent)?;
        Ok(intent)
    }

    /// Returns the sorted exact path list.
    #[must_use]
    pub fn paths(&self) -> Vec<String> {
        self.files.iter().map(|file| file.path.clone()).collect()
    }
}

/// Immutable index tree observed after exact task paths were staged.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitStagedCommit {
    version: u32,
    /// Plan owning the staged operation.
    pub plan_id: PlanId,
    /// Task owning every staged path.
    pub task_id: TaskId,
    /// Parent commit against which the index was prepared.
    pub parent_head: String,
    /// Exact tree object produced by the staged index.
    pub tree: String,
    /// Sorted exact staged paths.
    pub files: Vec<String>,
    /// Time at which the staged tree was verified.
    pub staged_at: Timestamp,
}

impl GitStagedCommit {
    /// Creates a staged record bound to one prepared intent.
    ///
    /// # Errors
    ///
    /// Returns an invalid-output error when the staged tree or paths do not
    /// match the prepared operation.
    pub fn new(
        intent: &GitCommitIntent,
        tree: String,
        staged_at: Timestamp,
    ) -> Result<Self, GitError> {
        let staged = Self {
            version: COMMIT_JOURNAL_VERSION,
            plan_id: intent.plan_id.clone(),
            task_id: intent.task_id.clone(),
            parent_head: intent.parent_head.clone(),
            tree,
            files: intent.paths(),
            staged_at,
        };
        validate_staged(intent, &staged)?;
        Ok(staged)
    }
}

/// Immutable terminal task-commit result published after evidence and plan state.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitCommitCompletion {
    version: u32,
    /// Plan whose commit gate completed.
    pub plan_id: PlanId,
    /// Task whose commit gate completed.
    pub task_id: TaskId,
    /// Full created commit object ID.
    pub commit: String,
    /// Exact parent commit.
    pub parent: String,
    /// Exact committed tree.
    pub tree: String,
    /// Exact committed one-line message.
    pub message: String,
    /// Sorted exact committed paths.
    pub files: Vec<String>,
    /// Immutable Commit evidence attached to the task gate.
    pub evidence_id: EvidenceId,
    /// Plan revision after the commit gate was recorded.
    pub recorded_plan_revision: u64,
    /// Time at which terminal journal state was published.
    pub completed_at: Timestamp,
}

/// Inputs used to create one terminal task-commit result.
pub struct GitCommitCompletionInput {
    /// Full created commit object ID.
    pub commit: String,
    /// Exact parent commit.
    pub parent: String,
    /// Exact committed tree.
    pub tree: String,
    /// Exact committed one-line message.
    pub message: String,
    /// Sorted exact committed paths.
    pub files: Vec<String>,
    /// Immutable Commit evidence attached to the task gate.
    pub evidence_id: EvidenceId,
    /// Plan revision after recording the task gate.
    pub recorded_plan_revision: u64,
    /// Terminal publication timestamp.
    pub completed_at: Timestamp,
}

impl GitCommitCompletion {
    /// Creates a terminal result bound to prepared and staged state.
    ///
    /// # Errors
    ///
    /// Returns an invalid-output error when commit identity, tree, message,
    /// files, evidence, or revision disagrees with the journal.
    pub fn new(
        intent: &GitCommitIntent,
        staged: &GitStagedCommit,
        input: GitCommitCompletionInput,
    ) -> Result<Self, GitError> {
        let completion = Self {
            version: COMMIT_JOURNAL_VERSION,
            plan_id: intent.plan_id.clone(),
            task_id: intent.task_id.clone(),
            commit: input.commit,
            parent: input.parent,
            tree: input.tree,
            message: input.message,
            files: input.files,
            evidence_id: input.evidence_id,
            recorded_plan_revision: input.recorded_plan_revision,
            completed_at: input.completed_at,
        };
        validate_completion(intent, staged, &completion)?;
        Ok(completion)
    }
}

/// Current immutable journal phases for one task commit.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GitCommitJournal {
    /// Prepared pre-index intent.
    pub intent: GitCommitIntent,
    /// Verified staged tree when staging completed.
    pub staged: Option<GitStagedCommit>,
    /// Terminal result after evidence and plan publication.
    pub completion: Option<GitCommitCompletion>,
}

/// Store for immutable task-commit journal phases.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitCommitJournalStore {
    project_root: PathBuf,
}

impl GitCommitJournalStore {
    /// Creates a task-commit journal store for one initialized project.
    #[must_use]
    pub fn new(project_root: impl Into<PathBuf>) -> Self {
        Self {
            project_root: project_root.into(),
        }
    }

    /// Returns the prepared intent path.
    #[must_use]
    pub fn intent_path(&self, plan_id: &PlanId, task_id: &TaskId) -> PathBuf {
        self.operation_directory(plan_id, task_id)
            .join("intent.json")
    }

    /// Returns the verified staged-tree path.
    #[must_use]
    pub fn staged_path(&self, plan_id: &PlanId, task_id: &TaskId) -> PathBuf {
        self.operation_directory(plan_id, task_id)
            .join("staged.json")
    }

    /// Returns the terminal completion path.
    #[must_use]
    pub fn completion_path(&self, plan_id: &PlanId, task_id: &TaskId) -> PathBuf {
        self.operation_directory(plan_id, task_id)
            .join("completion.json")
    }

    /// Loads one journal without creating filesystem state.
    ///
    /// # Errors
    ///
    /// Returns an invalid-output or environment error for missing phases,
    /// malformed bytes, unsupported versions, or inconsistent identities.
    pub fn load(
        &self,
        plan_id: &PlanId,
        task_id: &TaskId,
    ) -> Result<Option<GitCommitJournal>, GitError> {
        let intent_path = self.intent_path(plan_id, task_id);
        let staged_path = self.staged_path(plan_id, task_id);
        let completion_path = self.completion_path(plan_id, task_id);
        if !intent_path.exists() {
            if staged_path.exists() || completion_path.exists() {
                return Err(invalid("Commit journal phase exists without an intent"));
            }
            return Ok(None);
        }
        let intent: GitCommitIntent = read_json(&intent_path)?;
        validate_intent(&intent)?;
        if &intent.plan_id != plan_id || &intent.task_id != task_id {
            return Err(invalid(
                "Commit journal path and operation identity disagree",
            ));
        }
        let staged = staged_path
            .exists()
            .then(|| read_json(&staged_path))
            .transpose()?;
        if let Some(staged) = &staged {
            validate_staged(&intent, staged)?;
        }
        let completion = completion_path
            .exists()
            .then(|| read_json(&completion_path))
            .transpose()?;
        if completion.is_some() && staged.is_none() {
            return Err(invalid("Commit completion exists without staged state"));
        }
        if let (Some(staged), Some(completion)) = (&staged, &completion) {
            validate_completion(&intent, staged, completion)?;
        }
        Ok(Some(GitCommitJournal {
            intent,
            staged,
            completion,
        }))
    }

    /// Acquires the bounded project commit-operation lock.
    ///
    /// # Errors
    ///
    /// Returns an unavailable error for missing project state, I/O failure, or
    /// lock timeout.
    pub fn lock(&self) -> Result<GitCommitJournalLock, GitError> {
        let mino_directory = self.project_root.join(".mino");
        if !mino_directory.is_dir() {
            return Err(unavailable(format!(
                "Mino state directory {} is missing",
                mino_directory.display()
            )));
        }
        let git_directory = mino_directory.join("git");
        fs::create_dir_all(&git_directory)
            .map_err(|error| path_error("create", &git_directory, &error))?;
        GitCommitJournalLock::acquire(&git_directory.join("commit.lock"))
    }

    /// Publishes or replays a semantically identical prepared intent.
    ///
    /// # Errors
    ///
    /// Returns a policy error for a conflicting operation or an environment
    /// error when immutable publication fails.
    pub fn prepare(&self, candidate: GitCommitIntent) -> Result<GitCommitIntent, GitError> {
        validate_intent(&candidate)?;
        if let Some(existing) = self.load(&candidate.plan_id, &candidate.task_id)? {
            if same_intent(&existing.intent, &candidate) {
                return Ok(existing.intent);
            }
            return Err(policy(format!(
                "Task {} already has a different prepared commit operation",
                candidate.task_id
            )));
        }
        let directory = self.operation_directory(&candidate.plan_id, &candidate.task_id);
        fs::create_dir_all(&directory).map_err(|error| path_error("create", &directory, &error))?;
        write_new_json(
            &self.intent_path(&candidate.plan_id, &candidate.task_id),
            &candidate,
        )?;
        Ok(candidate)
    }

    /// Publishes or replays the verified staged tree.
    ///
    /// # Errors
    ///
    /// Returns an invalid-output error for inconsistent phases or an
    /// environment error when publication fails.
    pub fn record_staged(
        &self,
        intent: &GitCommitIntent,
        candidate: GitStagedCommit,
    ) -> Result<GitStagedCommit, GitError> {
        validate_staged(intent, &candidate)?;
        let journal = self
            .load(&intent.plan_id, &intent.task_id)?
            .ok_or_else(|| invalid("Staged commit has no prepared intent"))?;
        if !same_intent(&journal.intent, intent) {
            return Err(invalid("Staged commit intent does not match stored state"));
        }
        if let Some(existing) = journal.staged {
            if same_staged(&existing, &candidate) {
                return Ok(existing);
            }
            return Err(invalid("Stored staged tree conflicts with current index"));
        }
        write_new_json(
            &self.staged_path(&intent.plan_id, &intent.task_id),
            &candidate,
        )?;
        Ok(candidate)
    }

    /// Publishes or replays the terminal task-commit result.
    ///
    /// # Errors
    ///
    /// Returns an invalid-output error for inconsistent phases or an
    /// environment error when publication fails.
    pub fn complete(
        &self,
        intent: &GitCommitIntent,
        staged: &GitStagedCommit,
        candidate: GitCommitCompletion,
    ) -> Result<GitCommitCompletion, GitError> {
        validate_completion(intent, staged, &candidate)?;
        let journal = self
            .load(&intent.plan_id, &intent.task_id)?
            .ok_or_else(|| invalid("Commit completion has no prepared intent"))?;
        if !same_intent(&journal.intent, intent)
            || journal
                .staged
                .as_ref()
                .is_none_or(|value| !same_staged(value, staged))
        {
            return Err(invalid("Commit completion does not match stored phases"));
        }
        if let Some(existing) = journal.completion {
            if same_completion(&existing, &candidate) {
                return Ok(existing);
            }
            return Err(invalid(
                "Stored commit completion conflicts with observed result",
            ));
        }
        write_new_json(
            &self.completion_path(&intent.plan_id, &intent.task_id),
            &candidate,
        )?;
        Ok(candidate)
    }

    fn operation_directory(&self, plan_id: &PlanId, task_id: &TaskId) -> PathBuf {
        self.project_root
            .join(".mino/git/commits")
            .join(plan_id.as_str())
            .join(task_id.as_str())
    }
}

/// Held advisory lock for one project commit operation.
#[derive(Debug)]
pub struct GitCommitJournalLock {
    file: std::fs::File,
}

impl GitCommitJournalLock {
    fn acquire(path: &Path) -> Result<Self, GitError> {
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .truncate(false)
            .write(true)
            .open(path)
            .map_err(|error| path_error("open commit lock", path, &error))?;
        let started_at = Instant::now();
        loop {
            match FileExt::try_lock(&file) {
                Ok(()) => return Ok(Self { file }),
                Err(TryLockError::WouldBlock) if started_at.elapsed() < COMMIT_LOCK_TIMEOUT => {
                    thread::sleep(COMMIT_LOCK_RETRY);
                }
                Err(TryLockError::WouldBlock) => {
                    return Err(unavailable(format!(
                        "Timed out acquiring commit-operation lock {}",
                        path.display()
                    )));
                }
                Err(TryLockError::Error(error)) => {
                    return Err(path_error("lock commit operation", path, &error));
                }
            }
        }
    }
}

impl Drop for GitCommitJournalLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

fn validate_intent(intent: &GitCommitIntent) -> Result<(), GitError> {
    if intent.version != COMMIT_JOURNAL_VERSION
        || intent.plan_revision == 0
        || intent.common_dir.is_empty()
        || intent.worktree.is_empty()
        || intent.branch.is_empty()
        || !is_object_id(&intent.parent_head)
        || intent.message.trim().is_empty()
        || intent.message.contains(['\r', '\n'])
        || intent.files.is_empty()
        || !intent
            .files
            .windows(2)
            .all(|pair| pair[0].path < pair[1].path)
        || intent.files.iter().any(invalid_snapshot)
    {
        Err(invalid(
            "Prepared commit intent is incomplete or inconsistent",
        ))
    } else {
        Ok(())
    }
}

fn invalid_snapshot(snapshot: &CommitFileSnapshot) -> bool {
    let path = Path::new(&snapshot.path);
    snapshot.path.is_empty()
        || snapshot.path.contains('\\')
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
        || !snapshot.digest.starts_with("sha256:")
        || snapshot.digest.len() != 71
        || !snapshot.digest[7..]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
        || snapshot.kind == CommitFileSnapshotKind::Deleted
            && (snapshot.length != 0 || snapshot.executable)
        || !matches!(
            (snapshot.index_status, snapshot.worktree_status),
            ('.', 'M' | 'D' | 'T') | ('?', '?')
        )
        || (snapshot.kind == CommitFileSnapshotKind::Deleted) != (snapshot.worktree_status == 'D')
}

fn validate_staged(intent: &GitCommitIntent, staged: &GitStagedCommit) -> Result<(), GitError> {
    if staged.version != COMMIT_JOURNAL_VERSION
        || staged.plan_id != intent.plan_id
        || staged.task_id != intent.task_id
        || staged.parent_head != intent.parent_head
        || !is_object_id(&staged.tree)
        || staged.files != intent.paths()
    {
        Err(invalid("Staged commit does not match its prepared intent"))
    } else {
        Ok(())
    }
}

fn validate_completion(
    intent: &GitCommitIntent,
    staged: &GitStagedCommit,
    completion: &GitCommitCompletion,
) -> Result<(), GitError> {
    if completion.version != COMMIT_JOURNAL_VERSION
        || completion.plan_id != intent.plan_id
        || completion.task_id != intent.task_id
        || !is_object_id(&completion.commit)
        || completion.parent != intent.parent_head
        || completion.tree != staged.tree
        || completion.message != intent.message
        || completion.files != staged.files
        || completion.recorded_plan_revision <= intent.plan_revision
    {
        Err(invalid(
            "Commit completion does not match prepared and staged state",
        ))
    } else {
        Ok(())
    }
}

fn same_intent(left: &GitCommitIntent, right: &GitCommitIntent) -> bool {
    left.version == right.version
        && left.plan_id == right.plan_id
        && left.plan_revision == right.plan_revision
        && left.task_id == right.task_id
        && left.common_dir == right.common_dir
        && left.worktree == right.worktree
        && left.branch == right.branch
        && left.parent_head == right.parent_head
        && left.message == right.message
        && left.files == right.files
}

fn same_staged(left: &GitStagedCommit, right: &GitStagedCommit) -> bool {
    left.version == right.version
        && left.plan_id == right.plan_id
        && left.task_id == right.task_id
        && left.parent_head == right.parent_head
        && left.tree == right.tree
        && left.files == right.files
}

fn same_completion(left: &GitCommitCompletion, right: &GitCommitCompletion) -> bool {
    left.version == right.version
        && left.plan_id == right.plan_id
        && left.task_id == right.task_id
        && left.commit == right.commit
        && left.parent == right.parent
        && left.tree == right.tree
        && left.message == right.message
        && left.files == right.files
        && left.evidence_id == right.evidence_id
        && left.recorded_plan_revision == right.recorded_plan_revision
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T, GitError> {
    let metadata = fs::metadata(path).map_err(|error| path_error("inspect", path, &error))?;
    if metadata.len() == 0 || metadata.len() > MAX_COMMIT_JOURNAL_BYTES {
        return Err(invalid(format!(
            "Commit journal {} has an invalid size",
            path.display()
        )));
    }
    let bytes = fs::read(path).map_err(|error| path_error("read", path, &error))?;
    serde_json::from_slice(&bytes).map_err(|error| {
        invalid(format!(
            "Failed to parse commit journal {}: {error}",
            path.display()
        ))
    })
}

fn write_new_json(path: &Path, value: &impl Serialize) -> Result<(), GitError> {
    let mut bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| invalid(format!("Failed to encode commit journal: {error}")))?;
    bytes.push(b'\n');
    let parent = path
        .parent()
        .ok_or_else(|| invalid("Commit journal path has no parent directory"))?;
    let sequence = NEXT_COMMIT_FILE.fetch_add(1, Ordering::Relaxed);
    let temporary = parent.join(format!(
        ".mino-commit-{}-{sequence}.tmp",
        std::process::id()
    ));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .map_err(|error| path_error("create", &temporary, &error))?;
    if let Err(error) = file.write_all(&bytes).and_then(|()| file.sync_all()) {
        let _ = fs::remove_file(&temporary);
        return Err(path_error("write", &temporary, &error));
    }
    drop(file);
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(path_error("publish", path, &error));
    }
    sync_directory(parent)
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

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<(), GitError> {
    File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|error| path_error("synchronize directory", path, &error))
}

#[cfg(not(unix))]
fn sync_directory(path: &Path) -> Result<(), GitError> {
    fs::metadata(path)
        .map(|_| ())
        .map_err(|error| path_error("inspect directory", path, &error))
}
