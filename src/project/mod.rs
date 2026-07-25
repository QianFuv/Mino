//! Project discovery, initialization, inspection, and diagnosis services.

mod config;
mod doctor;
mod root;
mod scan;

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::{ErrorCategory, MinoError};

pub use config::{
    CatalogConfig, LockedStandard, ProjectConfig, ProjectLayout, ProtocolLock, StandardsLock,
};
pub use doctor::{DoctorFinding, DoctorReport, FindingSeverity, diagnose};
pub use root::{ProjectRoot, RootSource, discover, discover_for_init};
pub use scan::{Language, LanguageScore, ProjectScan, ScanEvidence, WorkspaceScan, scan_root};

use config::{create_file, parse_toml, serialize_toml};

/// Result of idempotently initializing project-owned Mino state.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProjectInitReport {
    /// Selected project root.
    pub root: PathBuf,
    /// Evidence used to select the root.
    pub root_source: RootSource,
    /// Newly created Mino-owned files.
    pub created_files: Vec<PathBuf>,
    /// Existing compatible Mino-owned files left unchanged.
    pub existing_files: Vec<PathBuf>,
    /// Drift, corruption, and missing integration findings.
    pub findings: Vec<DoctorFinding>,
}

impl ProjectInitReport {
    /// Returns whether initialization found no error-severity state defects.
    #[must_use]
    pub fn is_healthy(&self) -> bool {
        self.findings
            .iter()
            .all(|finding| finding.severity != FindingSeverity::Error)
    }
}

/// Read-only project state returned by `project show`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProjectShowReport {
    /// Selected project root.
    pub root: PathBuf,
    /// Evidence used to select the root.
    pub root_source: RootSource,
    /// Parsed configuration when valid.
    pub config: Option<ProjectConfig>,
    /// Parsed protocol lock when valid.
    pub protocol_lock: Option<ProtocolLock>,
    /// Parsed standards lock when valid.
    pub standards_lock: Option<StandardsLock>,
    /// Current doctor findings.
    pub doctor: DoctorReport,
}

/// Idempotently creates only missing project-owned Mino state.
///
/// # Errors
///
/// Returns an environment-unavailable error when root discovery or filesystem
/// creation fails. Existing corrupt files are preserved and reported.
pub fn initialize(start: &Path) -> Result<ProjectInitReport, MinoError> {
    let project_root = discover_for_init(start)?;
    let layout = ProjectLayout::new(project_root.path());
    fs::create_dir_all(layout.plans_directory()).map_err(|error| {
        MinoError::new(
            ErrorCategory::EnvironmentUnavailable,
            format!(
                "Failed to create {}: {error}",
                layout.plans_directory().display()
            ),
        )
    })?;
    fs::create_dir_all(layout.standards_cache()).map_err(|error| {
        MinoError::new(
            ErrorCategory::EnvironmentUnavailable,
            format!(
                "Failed to create {}: {error}",
                layout.standards_cache().display()
            ),
        )
    })?;
    let mut created_files = Vec::new();
    let mut existing_files = Vec::new();
    let mut initialization_findings = Vec::new();
    ensure_config_file(
        &layout.config(),
        &mut created_files,
        &mut existing_files,
        &mut initialization_findings,
    )?;
    ensure_owned_file(
        &layout.protocol_lock(),
        &ProtocolLock::default(),
        "protocol_lock_mismatch",
        &mut created_files,
        &mut existing_files,
        &mut initialization_findings,
    )?;
    ensure_standards_lock(
        &layout.standards_lock(),
        &mut created_files,
        &mut existing_files,
        &mut initialization_findings,
    )?;
    let mut doctor = diagnose(&layout)?;
    doctor.findings.extend(initialization_findings);
    doctor
        .findings
        .sort_by(|left, right| left.code.cmp(&right.code));
    Ok(ProjectInitReport {
        root: project_root.path().to_path_buf(),
        root_source: project_root.source(),
        created_files,
        existing_files,
        findings: doctor.findings,
    })
}

