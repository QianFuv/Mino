//! Read-only project, lock, transaction, projection, and integration diagnosis.

use std::path::{Path, PathBuf};

use crate::domain::Plan;
use crate::git::{ActiveBindingStatus, ActiveBindingStore, GitAdapter};
use crate::integration::{IntegrationFindingSeverity, inspect_project, inspect_transactions};
use crate::managed_fs::{
    ManagedDirEntry, ManagedEntryKind, ManagedFsErrorKind, ManagedPath, ProjectFs,
};
use crate::render::{ProjectionStatus, check_managed_projection, render_plan};
use crate::store::PlanStore;
use crate::{ErrorCategory, MinoError};
use serde::Serialize;

use super::config::{
    PROTOCOL_LOCK_VERSION, ProjectConfig, ProjectLayout, ProtocolLock, STANDARDS_LOCK_VERSION,
    StandardsLock, parse_managed_toml,
};
use super::{PlanningAuthorityService, ProjectPlanSelectionStore};

const MAX_PLAN_STATE_BYTES: u64 = 8 * 1_024 * 1_024;

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
    let filesystem = ProjectFs::open(layout.root()).map_err(|error| {
        MinoError::new(ErrorCategory::EnvironmentUnavailable, error.to_string())
    })?;
    let has_managed_root = inspect_required_directory(
        &filesystem,
        &ProjectLayout::mino_managed(),
        "mino_directory_missing",
        "The Mino state directory is missing",
        &mut findings,
    );
    let has_safe_projection_directory = inspect_optional_directory(
        &filesystem,
        &ProjectLayout::projection_directory_managed(),
        &mut findings,
    );
    if has_managed_root {
        inspect_config(layout, &filesystem, &mut findings);
        inspect_protocol_lock(layout, &filesystem, &mut findings);
        inspect_standards_lock(layout, &filesystem, &mut findings);
        inspect_integration_transactions(layout, &mut findings);
        inspect_transactions_and_projections(
            layout,
            &filesystem,
            has_safe_projection_directory,
            &mut findings,
        );
        inspect_active_binding(layout, &filesystem, &mut findings);
        inspect_plan_selection(layout, &filesystem, &mut findings);
    }
    inspect_integrations(layout, &mut findings)?;
    inspect_planning_authority(layout, &mut findings);
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

fn inspect_planning_authority(layout: &ProjectLayout, findings: &mut Vec<DoctorFinding>) {
    match PlanningAuthorityService::new(layout.root()).status() {
        Ok(status) if status.blocks_durable_planning => {
            let is_declined =
                status.block_reason.as_deref() == Some("mino_durable_planning_declined");
            findings.push(DoctorFinding::new(
                if is_declined {
                    "mino_durable_planning_declined"
                } else {
                    "legacy_planning_authority_conflict"
                },
                FindingSeverity::Error,
                status.block_reason.map_or_else(
                    || "Durable planning authority is unresolved".to_owned(),
                    |reason| format!("Durable planning authority is blocked: {reason}"),
                ),
                Some(layout.agents_file()),
            ));
        }
        Ok(_) => {}
        Err(error) => findings.push(DoctorFinding::new(
            "planning_authority_unreadable",
            FindingSeverity::Error,
            error.to_string(),
            Some(layout.authority()),
        )),
    }
}

