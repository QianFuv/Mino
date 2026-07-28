//! Repository, worktree, HEAD, index, and porcelain fact inspection.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::command::{GitCommandOutput, run_probe, run_read_only};
use super::porcelain::{GitStatusEntry, GitStatusKind, parse_porcelain_v2};
use super::{GitError, GitErrorKind};

/// Current repository HEAD classification.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GitHeadState {
    /// The inspected path is not in a Git repository.
    NotRepository,
    /// A branch has a current commit.
    Branch,
    /// HEAD names a branch without a first commit.
    Unborn,
    /// HEAD points directly to a commit.
    Detached,
    /// A bare repository has no worktree status surface.
    Bare,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum GitRootProbe {
    Found(PathBuf),
    NotRepository,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum GitReadinessProbe {
    NotRepository,
    Repository {
        is_clean: bool,
        branch: Option<String>,
        base_commit: Option<String>,
    },
}

/// Explicit repository availability returned by Git inspection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GitAvailability {
    /// Git confirmed that the supplied path is not inside a repository.
    NotRepository,
    /// Git returned complete, validated repository facts.
    Available(Box<GitFacts>),
}

/// Complete read-only Git facts for one supplied path.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GitFacts {
    /// Whether Git recognizes a repository at the supplied path.
    pub repository: bool,
    /// Whether the repository exposes a working tree.
    pub is_worktree: bool,
    /// Canonical inspected start directory.
    pub inspected_path: PathBuf,
    /// Canonical worktree root when present.
    pub worktree: Option<PathBuf>,
    /// Canonical shared Git common directory.
    pub common_dir: Option<PathBuf>,
    /// Canonical worktree-specific Git directory.
    pub git_dir: Option<PathBuf>,
    /// Resolved index path for the current worktree.
    pub index_file: Option<PathBuf>,
    /// Current branch name, including an unborn branch.
    pub branch: Option<String>,
    /// Full current commit object ID.
    pub head: Option<String>,
    /// Explicit HEAD classification.
    pub head_state: GitHeadState,
    /// Optional upstream name from porcelain headers.
    pub upstream: Option<String>,
    /// Optional ahead count from porcelain headers.
    pub ahead: Option<u64>,
    /// Optional behind count from porcelain headers.
    pub behind: Option<u64>,
    /// Sorted detailed status entries.
    pub status: Vec<GitStatusEntry>,
    /// Sorted paths with index changes.
    pub staged_paths: Vec<String>,
    /// Sorted paths with worktree changes, including untracked paths.
    pub unstaged_paths: Vec<String>,
    /// Sorted untracked paths.
    pub untracked_paths: Vec<String>,
    /// Whether no staged, unstaged, unmerged, or untracked entries exist.
    pub is_clean: bool,
}

impl GitFacts {
    /// Returns whether a normal branch is checked out.
    #[must_use]
    pub fn has_branch(&self) -> bool {
        self.head_state == GitHeadState::Branch || self.head_state == GitHeadState::Unborn
    }
}

/// Read-only adapter for deterministic Git repository facts.
#[derive(Clone, Debug)]
pub struct GitAdapter {
    start: PathBuf,
}

impl GitAdapter {
    /// Creates an adapter rooted at the supplied path.
    #[must_use]
    pub fn new(start: impl Into<PathBuf>) -> Self {
        Self {
            start: start.into(),
        }
    }

    /// Inspects repository, worktree, HEAD, index, and porcelain-v2 facts.
    ///
    /// # Errors
    ///
    /// Returns an error when the start path or Git executable is unavailable,
    /// or when Git violates the expected machine-readable contracts.
    pub fn inspect(&self) -> Result<GitFacts, GitError> {
        let inspected_path = canonical_directory(&self.start)?;
        match inspect_availability_at(inspected_path.clone())? {
            GitAvailability::NotRepository => Ok(not_repository(inspected_path)),
            GitAvailability::Available(facts) => Ok(*facts),
        }
    }

    /// Distinguishes an explicit non-repository from validated Git facts.
    ///
    /// # Errors
    ///
    /// Returns an error when Git is unavailable, repository metadata is
    /// present but unusable, or machine-readable output is invalid.
    pub fn inspect_availability(&self) -> Result<GitAvailability, GitError> {
        inspect_availability_at(canonical_directory(&self.start)?)
    }

