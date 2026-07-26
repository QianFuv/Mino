//! Projection comparison and guarded filesystem publication.

#[cfg(unix)]
use std::fs::File;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use super::{RenderError, RenderErrorKind, RenderedPlan};
use crate::managed_fs::{
    ManagedEntryKind, ManagedFsError, ManagedFsErrorKind, ManagedPath, ProjectFs,
};
use crate::store::sha256_digest;

static NEXT_TEMPORARY_FILE: AtomicU64 = AtomicU64::new(1);

/// Relationship between a filesystem projection and expected rendered bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProjectionStatus {
    /// No projection exists at the requested path.
    Missing,
    /// The projection is byte-identical to the expected rendering.
    Current,
    /// The projection exists but contains different bytes.
    Drifted,
}

/// Digest-bearing result of checking a projection without modifying it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectionCheck {
    status: ProjectionStatus,
    expected_digest: String,
    actual_digest: Option<String>,
}

impl ProjectionCheck {
    /// Returns the projection relationship.
    #[must_use]
    pub const fn status(&self) -> ProjectionStatus {
        self.status
    }

    /// Returns the digest of expected rendered bytes.
    #[must_use]
    pub fn expected_digest(&self) -> &str {
        &self.expected_digest
    }

    /// Returns the digest of existing bytes when a file exists.
    #[must_use]
    pub fn actual_digest(&self) -> Option<&str> {
        self.actual_digest.as_deref()
    }
}

/// Result of a guarded projection publication.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProjectionWriteOutcome {
    /// A missing projection was created.
    Created,
    /// A verified prior rendering was replaced.
    Updated,
    /// Existing bytes already matched the requested rendering.
    Unchanged,
}

