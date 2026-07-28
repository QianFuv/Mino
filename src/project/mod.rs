//! Project discovery, initialization, inspection, and diagnosis services.

mod authority;
mod config;
mod doctor;
mod import;
mod migrate;
mod root;
mod scan;
mod selection;

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::integration::{IntegrationOptions, IntegrationReport, integrate_project};
use crate::managed_fs::{ManagedPath, ProjectFs};
use crate::protocol::ProtocolRegistry;
use crate::{ErrorCategory, MinoError};

pub use authority::{
    PlanningAuthorityApplyRequest, PlanningAuthorityDecision, PlanningAuthorityDecisionRequest,
    PlanningAuthorityMutationReport, PlanningAuthorityProposal, PlanningAuthorityService,
    PlanningAuthorityStatus,
};
pub use config::{
    CatalogConfig, LockedStandard, ProjectConfig, ProjectLayout, ProtocolLock, StandardsLock,
};
pub use doctor::{DoctorFinding, DoctorReport, FindingSeverity, diagnose};
pub use import::{
    LegacyPlanMapping, LegacyPlanMappingDisposition, LegacyPlanParseReport, LegacyPlanSource,
    LegacyPlanWarning, parse_legacy_plan, verify_legacy_plan_source,
};
pub use migrate::{
    LegacyDocumentKind, LegacyFinding, LegacyInput, LegacyMapping, LegacyMappingDisposition,
    LegacyMigrationReport, LegacyPlanningClause, LegacyPlanningClauseKind, LegacyProposedChange,
    LegacySourceSummary, analyze_legacy,
};
pub use root::{ProjectRoot, RootSource, discover, discover_for_init};
pub use scan::{
    Language, LanguageScore, ProjectScan, ScanEvidence, ScanLimits, WorkspaceScan, scan_root,
    scan_root_with_limits,
};
pub use selection::{
    PlanSelectionRequest, PlanSelectionWriteReport, ProjectPlanSelection, ProjectPlanSelectionStore,
};

use authority::ensure_authority_state;
pub(crate) use authority::{authority_status_action, require_durable_planning_authority};
use config::{create_file, map_managed_error, parse_managed_toml, serialize_toml};

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
    /// Repository Skill and owned-block reconciliation results.
    pub integrations: IntegrationReport,
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
    initialize_with_options(start, IntegrationOptions::default())
}

/// Initializes project state and applies only explicitly selected integrations.
///
/// The bundled Skill is installed automatically. Repository instruction and
/// ignore blocks are modified only when their corresponding option is true.
///
/// # Errors
///
/// Returns an environment or drift error when project state or a verified
/// integration write cannot be created safely.
pub fn initialize_with_options(
    start: &Path,
    integration_options: IntegrationOptions,
) -> Result<ProjectInitReport, MinoError> {
    ProtocolRegistry::current().map_err(|error| {
        MinoError::new(
            ErrorCategory::DriftDetected,
            format!("Cannot initialize with an invalid protocol bundle: {error}"),
        )
    })?;
    let project_root = discover_for_init(start)?;
    let layout = ProjectLayout::new(project_root.path());
    let filesystem = ProjectFs::open(layout.root()).map_err(map_managed_error)?;
    filesystem
        .ensure_directory(&ProjectLayout::plans_directory_managed())
        .map_err(map_managed_error)?;
    filesystem
        .ensure_directory(&ProjectLayout::standards_cache_managed())
        .map_err(map_managed_error)?;
    let mut created_files = Vec::new();
    let mut existing_files = Vec::new();
    let mut initialization_findings = Vec::new();
    ensure_config_file(
        &filesystem,
        &ProjectLayout::config_managed(),
        &mut created_files,
        &mut existing_files,
        &mut initialization_findings,
    )?;
    ensure_owned_file(
        &filesystem,
        &ProjectLayout::protocol_lock_managed(),
        &ProtocolLock::default(),
        "protocol_lock_mismatch",
        &mut created_files,
        &mut existing_files,
        &mut initialization_findings,
    )?;
    ensure_standards_lock(
        &filesystem,
        &ProjectLayout::standards_lock_managed(),
        &mut created_files,
        &mut existing_files,
        &mut initialization_findings,
    )?;
    let integrations = integrate_project(layout.root(), integration_options)?;
    if let Some(authority) = ensure_authority_state(layout.root())? {
        if authority.created {
            created_files.push(authority.path);
        } else {
            existing_files.push(authority.path);
        }
    }
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
        integrations,
    })
}

