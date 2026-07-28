//! Bounded artifact capture and immutable content-addressed blob publication.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::domain::{EvidenceType, Redaction};
use crate::managed_fs::{ManagedFsError, ManagedFsErrorKind, ManagedPath, ProjectFs};
use crate::runner::Redactor;
use crate::store::sha256_digest;

use super::{EvidenceError, EvidenceErrorKind};

pub(super) const MAX_ARTIFACT_BYTES: u64 = 16 * 1_024 * 1_024;
static NEXT_PENDING_BLOB: AtomicU64 = AtomicU64::new(1);

pub(crate) struct PreparedArtifact {
    pub protocol_path: String,
    pub digest: String,
    pub bytes: Vec<u8>,
    pub redactions: Vec<Redaction>,
}

pub(crate) fn prepare_artifact(
    project_root: &Path,
    relative_path: &Path,
    kind: EvidenceType,
    redactor: &Redactor,
) -> Result<PreparedArtifact, EvidenceError> {
    let canonical_root = project_root
        .canonicalize()
        .map_err(|error| io_error("resolve project root", project_root, &error))?;
    validate_relative_path(relative_path)?;
    let canonical_path = canonical_root
        .join(relative_path)
        .canonicalize()
        .map_err(|error| io_error("resolve evidence artifact", relative_path, &error))?;
    if !canonical_path.starts_with(&canonical_root) || !canonical_path.is_file() {
        return Err(invalid(
            "Evidence artifact resolves outside the project or is not a file",
        ));
    }
    let metadata = fs::metadata(&canonical_path)
        .map_err(|error| io_error("inspect evidence artifact", &canonical_path, &error))?;
    if metadata.len() > MAX_ARTIFACT_BYTES {
        return Err(invalid(format!(
            "Evidence artifact exceeds the {MAX_ARTIFACT_BYTES}-byte limit"
        )));
    }
    let mut bytes = Vec::new();
    File::open(&canonical_path)
        .map_err(|error| io_error("open evidence artifact", &canonical_path, &error))?
        .take(MAX_ARTIFACT_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| io_error("read evidence artifact", &canonical_path, &error))?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_ARTIFACT_BYTES {
        bytes.fill(0);
        return Err(invalid(format!(
            "Evidence artifact exceeds the {MAX_ARTIFACT_BYTES}-byte limit"
        )));
    }
    let (stored_bytes, redactions) = redact_artifact(bytes, kind, redactor)?;
    let normalized_path = relative_path
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part),
            Component::CurDir
            | Component::ParentDir
            | Component::RootDir
            | Component::Prefix(_) => None,
        })
        .collect::<PathBuf>();
    let protocol_path = normalized_path.to_str().ok_or_else(|| {
        invalid(format!(
            "Evidence artifact path {} is not UTF-8",
            relative_path.display()
        ))
    })?;
    Ok(PreparedArtifact {
        protocol_path: protocol_path.replace('\\', "/"),
        digest: sha256_digest(&stored_bytes),
        bytes: stored_bytes,
        redactions,
    })
}

pub(crate) fn blob_path(
    blob_directory: &ManagedPath,
    digest: &str,
) -> Result<ManagedPath, EvidenceError> {
    let hexadecimal = validated_digest(digest)?;
    blob_directory
        .join(format!("{hexadecimal}.blob"))
        .map_err(managed_error)
}

pub(crate) fn publish_blob(
    filesystem: &ProjectFs,
    blob_directory: &ManagedPath,
    artifact: &PreparedArtifact,
) -> Result<bool, EvidenceError> {
    let path = blob_path(blob_directory, &artifact.digest)?;
    publish_immutable(filesystem, &path, &artifact.bytes)
}