    pub(crate) fn index_entries(&self) -> Result<Vec<u8>, GitError> {
        let root = canonical_directory(&self.start)?;
        let output = run_read_only(&root, ["ls-files", "--stage", "-z", "--full-name"])?;
        require_success(&output, "Git index inspection")?;
        Ok(output.stdout)
    }

    pub(crate) fn probe_readiness(&self) -> Result<GitReadinessProbe, GitError> {
        let root = canonical_directory(&self.start)?;
        let inside = run_probe(&root, ["rev-parse", "--is-inside-work-tree"])?;
        if !inside.success {
            return if is_explicit_not_repository(&root, &inside)? {
                Ok(GitReadinessProbe::NotRepository)
            } else {
                Err(failed_command(&inside, "Git worktree probe"))
            };
        }
        if output_text(&inside, "Git worktree probe")? != "true" {
            return Ok(GitReadinessProbe::NotRepository);
        }
        let status = run_probe(
            &root,
            [
                "status",
                "--porcelain=v1",
                "-z",
                "--untracked-files=all",
                "--no-renames",
            ],
        )?;
        require_success(&status, "Git readiness status")?;
        let branch_output = run_probe(&root, ["branch", "--show-current"])?;
        require_success(&branch_output, "Git branch probe")?;
        let branch = optional_output_text(&branch_output, "Git branch probe")?;
        let head_output = run_probe(&root, ["rev-parse", "--short", "HEAD"])?;
        let base_commit = if head_output.success {
            Some(output_text(&head_output, "Git HEAD probe")?)
        } else if branch.is_some() {
            None
        } else {
            return Err(failed_command(&head_output, "Git HEAD probe"));
        };
        Ok(GitReadinessProbe::Repository {
            is_clean: status.stdout.is_empty(),
            branch,
            base_commit,
        })
    }
}

pub(crate) fn probe_root(start: &Path) -> Result<GitRootProbe, GitError> {
    let start = canonical_directory(start)?;
    let output = run_probe(&start, ["rev-parse", "--show-toplevel"])?;
    if !output.success {
        return if is_explicit_not_repository(&start, &output)? {
            Ok(GitRootProbe::NotRepository)
        } else {
            Err(failed_command(&output, "Git root probe"))
        };
    }
    let path = PathBuf::from(output_text(&output, "Git root probe")?);
    let canonical = path.canonicalize().map_err(|error| {
        GitError::new(
            GitErrorKind::Unavailable,
            format!("Failed to resolve Git root {}: {error}", path.display()),
        )
    })?;
    if canonical.is_dir() {
        Ok(GitRootProbe::Found(canonical))
    } else {
        Err(invalid("Git root probe did not return a directory"))
    }
}

fn inspect_availability_at(inspected_path: PathBuf) -> Result<GitAvailability, GitError> {
    let inside = run_probe(&inspected_path, ["rev-parse", "--is-inside-work-tree"])?;
    if !inside.success {
        return if is_explicit_not_repository(&inspected_path, &inside)? {
            Ok(GitAvailability::NotRepository)
        } else {
            Err(failed_command(&inside, "Git worktree probe"))
        };
    }
    match output_text(&inside, "Git worktree probe")?.as_str() {
        "true" => inspect_worktree(inspected_path)
            .map(Box::new)
            .map(GitAvailability::Available),
        "false" => inspect_bare_or_non_repository(inspected_path),
        _ => Err(invalid("Git returned an invalid worktree probe value")),
    }
}

