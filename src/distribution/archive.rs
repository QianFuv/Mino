//! Deterministic stored-ZIP construction and strict inventory verification.

use std::fs::{self, OpenOptions};
use std::io::{Cursor, Read, Write};
use std::path::{Component, Path};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, ZipArchive, ZipWriter};

use crate::store::sha256_digest;
use crate::{ErrorCategory, MinoError};

use super::manifest::ArtifactFile;

pub(super) struct ArchiveInput {
    pub(super) path: String,
    pub(super) bytes: Vec<u8>,
    pub(super) mode: u32,
}

pub(super) fn build_archive(files: &[ArchiveInput]) -> Result<Vec<u8>, MinoError> {
    validate_inputs(files)?;
    let cursor = Cursor::new(Vec::new());
    let mut writer = ZipWriter::new(cursor);
    for file in files {
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Stored)
            .last_modified_time(DateTime::default())
            .unix_permissions(file.mode);
        writer
            .start_file(&file.path, options)
            .map_err(|error| archive_error(format!("Failed to start {}: {error}", file.path)))?;
        writer
            .write_all(&file.bytes)
            .map_err(|error| archive_error(format!("Failed to write {}: {error}", file.path)))?;
    }
    let cursor = writer
        .finish()
        .map_err(|error| archive_error(format!("Failed to finalize plugin ZIP: {error}")))?;
    Ok(cursor.into_inner())
}

pub(super) fn inventory(files: &[ArchiveInput]) -> Result<Vec<ArtifactFile>, MinoError> {
    validate_inputs(files)?;
    files
        .iter()
        .map(|file| {
            Ok(ArtifactFile {
                path: file.path.clone(),
                digest: sha256_digest(&file.bytes),
                bytes: u64::try_from(file.bytes.len()).map_err(|_| {
                    archive_error(format!("Archive file {} is too large", file.path))
                })?,
                mode: file.mode,
            })
        })
        .collect()
}

pub(super) fn verify_archive(
    archive_bytes: &[u8],
    expected: &[ArtifactFile],
) -> Result<(), MinoError> {
    if expected.is_empty()
        || !expected.windows(2).all(|pair| pair[0].path < pair[1].path)
        || expected
            .iter()
            .map(|file| file.path.as_str())
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != expected.len()
    {
        return Err(archive_error(
            "Expected ZIP inventory must be non-empty, unique, and sorted",
        ));
    }
    let cursor = Cursor::new(archive_bytes);
    let mut archive = ZipArchive::new(cursor)
        .map_err(|error| archive_error(format!("Failed to open plugin ZIP: {error}")))?;
    if archive.len() != expected.len() {
        return Err(archive_error(format!(
            "Plugin ZIP has {} entries but {} were expected",
            archive.len(),
            expected.len()
        )));
    }
    for (index, expected_file) in expected.iter().enumerate() {
        let mut entry = archive.by_index(index).map_err(|error| {
            archive_error(format!("Failed to read plugin ZIP entry {index}: {error}"))
        })?;
        let unix_mode = entry.unix_mode();
        let has_expected_mode = unix_mode.is_some_and(|mode| {
            mode & 0o170_000 == 0o100_000 && mode & 0o777 == expected_file.mode
        });
        if entry.name() != expected_file.path
            || entry.is_dir()
            || entry.compression() != CompressionMethod::Stored
            || entry.last_modified() != Some(DateTime::default())
            || !has_expected_mode
            || entry.enclosed_name().is_none()
            || entry.size() != expected_file.bytes
        {
            return Err(archive_error(format!(
                "Plugin ZIP entry {} has unsafe or nondeterministic metadata: expected path={} mode={:o} bytes={}, directory={} compression={:?} modified={:?} mode={:?} enclosed={} bytes={}",
                entry.name(),
                expected_file.path,
                expected_file.mode,
                expected_file.bytes,
                entry.is_dir(),
                entry.compression(),
                entry.last_modified(),
                unix_mode.map(|mode| format!("{mode:o}")),
                entry.enclosed_name().is_some(),
                entry.size()
            )));
        }
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).map_err(|error| {
            archive_error(format!(
                "Failed to read ZIP entry {}: {error}",
                entry.name()
            ))
        })?;
        if sha256_digest(&bytes) != expected_file.digest {
            return Err(archive_error(format!(
                "Plugin ZIP entry {} differs from its manifest digest",
                expected_file.path
            )));
        }
    }
    Ok(())
}

pub(super) fn extract_archive(
    archive_bytes: &[u8],
    expected: &[ArtifactFile],
    destination: &Path,
) -> Result<(), MinoError> {
    verify_archive(archive_bytes, expected)?;
    fs::create_dir_all(destination).map_err(|error| {
        archive_error(format!(
            "Failed to create archive extraction root {}: {error}",
            destination.display()
        ))
    })?;
    let cursor = Cursor::new(archive_bytes);
    let mut archive = ZipArchive::new(cursor)
        .map_err(|error| archive_error(format!("Failed to reopen plugin ZIP: {error}")))?;
    for (index, expected_file) in expected.iter().enumerate() {
        #[cfg(not(unix))]
        let _ = expected_file;
        let mut entry = archive.by_index(index).map_err(|error| {
            archive_error(format!("Failed to read plugin ZIP entry {index}: {error}"))
        })?;
        let relative = entry
            .enclosed_name()
            .ok_or_else(|| archive_error(format!("Unsafe ZIP entry path {}", entry.name())))?;
        let path = destination.join(relative);
        let parent = path
            .parent()
            .ok_or_else(|| archive_error(format!("ZIP entry {} has no parent", path.display())))?;
        fs::create_dir_all(parent).map_err(|error| {
            archive_error(format!(
                "Failed to create extracted directory {}: {error}",
                parent.display()
            ))
        })?;
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .map_err(|error| {
                archive_error(format!(
                    "Failed to create extracted file {}: {error}",
                    path.display()
                ))
            })?;
        std::io::copy(&mut entry, &mut file).map_err(|error| {
            archive_error(format!("Failed to extract {}: {error}", path.display()))
        })?;
        file.sync_all().map_err(|error| {
            archive_error(format!(
                "Failed to sync extracted file {}: {error}",
                path.display()
            ))
        })?;
        #[cfg(unix)]
        fs::set_permissions(&path, fs::Permissions::from_mode(expected_file.mode)).map_err(
            |error| {
                archive_error(format!(
                    "Failed to set extracted mode for {}: {error}",
                    path.display()
                ))
            },
        )?;
    }
    Ok(())
}

pub(super) fn safe_archive_path(value: &str) -> Result<(), MinoError> {
    if value.is_empty() || value.contains('\\') || value.len() > 512 {
        return Err(archive_error(format!("Unsafe archive path {value:?}")));
    }
    let path = Path::new(value);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        || !value.starts_with("mino/")
    {
        return Err(archive_error(format!("Unsafe archive path {value:?}")));
    }
    Ok(())
}

fn validate_inputs(files: &[ArchiveInput]) -> Result<(), MinoError> {
    if files.is_empty() || !files.windows(2).all(|pair| pair[0].path < pair[1].path) {
        return Err(archive_error(
            "Archive inputs must be non-empty and sorted by unique path",
        ));
    }
    for file in files {
        safe_archive_path(&file.path)?;
        if file.bytes.is_empty() || !matches!(file.mode, 0o644 | 0o755) {
            return Err(archive_error(format!(
                "Archive input {} has empty bytes or an invalid mode",
                file.path
            )));
        }
    }
    Ok(())
}

fn archive_error(message: impl Into<String>) -> MinoError {
    MinoError::new(ErrorCategory::DriftDetected, message)
}
