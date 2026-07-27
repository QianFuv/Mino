//! Deterministic workspace content identities for verification evidence.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Component, Path};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{DomainError, DomainErrorKind, TaskId};

pub(crate) const WORKSPACE_EXTENSION_KEY: &str = "workspace";

/// Repository capabilities observed while a workspace fingerprint was captured.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceRepositoryMode {
    /// Git supplied HEAD, index, and status identity.
    Git,
    /// The project was captured without a Git work tree.
    NonGit,
}

/// Verification scope represented by a workspace fingerprint.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceScopeKind {
    /// One task's declared File Map.
    Task,
    /// The complete plan File Map and final workspace state.
    Global,
}

/// Stable filesystem object classifications admitted to a fingerprint.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceFileKind {
    /// The scoped path does not currently exist.
    Missing,
    /// The scoped path is a regular file.
    Regular,
    /// The scoped path is a directory.
    Directory,
}

/// Expected immutable Git tree identity for one regular worktree file.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceGitEntry {
    blob_oid: String,
    mode: String,
}

impl WorkspaceGitEntry {
    /// Creates one normalized regular-file Git tree entry.
    ///
    /// # Errors
    ///
    /// Returns an invariant error for a malformed object ID or unsupported mode.
    pub fn new(blob_oid: &str, mode: &str) -> Result<Self, DomainError> {
        let entry = Self {
            blob_oid: blob_oid.to_ascii_lowercase(),
            mode: mode.to_owned(),
        };
        if valid_git_entry(&entry) {
            Ok(entry)
        } else {
            Err(invariant("Workspace Git entry is malformed"))
        }
    }

    /// Returns the expected Git blob object ID.
    #[must_use]
    pub fn blob_oid(&self) -> &str {
        &self.blob_oid
    }

    /// Returns the expected regular-file Git mode.
    #[must_use]
    pub fn mode(&self) -> &str {
        &self.mode
    }
}

/// One normalized Git status entry included in a fingerprint.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceStatusEntry {
    path: String,
    original_path: Option<String>,
    index_status: String,
    worktree_status: String,
    submodule: String,
    kind: String,
}

impl WorkspaceStatusEntry {
    pub(crate) fn new(
        path: String,
        original_path: Option<String>,
        index_status: char,
        worktree_status: char,
        submodule: String,
        kind: String,
    ) -> Self {
        Self {
            path,
            original_path,
            index_status: index_status.to_string(),
            worktree_status: worktree_status.to_string(),
            submodule,
            kind,
        }
    }

    /// Returns the normalized project-relative path.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }
}

/// One exact path state included in a workspace fingerprint.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceFileSnapshot {
    path: String,
    kind: WorkspaceFileKind,
    length: u64,
    executable: bool,
    sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expected_git_entry: Option<WorkspaceGitEntry>,
}

impl WorkspaceFileSnapshot {
    pub(crate) fn new(
        path: String,
        kind: WorkspaceFileKind,
        length: u64,
        executable: bool,
        sha256: String,
        expected_git_entry: Option<WorkspaceGitEntry>,
    ) -> Self {
        Self {
            path,
            kind,
            length,
            executable,
            sha256,
            expected_git_entry,
        }
    }

    /// Returns the normalized project-relative path.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Returns the observed filesystem object classification.
    #[must_use]
    pub const fn kind(&self) -> WorkspaceFileKind {
        self.kind
    }

    /// Returns the regular-file byte length, or zero for non-files.
    #[must_use]
    pub const fn length(&self) -> u64 {
        self.length
    }

    /// Returns whether an executable mode was observed.
    #[must_use]
    pub const fn executable(&self) -> bool {
        self.executable
    }

    /// Returns the SHA-256 content or stable object-state digest.
    #[must_use]
    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    /// Returns the Git blob and mode expected from these worktree bytes.
    #[must_use]
    pub const fn expected_git_entry(&self) -> Option<&WorkspaceGitEntry> {
        self.expected_git_entry.as_ref()
    }
}

/// The declared paths used to recapture a task or global verification scope.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceFingerprintScope {
    kind: WorkspaceScopeKind,
    task_id: Option<TaskId>,
    patterns: Vec<String>,
}

impl WorkspaceFingerprintScope {
    pub(crate) fn new(
        kind: WorkspaceScopeKind,
        task_id: Option<TaskId>,
        mut patterns: Vec<String>,
    ) -> Self {
        patterns.sort();
        patterns.dedup();
        Self {
            kind,
            task_id,
            patterns,
        }
    }

    /// Returns whether this is a task or global verification scope.
    #[must_use]
    pub const fn kind(&self) -> WorkspaceScopeKind {
        self.kind
    }