fn ensure_config_file(
    path: &Path,
    created_files: &mut Vec<PathBuf>,
    existing_files: &mut Vec<PathBuf>,
    findings: &mut Vec<DoctorFinding>,
) -> Result<(), MinoError> {
    if !path.exists() {
        create_file(path, &serialize_toml(&ProjectConfig::default())?)?;
        created_files.push(path.to_path_buf());
        return Ok(());
    }
    match parse_toml::<ProjectConfig>(path) {
        Ok(config) if config.is_supported() => existing_files.push(path.to_path_buf()),
        Ok(_) => findings.push(DoctorFinding::new(
            "config_drift",
            FindingSeverity::Error,
            format!(
                "Existing project configuration {} is unsupported and was preserved",
                path.display()
            ),
            Some(path.to_path_buf()),
        )),
        Err(error) => findings.push(DoctorFinding::new(
            "config_drift_corrupt",
            FindingSeverity::Error,
            format!(
                "Existing project configuration {} was preserved: {error}",
                path.display()
            ),
            Some(path.to_path_buf()),
        )),
    }
    Ok(())
}

fn ensure_standards_lock(
    path: &Path,
    created_files: &mut Vec<PathBuf>,
    existing_files: &mut Vec<PathBuf>,
    findings: &mut Vec<DoctorFinding>,
) -> Result<(), MinoError> {
    if !path.exists() {
        create_file(path, &serialize_toml(&StandardsLock::default())?)?;
        created_files.push(path.to_path_buf());
        return Ok(());
    }
    match parse_toml::<StandardsLock>(path) {
        Ok(lock) if lock.is_supported() => existing_files.push(path.to_path_buf()),
        Ok(_) => findings.push(DoctorFinding::new(
            "standards_lock_drift",
            FindingSeverity::Error,
            format!(
                "Existing standards lock {} is unsupported and was preserved",
                path.display()
            ),
            Some(path.to_path_buf()),
        )),
        Err(error) => findings.push(DoctorFinding::new(
            "standards_lock_drift_corrupt",
            FindingSeverity::Error,
            format!(
                "Existing standards lock {} was preserved: {error}",
                path.display()
            ),
            Some(path.to_path_buf()),
        )),
    }
    Ok(())
}

/// Loads project state and doctor findings without mutation.
///
/// # Errors
///
/// Returns an error when no project root can be discovered or diagnosis cannot
/// enumerate the project state.
pub fn show(start: &Path) -> Result<ProjectShowReport, MinoError> {
    let project_root = discover(start)?;
    let layout = ProjectLayout::new(project_root.path());
    let config = parse_toml(&layout.config()).ok();
    let protocol_lock = parse_toml(&layout.protocol_lock()).ok();
    let standards_lock = parse_toml(&layout.standards_lock()).ok();
    let doctor = diagnose(&layout)?;
    Ok(ProjectShowReport {
        root: project_root.path().to_path_buf(),
        root_source: project_root.source(),
        config,
        protocol_lock,
        standards_lock,
        doctor,
    })
}

/// Runs project doctor after discovering an existing root.
///
/// # Errors
///
/// Returns an error when no root can be found or project state cannot be read.
pub fn doctor(start: &Path) -> Result<DoctorReport, MinoError> {
    let project_root = discover(start)?;
    diagnose(&ProjectLayout::new(project_root.path()))
}

/// Discovers an existing project root and scans its workspaces and languages.
///
/// # Errors
///
/// Returns an error when root discovery or ignore-aware traversal fails.
pub fn scan(start: &Path) -> Result<ProjectScan, MinoError> {
    let project_root = discover(start)?;
    scan_root(project_root.path())
}

fn ensure_owned_file<T>(
    path: &Path,
    expected: &T,
    drift_code: &str,
    created_files: &mut Vec<PathBuf>,
    existing_files: &mut Vec<PathBuf>,
    findings: &mut Vec<DoctorFinding>,
) -> Result<(), MinoError>
where
    T: for<'de> serde::Deserialize<'de> + PartialEq + Serialize,
{
    if !path.exists() {
        create_file(path, &serialize_toml(expected)?)?;
        created_files.push(path.to_path_buf());
        return Ok(());
    }
    match parse_toml::<T>(path) {
        Ok(actual) if &actual == expected => existing_files.push(path.to_path_buf()),
        Ok(_) => findings.push(DoctorFinding::new(
            drift_code,
            FindingSeverity::Error,
            format!(
                "Existing Mino-owned file {} differs and was preserved",
                path.display()
            ),
            Some(path.to_path_buf()),
        )),
        Err(error) => findings.push(DoctorFinding::new(
            format!("{drift_code}_corrupt"),
            FindingSeverity::Error,
            format!(
                "Existing Mino-owned file {} was preserved: {error}",
                path.display()
            ),
            Some(path.to_path_buf()),
        )),
    }
    Ok(())
}