fn inspect_worktree(inspected_path: PathBuf) -> Result<GitFacts, GitError> {
    let worktree = git_existing_path(&inspected_path, &["rev-parse", "--show-toplevel"], true)?;
    let common_dir = git_existing_path(&worktree, &["rev-parse", "--git-common-dir"], true)?;
    let git_dir = git_existing_path(&worktree, &["rev-parse", "--git-dir"], true)?;
    let index_file = git_existing_path(&worktree, &["rev-parse", "--git-path", "index"], false)?;
    let output = run_read_only(
        &worktree,
        [
            "status",
            "--porcelain=v2",
            "--branch",
            "-z",
            "--untracked-files=all",
            "--ignored=no",
            "--no-renames",
        ],
    )?;
    require_success(&output, "Git status")?;
    let parsed = parse_porcelain_v2(&output.stdout)?;
    let branch_head = parsed
        .branch_head
        .as_deref()
        .ok_or_else(|| invalid("Git status omitted the branch.head header"))?;
    let (head_state, branch) = if parsed.branch_oid.is_none() {
        if branch_head == "(detached)" {
            return Err(invalid("An unborn Git HEAD cannot be detached"));
        }
        (GitHeadState::Unborn, Some(branch_head.to_owned()))
    } else if branch_head == "(detached)" {
        (GitHeadState::Detached, None)
    } else {
        (GitHeadState::Branch, Some(branch_head.to_owned()))
    };
    let staged_paths = selected_paths(&parsed.entries, GitStatusEntry::is_staged);
    let unstaged_paths = selected_paths(&parsed.entries, GitStatusEntry::is_unstaged);
    let untracked_paths = parsed
        .entries
        .iter()
        .filter(|entry| entry.kind == GitStatusKind::Untracked)
        .map(|entry| entry.path.clone())
        .collect::<Vec<_>>();
    let is_clean = parsed.entries.is_empty();
    Ok(GitFacts {
        repository: true,
        is_worktree: true,
        inspected_path,
        worktree: Some(worktree),
        common_dir: Some(common_dir),
        git_dir: Some(git_dir),
        index_file: Some(index_file),
        branch,
        head: parsed.branch_oid,
        head_state,
        upstream: parsed.branch_upstream,
        ahead: parsed.ahead,
        behind: parsed.behind,
        status: parsed.entries,
        staged_paths,
        unstaged_paths,
        untracked_paths,
        is_clean,
    })
}

fn inspect_bare_or_non_repository(inspected_path: PathBuf) -> Result<GitAvailability, GitError> {
    let bare = run_read_only(&inspected_path, ["rev-parse", "--is-bare-repository"])?;
    if !bare.success {
        return if is_explicit_not_repository(&inspected_path, &bare)? {
            Ok(GitAvailability::NotRepository)
        } else {
            Err(failed_command(&bare, "Git bare-repository probe"))
        };
    }
    if output_text(&bare, "Git bare-repository probe")? != "true" {
        return if has_git_marker(&inspected_path)? {
            Err(GitError::new(
                GitErrorKind::Unavailable,
                "Git repository metadata is present but unusable",
            ))
        } else {
            Ok(GitAvailability::NotRepository)
        };
    }
    let common_dir = git_existing_path(&inspected_path, &["rev-parse", "--git-common-dir"], true)?;
    let git_dir = git_existing_path(&inspected_path, &["rev-parse", "--git-dir"], true)?;
    Ok(GitAvailability::Available(Box::new(GitFacts {
        repository: true,
        is_worktree: false,
        inspected_path,
        worktree: None,
        common_dir: Some(common_dir),
        git_dir: Some(git_dir),
        index_file: None,
        branch: None,
        head: None,
        head_state: GitHeadState::Bare,
        upstream: None,
        ahead: None,
        behind: None,
        status: Vec::new(),
        staged_paths: Vec::new(),
        unstaged_paths: Vec::new(),
        untracked_paths: Vec::new(),
        is_clean: false,
    })))
}

fn not_repository(inspected_path: PathBuf) -> GitFacts {
    GitFacts {
        repository: false,
        is_worktree: false,
        inspected_path,
        worktree: None,
        common_dir: None,
        git_dir: None,
        index_file: None,
        branch: None,
        head: None,
        head_state: GitHeadState::NotRepository,
        upstream: None,
        ahead: None,
        behind: None,
        status: Vec::new(),
        staged_paths: Vec::new(),
        unstaged_paths: Vec::new(),
        untracked_paths: Vec::new(),
        is_clean: false,
    }
}

fn selected_paths(
    entries: &[GitStatusEntry],
    predicate: impl Fn(&GitStatusEntry) -> bool,
) -> Vec<String> {
    entries
        .iter()
        .filter(|entry| predicate(entry))
        .map(|entry| entry.path.clone())
        .collect()
}

