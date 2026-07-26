//! Worktree-aware active-plan binding storage and stale-identity detection.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use fs4::{FileExt, TryLockError};
use serde::{Deserialize, Serialize};

use crate::domain::{PlanId, Timestamp};
use crate::managed_fs::{ManagedFsError, ManagedFsErrorKind, ManagedPath, ProjectFs};

use super::{GitError, GitErrorKind, GitFacts};

const ACTIVE_BINDINGS_VERSION: u32 = 1;
const BINDING_LOCK_TIMEOUT: Duration = Duration::from_secs(2);
const BINDING_LOCK_RETRY: Duration = Duration::from_millis(10);
static NEXT_BINDING_FILE: AtomicU64 = AtomicU64::new(1);

/// One plan bound to an exact repository worktree identity.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActivePlanBinding {
    /// Canonical shared Git common-directory path.
    pub common_dir: String,
    /// Canonical worktree-root path.
    pub worktree: String,
    /// Bound branch name, including an unborn branch.
    pub branch: Option<String>,
    /// Exact detached commit identity when no branch is bound.
    pub detached_head: Option<String>,
    /// Observed HEAD when binding occurred.
    pub head_at_bind: Option<String>,
    /// Bound plan identifier.
    pub plan_id: PlanId,
    /// Observed plan revision when binding occurred.
    pub plan_revision: u64,
    /// Audit timestamp for the binding declaration.
    pub bound_at: Timestamp,
}

/// Current relationship between active bindings and inspected Git facts.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActiveBindingStatus {
    /// No active binding file exists.
    Missing,
    /// One binding matches the current worktree and branch or detached HEAD.
    Current,
    /// Bindings exist only for another worktree or repository identity.
    ForeignWorktree,
    /// The worktree is now on a different branch.
    StaleBranch,
    /// A detached binding no longer identifies the same commit or mode.
    StaleHead,
    /// Active bindings exist but the current path is not a Git worktree.
    NotRepository,
}

/// Read-only binding resolution for current Git facts.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ActiveBindingResolution {
    /// Stable resolution status.
    pub status: ActiveBindingStatus,
    /// Matching or stale same-worktree binding when one exists.
    pub binding: Option<ActivePlanBinding>,
}

/// Result of an idempotent active-plan bind operation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ActiveBindingWriteReport {
    /// Persisted binding.
    pub binding: ActivePlanBinding,
    /// Whether identical binding bytes were already active.
    pub replayed: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ActiveBindingsFile {
    version: u32,
    bindings: Vec<ActivePlanBinding>,
}

impl Default for ActiveBindingsFile {
    fn default() -> Self {
        Self {
            version: ACTIVE_BINDINGS_VERSION,
            bindings: Vec::new(),
        }
    }
}

/// Project-local store for worktree-keyed active-plan bindings.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActiveBindingStore {
    project_root: PathBuf,
}

impl ActiveBindingStore {
    /// Creates a binding store rooted at one initialized Mino project.
    #[must_use]
    pub fn new(project_root: impl Into<PathBuf>) -> Self {
        Self {
            project_root: project_root.into(),
        }
    }

    /// Returns the deterministic active binding file path.
    #[must_use]
    pub fn path(&self) -> PathBuf {
        self.project_root.join(".mino/active.json")
    }

    /// Resolves current facts against the stored bindings.
    ///
    /// # Errors
    ///
    /// Returns an invalid-output or environment error for malformed, unsafe,
    /// unreadable, or unsupported binding state.
    pub fn resolve(&self, facts: &GitFacts) -> Result<ActiveBindingResolution, GitError> {
        let Some(file) = self.load()? else {
            return Ok(ActiveBindingResolution {
                status: ActiveBindingStatus::Missing,
                binding: None,
            });
        };
        Ok(resolve_bindings(&file.bindings, facts))
    }

