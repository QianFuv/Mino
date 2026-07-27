//! Pure task File Map, commit-scope, status, and content-snapshot policy.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Component, Path};

use sha2::{Digest, Sha256};

use crate::domain::{FileChange, Plan, Task};

use super::{
    CommitFileSnapshot, CommitFileSnapshotKind, GitError, GitErrorKind, GitFacts, GitStatusEntry,
    GitStatusKind, expected_worktree_entries, inspect_tree_entries, matches_file_map_path,
};

const MAX_COMMIT_FILE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_COMMIT_TOTAL_BYTES: u64 = 256 * 1024 * 1024;
const HASH_BUFFER_BYTES: usize = 64 * 1024;

/// Returns the exact sorted changed entries eligible for one task commit.
///
/// # Errors
///
/// Returns a policy error for pre-staged, unmerged, renamed, submodule,
/// Mino-owned, out-of-File-Map, out-of-scope, or empty change sets.
pub fn task_commit_entries(
    plan: &Plan,
    task: &Task,
    facts: &GitFacts,
) -> Result<Vec<GitStatusEntry>, GitError> {
    if !facts.staged_paths.is_empty() {
        return Err(policy(format!(
            "Task commit requires an initially empty index; staged paths: {}",
            facts.staged_paths.join(", ")
        )));
    }
    if facts.status.is_empty() {
        return Err(policy(format!(
            "Task {} has no changed files to commit",
            task.id()
        )));
    }
    let mut outside_file_map = Vec::new();
    let mut outside_scope = Vec::new();
    let mut entries = Vec::new();
    for entry in &facts.status {
        validate_entry_shape(entry)?;
        if is_mino_owned_path(plan, &entry.path) {
            return Err(policy(format!(
                "Mino-owned path {} cannot be included in a task commit",
                entry.path
            )));
        }
        if !task.file_map().iter().any(|file| {
            matches_file_map_path(file.path(), &entry.path)
                && compatible_change(file.change(), entry)
        }) {
            outside_file_map.push(entry.path.clone());
        }
        let gate = task
            .commit_gate()
            .ok_or_else(|| policy(format!("Task {} has no declared commit gate", task.id())))?;
        if !gate
            .scope()
            .iter()
            .any(|scope| matches_file_map_path(scope, &entry.path))
        {
            outside_scope.push(entry.path.clone());
        }
        entries.push(entry.clone());
    }
    if !outside_file_map.is_empty() {
        return Err(policy(format!(
            "Changed paths outside task {} File Map: {}",
            task.id(),
            outside_file_map.join(", ")
        )));
    }
    if !outside_scope.is_empty() {
        return Err(policy(format!(
            "Changed paths outside task {} commit scope: {}",
            task.id(),
            outside_scope.join(", ")
        )));
    }
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    if !entries.windows(2).all(|pair| pair[0].path < pair[1].path) {
        return Err(invalid("Task commit entries are not uniquely sorted"));
    }
    Ok(entries)
}

