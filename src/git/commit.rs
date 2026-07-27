//! Explicit-path staging, hook-aware commit execution, and object verification.

use std::collections::BTreeMap;
use std::path::{Component, Path};

use serde::Serialize;

use crate::domain::WorkspaceGitEntry;

use super::command::{GitCommandOutput, run_mutating, run_read_only};
use super::{GitError, GitErrorKind};

const MAX_PATH_ARGUMENT_BYTES: usize = 12 * 1024;

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

/// One exact regular-file entry read from a Git index or tree.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GitTreeEntry {
    path: String,
    entry: WorkspaceGitEntry,
}

impl GitTreeEntry {
    /// Returns the normalized repository-relative path.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Returns the immutable blob object ID and regular-file mode.
    #[must_use]
    pub const fn entry(&self) -> &WorkspaceGitEntry {
        &self.entry
    }
}

/// Computes the filtered blob and staging mode expected for regular worktree files.
///
/// Paths must be sorted, unique, safe repository-relative names. Git applies
/// built-in attributes while hashing but does not write the resulting objects.
///
/// # Errors
///
/// Returns a policy, unavailable, or invalid-output error for unsafe paths,
/// unsupported index modes, failed attribute conversion, or malformed Git output.
pub fn expected_worktree_entries(
    root: &Path,
    files: &[(String, bool)],
) -> Result<Vec<GitTreeEntry>, GitError> {
    validate_requested_paths(files.iter().map(|(path, _)| path.as_str()))?;
    if files.is_empty() {
        return Ok(Vec::new());
    }
    let filemode = core_filemode(root)?;
    let index_modes = index_modes(root, files)?;
    let mut entries = Vec::with_capacity(files.len());
    for range in path_argument_ranges(files.iter().map(|(path, _)| path.as_str())) {
        let mut arguments = vec!["hash-object".to_owned(), "--".to_owned()];
        arguments.extend(files[range.clone()].iter().map(|(path, _)| path.clone()));
        let output = run_read_only(root, arguments)?;
        require_success(&output, "Git filtered blob inspection")?;
        let object_ids = output
            .stdout
            .split(|byte| *byte == b'\n')
            .map(|line| line.strip_suffix(b"\r").unwrap_or(line))
            .filter(|line| !line.is_empty())
            .map(|line| {
                let value = utf8(line, "Git filtered blob object ID")?;
                if is_object_id(value) {
                    Ok(value.to_ascii_lowercase())
                } else {
                    Err(invalid(
                        "Git filtered blob inspection returned an invalid object ID",
                    ))
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        if object_ids.len() != range.len() {
            return Err(invalid(
                "Git filtered blob inspection returned an invalid object count",
            ));
        }
        for ((path, executable), blob_oid) in files[range].iter().zip(object_ids) {
            let mode = if filemode || !index_modes.contains_key(path) {
                if *executable { "100755" } else { "100644" }
            } else {
                index_modes[path].as_str()
            };
            let entry = WorkspaceGitEntry::new(&blob_oid, mode)
                .map_err(|error| invalid(error.to_string()))?;
            entries.push(GitTreeEntry {
                path: path.clone(),
                entry,
            });
        }
    }
    Ok(entries)
}

/// Reads exact regular-file entries from a commit or tree object.
///
/// Missing requested paths are omitted from the result. The returned entries
/// are sorted by normalized path.
///
/// # Errors
///
/// Returns an unavailable or invalid-output error for an invalid revision,
/// unsafe request, unsupported tree entry, or malformed Git output.
pub fn inspect_tree_entries(
    root: &Path,
    revision: &str,
    paths: &[String],
) -> Result<Vec<GitTreeEntry>, GitError> {
    if !is_object_id(revision) {
        return Err(invalid("Git tree inspection requires a full object ID"));
    }
    validate_requested_paths(paths.iter().map(String::as_str))?;
    let mut entries = BTreeMap::new();
    for range in path_argument_ranges(paths.iter().map(String::as_str)) {
        let mut arguments = vec![
            "ls-tree".to_owned(),
            "-z".to_owned(),
            revision.to_owned(),
            "--".to_owned(),
        ];
        arguments.extend(paths[range].iter().map(|path| format!(":(literal){path}")));
        let output = run_read_only(root, arguments)?;
        require_success(&output, "Git tree entry inspection")?;
        for record in output.stdout.split(|byte| *byte == 0) {
            if record.is_empty() {
                continue;
            }
            let separator = record
                .iter()
                .position(|byte| *byte == b'\t')
                .ok_or_else(|| invalid("Git tree entry inspection returned a malformed record"))?;
            let identity = &record[..separator];
            let path = &record[separator + 1..];
            let fields = identity.split(|byte| *byte == b' ').collect::<Vec<_>>();
            if fields.len() != 3 || fields[1] != b"blob" {
                return Err(invalid(
                    "Git tree entry inspection returned a non-blob entry",
                ));
            }
            let mode = utf8(fields[0], "Git tree entry mode")?;
            let blob_oid = utf8(fields[2], "Git tree entry object ID")?;
            let path = utf8(path, "Git tree entry path")?;
            validate_path(path)?;
            if paths
                .binary_search_by(|candidate| candidate.as_str().cmp(path))
                .is_err()
            {
                return Err(invalid(
                    "Git tree entry inspection returned an unrequested path",
                ));
            }
            let entry = WorkspaceGitEntry::new(blob_oid, mode)
                .map_err(|error| invalid(error.to_string()))?;
            if entries.insert(path.to_owned(), entry).is_some() {
                return Err(invalid(
                    "Git tree entry inspection returned a duplicate path",
                ));
            }
        }
    }
    Ok(entries
        .into_iter()
        .map(|(path, entry)| GitTreeEntry { path, entry })
        .collect())
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

fn core_filemode(root: &Path) -> Result<bool, GitError> {
    let output = run_read_only(root, ["config", "--type=bool", "--get", "core.filemode"])?;
    if !output.success {
        if output.exit_code == Some(1) && output.stdout.is_empty() {
            return Ok(true);
        }
        require_success(&output, "Git core.filemode inspection")?;
    }
    match output_text(&output, "Git core.filemode inspection")?.as_str() {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(invalid(
            "Git core.filemode inspection returned an invalid value",
        )),
    }
}

fn index_modes(
    root: &Path,
    files: &[(String, bool)],
) -> Result<BTreeMap<String, String>, GitError> {
    let paths = files
        .iter()
        .map(|(path, _)| path.as_str())
        .collect::<Vec<_>>();
    let mut modes = BTreeMap::new();
    for range in path_argument_ranges(paths.iter().copied()) {
        let mut arguments = vec![
            "ls-files".to_owned(),
            "--stage".to_owned(),
            "-z".to_owned(),
            "--".to_owned(),
        ];
        arguments.extend(paths[range].iter().map(|path| format!(":(literal){path}")));
        let output = run_read_only(root, arguments)?;
        require_success(&output, "Git index mode inspection")?;
        for record in output.stdout.split(|byte| *byte == 0) {
            if record.is_empty() {
                continue;
            }
            let separator = record
                .iter()
                .position(|byte| *byte == b'\t')
                .ok_or_else(|| invalid("Git index mode inspection returned a malformed record"))?;
            let identity = &record[..separator];
            let path = utf8(&record[separator + 1..], "Git index mode path")?;
            let fields = identity.split(|byte| *byte == b' ').collect::<Vec<_>>();
            if fields.len() != 3
                || fields[2] != b"0"
                || !is_object_id(utf8(fields[1], "Git index object ID")?)
            {
                return Err(invalid(
                    "Git index mode inspection returned an unsupported entry",
                ));
            }
            let mode = utf8(fields[0], "Git index mode")?;
            if !matches!(mode, "100644" | "100755") {
                return Err(policy(format!(
                    "Git index path {path} has unsupported mode {mode}"
                )));
            }
            if paths.binary_search(&path).is_err()
                || modes.insert(path.to_owned(), mode.to_owned()).is_some()
            {
                return Err(invalid(
                    "Git index mode inspection returned an unexpected or duplicate path",
                ));
            }
        }
    }
    Ok(modes)
}

fn validate_requested_paths<'a>(paths: impl Iterator<Item = &'a str>) -> Result<(), GitError> {
    let paths = paths.collect::<Vec<_>>();
    for path in &paths {
        validate_path(path)?;
        if path.len() > MAX_PATH_ARGUMENT_BYTES {
            return Err(policy(format!(
                "Git path exceeds the bounded argument limit: {path}"
            )));
        }
    }
    if !paths.windows(2).all(|pair| pair[0] < pair[1]) {
        return Err(invalid("Git entry paths must be sorted and unique"));
    }
    Ok(())
}

fn validate_path(path: &str) -> Result<(), GitError> {
    let parsed = Path::new(path);
    if path.is_empty()
        || path.contains('\\')
        || parsed.is_absolute()
        || parsed
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        Err(invalid(format!("Git entry path is unsafe: {path}")))
    } else {
        Ok(())
    }
}

fn path_argument_ranges<'a>(paths: impl Iterator<Item = &'a str>) -> Vec<std::ops::Range<usize>> {
    let mut ranges = Vec::new();
    let mut start = 0;
    let mut count = 0;
    let mut bytes = 0_usize;
    for (index, path) in paths.enumerate() {
        let next = path.len().saturating_add(12);
        if index > start && bytes.saturating_add(next) > MAX_PATH_ARGUMENT_BYTES {
            ranges.push(start..index);
            start = index;
            bytes = 0;
        }
        bytes = bytes.saturating_add(next);
        count = index + 1;
    }
    if start < count {
        ranges.push(start..count);
    }
    ranges
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
