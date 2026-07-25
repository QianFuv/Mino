//! Repository-level Skill installation and owned integration block management.

mod agents_block;
mod gitignore_block;
mod skill;

#[cfg(unix)]
use std::fs::File;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;

use crate::{ErrorCategory, MinoError};

static NEXT_INTEGRATION_FILE: AtomicU64 = AtomicU64::new(1);

/// Repository integration surfaces managed or inspected by Mino.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationArtifactKind {
    /// Repository-local Mino Skill bundle.
    Skill,
    /// Stable Mino workflow block in `AGENTS.md`.
    AgentsBlock,
    /// Runtime-state exclusions in `.gitignore`.
    GitignoreBlock,
}

/// Observable relationship between one repository artifact and Mino's bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationStatus {
    /// Missing Mino-owned bytes were created.
    Created,
    /// Existing Mino-owned bytes were replaced after ownership validation.
    Updated,
    /// Existing bytes already match the current bundle or managed block.
    Current,
    /// The artifact is absent and apply was not requested.
    Missing,
    /// Valid ownership markers surround stale managed bytes.
    Drift,
    /// Ownership is absent, ambiguous, malformed, or unsafe to modify.
    Conflict,
}

impl IntegrationStatus {
    const fn is_changed(self) -> bool {
        matches!(self, Self::Created | Self::Updated)
    }
}

/// Result for one inspected or reconciled repository integration artifact.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct IntegrationArtifact {
    /// Stable artifact category.
    pub kind: IntegrationArtifactKind,
    /// Repository path associated with the artifact.
    pub path: PathBuf,
    /// Inspection or reconciliation result.
    pub status: IntegrationStatus,
    /// Proposed complete block or remediation summary when no write occurred.
    pub proposal: Option<String>,
}

/// Severity assigned to an integration finding.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationFindingSeverity {
    /// The integration is incomplete but can be applied explicitly.
    Warning,
    /// Automatic modification is unsafe and the existing bytes are preserved.
    Error,
}

/// Stable diagnosis for one missing, drifted, or conflicting integration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct IntegrationFinding {
    /// Stable machine-readable finding code.
    pub code: String,
    /// Finding severity.
    pub severity: IntegrationFindingSeverity,
    /// Concise explanation of the observed state.
    pub message: String,
    /// Repository path associated with the finding.
    pub path: PathBuf,
}

/// Explicit apply choices for repository files outside `.mino/`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct IntegrationOptions {
    /// Apply or refresh the owned Mino block in `AGENTS.md`.
    pub apply_agents_block: bool,
    /// Apply or refresh the owned runtime block in `.gitignore`.
    pub apply_gitignore_block: bool,
}

/// Complete deterministic report for repository integration state.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct IntegrationReport {
    /// Artifact results ordered by stable category and path.
    pub artifacts: Vec<IntegrationArtifact>,
    /// Findings ordered by stable code and path.
    pub findings: Vec<IntegrationFinding>,
    /// Paths changed during this invocation.
    pub changed_paths: Vec<PathBuf>,
}

impl IntegrationReport {
    /// Returns whether every repository integration is current.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.findings.is_empty()
            && self.artifacts.iter().all(|artifact| {
                matches!(
                    artifact.status,
                    IntegrationStatus::Created
                        | IntegrationStatus::Updated
                        | IntegrationStatus::Current
                )
            })
    }
}

/// Installs the bundled Skill and optionally applies owned repository blocks.
///
/// Existing unowned or malformed content is reported and preserved.
///
/// # Errors
///
/// Returns an environment or drift error when repository paths cannot be read
/// or a verified write cannot be published safely.
pub fn integrate_project(
    root: &Path,
    options: IntegrationOptions,
) -> Result<IntegrationReport, MinoError> {
    let mut report = IntegrationReport::default();
    skill::reconcile(root, true, &mut report)?;
    agents_block::reconcile(root, options.apply_agents_block, &mut report)?;
    gitignore_block::reconcile(root, options.apply_gitignore_block, &mut report)?;
    finish_report(&mut report);
    Ok(report)
}