    /// Binds one non-Done plan to the current worktree and branch or detached HEAD.
    ///
    /// # Errors
    ///
    /// Returns an error for non-worktree facts, malformed existing state, lock
    /// timeout, or atomic publication failure.
    pub fn bind(
        &self,
        facts: &GitFacts,
        plan_id: PlanId,
        plan_revision: u64,
        bound_at: Timestamp,
    ) -> Result<ActiveBindingWriteReport, GitError> {
        let filesystem = self.filesystem()?;
        let candidate = binding_from_facts(facts, plan_id, plan_revision, bound_at)?;
        let mino_directory = managed_path(".mino");
        if !filesystem
            .is_directory(&mino_directory)
            .map_err(managed_git_error)?
        {
            return Err(GitError::new(
                GitErrorKind::Unavailable,
                format!(
                    "Mino state directory {} is missing",
                    filesystem.display_path(&mino_directory).display()
                ),
            ));
        }
        let lock_path = mino_directory
            .join("active.lock")
            .map_err(managed_git_error)?;
        let _lock = BindingLock::acquire(&filesystem, &lock_path)?;
        let mut file = Self::load_with(&filesystem)?.unwrap_or_default();
        if let Some(existing) = file
            .bindings
            .iter()
            .find(|binding| binding.worktree == candidate.worktree)
            && same_binding(existing, &candidate)
        {
            return Ok(ActiveBindingWriteReport {
                binding: existing.clone(),
                replayed: true,
            });
        }
        file.bindings
            .retain(|binding| binding.worktree != candidate.worktree);
        file.bindings.push(candidate.clone());
        file.bindings.sort_by(binding_order);
        validate_file(&file)?;
        publish_file(&filesystem, &managed_path(".mino/active.json"), &file)?;
        Ok(ActiveBindingWriteReport {
            binding: candidate,
            replayed: false,
        })
    }

    fn load(&self) -> Result<Option<ActiveBindingsFile>, GitError> {
        let filesystem = self.filesystem()?;
        Self::load_with(&filesystem)
    }

    fn load_with(filesystem: &ProjectFs) -> Result<Option<ActiveBindingsFile>, GitError> {
        let path = managed_path(".mino/active.json");
        if !filesystem.exists(&path).map_err(managed_git_error)? {
            return Ok(None);
        }
        let bytes = filesystem.read(&path).map_err(managed_git_error)?;
        let file: ActiveBindingsFile = serde_json::from_slice(&bytes).map_err(|error| {
            GitError::new(
                GitErrorKind::InvalidOutput,
                format!(
                    "Failed to parse active bindings {}: {error}",
                    filesystem.display_path(&path).display()
                ),
            )
        })?;
        validate_file(&file)?;
        Ok(Some(file))
    }

    fn filesystem(&self) -> Result<ProjectFs, GitError> {
        ProjectFs::open(&self.project_root).map_err(managed_git_error)
    }
}

fn same_binding(left: &ActivePlanBinding, right: &ActivePlanBinding) -> bool {
    left.common_dir == right.common_dir
        && left.worktree == right.worktree
        && left.branch == right.branch
        && left.detached_head == right.detached_head
        && left.head_at_bind == right.head_at_bind
        && left.plan_id == right.plan_id
        && left.plan_revision == right.plan_revision
}

fn binding_from_facts(
    facts: &GitFacts,
    plan_id: PlanId,
    plan_revision: u64,
    bound_at: Timestamp,
) -> Result<ActivePlanBinding, GitError> {
    if !facts.repository || !facts.is_worktree {
        return Err(GitError::new(
            GitErrorKind::PolicyViolation,
            "Active plans can only bind to a Git worktree",
        ));
    }
    let common_dir = path_string(
        facts
            .common_dir
            .as_deref()
            .ok_or_else(|| invalid("Git facts have no common directory"))?,
    )?;
    let worktree = path_string(
        facts
            .worktree
            .as_deref()
            .ok_or_else(|| invalid("Git facts have no worktree root"))?,
    )?;
    let detached_head = if facts.branch.is_none() {
        Some(
            facts
                .head
                .clone()
                .ok_or_else(|| invalid("Detached Git facts have no HEAD object ID"))?,
        )
    } else {
        None
    };
    Ok(ActivePlanBinding {
        common_dir,
        worktree,
        branch: facts.branch.clone(),
        detached_head,
        head_at_bind: facts.head.clone(),
        plan_id,
        plan_revision,
        bound_at,
    })
}

