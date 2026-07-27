//! Bounded workspace fingerprint capture for verification freshness.

use std::collections::BTreeSet;
use std::path::Path;

use ignore::WalkBuilder;

use crate::domain::{
    FileChange, Plan, TaskId, WorkspaceFileKind, WorkspaceFileSnapshot, WorkspaceFingerprint,
    WorkspaceFingerprintScope, WorkspaceRepositoryMode, WorkspaceScopeKind, WorkspaceStatusEntry,
};
use crate::git::{GitAdapter, GitError, GitStatusEntry, GitStatusKind, matches_file_map_path};
use crate::managed_fs::{ManagedEntryKind, ManagedFsError, ManagedPath, ProjectFs};
use crate::store::sha256_digest;
use crate::{ErrorCategory, MinoError};

const MAX_FINGERPRINT_FILES: usize = 100_000;
const MAX_FINGERPRINT_FILE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_FINGERPRINT_TOTAL_BYTES: u64 = 256 * 1024 * 1024;

pub(crate) fn capture_workspace_fingerprint(
    root: &Path,
    plan: &Plan,
    task_id: Option<&TaskId>,
) -> Result<WorkspaceFingerprint, MinoError> {
    let filesystem = ProjectFs::open(root).map_err(managed_error)?;
    let (scope_kind, patterns) = match task_id {
        Some(task_id) => {
            let task = plan.task(task_id).ok_or_else(|| {
                MinoError::new(
                    ErrorCategory::IncompleteOrValidation,
                    format!("Task {task_id} does not exist"),
                )
            })?;
            (
                WorkspaceScopeKind::Task,
                task.file_map()
                    .iter()
                    .filter(|entry| entry.change() != FileChange::NotApplicable)
                    .map(|entry| entry.path().to_owned())
                    .collect::<Vec<_>>(),
            )
        }
        None => (
            WorkspaceScopeKind::Global,
            plan.tasks()
                .iter()
                .flat_map(crate::domain::Task::file_map)
                .filter(|entry| entry.change() != FileChange::NotApplicable)
                .map(|entry| entry.path().to_owned())
                .collect::<Vec<_>>(),
        ),
    };
    let scope = WorkspaceFingerprintScope::new(scope_kind, task_id.cloned(), patterns);
    let adapter = GitAdapter::new(root);
    let facts = adapter.inspect().map_err(|error| git_error(&error))?;
    let (repository_mode, head, index_tree, status_entries) = if facts.repository {
        if !facts.is_worktree {
            return Err(MinoError::new(
                ErrorCategory::EnvironmentUnavailable,
                "Workspace fingerprints require a Git work tree or a non-Git project",
            ));
        }
        let index_entries = adapter.index_entries().map_err(|error| git_error(&error))?;
        let status_entries = facts
            .status
            .iter()
            .filter(|entry| status_in_scope(plan, &scope, entry))
            .map(status_entry)
            .collect::<Vec<_>>();
        (
            WorkspaceRepositoryMode::Git,
            facts.head,
            Some(sha256_digest(&index_entries)),
            status_entries,
        )
    } else {
        (WorkspaceRepositoryMode::NonGit, None, None, Vec::new())
    };
    let mut paths = scoped_paths(&filesystem, &scope)?;
    for entry in &status_entries {
        paths.insert(entry.path().to_owned());
    }
    let file_snapshots = capture_snapshots(&filesystem, paths)?;
    WorkspaceFingerprint::new(
        repository_mode,
        head,
        index_tree,
        status_entries,
        scope,
        file_snapshots,
    )
    .map_err(|error| MinoError::new(ErrorCategory::DriftDetected, error.to_string()))
}

pub(crate) fn recapture_workspace_fingerprint(
    root: &Path,
    plan: &Plan,
    expected: &WorkspaceFingerprint,
) -> Result<WorkspaceFingerprint, MinoError> {
    let current = capture_workspace_fingerprint(root, plan, expected.scope().task_id())?;
    if current.scope().kind() != expected.scope().kind() {
        return Err(MinoError::new(
            ErrorCategory::DriftDetected,
            "Workspace fingerprint scope changed classification",
        ));
    }
    Ok(current)
}

pub(crate) fn workspace_fingerprint_is_current(
    root: &Path,
    plan: &Plan,
    expected: &WorkspaceFingerprint,
) -> Result<bool, MinoError> {
    recapture_workspace_fingerprint(root, plan, expected)
        .map(|current| current.fingerprint_digest() == expected.fingerprint_digest())
}

fn status_in_scope(plan: &Plan, scope: &WorkspaceFingerprintScope, entry: &GitStatusEntry) -> bool {
    if is_mino_owned_path(plan, &entry.path) {
        return false;
    }
    scope.kind() == WorkspaceScopeKind::Global
        || scope
            .patterns()
            .iter()
            .any(|pattern| matches_file_map_path(pattern, &entry.path))
}

fn status_entry(entry: &GitStatusEntry) -> WorkspaceStatusEntry {
    WorkspaceStatusEntry::new(
        entry.path.clone(),
        entry.original_path.clone(),
        entry.index_status,
        entry.worktree_status,
        entry.submodule.clone(),
        match entry.kind {
            GitStatusKind::Ordinary => "ordinary",
            GitStatusKind::RenamedOrCopied => "renamed_or_copied",
            GitStatusKind::Unmerged => "unmerged",
            GitStatusKind::Untracked => "untracked",
            GitStatusKind::Ignored => "ignored",
        }
        .to_owned(),
    )
}