fn ensure_config_file(
    filesystem: &ProjectFs,
    path: &ManagedPath,
    created_files: &mut Vec<PathBuf>,
    existing_files: &mut Vec<PathBuf>,
    findings: &mut Vec<DoctorFinding>,
) -> Result<(), MinoError> {
    let display_path = filesystem.display_path(path);
    if !filesystem.exists(path).map_err(map_managed_error)? {
        create_file(
            filesystem,
            path,
            &serialize_toml(&ProjectConfig::default())?,
        )?;
        created_files.push(display_path);
        return Ok(());
    }
    match parse_managed_toml::<ProjectConfig>(filesystem, path) {
        Ok(config) if config.is_supported() => existing_files.push(display_path),
        Ok(_) => findings.push(DoctorFinding::new(
            "config_drift",
            FindingSeverity::Error,
            format!(
                "Existing project configuration {} is unsupported and was preserved",
                display_path.display()
            ),
            Some(display_path),
        )),
        Err(error) if error.category() == ErrorCategory::DriftDetected => return Err(error),
        Err(error) => findings.push(DoctorFinding::new(
            "config_drift_corrupt",
            FindingSeverity::Error,
            format!(
                "Existing project configuration {} was preserved: {error}",
                display_path.display()
            ),
            Some(display_path),
        )),
    }
    Ok(())
}

fn ensure_standards_lock(
    filesystem: &ProjectFs,
    path: &ManagedPath,
    created_files: &mut Vec<PathBuf>,
    existing_files: &mut Vec<PathBuf>,
    findings: &mut Vec<DoctorFinding>,
) -> Result<(), MinoError> {
    let display_path = filesystem.display_path(path);
    if !filesystem.exists(path).map_err(map_managed_error)? {
        create_file(
            filesystem,
            path,
            &serialize_toml(&StandardsLock::default())?,
        )?;
        created_files.push(display_path);
        return Ok(());
    }
    match parse_managed_toml::<StandardsLock>(filesystem, path) {
        Ok(lock) if lock.is_supported() => existing_files.push(display_path),
        Ok(_) => findings.push(DoctorFinding::new(
            "standards_lock_drift",
            FindingSeverity::Error,
            format!(
                "Existing standards lock {} is unsupported and was preserved",
                display_path.display()
            ),
            Some(display_path),
        )),
        Err(error) if error.category() == ErrorCategory::DriftDetected => return Err(error),
        Err(error) => findings.push(DoctorFinding::new(
            "standards_lock_drift_corrupt",
            FindingSeverity::Error,
            format!(
                "Existing standards lock {} was preserved: {error}",
                display_path.display()
            ),
            Some(display_path),
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
    let filesystem = ProjectFs::open(layout.root()).map_err(map_managed_error)?;
    let config = parse_managed_toml(&filesystem, &ProjectLayout::config_managed()).ok();
    let protocol_lock =
        parse_managed_toml(&filesystem, &ProjectLayout::protocol_lock_managed()).ok();
    let standards_lock =
        parse_managed_toml(&filesystem, &ProjectLayout::standards_lock_managed()).ok();
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
    filesystem: &ProjectFs,
    path: &ManagedPath,
    expected: &T,
    drift_code: &str,
    created_files: &mut Vec<PathBuf>,
    existing_files: &mut Vec<PathBuf>,
    findings: &mut Vec<DoctorFinding>,
) -> Result<(), MinoError>
where
    T: for<'de> serde::Deserialize<'de> + PartialEq + Serialize,
{
    let display_path = filesystem.display_path(path);
    if !filesystem.exists(path).map_err(map_managed_error)? {
        create_file(filesystem, path, &serialize_toml(expected)?)?;
        created_files.push(display_path);
        return Ok(());
    }
    match parse_managed_toml::<T>(filesystem, path) {
        Ok(actual) if &actual == expected => existing_files.push(display_path),
        Ok(_) => findings.push(DoctorFinding::new(
            drift_code,
            FindingSeverity::Error,
            format!(
                "Existing Mino-owned file {} differs and was preserved",
                display_path.display()
            ),
            Some(display_path),
        )),
        Err(error) if error.category() == ErrorCategory::DriftDetected => return Err(error),
        Err(error) => findings.push(DoctorFinding::new(
            format!("{drift_code}_corrupt"),
            FindingSeverity::Error,
            format!(
                "Existing Mino-owned file {} was preserved: {error}",
                display_path.display()
            ),
            Some(display_path),
        )),
    }
    Ok(())
}