fn resolve_bindings(bindings: &[ActivePlanBinding], facts: &GitFacts) -> ActiveBindingResolution {
    if !facts.repository || !facts.is_worktree {
        return ActiveBindingResolution {
            status: ActiveBindingStatus::NotRepository,
            binding: None,
        };
    }
    let Some(worktree) = facts.worktree.as_deref().and_then(path_string_lossless) else {
        return ActiveBindingResolution {
            status: ActiveBindingStatus::NotRepository,
            binding: None,
        };
    };
    let Some(binding) = bindings
        .iter()
        .find(|binding| binding.worktree == worktree)
        .cloned()
    else {
        return ActiveBindingResolution {
            status: ActiveBindingStatus::ForeignWorktree,
            binding: None,
        };
    };
    let common_matches = facts
        .common_dir
        .as_deref()
        .and_then(path_string_lossless)
        .is_some_and(|common_dir| common_dir == binding.common_dir);
    if !common_matches {
        return ActiveBindingResolution {
            status: ActiveBindingStatus::ForeignWorktree,
            binding: Some(binding),
        };
    }
    let status = if let Some(branch) = &binding.branch {
        if facts.branch.as_ref() == Some(branch) {
            ActiveBindingStatus::Current
        } else {
            ActiveBindingStatus::StaleBranch
        }
    } else if facts.branch.is_none() && facts.head == binding.detached_head {
        ActiveBindingStatus::Current
    } else {
        ActiveBindingStatus::StaleHead
    };
    ActiveBindingResolution {
        status,
        binding: Some(binding),
    }
}

fn validate_file(file: &ActiveBindingsFile) -> Result<(), GitError> {
    if file.version != ACTIVE_BINDINGS_VERSION {
        return Err(invalid(format!(
            "Active binding version {} is unsupported",
            file.version
        )));
    }
    if !file.bindings.windows(2).all(|pair| {
        binding_key(&pair[0]) < binding_key(&pair[1]) && pair[0].worktree != pair[1].worktree
    }) {
        return Err(invalid(
            "Active bindings must be uniquely sorted by repository and worktree",
        ));
    }
    for binding in &file.bindings {
        if binding.common_dir.is_empty()
            || binding.worktree.is_empty()
            || binding.plan_revision == 0
            || binding.branch.is_some() == binding.detached_head.is_some()
        {
            return Err(invalid(
                "Active binding fields are incomplete or inconsistent",
            ));
        }
    }
    Ok(())
}

fn binding_order(left: &ActivePlanBinding, right: &ActivePlanBinding) -> std::cmp::Ordering {
    binding_key(left).cmp(&binding_key(right))
}

fn binding_key(binding: &ActivePlanBinding) -> (&str, &str, Option<&str>, Option<&str>) {
    (
        &binding.common_dir,
        &binding.worktree,
        binding.branch.as_deref(),
        binding.detached_head.as_deref(),
    )
}