    /// Returns the owning task for task-scoped verification.
    #[must_use]
    pub const fn task_id(&self) -> Option<&TaskId> {
        self.task_id.as_ref()
    }

    /// Returns sorted File Map patterns used to recapture the scope.
    #[must_use]
    pub fn patterns(&self) -> &[String] {
        &self.patterns
    }
}

/// A canonical identity for the code and repository state verified by a check.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceFingerprint {
    repository_mode: WorkspaceRepositoryMode,
    head: Option<String>,
    index_tree: Option<String>,
    status_entries: Vec<WorkspaceStatusEntry>,
    scope: WorkspaceFingerprintScope,
    file_snapshots: Vec<WorkspaceFileSnapshot>,
    fingerprint_digest: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UncheckedWorkspaceFingerprint {
    repository_mode: WorkspaceRepositoryMode,
    head: Option<String>,
    index_tree: Option<String>,
    status_entries: Vec<WorkspaceStatusEntry>,
    scope: WorkspaceFingerprintScope,
    file_snapshots: Vec<WorkspaceFileSnapshot>,
    fingerprint_digest: String,
}

impl TryFrom<UncheckedWorkspaceFingerprint> for WorkspaceFingerprint {
    type Error = DomainError;

    fn try_from(unchecked: UncheckedWorkspaceFingerprint) -> Result<Self, Self::Error> {
        let fingerprint = Self {
            repository_mode: unchecked.repository_mode,
            head: unchecked.head,
            index_tree: unchecked.index_tree,
            status_entries: unchecked.status_entries,
            scope: unchecked.scope,
            file_snapshots: unchecked.file_snapshots,
            fingerprint_digest: unchecked.fingerprint_digest,
        };
        fingerprint.validate()?;
        Ok(fingerprint)
    }
}

impl<'de> Deserialize<'de> for WorkspaceFingerprint {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let unchecked = UncheckedWorkspaceFingerprint::deserialize(deserializer)?;
        Self::try_from(unchecked).map_err(serde::de::Error::custom)
    }
}

impl WorkspaceFingerprint {
    pub(crate) fn new(
        repository_mode: WorkspaceRepositoryMode,
        head: Option<String>,
        index_tree: Option<String>,
        mut status_entries: Vec<WorkspaceStatusEntry>,
        scope: WorkspaceFingerprintScope,
        mut file_snapshots: Vec<WorkspaceFileSnapshot>,
    ) -> Result<Self, DomainError> {
        status_entries.sort_by(|left, right| left.path.cmp(&right.path));
        file_snapshots.sort_by(|left, right| left.path.cmp(&right.path));
        let mut fingerprint = Self {
            repository_mode,
            head,
            index_tree,
            status_entries,
            scope,
            file_snapshots,
            fingerprint_digest: String::new(),
        };
        fingerprint.fingerprint_digest = fingerprint.recompute_digest()?;
        fingerprint.validate()?;
        Ok(fingerprint)
    }

    /// Returns the repository mode used by the capture.
    #[must_use]
    pub const fn repository_mode(&self) -> WorkspaceRepositoryMode {
        self.repository_mode
    }

    /// Returns the full Git HEAD object ID when a committed HEAD exists.
    #[must_use]
    pub fn head(&self) -> Option<&str> {
        self.head.as_deref()
    }

    /// Returns the SHA-256 identity of canonical Git index entries.
    #[must_use]
    pub fn index_tree(&self) -> Option<&str> {
        self.index_tree.as_deref()
    }

    /// Returns normalized Git status entries relevant to this scope.
    #[must_use]
    pub fn status_entries(&self) -> &[WorkspaceStatusEntry] {
        &self.status_entries
    }

    /// Returns the persisted recapture scope.
    #[must_use]
    pub const fn scope(&self) -> &WorkspaceFingerprintScope {
        &self.scope
    }

    /// Returns exact sorted path snapshots.
    #[must_use]
    pub fn file_snapshots(&self) -> &[WorkspaceFileSnapshot] {
        &self.file_snapshots
    }

    /// Returns the canonical fingerprint payload digest.
    #[must_use]
    pub fn fingerprint_digest(&self) -> &str {
        &self.fingerprint_digest
    }

    /// Returns whether every regular Git file has a checked tree identity.
    #[must_use]
    pub fn has_complete_git_entries(&self) -> bool {
        self.repository_mode != WorkspaceRepositoryMode::Git
            || self.file_snapshots.iter().all(|snapshot| {
                (snapshot.kind == WorkspaceFileKind::Regular)
                    == snapshot.expected_git_entry.is_some()
            })
    }