/// Inspects every repository integration without modifying any path.
///
/// # Errors
///
/// Returns an environment error when an existing integration path cannot be
/// inspected safely.
pub fn inspect_project(root: &Path) -> Result<IntegrationReport, MinoError> {
    let mut report = IntegrationReport::default();
    skill::reconcile(root, false, &mut report)?;
    agents_block::reconcile(root, false, &mut report)?;
    gitignore_block::reconcile(root, false, &mut report)?;
    finish_report(&mut report);
    Ok(report)
}

struct ManagedBlockSpec {
    kind: IntegrationArtifactKind,
    relative_path: &'static str,
    start_marker: &'static str,
    end_marker: &'static str,
    block: &'static str,
    missing_code: &'static str,
    drift_code: &'static str,
    malformed_code: &'static str,
    missing_message: &'static str,
    drift_message: &'static str,
    malformed_message: &'static str,
}

fn reconcile_block(
    root: &Path,
    spec: &ManagedBlockSpec,
    should_apply: bool,
    report: &mut IntegrationReport,
) -> Result<(), MinoError> {
    let path = root.join(spec.relative_path);
    if let Err(error) = ensure_no_symlink(root, &path) {
        add_malformed_block(spec, path, error.message(), report);
        return Ok(());
    }
    match fs::symlink_metadata(&path) {
        Ok(metadata) if !metadata.is_file() => {
            add_malformed_block(
                spec,
                path,
                "The integration path exists but is not a regular file and was preserved",
                report,
            );
            return Ok(());
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return reconcile_missing_block(root, spec, &path, should_apply, report);
        }
        Err(error) => return Err(io_error("inspect", &path, &error)),
    }
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) => return Err(io_error("read", &path, &error)),
    };
    let Ok(contents) = std::str::from_utf8(&bytes) else {
        add_malformed_block(
            spec,
            path,
            "The integration file is not valid UTF-8 and was preserved",
            report,
        );
        return Ok(());
    };
    let raw_start_count = contents.matches(spec.start_marker).count();
    let raw_end_count = contents.matches(spec.end_marker).count();
    let line_start_count = marker_line_count(contents, spec.start_marker);
    let line_end_count = marker_line_count(contents, spec.end_marker);
    if raw_start_count == 0 && raw_end_count == 0 {
        return reconcile_absent_block(root, spec, path, &bytes, contents, should_apply, report);
    }
    if raw_start_count != 1 || raw_end_count != 1 || line_start_count != 1 || line_end_count != 1 {
        add_malformed_block(spec, path, spec.malformed_message, report);
        return Ok(());
    }
    let start = contents
        .find(spec.start_marker)
        .expect("validated start marker should exist");
    let end = contents
        .find(spec.end_marker)
        .expect("validated end marker should exist");
    if end <= start {
        add_malformed_block(spec, path, spec.malformed_message, report);
        return Ok(());
    }
    let managed_end = end + spec.end_marker.len();
    if &contents[start..managed_end] == spec.block {
        report
            .artifacts
            .push(artifact(spec.kind, path, IntegrationStatus::Current, None));
        return Ok(());
    }
    let replacement = format!(
        "{}{}{}",
        &contents[..start],
        spec.block,
        &contents[managed_end..]
    );
    if should_apply {
        let status = guarded_write(root, &path, Some(&bytes), replacement.as_bytes())?;
        report
            .artifacts
            .push(artifact(spec.kind, path, status, None));
    } else {
        report.artifacts.push(artifact(
            spec.kind,
            path.clone(),
            IntegrationStatus::Drift,
            Some(spec.block.to_owned()),
        ));
        report.findings.push(finding(
            spec.drift_code,
            IntegrationFindingSeverity::Warning,
            spec.drift_message,
            path,
        ));
    }
    Ok(())
}

fn reconcile_missing_block(
    root: &Path,
    spec: &ManagedBlockSpec,
    path: &Path,
    should_apply: bool,
    report: &mut IntegrationReport,
) -> Result<(), MinoError> {
    if should_apply {
        let replacement = format!("{}\n", spec.block);
        let status = guarded_write(root, path, None, replacement.as_bytes())?;
        report
            .artifacts
            .push(artifact(spec.kind, path.to_path_buf(), status, None));
    } else {
        add_missing_block(spec, path.to_path_buf(), report);
    }
    Ok(())
}

