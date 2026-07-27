//! Bounded workspace fingerprint capture for verification freshness.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

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
    capture_scope(root, plan, scope)
}

pub(crate) fn capture_workspace_baseline(
    root: &Path,
    plan: &Plan,
) -> Result<WorkspaceFingerprint, MinoError> {
    let mut patterns = vec!["**".to_owned()];
    patterns.extend(
        plan.tasks()
            .iter()
            .flat_map(crate::domain::Task::file_map)
            .filter(|entry| entry.change() != FileChange::NotApplicable)
            .map(|entry| entry.path().to_owned()),
    );
    capture_scope(
        root,
        plan,
        WorkspaceFingerprintScope::new(WorkspaceScopeKind::Global, None, patterns),
    )
}

pub(crate) fn capture_task_workspace_baseline(
    root: &Path,
    plan: &Plan,
    task_id: &TaskId,
) -> Result<WorkspaceFingerprint, MinoError> {
    let task = plan.task(task_id).ok_or_else(|| {
        MinoError::new(
            ErrorCategory::IncompleteOrValidation,
            format!("Task {task_id} does not exist"),
        )
    })?;
    let mut patterns = vec!["**".to_owned()];
    patterns.extend(
        task.file_map()
            .iter()
            .filter(|entry| entry.change() != FileChange::NotApplicable)
            .map(|entry| entry.path().to_owned()),
    );
    capture_scope(
        root,
        plan,
        WorkspaceFingerprintScope::new(WorkspaceScopeKind::Global, None, patterns),
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WorkspaceDeltaKind {
    Created,
    Modified,
    Deleted,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct WorkspaceDeltaEntry {
    path: String,
    kind: WorkspaceDeltaKind,
}

impl WorkspaceDeltaEntry {
    pub(crate) fn path(&self) -> &str {
        &self.path
    }

    pub(crate) const fn kind(&self) -> WorkspaceDeltaKind {
        self.kind
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct WorkspaceDelta {
    entries: Vec<WorkspaceDeltaEntry>,
    repository_head_changed: bool,
}

impl WorkspaceDelta {
    pub(crate) fn entries(&self) -> &[WorkspaceDeltaEntry] {
        &self.entries
    }

    pub(crate) const fn repository_head_changed(&self) -> bool {
        self.repository_head_changed
    }
}

pub(crate) fn workspace_delta(
    root: &Path,
    plan: &Plan,
    baseline: &WorkspaceFingerprint,
) -> Result<WorkspaceDelta, MinoError> {
    let current = recapture_workspace_fingerprint(root, plan, baseline)?;
    let before_files = baseline
        .file_snapshots()
        .iter()
        .map(|snapshot| (snapshot.path(), snapshot))
        .collect::<BTreeMap<_, _>>();
    let after_files = current
        .file_snapshots()
        .iter()
        .map(|snapshot| (snapshot.path(), snapshot))
        .collect::<BTreeMap<_, _>>();
    let before_status = baseline
        .status_entries()
        .iter()
        .map(|entry| (entry.path(), entry))
        .collect::<BTreeMap<_, _>>();
    let after_status = current
        .status_entries()
        .iter()
        .map(|entry| (entry.path(), entry))
        .collect::<BTreeMap<_, _>>();
    let paths = before_files
        .keys()
        .chain(after_files.keys())
        .chain(before_status.keys())
        .chain(after_status.keys())
        .copied()
        .collect::<BTreeSet<_>>();
    let entries = paths
        .into_iter()
        .filter(|path| {
            before_files.get(path) != after_files.get(path)
                || before_status.get(path) != after_status.get(path)
        })
        .map(|path| WorkspaceDeltaEntry {
            path: path.to_owned(),
            kind: delta_kind(
                before_files.get(path).copied(),
                after_files.get(path).copied(),
            ),
        })
        .collect::<Vec<_>>();
    let repository_head_changed = baseline.repository_mode() != current.repository_mode()
        || baseline.head() != current.head();
    Ok(WorkspaceDelta {
        entries,
        repository_head_changed,
    })
}

fn delta_kind(
    before: Option<&WorkspaceFileSnapshot>,
    after: Option<&WorkspaceFileSnapshot>,
) -> WorkspaceDeltaKind {
    let existed_before =
        before.is_some_and(|snapshot| snapshot.kind() != WorkspaceFileKind::Missing);
    let exists_after = after.is_some_and(|snapshot| snapshot.kind() != WorkspaceFileKind::Missing);
    match (existed_before, exists_after) {
        (false, true) => WorkspaceDeltaKind::Created,
        (true, false) => WorkspaceDeltaKind::Deleted,
        _ => WorkspaceDeltaKind::Modified,
    }
}

fn capture_scope(
    root: &Path,
    plan: &Plan,
    scope: WorkspaceFingerprintScope,
) -> Result<WorkspaceFingerprint, MinoError> {
    let filesystem = ProjectFs::open(root).map_err(managed_error)?;
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
    let mut paths = scoped_paths(&filesystem, plan, &scope)?;
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
    let current = capture_scope(root, plan, expected.scope().clone())?;
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
    plan: &Plan,
    scope: &WorkspaceFingerprintScope,
) -> Result<BTreeSet<String>, MinoError> {
    let mut paths = BTreeSet::new();
    let mut walk_patterns = Vec::new();
    for pattern in scope.patterns() {
        if is_mino_owned_path(plan, pattern) {
            continue;
        }
        let requires_walk = if pattern.contains('*') {
            true
        } else {
            let managed = ManagedPath::new(pattern).map_err(managed_error)?;
            filesystem.is_directory(&managed).map_err(managed_error)?
        };
        if requires_walk {
            walk_patterns.push(pattern.clone());
        } else {
            paths.insert(pattern.clone());
        }
    }
    if !walk_patterns.is_empty() {
        collect_matching_paths(
            filesystem,
            plan,
            filesystem.root(),
            &walk_patterns,
            true,
            &mut paths,
        )?;
    }
    let explicit_patterns = explicit_scope_patterns(plan, scope);
    for pattern in walk_patterns
        .iter()
        .filter(|pattern| explicit_patterns.contains(pattern.as_str()))
    {
        collect_explicit_matching_paths(filesystem, plan, pattern, &mut paths)?;
    }
    if paths.len() > MAX_FINGERPRINT_FILES {
        return Err(fingerprint_budget_error("file count"));
    }
    Ok(paths)
}

fn collect_matching_paths(
    filesystem: &ProjectFs,
    plan: &Plan,
    walk_root: &Path,
    patterns: &[String],
    standard_filters: bool,
    paths: &mut BTreeSet<String>,
) -> Result<(), MinoError> {
    let root = filesystem.root();
    let mut builder = WalkBuilder::new(walk_root);
    builder
        .standard_filters(standard_filters)
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
        if is_mino_owned_path(plan, &path) {
            continue;
        }
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

fn explicit_scope_patterns<'a>(
    plan: &'a Plan,
    scope: &WorkspaceFingerprintScope,
) -> BTreeSet<&'a str> {
    let scoped = scope
        .patterns()
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    plan.tasks()
        .iter()
        .flat_map(crate::domain::Task::file_map)
        .filter(|entry| entry.change() != FileChange::NotApplicable)
        .map(crate::domain::FileMapEntry::path)
        .filter(|pattern| scoped.contains(pattern))
        .collect()
}

fn collect_explicit_matching_paths(
    filesystem: &ProjectFs,
    plan: &Plan,
    pattern: &str,
    paths: &mut BTreeSet<String>,
) -> Result<(), MinoError> {
    let Some(walk_root) = explicit_walk_root(filesystem, plan, pattern)? else {
        return Ok(());
    };
    collect_matching_paths(
        filesystem,
        plan,
        &walk_root,
        &[pattern.to_owned()],
        false,
        paths,
    )
}

fn explicit_walk_root(
    filesystem: &ProjectFs,
    plan: &Plan,
    pattern: &str,
) -> Result<Option<PathBuf>, MinoError> {
    let prefix = pattern
        .split('/')
        .take_while(|segment| !segment.contains('*'))
        .collect::<Vec<_>>()
        .join("/");
    if prefix.is_empty() {
        return Ok(Some(filesystem.root().to_path_buf()));
    }
    if is_mino_owned_path(plan, &prefix) {
        return Ok(None);
    }
    let managed = ManagedPath::new(&prefix).map_err(managed_error)?;
    match filesystem.entry_kind(&managed).map_err(managed_error)? {
        None | Some(ManagedEntryKind::File) => Ok(None),
        Some(ManagedEntryKind::Directory) => Ok(Some(filesystem.root().join(prefix))),
        Some(ManagedEntryKind::Symlink) => Err(MinoError::new(
            ErrorCategory::PolicyViolation,
            format!("Workspace fingerprint path {prefix} is a symbolic link"),
        )),
        Some(ManagedEntryKind::Other) => Err(MinoError::new(
            ErrorCategory::PolicyViolation,
            format!("Workspace fingerprint path {prefix} has an unsupported file type"),
        )),
    }
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
    path == ".git"
        || path.starts_with(".git/")
        || path == ".mino"
        || path.starts_with(".mino/")
        || plan.metadata().markdown_path() == Some(path)
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