fn git_existing_path(
    root: &Path,
    arguments: &[&str],
    must_exist: bool,
) -> Result<PathBuf, GitError> {
    let output = run_read_only(root, arguments)?;
    require_success(&output, "Git path query")?;
    let value = output_text(&output, "Git path query")?;
    let path = PathBuf::from(value);
    let path = if path.is_absolute() {
        path
    } else {
        root.join(path)
    };
    if must_exist {
        path.canonicalize().map_err(|error| {
            GitError::new(
                GitErrorKind::Unavailable,
                format!("Failed to resolve Git path {}: {error}", path.display()),
            )
        })
    } else if path.exists() {
        path.canonicalize().map_err(|error| {
            GitError::new(
                GitErrorKind::Unavailable,
                format!("Failed to resolve Git path {}: {error}", path.display()),
            )
        })
    } else {
        let parent = path
            .parent()
            .ok_or_else(|| invalid("Git path has no parent directory"))?;
        let file_name = path
            .file_name()
            .ok_or_else(|| invalid("Git path has no file name"))?;
        Ok(parent
            .canonicalize()
            .map_err(|error| {
                GitError::new(
                    GitErrorKind::Unavailable,
                    format!(
                        "Failed to resolve Git path parent {}: {error}",
                        parent.display()
                    ),
                )
            })?
            .join(file_name))
    }
}

fn canonical_directory(path: &Path) -> Result<PathBuf, GitError> {
    let canonical = path.canonicalize().map_err(|error| {
        GitError::new(
            GitErrorKind::Unavailable,
            format!(
                "Failed to resolve Git inspection path {}: {error}",
                path.display()
            ),
        )
    })?;
    if canonical.is_dir() {
        Ok(canonical)
    } else if canonical.is_file() {
        canonical
            .parent()
            .map(Path::to_path_buf)
            .ok_or_else(|| invalid("Git inspection file has no parent directory"))
    } else {
        Err(GitError::new(
            GitErrorKind::Unavailable,
            format!(
                "Git inspection path {} is not a file or directory",
                canonical.display()
            ),
        ))
    }
}

fn require_success(output: &GitCommandOutput, operation: &str) -> Result<(), GitError> {
    if output.success {
        Ok(())
    } else {
        Err(failed_command(output, operation))
    }
}

fn output_text(output: &GitCommandOutput, operation: &str) -> Result<String, GitError> {
    let value = std::str::from_utf8(&output.stdout)
        .map_err(|_| invalid(format!("{operation} returned non-UTF-8 output")))?;
    let value = value.trim_end_matches(['\r', '\n']);
    if value.is_empty() || value.contains(['\r', '\n', '\0']) {
        Err(invalid(format!(
            "{operation} returned an invalid text value"
        )))
    } else {
        Ok(value.to_owned())
    }
}

fn optional_output_text(
    output: &GitCommandOutput,
    operation: &str,
) -> Result<Option<String>, GitError> {
    if output.stdout.is_empty() {
        return Ok(None);
    }
    output_text(output, operation).map(Some)
}

fn is_not_repository(output: &GitCommandOutput) -> bool {
    output.exit_code == Some(128)
        && String::from_utf8_lossy(&output.stderr).contains("not a git repository")
}

fn is_explicit_not_repository(
    inspected_path: &Path,
    output: &GitCommandOutput,
) -> Result<bool, GitError> {
    Ok(is_not_repository(output) && !has_git_marker(inspected_path)?)
}

fn has_git_marker(inspected_path: &Path) -> Result<bool, GitError> {
    for ancestor in inspected_path.ancestors() {
        match fs::symlink_metadata(ancestor.join(".git")) {
            Ok(_) => return Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => {
                return Err(GitError::new(
                    GitErrorKind::Unavailable,
                    "Git repository marker could not be inspected",
                ));
            }
        }
    }
    Ok(false)
}

fn failed_command(output: &GitCommandOutput, operation: &str) -> GitError {
    GitError::new(
        GitErrorKind::Unavailable,
        format!("{operation} failed with exit {:?}", output.exit_code),
    )
}

fn invalid(message: impl Into<String>) -> GitError {
    GitError::new(GitErrorKind::InvalidOutput, message)
}