fn reconcile_absent_block(
    root: &Path,
    spec: &ManagedBlockSpec,
    path: PathBuf,
    bytes: &[u8],
    contents: &str,
    should_apply: bool,
    report: &mut IntegrationReport,
) -> Result<(), MinoError> {
    if should_apply {
        let separator = if contents.is_empty() || contents.ends_with("\n\n") {
            ""
        } else if contents.ends_with('\n') {
            "\n"
        } else {
            "\n\n"
        };
        let replacement = format!("{contents}{separator}{}\n", spec.block);
        let status = guarded_write(root, &path, Some(bytes), replacement.as_bytes())?;
        report
            .artifacts
            .push(artifact(spec.kind, path, status, None));
    } else {
        add_missing_block(spec, path, report);
    }
    Ok(())
}

fn marker_line_count(contents: &str, marker: &str) -> usize {
    contents
        .lines()
        .filter(|line| line.strip_suffix('\r').unwrap_or(line) == marker)
        .count()
}

fn add_missing_block(spec: &ManagedBlockSpec, path: PathBuf, report: &mut IntegrationReport) {
    report.artifacts.push(artifact(
        spec.kind,
        path.clone(),
        IntegrationStatus::Missing,
        Some(spec.block.to_owned()),
    ));
    report.findings.push(finding(
        spec.missing_code,
        IntegrationFindingSeverity::Warning,
        spec.missing_message,
        path,
    ));
}

fn add_malformed_block(
    spec: &ManagedBlockSpec,
    path: PathBuf,
    message: impl Into<String>,
    report: &mut IntegrationReport,
) {
    report.artifacts.push(artifact(
        spec.kind,
        path.clone(),
        IntegrationStatus::Conflict,
        None,
    ));
    report.findings.push(finding(
        spec.malformed_code,
        IntegrationFindingSeverity::Error,
        message,
        path,
    ));
}

fn finish_report(report: &mut IntegrationReport) {
    report
        .artifacts
        .sort_by(|left, right| (left.kind, &left.path).cmp(&(right.kind, &right.path)));
    report
        .findings
        .sort_by(|left, right| (&left.code, &left.path).cmp(&(&right.code, &right.path)));
    report.changed_paths = report
        .artifacts
        .iter()
        .filter(|artifact| artifact.status.is_changed())
        .map(|artifact| artifact.path.clone())
        .collect();
    report.changed_paths.sort();
    report.changed_paths.dedup();
}

fn artifact(
    kind: IntegrationArtifactKind,
    path: PathBuf,
    status: IntegrationStatus,
    proposal: Option<String>,
) -> IntegrationArtifact {
    IntegrationArtifact {
        kind,
        path,
        status,
        proposal,
    }
}

fn finding(
    code: &str,
    severity: IntegrationFindingSeverity,
    message: impl Into<String>,
    path: PathBuf,
) -> IntegrationFinding {
    IntegrationFinding {
        code: code.to_owned(),
        severity,
        message: message.into(),
        path,
    }
}