    /// Validates path safety, ordering, repository fields, and the canonical digest.
    ///
    /// # Errors
    ///
    /// Returns an invariant error when any persisted fingerprint field is malformed.
    pub fn validate(&self) -> Result<(), DomainError> {
        let scope_matches = matches!(
            (self.scope.kind, self.scope.task_id.as_ref()),
            (WorkspaceScopeKind::Task, Some(_)) | (WorkspaceScopeKind::Global, None)
        );
        if !scope_matches
            || self
                .scope
                .patterns
                .iter()
                .any(|path| !is_safe_pattern(path))
            || !strictly_sorted(&self.scope.patterns)
        {
            return Err(invariant("Workspace fingerprint scope is malformed"));
        }
        if self
            .status_entries
            .iter()
            .any(|entry| !valid_status_entry(entry))
            || !strictly_sorted_by_path(&self.status_entries, |entry| &entry.path)
        {
            return Err(invariant(
                "Workspace fingerprint status entries are malformed",
            ));
        }
        if self
            .file_snapshots
            .iter()
            .any(|snapshot| !valid_file_snapshot(snapshot))
            || !strictly_sorted_by_path(&self.file_snapshots, |snapshot| &snapshot.path)
        {
            return Err(invariant(
                "Workspace fingerprint file snapshots are malformed",
            ));
        }
        match self.repository_mode {
            WorkspaceRepositoryMode::Git => {
                if self
                    .index_tree
                    .as_deref()
                    .is_none_or(|digest| !is_sha256(digest))
                    || self.head.as_deref().is_some_and(|head| !is_object_id(head))
                {
                    return Err(invariant("Git workspace fingerprint identity is malformed"));
                }
            }
            WorkspaceRepositoryMode::NonGit => {
                if self.head.is_some()
                    || self.index_tree.is_some()
                    || !self.status_entries.is_empty()
                    || self
                        .file_snapshots
                        .iter()
                        .any(|snapshot| snapshot.expected_git_entry.is_some())
                {
                    return Err(invariant(
                        "Non-Git workspace fingerprint contains Git-only identity",
                    ));
                }
            }
        }
        if self.fingerprint_digest != self.recompute_digest()? {
            return Err(invariant(
                "Workspace fingerprint digest does not match its payload",
            ));
        }
        Ok(())
    }

    fn recompute_digest(&self) -> Result<String, DomainError> {
        #[derive(Serialize)]
        struct DigestInput<'a> {
            repository_mode: WorkspaceRepositoryMode,
            head: &'a Option<String>,
            index_tree: &'a Option<String>,
            status_entries: &'a [WorkspaceStatusEntry],
            scope: &'a WorkspaceFingerprintScope,
            file_snapshots: &'a [WorkspaceFileSnapshot],
        }

        let value = serde_json::to_value(DigestInput {
            repository_mode: self.repository_mode,
            head: &self.head,
            index_tree: &self.index_tree,
            status_entries: &self.status_entries,
            scope: &self.scope,
            file_snapshots: &self.file_snapshots,
        })
        .map_err(|error| invariant(format!("Failed to encode workspace fingerprint: {error}")))?;
        let mut bytes = Vec::new();
        write_canonical_value(&mut bytes, &value)?;
        let digest = Sha256::digest(bytes);
        Ok(format!("sha256:{digest:x}"))
    }
}

/// Persisted plan and task baselines used to attribute workspace changes.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceProtocolState {
    plan_baseline: Option<WorkspaceFingerprint>,
    task_baselines: BTreeMap<TaskId, WorkspaceFingerprint>,
}

impl WorkspaceProtocolState {
    /// Returns the workspace observed at plan approval or legacy execution start.
    #[must_use]
    pub const fn plan_baseline(&self) -> Option<&WorkspaceFingerprint> {
        self.plan_baseline.as_ref()
    }

    /// Returns the workspace observed when the selected task most recently started.
    #[must_use]
    pub fn task_baseline(&self, task_id: &TaskId) -> Option<&WorkspaceFingerprint> {
        self.task_baselines.get(task_id)
    }

    /// Returns task baselines in stable task identifier order.
    #[must_use]
    pub const fn task_baselines(&self) -> &BTreeMap<TaskId, WorkspaceFingerprint> {
        &self.task_baselines
    }

    pub(crate) fn record_plan_baseline(
        &mut self,
        baseline: WorkspaceFingerprint,
    ) -> Result<(), DomainError> {
        validate_baseline(&baseline)?;
        self.plan_baseline = Some(baseline);
        self.task_baselines.clear();
        Ok(())
    }

