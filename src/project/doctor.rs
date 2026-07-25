//! Read-only project, lock, transaction, projection, and integration diagnosis.

use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

use crate::domain::Plan;
use crate::git::{ActiveBindingStatus, ActiveBindingStore, GitAdapter};
use crate::integration::{IntegrationFindingSeverity, inspect_project};
use crate::render::{ProjectionStatus, check_projection, render_plan};
use crate::{ErrorCategory, MinoError};

use super::config::{
    PROTOCOL_LOCK_VERSION, ProjectConfig, ProjectLayout, ProtocolLock, STANDARDS_LOCK_VERSION,
    StandardsLock, parse_toml,
};

/// Severity assigned to a deterministic doctor finding.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingSeverity {
    /// State is corrupt, incompatible, or unsafe to use.
    Error,
    /// Integration or projected state is incomplete.
    Warning,
    /// A non-blocking fact is reported for visibility.
    Info,
}

/// One stable project-doctor finding.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DoctorFinding {
    /// Stable machine-readable finding code.
    pub code: String,
    /// Finding severity.
    pub severity: FindingSeverity,
    /// Human-readable explanation.
    pub message: String,
    /// Related repository path when applicable.
    pub path: Option<PathBuf>,
}

impl DoctorFinding {
    pub(crate) fn new(
        code: impl Into<String>,
        severity: FindingSeverity,
        message: impl Into<String>,
        path: Option<PathBuf>,
    ) -> Self {
        Self {
            code: code.into(),
            severity,
            message: message.into(),
            path,
        }
    }
}

/// Complete read-only diagnosis for a project root.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DoctorReport {
    /// Diagnosed project root.
    pub root: PathBuf,
    /// Stable findings in deterministic category and path order.
    pub findings: Vec<DoctorFinding>,
}

impl DoctorReport {
    /// Returns whether no error-severity findings exist.
    #[must_use]
    pub fn is_healthy(&self) -> bool {
        self.findings
            .iter()
            .all(|finding| finding.severity != FindingSeverity::Error)
    }

    /// Returns whether no findings of any severity exist.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.findings.is_empty()
    }
}

/// Runs all read-only project lifecycle checks.
///
/// # Errors
///
/// Returns an environment-unavailable error when project directories cannot be
/// enumerated. Individual malformed owned files are returned as findings.
pub fn diagnose(layout: &ProjectLayout) -> Result<DoctorReport, MinoError> {
    let mut findings = Vec::new();
    inspect_config(layout, &mut findings);
    inspect_protocol_lock(layout, &mut findings);
    inspect_standards_lock(layout, &mut findings);
    inspect_transactions_and_projections(layout, &mut findings)?;
    inspect_active_binding(layout, &mut findings);
    inspect_integrations(layout, &mut findings)?;
    findings.sort_by(|left, right| {
        left.code
            .cmp(&right.code)
            .then_with(|| left.path.cmp(&right.path))
    });
    Ok(DoctorReport {
        root: layout.root().to_path_buf(),
        findings,
    })
}

fn inspect_active_binding(layout: &ProjectLayout, findings: &mut Vec<DoctorFinding>) {
    let path = layout.active_bindings();
    if !path.exists() {
        return;
    }
    let facts = match GitAdapter::new(layout.root()).inspect() {
        Ok(facts) => facts,
        Err(error) => {
            findings.push(DoctorFinding::new(
                "active_binding_git_unavailable",
                FindingSeverity::Error,
                error.to_string(),
                Some(path),
            ));
            return;
        }
    };
    let resolution = match ActiveBindingStore::new(layout.root()).resolve(&facts) {
        Ok(resolution) => resolution,
        Err(error) => {
            findings.push(DoctorFinding::new(
                "active_binding_corrupt",
                FindingSeverity::Error,
                error.to_string(),
                Some(path),
            ));
            return;
        }
    };
    let code = match resolution.status {
        ActiveBindingStatus::Current => {
            if resolution.binding.as_ref().is_some_and(|binding| {
                layout
                    .plans_directory()
                    .join(binding.plan_id.as_str())
                    .join("plan.json")
                    .is_file()
            }) {
                return;
            }
            "active_binding_plan_missing"
        }
        ActiveBindingStatus::ForeignWorktree => "active_binding_worktree_mismatch",
        ActiveBindingStatus::StaleBranch => "active_binding_branch_stale",
        ActiveBindingStatus::StaleHead => "active_binding_head_stale",
        ActiveBindingStatus::NotRepository => "active_binding_repository_missing",
        ActiveBindingStatus::Missing => return,
    };
    findings.push(DoctorFinding::new(
        code,
        FindingSeverity::Error,
        format!(
            "Active plan binding does not match the current Git identity: {:?}",
            resolution.status
        ),
        Some(path),
    ));
}