pub(crate) fn publish_immutable(
    filesystem: &ProjectFs,
    path: &ManagedPath,
    bytes: &[u8],
) -> Result<bool, EvidenceError> {
    if filesystem.exists(path).map_err(managed_error)? {
        let existing = filesystem
            .read_bounded(path, MAX_ARTIFACT_BYTES)
            .map_err(managed_error)?;
        if existing == bytes {
            return Ok(true);
        }
        return Err(EvidenceError::new(
            EvidenceErrorKind::CorruptStore,
            format!(
                "Immutable artifact {} has conflicting bytes",
                filesystem.display_path(path).display()
            ),
        ));
    }
    let parent = path.parent().ok_or_else(|| {
        EvidenceError::new(
            EvidenceErrorKind::Io,
            format!(
                "Immutable artifact {} has no parent",
                filesystem.display_path(path).display()
            ),
        )
    })?;
    filesystem
        .ensure_directory(&parent)
        .map_err(managed_error)?;
    let sequence = NEXT_PENDING_BLOB.fetch_add(1, Ordering::Relaxed);
    let file_name = path
        .as_path()
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("artifact");
    let pending = parent
        .join(format!(
            ".{file_name}.{}.{}.pending",
            std::process::id(),
            sequence
        ))
        .map_err(managed_error)?;
    let mut file = filesystem
        .create_new_file(&pending)
        .map_err(managed_error)?;
    if let Err(error) = file.write_all(bytes).and_then(|()| file.sync_all()) {
        drop(file);
        let _ = filesystem.remove_file_if_exists(&pending);
        return Err(io_error(
            "write immutable artifact",
            &filesystem.display_path(&pending),
            &error,
        ));
    }
    drop(file);
    if let Err(error) = filesystem.rename(&pending, path) {
        let _ = filesystem.remove_file_if_exists(&pending);
        if filesystem.exists(path).map_err(managed_error)? {
            let existing = filesystem
                .read_bounded(path, MAX_ARTIFACT_BYTES)
                .map_err(managed_error)?;
            if existing == bytes {
                return Ok(true);
            }
            return Err(EvidenceError::new(
                EvidenceErrorKind::CorruptStore,
                format!(
                    "Immutable artifact {} was concurrently published with different bytes",
                    filesystem.display_path(path).display()
                ),
            ));
        }
        return Err(managed_error(error));
    }
    filesystem.sync_parent(path).map_err(managed_error)?;
    Ok(false)
}

pub(crate) fn validated_digest(digest: &str) -> Result<&str, EvidenceError> {
    let hexadecimal = digest.strip_prefix("sha256:").unwrap_or_default();
    if hexadecimal.len() == 64
        && hexadecimal
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(hexadecimal)
    } else {
        Err(invalid(
            "Evidence blob digest is not a lowercase SHA-256 value",
        ))
    }
}

fn validate_relative_path(path: &Path) -> Result<(), EvidenceError> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::CurDir | Component::Normal(_)))
        || path
            .components()
            .next()
            .is_some_and(|component| component.as_os_str() == ".mino")
    {
        Err(invalid(
            "Evidence artifact path must be project-relative and outside .mino",
        ))
    } else {
        Ok(())
    }
}

fn redact_artifact(
    mut bytes: Vec<u8>,
    kind: EvidenceType,
    redactor: &Redactor,
) -> Result<(Vec<u8>, Vec<Redaction>), EvidenceError> {
    let must_be_text = matches!(kind, EvidenceType::GitDiff | EvidenceType::Log);
    let should_redact =
        must_be_text || kind == EvidenceType::File && std::str::from_utf8(&bytes).is_ok();
    if !should_redact {
        return Ok((bytes, Vec::new()));
    }
    let text = String::from_utf8(std::mem::take(&mut bytes)).map_err(|error| {
        let message = format!("Text evidence artifact is not valid UTF-8: {error}");
        let mut recovered = error.into_bytes();
        recovered.fill(0);
        invalid(message)
    })?;
    let (redacted, applied, capture_blocked) = redactor.redact_checked(&text, &[]).into_parts();
    if capture_blocked {
        return Err(invalid(
            "Evidence capture was blocked by the residual credential scan",
        ));
    }
    let redactions = applied
        .into_iter()
        .map(|redaction| Redaction::new(redaction.rule_id(), redaction.replacements()))
        .collect();
    Ok((redacted.into_bytes(), redactions))
}

fn invalid(message: impl Into<String>) -> EvidenceError {
    EvidenceError::new(EvidenceErrorKind::InvalidRequest, message)
}

fn io_error(action: &str, path: &Path, error: &std::io::Error) -> EvidenceError {
    EvidenceError::new(
        EvidenceErrorKind::Io,
        format!("Failed to {action} {}: {error}", path.display()),
    )
}

fn managed_error(error: ManagedFsError) -> EvidenceError {
    let kind = match error.kind() {
        ManagedFsErrorKind::InvalidPath | ManagedFsErrorKind::UnsafeComponent => {
            EvidenceErrorKind::CorruptStore
        }
        ManagedFsErrorKind::Io => EvidenceErrorKind::Io,
    };
    EvidenceError::new(kind, error.into_message())
}