    pub(crate) fn record_task_baseline(
        &mut self,
        task_id: TaskId,
        baseline: WorkspaceFingerprint,
    ) -> Result<(), DomainError> {
        validate_baseline(&baseline)?;
        self.task_baselines.insert(task_id, baseline);
        Ok(())
    }

    pub(crate) fn remove_task_baseline(&mut self, task_id: &TaskId) {
        self.task_baselines.remove(task_id);
    }

    pub(crate) fn validate(&self, task_ids: &BTreeSet<&TaskId>) -> Result<(), DomainError> {
        if let Some(baseline) = &self.plan_baseline {
            validate_baseline(baseline)?;
        }
        for (task_id, baseline) in &self.task_baselines {
            if !task_ids.contains(task_id) {
                return Err(invariant(format!(
                    "Workspace baseline references missing task {task_id}"
                )));
            }
            validate_baseline(baseline)?;
        }
        Ok(())
    }
}

fn validate_baseline(baseline: &WorkspaceFingerprint) -> Result<(), DomainError> {
    baseline.validate()?;
    if baseline.scope.kind != WorkspaceScopeKind::Global
        || baseline.scope.task_id.is_some()
        || !baseline
            .scope
            .patterns
            .iter()
            .any(|pattern| pattern == "**")
    {
        Err(invariant(
            "Workspace baselines must capture the complete project scope",
        ))
    } else {
        Ok(())
    }
}

fn valid_status_entry(entry: &WorkspaceStatusEntry) -> bool {
    is_safe_path(&entry.path)
        && entry.original_path.as_deref().is_none_or(is_safe_path)
        && entry.index_status.chars().count() == 1
        && entry.worktree_status.chars().count() == 1
        && !entry.submodule.trim().is_empty()
        && !entry.kind.trim().is_empty()
}

fn valid_file_snapshot(snapshot: &WorkspaceFileSnapshot) -> bool {
    is_safe_path(&snapshot.path)
        && is_sha256(&snapshot.sha256)
        && (snapshot.kind == WorkspaceFileKind::Regular
            || snapshot.length == 0 && !snapshot.executable)
        && snapshot.expected_git_entry.as_ref().is_none_or(|entry| {
            snapshot.kind == WorkspaceFileKind::Regular && valid_git_entry(entry)
        })
}

fn valid_git_entry(entry: &WorkspaceGitEntry) -> bool {
    matches!(entry.blob_oid.len(), 40 | 64)
        && entry
            .blob_oid
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        && matches!(entry.mode.as_str(), "100644" | "100755")
}

fn is_safe_pattern(value: &str) -> bool {
    is_safe_path(value)
        && value
            .split('/')
            .all(|segment| !segment.is_empty() && segment != "." && segment != "..")
}

fn is_safe_path(value: &str) -> bool {
    let path = Path::new(value);
    !value.is_empty()
        && !value.contains('\\')
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn strictly_sorted(values: &[String]) -> bool {
    values.windows(2).all(|pair| pair[0] < pair[1])
        && values.iter().collect::<BTreeSet<_>>().len() == values.len()
}

fn strictly_sorted_by_path<T, F>(values: &[T], path: F) -> bool
where
    F: Fn(&T) -> &str,
{
    values
        .windows(2)
        .all(|pair| path(&pair[0]) < path(&pair[1]))
}

fn is_sha256(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("sha256:")
        && value[7..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn is_object_id(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn write_canonical_value(output: &mut Vec<u8>, value: &Value) -> Result<(), DomainError> {
    match value {
        Value::Object(object) => {
            output.push(b'{');
            let mut keys = object.keys().collect::<Vec<_>>();
            keys.sort_unstable();
            for (index, key) in keys.into_iter().enumerate() {
                if index != 0 {
                    output.push(b',');
                }
                serde_json::to_writer(&mut *output, key).map_err(|error| json_error(&error))?;
                output.push(b':');
                write_canonical_value(output, &object[key])?;
            }
            output.push(b'}');
        }
        Value::Array(values) => {
            output.push(b'[');
            for (index, item) in values.iter().enumerate() {
                if index != 0 {
                    output.push(b',');
                }
                write_canonical_value(output, item)?;
            }
            output.push(b']');
        }
        _ => serde_json::to_writer(&mut *output, value).map_err(|error| json_error(&error))?,
    }
    output.flush().map_err(|error| {
        invariant(format!(
            "Failed to finalize canonical workspace fingerprint: {error}"
        ))
    })
}

fn json_error(error: &serde_json::Error) -> DomainError {
    invariant(format!(
        "Failed to write canonical workspace fingerprint: {error}"
    ))
}

fn invariant(message: impl Into<String>) -> DomainError {
    DomainError::new(DomainErrorKind::InvariantViolation, message)
}