fn scoped_paths(
    filesystem: &ProjectFs,
    scope: &WorkspaceFingerprintScope,
) -> Result<BTreeSet<String>, MinoError> {
    let mut paths = BTreeSet::new();
    let mut requires_walk = false;
    for pattern in scope.patterns() {
        if pattern.contains('*') {
            requires_walk = true;
        } else {
            let managed = ManagedPath::new(pattern).map_err(managed_error)?;
            if filesystem.is_directory(&managed).map_err(managed_error)? {
                requires_walk = true;
            }
            paths.insert(pattern.clone());
        }
    }
    if requires_walk {
        collect_matching_paths(filesystem, scope.patterns(), &mut paths)?;
    }
    if paths.len() > MAX_FINGERPRINT_FILES {
        return Err(fingerprint_budget_error("file count"));
    }
    Ok(paths)
}

fn collect_matching_paths(
    filesystem: &ProjectFs,
    patterns: &[String],
    paths: &mut BTreeSet<String>,
) -> Result<(), MinoError> {
    let root = filesystem.root();
    let mut builder = WalkBuilder::new(root);
    builder
        .standard_filters(false)
        .hidden(false)
        .parents(false)
        .follow_links(false)
        .sort_by_file_path(Path::cmp)
        .filter_entry(|entry| {
            entry.depth() == 0
                || !entry.file_type().is_some_and(|kind| kind.is_dir())
                || !matches!(entry.file_name().to_str(), Some(".git" | ".mino"))
        });
    for entry in builder.build() {
        let entry = entry.map_err(|error| {
            MinoError::new(
                ErrorCategory::EnvironmentUnavailable,
                format!("Failed to enumerate workspace fingerprint paths: {error}"),
            )
        })?;
        if entry.depth() == 0 {
            continue;
        }
        let relative = entry.path().strip_prefix(root).map_err(|error| {
            MinoError::new(
                ErrorCategory::EnvironmentUnavailable,
                format!(
                    "Failed to normalize workspace path {}: {error}",
                    entry.path().display()
                ),
            )
        })?;
        let path = protocol_path(relative)?;
        if patterns.iter().any(|pattern| {
            matches_file_map_path(pattern, &path)
                || path
                    .strip_prefix(pattern)
                    .is_some_and(|suffix| suffix.starts_with('/'))
        }) {
            if entry.path_is_symlink() {
                return Err(MinoError::new(
                    ErrorCategory::PolicyViolation,
                    format!("Workspace fingerprint path {path} is a symbolic link"),
                ));
            }
            paths.insert(path);
            if paths.len() > MAX_FINGERPRINT_FILES {
                return Err(fingerprint_budget_error("file count"));
            }
        }
    }
    Ok(())
}

fn capture_snapshots(
    filesystem: &ProjectFs,
    paths: BTreeSet<String>,
) -> Result<Vec<WorkspaceFileSnapshot>, MinoError> {
    let mut total_bytes = 0_u64;
    paths
        .into_iter()
        .map(|path| {
            let managed = ManagedPath::new(&path).map_err(managed_error)?;
            match filesystem.entry_kind(&managed).map_err(managed_error)? {
                None => Ok(WorkspaceFileSnapshot::new(
                    path,
                    WorkspaceFileKind::Missing,
                    0,
                    false,
                    sha256_digest(b"mino.workspace.missing/v1"),
                )),
                Some(ManagedEntryKind::Directory) => Ok(WorkspaceFileSnapshot::new(
                    path,
                    WorkspaceFileKind::Directory,
                    0,
                    false,
                    sha256_digest(b"mino.workspace.directory/v1"),
                )),
                Some(ManagedEntryKind::File) => {
                    let (bytes, metadata) = filesystem
                        .read_bounded_with_metadata(&managed, MAX_FINGERPRINT_FILE_BYTES)
                        .map_err(managed_error)?;
                    let length = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
                    total_bytes = total_bytes
                        .checked_add(length)
                        .ok_or_else(|| fingerprint_budget_error("aggregate byte count"))?;
                    if total_bytes > MAX_FINGERPRINT_TOTAL_BYTES {
                        return Err(fingerprint_budget_error("aggregate byte count"));
                    }
                    Ok(WorkspaceFileSnapshot::new(
                        path,
                        WorkspaceFileKind::Regular,
                        length,
                        is_executable(&metadata),
                        sha256_digest(&bytes),
                    ))
                }
                Some(ManagedEntryKind::Symlink) => Err(MinoError::new(
                    ErrorCategory::PolicyViolation,
                    format!("Workspace fingerprint path {path} is a symbolic link"),
                )),
                Some(ManagedEntryKind::Other) => Err(MinoError::new(
                    ErrorCategory::PolicyViolation,
                    format!("Workspace fingerprint path {path} has an unsupported file type"),
                )),
            }
        })
        .collect()
}

fn protocol_path(path: &Path) -> Result<String, MinoError> {
    path.to_str()
        .map(|value| value.replace('\\', "/"))
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            MinoError::new(
                ErrorCategory::IncompleteOrValidation,
                format!(
                    "Workspace fingerprint path {} is not valid UTF-8",
                    path.display()
                ),
            )
        })
}

fn is_mino_owned_path(plan: &Plan, path: &str) -> bool {
    path == ".mino" || path.starts_with(".mino/") || plan.metadata().markdown_path() == Some(path)
}

#[cfg(unix)]
fn is_executable(metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;

    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_metadata: &std::fs::Metadata) -> bool {
    false
}

fn fingerprint_budget_error(resource: &str) -> MinoError {
    MinoError::new(
        ErrorCategory::EnvironmentUnavailable,
        format!("Workspace fingerprint exceeded its {resource} budget"),
    )
}

fn managed_error(error: ManagedFsError) -> MinoError {
    MinoError::new(ErrorCategory::EnvironmentUnavailable, error.into_message())
}

fn git_error(error: &GitError) -> MinoError {
    MinoError::new(ErrorCategory::EnvironmentUnavailable, error.to_string())
}