/// Captures bounded content fingerprints before the Git index is changed.
///
/// # Errors
///
/// Returns a policy or environment error for unsafe paths, unsupported file
/// kinds, status/filesystem disagreement, oversized input, or read failure.
pub fn capture_commit_snapshots(
    root: &Path,
    entries: &[GitStatusEntry],
) -> Result<Vec<CommitFileSnapshot>, GitError> {
    let mut snapshots = Vec::with_capacity(entries.len());
    let mut total_bytes = 0_u64;
    for entry in entries {
        let path = safe_join(root, &entry.path)?;
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => Some(metadata),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(path_error("inspect", &path, &error)),
        };
        let snapshot = match metadata {
            None if entry.index_status == 'D' || entry.worktree_status == 'D' => {
                CommitFileSnapshot {
                    path: entry.path.clone(),
                    kind: CommitFileSnapshotKind::Deleted,
                    digest: deletion_digest(),
                    length: 0,
                    executable: false,
                    expected_git_entry: None,
                    index_status: entry.index_status,
                    worktree_status: entry.worktree_status,
                }
            }
            None => {
                return Err(invalid(format!(
                    "Changed path {} disappeared before commit preparation",
                    entry.path
                )));
            }
            Some(metadata) if metadata.file_type().is_symlink() => {
                return Err(policy(format!(
                    "Task commit does not support symbolic link path {}",
                    entry.path
                )));
            }
            Some(metadata) if !metadata.is_file() => {
                return Err(policy(format!(
                    "Task commit supports only regular files and deletions: {}",
                    entry.path
                )));
            }
            Some(metadata) => {
                total_bytes = total_bytes.checked_add(metadata.len()).ok_or_else(|| {
                    policy("Task commit file sizes overflowed the policy counter")
                })?;
                if metadata.len() > MAX_COMMIT_FILE_BYTES || total_bytes > MAX_COMMIT_TOTAL_BYTES {
                    return Err(policy(format!(
                        "Task commit content exceeds the bounded snapshot limit at {}",
                        entry.path
                    )));
                }
                CommitFileSnapshot {
                    path: entry.path.clone(),
                    kind: CommitFileSnapshotKind::File,
                    digest: hash_file(&path)?,
                    length: metadata.len(),
                    executable: is_executable(&metadata),
                    expected_git_entry: None,
                    index_status: entry.index_status,
                    worktree_status: entry.worktree_status,
                }
            }
        };
        snapshots.push(snapshot);
    }
    let files = snapshots
        .iter()
        .filter(|snapshot| snapshot.kind == CommitFileSnapshotKind::File)
        .map(|snapshot| (snapshot.path.clone(), snapshot.executable))
        .collect::<Vec<_>>();
    let mut expected = expected_worktree_entries(root, &files)?
        .into_iter()
        .map(|entry| (entry.path().to_owned(), entry.entry().clone()))
        .collect::<BTreeMap<_, _>>();
    for snapshot in &mut snapshots {
        if snapshot.kind == CommitFileSnapshotKind::File {
            snapshot.expected_git_entry =
                Some(expected.remove(&snapshot.path).ok_or_else(|| {
                    invalid(format!(
                        "Git did not return an expected blob identity for {}",
                        snapshot.path
                    ))
                })?);
        }
    }
    if !expected.is_empty() {
        return Err(invalid("Git returned unexpected prepared blob identities"));
    }
    Ok(snapshots)
}

/// Verifies that working-tree content still matches a prepared intent.
///
/// # Errors
///
/// Returns a drift or environment error when a path changed, became an
/// unsupported type, exceeded bounds, or could not be read.
pub fn verify_commit_snapshots(
    root: &Path,
    expected: &[CommitFileSnapshot],
) -> Result<(), GitError> {
    let entries = expected
        .iter()
        .map(|snapshot| GitStatusEntry {
            path: snapshot.path.clone(),
            original_path: None,
            index_status: snapshot.index_status,
            worktree_status: snapshot.worktree_status,
            submodule: "N...".to_owned(),
            kind: if snapshot.index_status == '?' {
                GitStatusKind::Untracked
            } else {
                GitStatusKind::Ordinary
            },
        })
        .collect::<Vec<_>>();
    let actual = capture_commit_snapshots(root, &entries)?;
    if actual == expected {
        Ok(())
    } else {
        Err(invalid(
            "Task file content or mode changed after commit intent preparation",
        ))
    }
}

/// Verifies that a Git commit or tree contains the exact prepared blobs and modes.
///
/// # Errors
///
/// Returns a drift-style invalid-output error when a file entry differs or a
/// prepared deletion remains present in the supplied tree.
pub fn verify_tree_matches_commit_snapshots(
    root: &Path,
    revision: &str,
    expected: &[CommitFileSnapshot],
) -> Result<(), GitError> {
    let paths = expected
        .iter()
        .map(|snapshot| snapshot.path.clone())
        .collect::<Vec<_>>();
    let actual = inspect_tree_entries(root, revision, &paths)?
        .into_iter()
        .map(|entry| (entry.path().to_owned(), entry.entry().clone()))
        .collect::<BTreeMap<_, _>>();
    for snapshot in expected {
        match snapshot.kind {
            CommitFileSnapshotKind::File
                if actual.get(&snapshot.path) == snapshot.expected_git_entry.as_ref() => {}
            CommitFileSnapshotKind::Deleted if !actual.contains_key(&snapshot.path) => {}
            _ => {
                return Err(invalid(format!(
                    "Git tree entry for {} differs from checked worktree content",
                    snapshot.path
                )));
            }
        }
    }
    Ok(())
}