fn inspect_config(layout: &ProjectLayout, findings: &mut Vec<DoctorFinding>) {
    let path = layout.config();
    if !path.exists() {
        findings.push(DoctorFinding::new(
            "config_missing",
            FindingSeverity::Error,
            "The project configuration is missing",
            Some(path),
        ));
        return;
    }
    match parse_toml::<ProjectConfig>(&path) {
        Ok(config) if config.is_supported() => {}
        Ok(_) => findings.push(DoctorFinding::new(
            "config_drift",
            FindingSeverity::Error,
            "The project configuration version or root semantics are unsupported",
            Some(path),
        )),
        Err(error) => findings.push(DoctorFinding::new(
            "config_corrupt",
            FindingSeverity::Error,
            error.to_string(),
            Some(path),
        )),
    }
}

fn inspect_protocol_lock(layout: &ProjectLayout, findings: &mut Vec<DoctorFinding>) {
    let path = layout.protocol_lock();
    inspect_owned_toml(
        &path,
        &ProtocolLock::default(),
        "protocol_lock_missing",
        "protocol_lock_corrupt",
        "protocol_lock_mismatch",
        findings,
    );
    if let Ok(lock) = parse_toml::<ProtocolLock>(&path)
        && lock.lock_version != PROTOCOL_LOCK_VERSION
    {
        findings.push(DoctorFinding::new(
            "protocol_lock_version_unsupported",
            FindingSeverity::Error,
            format!("Protocol lock version {} is unsupported", lock.lock_version),
            Some(path),
        ));
    }
}

fn inspect_standards_lock(layout: &ProjectLayout, findings: &mut Vec<DoctorFinding>) {
    let path = layout.standards_lock();
    if !path.exists() {
        findings.push(DoctorFinding::new(
            "standards_lock_missing",
            FindingSeverity::Error,
            "The standards lock is missing",
            Some(path),
        ));
        return;
    }
    let lock = match parse_toml::<StandardsLock>(&path) {
        Ok(lock) => lock,
        Err(error) => {
            findings.push(DoctorFinding::new(
                "standards_lock_corrupt",
                FindingSeverity::Error,
                error.to_string(),
                Some(path),
            ));
            return;
        }
    };
    if lock.lock_version != STANDARDS_LOCK_VERSION {
        findings.push(DoctorFinding::new(
            "standards_lock_version_unsupported",
            FindingSeverity::Error,
            format!(
                "Standards lock version {} is unsupported",
                lock.lock_version
            ),
            Some(path.clone()),
        ));
    }
    if !lock
        .packages
        .windows(2)
        .all(|pair| pair[0].package_id < pair[1].package_id)
    {
        findings.push(DoctorFinding::new(
            "standards_lock_order_invalid",
            FindingSeverity::Error,
            "Standards packages must have unique IDs in ascending order",
            Some(path),
        ));
    }
}

fn inspect_owned_toml<T>(
    path: &Path,
    expected: &T,
    missing_code: &str,
    corrupt_code: &str,
    drift_code: &str,
    findings: &mut Vec<DoctorFinding>,
) where
    T: for<'de> serde::Deserialize<'de> + PartialEq,
{
    if !path.exists() {
        findings.push(DoctorFinding::new(
            missing_code,
            FindingSeverity::Error,
            format!("Required Mino file {} is missing", path.display()),
            Some(path.to_path_buf()),
        ));
        return;
    }
    match parse_toml::<T>(path) {
        Ok(actual) if &actual == expected => {}
        Ok(_) => findings.push(DoctorFinding::new(
            drift_code,
            FindingSeverity::Error,
            format!(
                "Mino-owned file {} differs from the supported value",
                path.display()
            ),
            Some(path.to_path_buf()),
        )),
        Err(error) => findings.push(DoctorFinding::new(
            corrupt_code,
            FindingSeverity::Error,
            error.to_string(),
            Some(path.to_path_buf()),
        )),
    }
}