fn inspect_plan_selection(
    layout: &ProjectLayout,
    filesystem: &ProjectFs,
    findings: &mut Vec<DoctorFinding>,
) {
    let path = layout.plan_selection();
    let managed_path = ProjectLayout::plan_selection_managed();
    if !inspect_optional_file(filesystem, &managed_path, findings) {
        return;
    }
    let selection = match ProjectPlanSelectionStore::new(layout.root()).inspect() {
        Ok(Some(selection)) => selection,
        Ok(None) => return,
        Err(error) => {
            findings.push(DoctorFinding::new(
                "plan_selection_corrupt",
                FindingSeverity::Error,
                error.to_string(),
                Some(path),
            ));
            return;
        }
    };
    if selection.selected_plan.is_none() && !selection.alternatives.is_empty() {
        findings.push(DoctorFinding::new(
            "plan_selection_required",
            FindingSeverity::Warning,
            "Live plan alternatives require an explicit selected plan",
            Some(path.clone()),
        ));
    }
    let store = PlanStore::new(layout.root());
    for plan_id in selection.candidates() {
        match store.load_plan(plan_id) {
            Ok(plan) if plan.status() != crate::domain::PlanStatus::Done && !plan.is_archived() => {
            }
            Ok(_) => findings.push(DoctorFinding::new(
                "plan_selection_inactive",
                FindingSeverity::Error,
                format!("Project selection references inactive plan {plan_id}"),
                Some(path.clone()),
            )),
            Err(error) => findings.push(DoctorFinding::new(
                "plan_selection_plan_missing",
                FindingSeverity::Error,
                format!("Project selection references unavailable plan {plan_id}: {error}"),
                Some(path.clone()),
            )),
        }
    }
}

fn inspect_required_directory(
    filesystem: &ProjectFs,
    path: &ManagedPath,
    missing_code: &str,
    missing_message: &str,
    findings: &mut Vec<DoctorFinding>,
) -> bool {
    match filesystem.entry_kind(path) {
        Ok(Some(ManagedEntryKind::Directory)) => true,
        Ok(None) => {
            findings.push(DoctorFinding::new(
                missing_code,
                FindingSeverity::Error,
                missing_message,
                Some(filesystem.display_path(path)),
            ));
            false
        }
        Ok(Some(kind)) => {
            push_unsafe_managed_path(filesystem, path, &format!("found {kind:?}"), findings);
            false
        }
        Err(error) => {
            push_managed_error(filesystem, path, &error.to_string(), error.kind(), findings);
            false
        }
    }
}

fn inspect_optional_directory(
    filesystem: &ProjectFs,
    path: &ManagedPath,
    findings: &mut Vec<DoctorFinding>,
) -> bool {
    match filesystem.entry_kind(path) {
        Ok(Some(ManagedEntryKind::Directory) | None) => true,
        Ok(Some(kind)) => {
            push_unsafe_managed_path(filesystem, path, &format!("found {kind:?}"), findings);
            false
        }
        Err(error) => {
            push_managed_error(filesystem, path, &error.to_string(), error.kind(), findings);
            false
        }
    }
}

fn inspect_required_file(
    filesystem: &ProjectFs,
    path: &ManagedPath,
    missing_code: &str,
    missing_message: &str,
    findings: &mut Vec<DoctorFinding>,
) -> bool {
    match filesystem.entry_kind(path) {
        Ok(Some(ManagedEntryKind::File)) => true,
        Ok(None) => {
            findings.push(DoctorFinding::new(
                missing_code,
                FindingSeverity::Error,
                missing_message,
                Some(filesystem.display_path(path)),
            ));
            false
        }
        Ok(Some(kind)) => {
            push_unsafe_managed_path(filesystem, path, &format!("found {kind:?}"), findings);
            false
        }
        Err(error) => {
            push_managed_error(filesystem, path, &error.to_string(), error.kind(), findings);
            false
        }
    }
}

fn inspect_optional_file(
    filesystem: &ProjectFs,
    path: &ManagedPath,
    findings: &mut Vec<DoctorFinding>,
) -> bool {
    match filesystem.entry_kind(path) {
        Ok(Some(ManagedEntryKind::File)) => true,
        Ok(None) => false,
        Ok(Some(kind)) => {
            push_unsafe_managed_path(filesystem, path, &format!("found {kind:?}"), findings);
            false
        }
        Err(error) => {
            push_managed_error(filesystem, path, &error.to_string(), error.kind(), findings);
            false
        }
    }
}