fn guarded_write(
    root: &Path,
    path: &Path,
    expected: Option<&[u8]>,
    replacement: &[u8],
) -> Result<IntegrationStatus, MinoError> {
    ensure_no_symlink(root, path)?;
    match fs::read(path) {
        Ok(actual) if actual == replacement => Ok(IntegrationStatus::Current),
        Ok(actual) if expected.is_some_and(|expected| actual == expected) => {
            guarded_replace(path, &actual, replacement)?;
            Ok(IntegrationStatus::Updated)
        }
        Ok(_) => Err(MinoError::new(
            ErrorCategory::DriftDetected,
            format!(
                "Integration bytes changed before writing {}",
                path.display()
            ),
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && expected.is_none() => {
            create_file(path, replacement)?;
            Ok(IntegrationStatus::Created)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(MinoError::new(
            ErrorCategory::DriftDetected,
            format!(
                "Integration path disappeared before writing {}",
                path.display()
            ),
        )),
        Err(error) => Err(io_error("read", path, &error)),
    }
}

fn ensure_no_symlink(root: &Path, path: &Path) -> Result<(), MinoError> {
    let relative = path.strip_prefix(root).map_err(|_| {
        MinoError::new(
            ErrorCategory::PolicyViolation,
            format!(
                "Integration path {} is outside the project root",
                path.display()
            ),
        )
    })?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(MinoError::new(
                    ErrorCategory::DriftDetected,
                    format!(
                        "Integration path {} contains symbolic link {}",
                        path.display(),
                        current.display()
                    ),
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => return Err(io_error("inspect", &current, &error)),
        }
    }
    Ok(())
}

fn create_file(path: &Path, bytes: &[u8]) -> Result<(), MinoError> {
    let parent = parent_directory(path)?;
    fs::create_dir_all(parent).map_err(|error| io_error("create directory", parent, &error))?;
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|error| io_error("create", path, &error))?;
    if let Err(error) = file.write_all(bytes).and_then(|()| file.sync_all()) {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(io_error("write", path, &error));
    }
    drop(file);
    sync_directory(parent)
}

fn guarded_replace(path: &Path, expected: &[u8], replacement: &[u8]) -> Result<(), MinoError> {
    let actual = fs::read(path).map_err(|error| io_error("read", path, &error))?;
    if actual != expected {
        return Err(MinoError::new(
            ErrorCategory::DriftDetected,
            format!(
                "Integration bytes changed before replacing {}",
                path.display()
            ),
        ));
    }
    let parent = parent_directory(path)?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            MinoError::new(
                ErrorCategory::EnvironmentUnavailable,
                format!("Integration path {} has no UTF-8 file name", path.display()),
            )
        })?;
    let sequence = NEXT_INTEGRATION_FILE.fetch_add(1, Ordering::Relaxed);
    let temporary = parent.join(format!(
        ".{file_name}.mino-integration-{}-{sequence}.tmp",
        std::process::id()
    ));
    let backup = parent.join(format!(
        ".{file_name}.mino-integration-{}-{sequence}.bak",
        std::process::id()
    ));
    create_temporary(&temporary, replacement)?;
    if let Err(error) = fs::rename(path, &backup) {
        let _ = fs::remove_file(&temporary);
        return Err(io_error("back up", path, &error));
    }
    if let Err(error) = fs::rename(&temporary, path) {
        let restoration = fs::rename(&backup, path);
        let _ = fs::remove_file(&temporary);
        return match restoration {
            Ok(()) => Err(io_error("publish", path, &error)),
            Err(restoration_error) => Err(MinoError::new(
                ErrorCategory::EnvironmentUnavailable,
                format!(
                    "Failed to publish {}: {error}; restoration failed: {restoration_error}",
                    path.display()
                ),
            )),
        };
    }
    fs::remove_file(&backup).map_err(|error| io_error("remove backup", &backup, &error))?;
    sync_directory(parent)
}

fn create_temporary(path: &Path, bytes: &[u8]) -> Result<(), MinoError> {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|error| io_error("create", path, &error))?;
    if let Err(error) = file.write_all(bytes).and_then(|()| file.sync_all()) {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(io_error("write", path, &error));
    }
    Ok(())
}

fn parent_directory(path: &Path) -> Result<&Path, MinoError> {
    path.parent().ok_or_else(|| {
        MinoError::new(
            ErrorCategory::EnvironmentUnavailable,
            format!(
                "Integration path {} has no parent directory",
                path.display()
            ),
        )
    })
}

fn io_error(action: &str, path: &Path, error: &std::io::Error) -> MinoError {
    MinoError::new(
        ErrorCategory::EnvironmentUnavailable,
        format!("Failed to {action} {}: {error}", path.display()),
    )
}

#[cfg(unix)]
fn sync_directory(directory: &Path) -> Result<(), MinoError> {
    File::open(directory)
        .and_then(|file| file.sync_all())
        .map_err(|error| io_error("synchronize", directory, &error))
}

#[cfg(not(unix))]
fn sync_directory(directory: &Path) -> Result<(), MinoError> {
    fs::metadata(directory)
        .map(|_| ())
        .map_err(|error| io_error("inspect", directory, &error))
}