fn publish_file(
    filesystem: &ProjectFs,
    path: &ManagedPath,
    file: &ActiveBindingsFile,
) -> Result<(), GitError> {
    let mut bytes = serde_json::to_vec_pretty(file).map_err(|error| {
        GitError::new(
            GitErrorKind::InvalidOutput,
            format!("Failed to encode active bindings: {error}"),
        )
    })?;
    bytes.push(b'\n');
    let parent = path
        .parent()
        .ok_or_else(|| invalid("Active binding path has no parent directory"))?;
    let sequence = NEXT_BINDING_FILE.fetch_add(1, Ordering::Relaxed);
    let temporary = parent
        .join(format!(
            ".active.json.mino-bind-{}-{sequence}.tmp",
            std::process::id()
        ))
        .map_err(managed_git_error)?;
    let backup = parent
        .join(format!(
            ".active.json.mino-bind-{}-{sequence}.bak",
            std::process::id()
        ))
        .map_err(managed_git_error)?;
    write_new_file(filesystem, &temporary, &bytes)?;
    if !filesystem.exists(path).map_err(managed_git_error)? {
        filesystem
            .rename(&temporary, path)
            .map_err(managed_git_error)?;
        return filesystem.sync_parent(path).map_err(managed_git_error);
    }
    filesystem
        .rename(path, &backup)
        .map_err(managed_git_error)?;
    if let Err(error) = filesystem.rename(&temporary, path) {
        let restoration = filesystem.rename(&backup, path);
        let _ = filesystem.remove_file_if_exists(&temporary);
        return match restoration {
            Ok(()) => Err(managed_git_error(error)),
            Err(restoration_error) => Err(GitError::new(
                GitErrorKind::Unavailable,
                format!(
                    "Failed to publish {} ({error}) and restore its backup ({restoration_error})",
                    filesystem.display_path(path).display()
                ),
            )),
        };
    }
    filesystem.remove_file(&backup).map_err(managed_git_error)?;
    filesystem.sync_parent(path).map_err(managed_git_error)
}

fn write_new_file(
    filesystem: &ProjectFs,
    path: &ManagedPath,
    bytes: &[u8],
) -> Result<(), GitError> {
    let mut file = filesystem
        .create_new_file(path)
        .map_err(managed_git_error)?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| path_error("write", &filesystem.display_path(path), &error))
}

fn path_string(path: &Path) -> Result<String, GitError> {
    path_string_lossless(path).ok_or_else(|| {
        GitError::new(
            GitErrorKind::InvalidOutput,
            format!("Git identity path {} is not valid UTF-8", path.display()),
        )
    })
}

fn path_string_lossless(path: &Path) -> Option<String> {
    path.to_str().map(|value| value.replace('\\', "/"))
}

fn invalid(message: impl Into<String>) -> GitError {
    GitError::new(GitErrorKind::InvalidOutput, message)
}

fn path_error(action: &str, path: &Path, error: &std::io::Error) -> GitError {
    GitError::new(
        GitErrorKind::Unavailable,
        format!("Failed to {action} {}: {error}", path.display()),
    )
}

struct BindingLock {
    file: std::fs::File,
}

impl BindingLock {
    fn acquire(filesystem: &ProjectFs, path: &ManagedPath) -> Result<Self, GitError> {
        let file = filesystem.open_lock_file(path).map_err(managed_git_error)?;
        let started_at = Instant::now();
        loop {
            match FileExt::try_lock(&file) {
                Ok(()) => return Ok(Self { file }),
                Err(TryLockError::WouldBlock) if started_at.elapsed() < BINDING_LOCK_TIMEOUT => {
                    thread::sleep(BINDING_LOCK_RETRY);
                }
                Err(TryLockError::WouldBlock) => {
                    return Err(GitError::new(
                        GitErrorKind::Unavailable,
                        format!(
                            "Timed out acquiring active binding lock {}",
                            filesystem.display_path(path).display()
                        ),
                    ));
                }
                Err(TryLockError::Error(error)) => {
                    return Err(path_error(
                        "lock active bindings",
                        &filesystem.display_path(path),
                        &error,
                    ));
                }
            }
        }
    }
}

impl Drop for BindingLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

fn managed_path(path: &str) -> ManagedPath {
    ManagedPath::new(path).expect("static active binding path should be valid")
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