fn inspect_transactions_and_projections(
    layout: &ProjectLayout,
    findings: &mut Vec<DoctorFinding>,
) -> Result<(), MinoError> {
    let plans_directory = layout.plans_directory();
    if !plans_directory.exists() {
        findings.push(DoctorFinding::new(
            "plans_directory_missing",
            FindingSeverity::Error,
            "The Mino plans directory is missing",
            Some(plans_directory),
        ));
        return Ok(());
    }
    let entries = fs::read_dir(&plans_directory).map_err(|error| {
        MinoError::new(
            ErrorCategory::EnvironmentUnavailable,
            format!("Failed to inspect {}: {error}", plans_directory.display()),
        )
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            MinoError::new(
                ErrorCategory::EnvironmentUnavailable,
                format!("Failed to inspect a plan directory entry: {error}"),
            )
        })?;
        if !entry.file_type().is_ok_and(|file_type| file_type.is_dir()) {
            continue;
        }
        let plan_state_directory = entry.path();
        let transaction_directory = plan_state_directory.join("transaction");
        if transaction_directory.exists() {
            findings.push(DoctorFinding::new(
                "incomplete_transaction",
                FindingSeverity::Error,
                "A prepared storage transaction requires recovery",
                Some(transaction_directory),
            ));
        }
        let plan_path = plan_state_directory.join("plan.json");
        if plan_path.exists() {
            inspect_projection(layout, &plan_path, findings);
        }
    }
    Ok(())
}

fn inspect_projection(layout: &ProjectLayout, plan_path: &Path, findings: &mut Vec<DoctorFinding>) {
    let plan = match fs::read(plan_path)
        .map_err(|error| error.to_string())
        .and_then(|bytes| serde_json::from_slice::<Plan>(&bytes).map_err(|error| error.to_string()))
    {
        Ok(plan) => plan,
        Err(error) => {
            findings.push(DoctorFinding::new(
                "plan_state_corrupt",
                FindingSeverity::Error,
                format!("Failed to read {}: {error}", plan_path.display()),
                Some(plan_path.to_path_buf()),
            ));
            return;
        }
    };
    let projection = match projection_path(layout, &plan) {
        Ok(projection) => projection,
        Err(message) => {
            findings.push(DoctorFinding::new(
                "render_path_invalid",
                FindingSeverity::Error,
                message,
                Some(plan_path.to_path_buf()),
            ));
            return;
        }
    };
    let rendered = match render_plan(&plan) {
        Ok(rendered) => rendered,
        Err(error) => {
            findings.push(DoctorFinding::new(
                "render_failed",
                FindingSeverity::Error,
                error.to_string(),
                Some(projection),
            ));
            return;
        }
    };
    match check_projection(&projection, &rendered) {
        Ok(check) if check.status() == ProjectionStatus::Current => {}
        Ok(check) if check.status() == ProjectionStatus::Missing => {
            findings.push(DoctorFinding::new(
                "render_missing",
                FindingSeverity::Warning,
                "The managed Markdown projection is missing",
                Some(projection),
            ));
        }
        Ok(_) => findings.push(DoctorFinding::new(
            "render_drift",
            FindingSeverity::Error,
            "The managed Markdown projection differs from source state",
            Some(projection),
        )),
        Err(error) => findings.push(DoctorFinding::new(
            "render_unreadable",
            FindingSeverity::Error,
            error.to_string(),
            Some(projection),
        )),
    }
}

fn projection_path(layout: &ProjectLayout, plan: &Plan) -> Result<PathBuf, String> {
    let value = serde_json::to_value(plan).unwrap_or(Value::Null);
    let Some(path) = value["metadata"]["markdown_path"].as_str() else {
        return Ok(layout
            .root()
            .join("docs/plan")
            .join(format!("{}.md", plan.id())));
    };
    let relative = Path::new(path);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
    {
        return Err(format!(
            "Managed Markdown path {path} must remain inside the project root"
        ));
    }
    Ok(layout.root().join(relative))
}

fn inspect_integrations(
    layout: &ProjectLayout,
    findings: &mut Vec<DoctorFinding>,
) -> Result<(), MinoError> {
    for integration_finding in inspect_project(layout.root())?.findings {
        let severity = match integration_finding.severity {
            IntegrationFindingSeverity::Warning => FindingSeverity::Warning,
            IntegrationFindingSeverity::Error => FindingSeverity::Error,
        };
        findings.push(DoctorFinding::new(
            integration_finding.code,
            severity,
            integration_finding.message,
            Some(integration_finding.path),
        ));
    }
    Ok(())
}
