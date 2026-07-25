//! Explicit-path staging, hook-aware commit execution, and object verification.

use std::path::{Component, Path};

use serde::Serialize;

use super::command::{GitCommandOutput, run_mutating, run_read_only};
use super::{GitError, GitErrorKind};

/// Result of an authorized Git add or commit process.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GitMutationResult {
    /// Whether Git returned success.
    pub success: bool,
    /// Process exit code when supplied by the operating system.
    pub exit_code: Option<i32>,
    /// Bounded diagnostic stderr decoded lossily.
    pub stderr: String,
}

/// Verified immutable facts for one created commit object.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GitCommitObject {
    /// Full commit object ID.
    pub commit: String,
    /// Only parent commit object ID.
    pub parent: String,
    /// Full tree object ID.
    pub tree: String,
    /// Raw one-line commit message without trailing line endings.
    pub message: String,
    /// Sorted exact paths changed by this commit.
    pub files: Vec<String>,
}

/// Rejects task paths that would invoke a configured Git clean filter.
///
/// # Errors
///
/// Returns a policy error for any active filter and an unavailable or
/// invalid-output error for unexpected Git results.
pub fn ensure_no_clean_filters(root: &Path, paths: &[String]) -> Result<(), GitError> {
    if paths.is_empty() {
        return Err(policy(
            "Commit filter inspection requires at least one path",
        ));
    }
    let mut arguments = vec![
        "check-attr".to_owned(),
        "-z".to_owned(),
        "filter".to_owned(),
        "--".to_owned(),
    ];
    arguments.extend(paths.iter().cloned());
    let output = run_read_only(root, arguments)?;
    require_success(&output, "Git attribute inspection")?;
    let records = output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
        .collect::<Vec<_>>();
    if records.len() != paths.len() * 3 {
        return Err(invalid("Git attribute output has an invalid field count"));
    }
    for fields in records.chunks_exact(3) {
        let path = utf8(fields[0], "Git attribute path")?;
        let attribute = utf8(fields[1], "Git attribute name")?;
        let value = utf8(fields[2], "Git attribute value")?;
        if attribute != "filter" || !paths.iter().any(|expected| expected == path) {
            return Err(invalid(
                "Git attribute output does not match requested paths",
            ));
        }
        if !matches!(value, "unspecified" | "unset") {
            return Err(policy(format!(
                "Task path {path} uses unsupported Git clean filter {value}"
            )));
        }
    }
    Ok(())
}

/// Stages only the supplied exact repository-relative paths.
///
/// # Errors
///
/// Returns an unavailable or invalid-output error when Git cannot run within
/// the bounded process contract.
pub fn stage_commit_paths(root: &Path, paths: &[String]) -> Result<GitMutationResult, GitError> {
    if paths.is_empty() {
        return Err(policy("Task commit requires at least one exact path"));
    }
    let mut arguments = vec!["add".to_owned(), "--".to_owned()];
    arguments.extend(paths.iter().cloned());
    run_mutating(root, arguments).map(mutation_result)
}

/// Writes and returns the exact current index tree object ID.
///
/// # Errors
///
/// Returns an unavailable or invalid-output error when the index is unmerged
/// or Git does not return one full object ID.
pub fn write_index_tree(root: &Path) -> Result<String, GitError> {
    let output = run_mutating(root, ["write-tree"])?;
    require_success(&output, "Git write-tree")?;
    full_object_text(&output, "Git write-tree")
}

/// Runs one exact planned commit while preserving repository hooks.
///
/// GPG signing is explicitly disabled to avoid interactive or external signing
/// behavior. Hooks are not bypassed.
///
/// # Errors
///
/// Returns an unavailable or invalid-output error when Git cannot run within
/// the bounded process contract.
pub fn run_task_commit(root: &Path, message: &str) -> Result<GitMutationResult, GitError> {
    if message.trim().is_empty() || message.contains(['\r', '\n']) {
        return Err(policy("Task commit message must be one non-empty line"));
    }
    run_mutating(
        root,
        [
            "-c",
            "commit.gpgSign=false",
            "commit",
            "--no-gpg-sign",
            "--cleanup=verbatim",
            "--no-status",
            "-m",
            message,
        ],
    )
    .map(mutation_result)
}

