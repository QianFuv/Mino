//! Read-only changed-file inspection and narrow File Map path matching.

use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::path::{Component, Path};
use std::process::Command;

/// Stable categories for changed-file inspection failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GitChangeErrorKind {
    /// The Git executable or repository path is unavailable.
    Unavailable,
    /// Git returned malformed or non-UTF-8 porcelain output.
    InvalidOutput,
}

/// A typed read-only Git inspection failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitChangeError {
    kind: GitChangeErrorKind,
    message: String,
}

impl GitChangeError {
    fn new(kind: GitChangeErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    /// Returns the stable inspection error category.
    #[must_use]
    pub const fn kind(&self) -> GitChangeErrorKind {
        self.kind
    }

    /// Returns the explanatory failure message.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl Display for GitChangeError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for GitChangeError {}

/// One project-relative path and its two porcelain status columns.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChangedFile {
    path: String,
    index_status: char,
    worktree_status: char,
}

impl ChangedFile {
    /// Returns the normalized project-relative changed path.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Returns the Git index status column.
    #[must_use]
    pub const fn index_status(&self) -> char {
        self.index_status
    }

    /// Returns the Git worktree status column.
    #[must_use]
    pub const fn worktree_status(&self) -> char {
        self.worktree_status
    }

    /// Returns whether either status column reports deletion.
    #[must_use]
    pub const fn is_deleted(&self) -> bool {
        self.index_status == 'D' || self.worktree_status == 'D'
    }

    /// Returns whether the path is newly added or untracked.
    #[must_use]
    pub const fn is_added(&self) -> bool {
        self.index_status == 'A'
            || self.worktree_status == 'A'
            || self.index_status == '?'
            || self.worktree_status == '?'
    }
}

/// Deterministically sorted changed paths or an explicit non-repository result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitChangeSet {
    is_repository: bool,
    files: Vec<ChangedFile>,
}

impl GitChangeSet {
    /// Returns whether the inspected root belongs to a Git work tree.
    #[must_use]
    pub const fn is_repository(&self) -> bool {
        self.is_repository
    }

    /// Returns changed files sorted by normalized protocol path.
    #[must_use]
    pub fn files(&self) -> &[ChangedFile] {
        &self.files
    }
}

/// Inspects worktree and index paths without modifying Git state.
///
/// # Errors
///
/// Returns an error when Git cannot be launched or emits malformed porcelain
/// output. A directory outside Git is a successful non-repository result.
pub fn inspect_changes(root: &Path) -> Result<GitChangeSet, GitChangeError> {
    let probe = Command::new("git")
        .args([
            "-C",
            &root.to_string_lossy(),
            "rev-parse",
            "--is-inside-work-tree",
        ])
        .output()
        .map_err(|error| unavailable(root, &error))?;
    if !probe.status.success() || probe.stdout != b"true\n" && probe.stdout != b"true\r\n" {
        return Ok(GitChangeSet {
            is_repository: false,
            files: Vec::new(),
        });
    }
    let output = Command::new("git")
        .args([
            "-C",
            &root.to_string_lossy(),
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--no-renames",
        ])
        .output()
        .map_err(|error| unavailable(root, &error))?;
    if !output.status.success() {
        return Err(GitChangeError::new(
            GitChangeErrorKind::Unavailable,
            format!(
                "Git status failed for {}: {}",
                root.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ));
    }
    let mut files = output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
        .map(parse_record)
        .collect::<Result<Vec<_>, _>>()?;
    files.sort_by(|left, right| left.path.cmp(&right.path));
    if files.windows(2).any(|pair| pair[0].path == pair[1].path) {
        return Err(GitChangeError::new(
            GitChangeErrorKind::InvalidOutput,
            "Git porcelain output contains duplicate paths",
        ));
    }
    Ok(GitChangeSet {
        is_repository: true,
        files,
    })
}

/// Matches a normalized project path against an exact or narrow `*`/`**` pattern.
#[must_use]
pub fn matches_file_map_path(pattern: &str, path: &str) -> bool {
    let Some(pattern) = normalized_protocol_path(pattern) else {
        return false;
    };
    let Some(path) = normalized_protocol_path(path) else {
        return false;
    };
    let pattern = pattern.split('/').collect::<Vec<_>>();
    let path = path.split('/').collect::<Vec<_>>();
    matches_segments(&pattern, &path)
}

fn parse_record(record: &[u8]) -> Result<ChangedFile, GitChangeError> {
    if record.len() < 4 || record[2] != b' ' || !record[0].is_ascii() || !record[1].is_ascii() {
        return Err(GitChangeError::new(
            GitChangeErrorKind::InvalidOutput,
            "Git porcelain output contains an invalid status record",
        ));
    }
    let path = std::str::from_utf8(&record[3..]).map_err(|_| {
        GitChangeError::new(
            GitChangeErrorKind::InvalidOutput,
            "Git changed paths must be valid UTF-8",
        )
    })?;
    let path = normalized_protocol_path(path).ok_or_else(|| {
        GitChangeError::new(
            GitChangeErrorKind::InvalidOutput,
            format!("Git returned an unsafe changed path {path}"),
        )
    })?;
    Ok(ChangedFile {
        path,
        index_status: char::from(record[0]),
        worktree_status: char::from(record[1]),
    })
}

fn normalized_protocol_path(value: &str) -> Option<String> {
    let value = value.replace('\\', "/");
    let value = value.strip_prefix("./").unwrap_or(&value);
    let path = Path::new(value);
    if value.is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        None
    } else {
        Some(value.to_owned())
    }
}

fn matches_segments(pattern: &[&str], path: &[&str]) -> bool {
    match pattern.split_first() {
        None => path.is_empty(),
        Some((segment, remaining)) if *segment == "**" => {
            matches_segments(remaining, path)
                || !path.is_empty() && matches_segments(pattern, &path[1..])
        }
        Some((segment, remaining)) => {
            path.split_first()
                .is_some_and(|(path_segment, path_remaining)| {
                    matches_segment(segment.as_bytes(), path_segment.as_bytes())
                        && matches_segments(remaining, path_remaining)
                })
        }
    }
}

fn matches_segment(pattern: &[u8], path: &[u8]) -> bool {
    match pattern.split_first() {
        None => path.is_empty(),
        Some((b'*', remaining)) => {
            matches_segment(remaining, path)
                || !path.is_empty() && matches_segment(pattern, &path[1..])
        }
        Some((expected, remaining)) => {
            path.split_first().is_some_and(|(actual, path_remaining)| {
                expected == actual && matches_segment(remaining, path_remaining)
            })
        }
    }
}

fn unavailable(root: &Path, error: &std::io::Error) -> GitChangeError {
    GitChangeError::new(
        GitChangeErrorKind::Unavailable,
        format!(
            "Failed to inspect Git changes at {}: {error}",
            root.display()
        ),
    )
}