/// Checks a projection without modifying it.
///
/// # Errors
///
/// Returns an error when an existing projection cannot be read.
pub fn check_projection(
    path: &Path,
    expected: &RenderedPlan,
) -> Result<ProjectionCheck, RenderError> {
    match fs::read(path) {
        Ok(actual) => {
            let status = if actual == expected.as_bytes() {
                ProjectionStatus::Current
            } else {
                ProjectionStatus::Drifted
            };
            Ok(ProjectionCheck {
                status,
                expected_digest: expected.projection_digest().to_owned(),
                actual_digest: Some(sha256_digest(&actual)),
            })
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(ProjectionCheck {
            status: ProjectionStatus::Missing,
            expected_digest: expected.projection_digest().to_owned(),
            actual_digest: None,
        }),
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn check_managed_projection(
    filesystem: &ProjectFs,
    path: &ManagedPath,
    expected: &RenderedPlan,
) -> Result<ProjectionCheck, RenderError> {
    match filesystem.entry_kind(path).map_err(managed_render_error)? {
        Some(ManagedEntryKind::File) => {
            let actual = filesystem.read(path).map_err(managed_render_error)?;
            let status = if actual == expected.as_bytes() {
                ProjectionStatus::Current
            } else {
                ProjectionStatus::Drifted
            };
            Ok(ProjectionCheck {
                status,
                expected_digest: expected.projection_digest().to_owned(),
                actual_digest: Some(sha256_digest(&actual)),
            })
        }
        None => Ok(ProjectionCheck {
            status: ProjectionStatus::Missing,
            expected_digest: expected.projection_digest().to_owned(),
            actual_digest: None,
        }),
        Some(kind) => Err(RenderError::new(
            RenderErrorKind::Drift,
            format!(
                "Managed projection {} is {kind:?}, expected a regular file",
                filesystem.display_path(path).display()
            ),
        )),
    }
}

/// Creates or updates a projection without overwriting unrecognized bytes.
///
/// A legitimate update must provide the exact prior rendering. Existing bytes
/// that match neither the new nor prior rendering are reported as drift.
///
/// # Errors
///
/// Returns a drift error rather than overwriting a missing or edited prior
/// projection, and returns I/O errors from guarded publication.
pub fn write_projection(
    path: &Path,
    rendered: &RenderedPlan,
    prior: Option<&RenderedPlan>,
) -> Result<ProjectionWriteOutcome, RenderError> {
    match fs::read(path) {
        Ok(actual) if actual == rendered.as_bytes() => Ok(ProjectionWriteOutcome::Unchanged),
        Ok(actual) if prior.is_some_and(|prior| actual == prior.as_bytes()) => {
            guarded_replace(path, &actual, rendered.as_bytes())?;
            Ok(ProjectionWriteOutcome::Updated)
        }
        Ok(actual) => Err(drift_error(path, rendered.as_bytes(), Some(&actual))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && prior.is_none() => {
            create_projection(path, rendered.as_bytes())?;
            Ok(ProjectionWriteOutcome::Created)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Err(drift_error(path, rendered.as_bytes(), None))
        }
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn write_managed_projection(
    filesystem: &ProjectFs,
    path: &ManagedPath,
    rendered: &RenderedPlan,
    prior: Option<&RenderedPlan>,
) -> Result<ProjectionWriteOutcome, RenderError> {
    match filesystem.entry_kind(path).map_err(managed_render_error)? {
        Some(ManagedEntryKind::File) => {
            let actual = filesystem.read(path).map_err(managed_render_error)?;
            if actual == rendered.as_bytes() {
                Ok(ProjectionWriteOutcome::Unchanged)
            } else if prior.is_some_and(|prior| actual == prior.as_bytes()) {
                guarded_replace_managed(filesystem, path, &actual, rendered.as_bytes())?;
                Ok(ProjectionWriteOutcome::Updated)
            } else {
                Err(drift_error(
                    &filesystem.display_path(path),
                    rendered.as_bytes(),
                    Some(&actual),
                ))
            }
        }
        None if prior.is_none() => {
            create_managed_projection(filesystem, path, rendered.as_bytes())?;
            Ok(ProjectionWriteOutcome::Created)
        }
        None => Err(drift_error(
            &filesystem.display_path(path),
            rendered.as_bytes(),
            None,
        )),
        Some(kind) => Err(RenderError::new(
            RenderErrorKind::Drift,
            format!(
                "Managed projection {} is {kind:?}, expected a regular file",
                filesystem.display_path(path).display()
            ),
        )),
    }
}

fn create_projection(path: &Path, bytes: &[u8]) -> Result<(), RenderError> {
    let parent = parent_directory(path)?;
    fs::create_dir_all(parent)?;
    let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    sync_directory(parent)
}

fn guarded_replace(path: &Path, expected: &[u8], replacement: &[u8]) -> Result<(), RenderError> {
    let actual = fs::read(path)?;
    if actual != expected {
        return Err(drift_error(path, replacement, Some(&actual)));
    }
    let parent = parent_directory(path)?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            RenderError::new(
                RenderErrorKind::Io,
                format!("Projection path {} has no UTF-8 file name", path.display()),
            )
        })?;
    let sequence = NEXT_TEMPORARY_FILE.fetch_add(1, Ordering::Relaxed);
    let temporary = parent.join(format!(
        ".{file_name}.mino-{}-{sequence}.tmp",
        std::process::id()
    ));
    let backup = parent.join(format!(
        ".{file_name}.mino-{}-{sequence}.bak",
        std::process::id()
    ));
    write_temporary(&temporary, replacement)?;
    fs::rename(path, &backup)?;
    if let Err(error) = fs::rename(&temporary, path) {
        let restoration = fs::rename(&backup, path);
        let _ = fs::remove_file(&temporary);
        return match restoration {
            Ok(()) => Err(error.into()),
            Err(restoration_error) => Err(RenderError::new(
                RenderErrorKind::Io,
                format!(
                    "Projection replacement failed: {error}; restoration failed: {restoration_error}"
                ),
            )),
        };
    }
    fs::remove_file(backup)?;
    sync_directory(parent)
}

fn create_managed_projection(
    filesystem: &ProjectFs,
    path: &ManagedPath,
    bytes: &[u8],
) -> Result<(), RenderError> {
    let mut file = filesystem
        .create_new_file(path)
        .map_err(managed_render_error)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    filesystem.sync_parent(path).map_err(managed_render_error)
}

fn guarded_replace_managed(
    filesystem: &ProjectFs,
    path: &ManagedPath,
    expected: &[u8],
    replacement: &[u8],
) -> Result<(), RenderError> {
    let actual = filesystem.read(path).map_err(managed_render_error)?;
    if actual != expected {
        return Err(drift_error(
            &filesystem.display_path(path),
            replacement,
            Some(&actual),
        ));
    }
    let file_name = path
        .as_path()
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            RenderError::new(
                RenderErrorKind::Io,
                format!(
                    "Projection path {} has no UTF-8 file name",
                    filesystem.display_path(path).display()
                ),
            )
        })?;
    let sequence = NEXT_TEMPORARY_FILE.fetch_add(1, Ordering::Relaxed);
    let temporary = sibling_managed_path(
        path,
        format!(".{file_name}.mino-{}-{sequence}.tmp", std::process::id()),
    )?;
    let backup = sibling_managed_path(
        path,
        format!(".{file_name}.mino-{}-{sequence}.bak", std::process::id()),
    )?;
    let mut temporary_file = filesystem
        .create_new_file(&temporary)
        .map_err(managed_render_error)?;
    temporary_file.write_all(replacement)?;
    temporary_file.sync_all()?;
    drop(temporary_file);
    filesystem
        .rename(path, &backup)
        .map_err(managed_render_error)?;
    if let Err(error) = filesystem.rename(&temporary, path) {
        let restoration = filesystem.rename(&backup, path);
        let _ = filesystem.remove_file_if_exists(&temporary);
        return match restoration {
            Ok(()) => Err(managed_render_error(error)),
            Err(restoration_error) => Err(RenderError::new(
                RenderErrorKind::Io,
                format!(
                    "Projection replacement failed: {error}; restoration failed: {restoration_error}"
                ),
            )),
        };
    }
    filesystem
        .remove_file(&backup)
        .map_err(managed_render_error)?;
    filesystem.sync_parent(path).map_err(managed_render_error)
}

fn sibling_managed_path(path: &ManagedPath, file_name: String) -> Result<ManagedPath, RenderError> {
    match path.parent() {
        Some(parent) => parent.join(file_name).map_err(managed_render_error),
        None => ManagedPath::new(file_name).map_err(managed_render_error),
    }
}

fn managed_render_error(error: ManagedFsError) -> RenderError {
    let kind = match error.kind() {
        ManagedFsErrorKind::InvalidPath | ManagedFsErrorKind::UnsafeComponent => {
            RenderErrorKind::Drift
        }
        ManagedFsErrorKind::Io => RenderErrorKind::Io,
    };
    RenderError::new(kind, error.into_message())
}

fn write_temporary(path: &Path, bytes: &[u8]) -> Result<(), RenderError> {
    let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn drift_error(path: &Path, expected: &[u8], actual: Option<&[u8]>) -> RenderError {
    let actual_digest = actual.map_or_else(|| "missing".to_owned(), sha256_digest);
    RenderError::new(
        RenderErrorKind::Drift,
        format!(
            "Projection drift at {}: expected {}, found {actual_digest}",
            path.display(),
            sha256_digest(expected)
        ),
    )
}

fn parent_directory(path: &Path) -> Result<&Path, RenderError> {
    path.parent()
        .map(|parent| {
            if parent.as_os_str().is_empty() {
                Path::new(".")
            } else {
                parent
            }
        })
        .ok_or_else(|| {
            RenderError::new(
                RenderErrorKind::Io,
                format!("Projection path {} has no parent directory", path.display()),
            )
        })
}

#[cfg(unix)]
fn sync_directory(directory: &Path) -> Result<(), RenderError> {
    File::open(directory)?.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn sync_directory(directory: &Path) -> Result<(), RenderError> {
    let _ = fs::metadata(directory)?;
    Ok(())
}