/// Loads and strictly verifies one commit's parent, tree, message, and paths.
///
/// # Errors
///
/// Returns an unavailable or invalid-output error for a missing commit,
/// merge/root commit, malformed object data, unsafe path, or duplicate path.
pub fn inspect_commit(root: &Path, revision: &str) -> Result<GitCommitObject, GitError> {
    let commit = resolve_commit(root, revision)?;
    let parents = run_read_only(root, ["rev-list", "--parents", "-n", "1", &commit, "--"])?;
    require_success(&parents, "Git commit parent inspection")?;
    let parent_fields = output_text(&parents, "Git commit parent inspection")?
        .split_whitespace()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if parent_fields.len() != 2 || parent_fields[0] != commit || !is_object_id(&parent_fields[1]) {
        return Err(invalid("Task commit must have exactly one valid parent"));
    }
    let tree_revision = format!("{commit}^{{tree}}");
    let tree_output = run_read_only(
        root,
        [
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            &tree_revision,
        ],
    )?;
    require_success(&tree_output, "Git commit tree inspection")?;
    let tree = full_object_text(&tree_output, "Git commit tree inspection")?;
    let message_output = run_read_only(root, ["show", "-s", "--format=%B", &commit, "--"])?;
    require_success(&message_output, "Git commit message inspection")?;
    let message = std::str::from_utf8(&message_output.stdout)
        .map_err(|_| invalid("Git commit message is not valid UTF-8"))?
        .trim_end_matches(['\r', '\n'])
        .to_owned();
    if message.is_empty() || message.contains(['\r', '\n']) {
        return Err(invalid("Task commit message is not exactly one line"));
    }
    let files_output = run_read_only(
        root,
        [
            "diff-tree",
            "--root",
            "--no-commit-id",
            "--name-only",
            "-r",
            "-z",
            "--no-renames",
            &commit,
            "--",
        ],
    )?;
    require_success(&files_output, "Git commit path inspection")?;
    let files = parse_paths(&files_output.stdout)?;
    Ok(GitCommitObject {
        commit,
        parent: parent_fields[1].clone(),
        tree,
        message,
        files,
    })
}

fn resolve_commit(root: &Path, revision: &str) -> Result<String, GitError> {
    if !is_object_id(revision) {
        return Err(invalid("Commit revision must be a full object ID"));
    }
    let commit_revision = format!("{revision}^{{commit}}");
    let output = run_read_only(
        root,
        [
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            &commit_revision,
        ],
    )?;
    require_success(&output, "Git commit resolution")?;
    full_object_text(&output, "Git commit resolution")
}

fn parse_paths(bytes: &[u8]) -> Result<Vec<String>, GitError> {
    let mut paths = bytes
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
        .map(|record| {
            let path = utf8(record, "Git commit path")?;
            let parsed = Path::new(path);
            if path.is_empty()
                || path.contains('\\')
                || parsed.is_absolute()
                || parsed
                    .components()
                    .any(|component| !matches!(component, Component::Normal(_)))
            {
                Err(invalid(format!("Git commit contains unsafe path {path}")))
            } else {
                Ok(path.to_owned())
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    paths.sort();
    if paths.is_empty() || !paths.windows(2).all(|pair| pair[0] < pair[1]) {
        Err(invalid("Git commit paths are empty or duplicated"))
    } else {
        Ok(paths)
    }
}

fn mutation_result(output: GitCommandOutput) -> GitMutationResult {
    let GitCommandOutput {
        success,
        exit_code,
        stderr,
        ..
    } = output;
    GitMutationResult {
        success,
        exit_code,
        stderr: String::from_utf8_lossy(&stderr).trim().to_owned(),
    }
}

fn require_success(output: &GitCommandOutput, operation: &str) -> Result<(), GitError> {
    if output.success {
        Ok(())
    } else {
        Err(GitError::new(
            GitErrorKind::Unavailable,
            format!(
                "{operation} failed with exit {:?}: {}",
                output.exit_code,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ))
    }
}

fn full_object_text(output: &GitCommandOutput, operation: &str) -> Result<String, GitError> {
    let value = output_text(output, operation)?;
    if is_object_id(&value) {
        Ok(value.to_ascii_lowercase())
    } else {
        Err(invalid(format!(
            "{operation} returned an invalid object ID"
        )))
    }
}

fn output_text(output: &GitCommandOutput, operation: &str) -> Result<String, GitError> {
    let value = std::str::from_utf8(&output.stdout)
        .map_err(|_| invalid(format!("{operation} returned non-UTF-8 output")))?
        .trim_end_matches(['\r', '\n']);
    if value.is_empty() || value.contains(['\r', '\n', '\0']) {
        Err(invalid(format!("{operation} returned invalid text")))
    } else {
        Ok(value.to_owned())
    }
}

fn utf8<'a>(bytes: &'a [u8], label: &str) -> Result<&'a str, GitError> {
    std::str::from_utf8(bytes).map_err(|_| invalid(format!("{label} is not valid UTF-8")))
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