fn push_unsafe_managed_path(
    filesystem: &ProjectFs,
    path: &ManagedPath,
    detail: &str,
    findings: &mut Vec<DoctorFinding>,
) {
    findings.push(DoctorFinding::new(
        "managed_path_unsafe",
        FindingSeverity::Error,
        format!(
            "Managed path {} is unsafe: {detail}",
            filesystem.display_path(path).display()
        ),
        Some(filesystem.display_path(path)),
    ));
}

fn push_managed_error(
    filesystem: &ProjectFs,
    path: &ManagedPath,
    message: &str,
    kind: ManagedFsErrorKind,
    findings: &mut Vec<DoctorFinding>,
) {
    let code = match kind {
        ManagedFsErrorKind::InvalidPath | ManagedFsErrorKind::UnsafeComponent => {
            "managed_path_unsafe"
        }
        ManagedFsErrorKind::Io => "managed_path_unreadable",
    };
    findings.push(DoctorFinding::new(
        code,
        FindingSeverity::Error,
        message,
        Some(filesystem.display_path(path)),
    ));
}

fn inspect_active_binding(
    layout: &ProjectLayout,
    filesystem: &ProjectFs,
    findings: &mut Vec<DoctorFinding>,
) {
    let path = layout.active_bindings();
    let managed_path = ProjectLayout::active_bindings_managed();
    if !inspect_optional_file(filesystem, &managed_path, findings) {
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
                ManagedPath::new(format!(".mino/plans/{}/plan.json", binding.plan_id))
                    .ok()
                    .and_then(|path| filesystem.is_file(&path).ok())
                    == Some(true)
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

fn inspect_config(
    layout: &ProjectLayout,
    filesystem: &ProjectFs,
    findings: &mut Vec<DoctorFinding>,
) {
    let path = layout.config();
    let managed_path = ProjectLayout::config_managed();
    if !inspect_required_file(
        filesystem,
        &managed_path,
        "config_missing",
        "The project configuration is missing",
        findings,
    ) {
        return;
    }
    match parse_managed_toml::<ProjectConfig>(filesystem, &managed_path) {
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

fn inspect_protocol_lock(
    layout: &ProjectLayout,
    filesystem: &ProjectFs,
    findings: &mut Vec<DoctorFinding>,
) {
    let path = layout.protocol_lock();
    inspect_owned_toml(
        filesystem,
        &ProjectLayout::protocol_lock_managed(),
        &ProtocolLock::default(),
        "protocol_lock_missing",
        "protocol_lock_corrupt",
        "protocol_lock_mismatch",
        findings,
    );
    if let Ok(lock) =
        parse_managed_toml::<ProtocolLock>(filesystem, &ProjectLayout::protocol_lock_managed())
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

fn inspect_standards_lock(
    layout: &ProjectLayout,
    filesystem: &ProjectFs,
    findings: &mut Vec<DoctorFinding>,
) {
    let path = layout.standards_lock();
    let managed_path = ProjectLayout::standards_lock_managed();
    if !inspect_required_file(
        filesystem,
        &managed_path,
        "standards_lock_missing",
        "The standards lock is missing",
        findings,
    ) {
        return;
    }
    let lock = match parse_managed_toml::<StandardsLock>(filesystem, &managed_path) {
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
    filesystem: &ProjectFs,
    path: &ManagedPath,
    expected: &T,
    missing_code: &str,
    corrupt_code: &str,
    drift_code: &str,
    findings: &mut Vec<DoctorFinding>,
) where
    T: for<'de> serde::Deserialize<'de> + PartialEq,
{
    let display_path = filesystem.display_path(path);
    if !inspect_required_file(
        filesystem,
        path,
        missing_code,
        &format!("Required Mino file {} is missing", display_path.display()),
        findings,
    ) {
        return;
    }
    match parse_managed_toml::<T>(filesystem, path) {
        Ok(actual) if &actual == expected => {}
        Ok(_) => findings.push(DoctorFinding::new(
            drift_code,
            FindingSeverity::Error,
            format!(
                "Mino-owned file {} differs from the supported value",
                display_path.display()
            ),
            Some(display_path.clone()),
        )),
        Err(error) => findings.push(DoctorFinding::new(
            corrupt_code,
            FindingSeverity::Error,
            error.to_string(),
            Some(display_path),
        )),
    }
}

fn inspect_transactions_and_projections(
    layout: &ProjectLayout,
    filesystem: &ProjectFs,
    has_safe_projection_directory: bool,
    findings: &mut Vec<DoctorFinding>,
) {
    let plans_directory = layout.plans_directory();
    let managed_plans_directory = ProjectLayout::plans_directory_managed();
    if !inspect_required_directory(
        filesystem,
        &managed_plans_directory,
        "plans_directory_missing",
        "The Mino plans directory is missing",
        findings,
    ) {
        return;
    }
    let entries = match filesystem.read_directory(&managed_plans_directory) {
        Ok(entries) => entries,
        Err(error) => {
            push_managed_error(
                filesystem,
                &managed_plans_directory,
                &error.to_string(),
                error.kind(),
                findings,
            );
            return;
        }
    };
    for entry in entries {
        inspect_plan_entry(
            filesystem,
            &managed_plans_directory,
            &plans_directory,
            &entry,
            has_safe_projection_directory,
            findings,
        );
    }
}

fn inspect_plan_entry(
    filesystem: &ProjectFs,
    plans_directory: &ManagedPath,
    display_plans_directory: &Path,
    entry: &ManagedDirEntry,
    has_safe_projection_directory: bool,
    findings: &mut Vec<DoctorFinding>,
) {
    let plan_state_directory = match plans_directory.join(&entry.name) {
        Ok(path) => path,
        Err(error) => {
            findings.push(DoctorFinding::new(
                "managed_path_unsafe",
                FindingSeverity::Error,
                error.to_string(),
                Some(display_plans_directory.join(&entry.name)),
            ));
            return;
        }
    };
    if entry.kind != ManagedEntryKind::Directory {
        push_unsafe_managed_path(
            filesystem,
            &plan_state_directory,
            &format!("found {:?}, expected Directory", entry.kind),
            findings,
        );
        return;
    }
    let transaction_directory = plan_state_directory
        .join("transaction")
        .expect("static transaction directory should form a managed path");
    match filesystem.entry_kind(&transaction_directory) {
        Ok(Some(ManagedEntryKind::Directory)) => findings.push(DoctorFinding::new(
            "incomplete_transaction",
            FindingSeverity::Error,
            "A prepared storage transaction requires recovery",
            Some(filesystem.display_path(&transaction_directory)),
        )),
        Ok(None) => {}
        Ok(Some(kind)) => push_unsafe_managed_path(
            filesystem,
            &transaction_directory,
            &format!("found {kind:?}, expected Directory"),
            findings,
        ),
        Err(error) => push_managed_error(
            filesystem,
            &transaction_directory,
            &error.to_string(),
            error.kind(),
            findings,
        ),
    }
    let plan_path = plan_state_directory
        .join("plan.json")
        .expect("static plan file name should form a managed path");
    match filesystem.entry_kind(&plan_path) {
        Ok(Some(ManagedEntryKind::File)) if has_safe_projection_directory => {
            inspect_projection(filesystem, &plan_path, findings);
        }
        Ok(Some(ManagedEntryKind::File) | None) => {}
        Ok(Some(kind)) => push_unsafe_managed_path(
            filesystem,
            &plan_path,
            &format!("found {kind:?}, expected File"),
            findings,
        ),
        Err(error) => push_managed_error(
            filesystem,
            &plan_path,
            &error.to_string(),
            error.kind(),
            findings,
        ),
    }
}

fn inspect_projection(
    filesystem: &ProjectFs,
    plan_path: &ManagedPath,
    findings: &mut Vec<DoctorFinding>,
) {
    let display_plan_path = filesystem.display_path(plan_path);
    let plan = match filesystem
        .read_bounded(plan_path, MAX_PLAN_STATE_BYTES)
        .map_err(|error| error.to_string())
        .and_then(|bytes| serde_json::from_slice::<Plan>(&bytes).map_err(|error| error.to_string()))
    {
        Ok(plan) => plan,
        Err(error) => {
            findings.push(DoctorFinding::new(
                "plan_state_corrupt",
                FindingSeverity::Error,
                format!("Failed to read {}: {error}", display_plan_path.display()),
                Some(display_plan_path),
            ));
            return;
        }
    };
    let projection = match projection_path(&plan) {
        Ok(projection) => projection,
        Err(message) => {
            findings.push(DoctorFinding::new(
                "render_path_invalid",
                FindingSeverity::Error,
                message,
                Some(display_plan_path),
            ));
            return;
        }
    };
    let display_projection = filesystem.display_path(&projection);
    match filesystem.entry_kind(&projection) {
        Ok(Some(ManagedEntryKind::File) | None) => {}
        Ok(Some(kind)) => {
            push_unsafe_managed_path(
                filesystem,
                &projection,
                &format!("found {kind:?}, expected File"),
                findings,
            );
            return;
        }
        Err(error) => {
            push_managed_error(
                filesystem,
                &projection,
                &error.to_string(),
                error.kind(),
                findings,
            );
            return;
        }
    }
    let rendered = match render_plan(&plan) {
        Ok(rendered) => rendered,
        Err(error) => {
            findings.push(DoctorFinding::new(
                "render_failed",
                FindingSeverity::Error,
                error.to_string(),
                Some(display_projection),
            ));
            return;
        }
    };
    match check_managed_projection(filesystem, &projection, &rendered) {
        Ok(check) if check.status() == ProjectionStatus::Current => {}
        Ok(check) if check.status() == ProjectionStatus::Missing => {
            findings.push(DoctorFinding::new(
                "render_missing",
                FindingSeverity::Warning,
                "The managed Markdown projection is missing",
                Some(display_projection),
            ));
        }
        Ok(_) => findings.push(DoctorFinding::new(
            "render_drift",
            FindingSeverity::Error,
            "The managed Markdown projection differs from source state",
            Some(display_projection),
        )),
        Err(error) => findings.push(DoctorFinding::new(
            "render_unreadable",
            FindingSeverity::Error,
            error.to_string(),
            Some(display_projection),
        )),
    }
}

fn projection_path(plan: &Plan) -> Result<ManagedPath, String> {
    let path = plan
        .metadata()
        .markdown_path()
        .map_or_else(|| format!("docs/plan/{}.md", plan.id()), str::to_owned);
    if Path::new(&path)
        .extension()
        .is_none_or(|extension| extension != "md")
    {
        return Err(format!(
            "Managed Markdown path {path} must name a Markdown file"
        ));
    }
    ManagedPath::new(&path).map_err(|error| error.to_string())
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

fn inspect_integration_transactions(layout: &ProjectLayout, findings: &mut Vec<DoctorFinding>) {
    let inspections = match inspect_transactions(layout.root()) {
        Ok(inspections) => inspections,
        Err(error) => {
            findings.push(DoctorFinding::new(
                "integration_transaction_corrupt",
                FindingSeverity::Error,
                error.to_string(),
                Some(layout.integration_transactions()),
            ));
            return;
        }
    };
    for inspection in inspections {
        findings.push(DoctorFinding::new(
            if inspection.is_corrupt {
                "integration_transaction_corrupt"
            } else {
                "integration_transaction_pending"
            },
            FindingSeverity::Error,
            inspection.message,
            Some(inspection.path),
        ));
    }
}