/// Revalidates prepared paths against the current task File Map and commit scope.
///
/// # Errors
///
/// Returns a policy error when a later plan revision no longer authorizes an
/// exact prepared path or its captured change kind.
pub fn validate_commit_snapshot_scope(
    task: &Task,
    snapshots: &[CommitFileSnapshot],
) -> Result<(), GitError> {
    let gate = task
        .commit_gate()
        .ok_or_else(|| policy(format!("Task {} has no declared commit gate", task.id())))?;
    for snapshot in snapshots {
        let entry = GitStatusEntry {
            path: snapshot.path.clone(),
            original_path: None,
            index_status: snapshot.index_status,
            worktree_status: snapshot.worktree_status,
            submodule: "N...".to_owned(),
            kind: if snapshot.index_status == '?' {
                GitStatusKind::Untracked
            } else {
                GitStatusKind::Ordinary
            },
        };
        if !task.file_map().iter().any(|file| {
            matches_file_map_path(file.path(), &snapshot.path)
                && compatible_change(file.change(), &entry)
        }) || !gate
            .scope()
            .iter()
            .any(|scope| matches_file_map_path(scope, &snapshot.path))
        {
            return Err(policy(format!(
                "Prepared commit path {} is no longer authorized by task {}",
                snapshot.path,
                task.id()
            )));
        }
    }
    Ok(())
}

fn validate_entry_shape(entry: &GitStatusEntry) -> Result<(), GitError> {
    if matches!(
        entry.kind,
        GitStatusKind::Unmerged | GitStatusKind::RenamedOrCopied | GitStatusKind::Ignored
    ) || entry.is_submodule()
    {
        Err(policy(format!(
            "Task commit cannot safely stage {:?} path {}",
            entry.kind, entry.path
        )))
    } else {
        Ok(())
    }
}

fn compatible_change(change: FileChange, entry: &GitStatusEntry) -> bool {
    let is_added = entry.kind == GitStatusKind::Untracked
        || entry.index_status == 'A'
        || entry.worktree_status == 'A';
    let is_deleted = entry.index_status == 'D' || entry.worktree_status == 'D';
    match change {
        FileChange::Create => is_added,
        FileChange::Modify => !is_added && !is_deleted,
        FileChange::Delete => is_deleted,
        FileChange::Test => true,
        FileChange::NotApplicable => false,
    }
}

fn is_mino_owned_path(plan: &Plan, path: &str) -> bool {
    path == ".mino" || path.starts_with(".mino/") || plan.metadata().markdown_path() == Some(path)
}

fn safe_join(root: &Path, relative: &str) -> Result<std::path::PathBuf, GitError> {
    let path = Path::new(relative);
    if relative.is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        Err(invalid(format!("Unsafe task commit path {relative}")))
    } else {
        Ok(root.join(path))
    }
}

fn hash_file(path: &Path) -> Result<String, GitError> {
    let mut file = File::open(path).map_err(|error| path_error("open", path, &error))?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; HASH_BUFFER_BYTES].into_boxed_slice();
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|error| path_error("read", path, &error))?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(format!("sha256:{:x}", digest.finalize()))
}

fn deletion_digest() -> String {
    let digest = Sha256::digest(b"mino:deleted:v1");
    format!("sha256:{digest:x}")
}

#[cfg(unix)]
fn is_executable(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;

    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_metadata: &fs::Metadata) -> bool {
    false
}

fn invalid(message: impl Into<String>) -> GitError {
    GitError::new(GitErrorKind::InvalidOutput, message)
}

fn policy(message: impl Into<String>) -> GitError {
    GitError::new(GitErrorKind::PolicyViolation, message)
}

fn path_error(action: &str, path: &Path, error: &std::io::Error) -> GitError {
    GitError::new(
        GitErrorKind::Unavailable,
        format!("Failed to {action} {}: {error}", path.display()),
    )
}
