//! Versioned project-level active-plan selection and alternative tracking.

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use fs4::{FileExt, TryLockError};
use serde::{Deserialize, Serialize};

use crate::domain::{PlanId, RequestId, Timestamp};
use crate::managed_fs::{ManagedFsError, ManagedFsErrorKind, ManagedPath, ProjectFs};
use crate::{ErrorCategory, MinoError};

const SELECTION_SCHEMA_VERSION: u32 = 1;
const MAX_SELECTION_BYTES: u64 = 1024 * 1024;
const LOCK_TIMEOUT: Duration = Duration::from_secs(2);
const LOCK_RETRY: Duration = Duration::from_millis(10);
static NEXT_SELECTION_FILE: AtomicU64 = AtomicU64::new(1);

/// Current selected plan and every live alternative in deterministic order.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProjectPlanSelection {
    /// Optimistic-concurrency revision, or zero for derived legacy state.
    pub selection_revision: u64,
    /// Explicitly selected live plan when one exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected_plan: Option<PlanId>,
    /// Other live plans available for explicit selection.
    pub alternatives: Vec<PlanId>,
}

impl ProjectPlanSelection {
    /// Returns whether the project currently has no live plan candidates.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.selected_plan.is_none() && self.alternatives.is_empty()
    }

    /// Returns every live candidate with the selected plan first when present.
    #[must_use]
    pub fn candidates(&self) -> Vec<&PlanId> {
        self.selected_plan
            .iter()
            .chain(self.alternatives.iter())
            .collect()
    }
}

/// Approval-bound request for changing the selected project plan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlanSelectionRequest {
    /// Target live plan identifier.
    pub plan_id: PlanId,
    /// Required project selection revision.
    pub expected_selection_revision: u64,
    /// Idempotency UUID for this exact selection decision.
    pub request_id: RequestId,
    /// Actor recorded in the selection audit.
    pub actor: String,
    /// Auditable user approval reference.
    pub approval_reference: String,
    /// Non-empty reason for choosing the plan.
    pub reason: String,
    /// Complete canonical invoking command.
    pub command: Vec<String>,
    /// Timestamp captured once for the decision.
    pub selected_at: Timestamp,
}

/// Result of a project selection mutation or exact replay.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PlanSelectionWriteReport {
    /// Current selected plan and alternatives.
    #[serde(flatten)]
    pub selection: ProjectPlanSelection,
    /// Whether this exact request had already committed.
    pub replayed: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SelectionAudit {
    expected_selection_revision: u64,
    request_id: RequestId,
    selected_plan: PlanId,
    actor: String,
    approval_reference: String,
    reason: String,
    command: Vec<String>,
    selected_at: Timestamp,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SelectionFile {
    schema_version: u32,
    selection_revision: u64,
    selected_plan: Option<PlanId>,
    alternatives: Vec<PlanId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_selection: Option<SelectionAudit>,
}

/// Locked, bounded, atomically published project selection store.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectPlanSelectionStore {
    project_root: PathBuf,
}

impl ProjectPlanSelectionStore {
    /// Creates a project selection store under one initialized root.
    #[must_use]
    pub fn new(project_root: impl Into<PathBuf>) -> Self {
        Self {
            project_root: project_root.into(),
        }
    }

    /// Returns the deterministic selection state path.
    #[must_use]
    pub fn path(&self) -> PathBuf {
        self.project_root.join(".mino/plan-selection.json")
    }

    /// Resolves persisted choice against the supplied live plan set.
    ///
    /// Missing legacy state selects a sole candidate virtually and leaves a
    /// multiple-candidate project unselected until an explicit choice.
    ///
    /// # Errors
    ///
    /// Returns a drift or environment error for malformed, unsafe, oversized,
    /// or unreadable selection state.
    pub fn resolve(&self, live_plans: &[PlanId]) -> Result<ProjectPlanSelection, MinoError> {
        let filesystem = self.filesystem()?;
        let file = load_file(&filesystem)?;
        resolve_file(file.as_ref(), live_plans)
    }

    /// Returns the exact persisted state without reconciling live plans.
    ///
    /// # Errors
    ///
    /// Returns a drift or environment error for malformed, unsafe, oversized,
    /// or unreadable selection state.
    pub fn inspect(&self) -> Result<Option<ProjectPlanSelection>, MinoError> {
        let filesystem = self.filesystem()?;
        load_file(&filesystem)?.map_or(Ok(None), |file| {
            validate_file(&file)?;
            Ok(Some(view(&file)))
        })
    }

    /// Selects one live candidate with revision and request-ID protection.
    ///
    /// # Errors
    ///
    /// Returns a revision conflict for stale or reused requests, an approval or
    /// policy error for incomplete/no-op choices, or an environment error when
    /// locking or atomic publication fails.
    pub fn select(
        &self,
        request: PlanSelectionRequest,
        live_plans: &[PlanId],
    ) -> Result<PlanSelectionWriteReport, MinoError> {
        validate_request(&request)?;
        let filesystem = self.filesystem()?;
        let _lock = SelectionLock::acquire(&filesystem)?;
        let current = load_file(&filesystem)?;
        let current_view = resolve_file(current.as_ref(), live_plans)?;
        let target_revision = request
            .expected_selection_revision
            .checked_add(1)
            .ok_or_else(|| revision_error("Selection revision overflowed"))?;
        if let Some(file) = &current
            && file.selection_revision == target_revision
            && file
                .last_selection
                .as_ref()
                .is_some_and(|audit| same_request(audit, &request))
        {
            return Ok(PlanSelectionWriteReport {
                selection: resolve_file(Some(file), live_plans)?,
                replayed: true,
            });
        }
        if current_view.selection_revision != request.expected_selection_revision {
            return Err(revision_error(format!(
                "Project selection is revision {}, not expected revision {}",
                current_view.selection_revision, request.expected_selection_revision
            )));
        }
        let live = normalize_live_plans(live_plans)?;
        if !live.contains(&request.plan_id) {
            return Err(MinoError::new(
                ErrorCategory::PolicyViolation,
                format!("Plan {} is not a live project alternative", request.plan_id),
            ));
        }
        if current_view.selected_plan.as_ref() == Some(&request.plan_id) {
            return Err(MinoError::new(
                ErrorCategory::PolicyViolation,
                format!("Plan {} is already selected", request.plan_id),
            ));
        }
        let candidate = SelectionFile {
            schema_version: SELECTION_SCHEMA_VERSION,
            selection_revision: target_revision,
            selected_plan: Some(request.plan_id.clone()),
            alternatives: live
                .into_iter()
                .filter(|plan_id| plan_id != &request.plan_id)
                .collect(),
            last_selection: Some(SelectionAudit {
                expected_selection_revision: request.expected_selection_revision,
                request_id: request.request_id,
                selected_plan: request.plan_id,
                actor: request.actor,
                approval_reference: request.approval_reference,
                reason: request.reason,
                command: request.command,
                selected_at: request.selected_at,
            }),
        };
        validate_file(&candidate)?;
        publish_file(&filesystem, &candidate)?;
        Ok(PlanSelectionWriteReport {
            selection: view(&candidate),
            replayed: false,
        })
    }

    pub(crate) fn register_created(
        &self,
        plan_id: &PlanId,
        live_plans: &[PlanId],
    ) -> Result<ProjectPlanSelection, MinoError> {
        self.update_membership(live_plans, |current, live| {
            let selected_plan = current
                .and_then(|file| file.selected_plan.clone())
                .filter(|selected| live.contains(selected))
                .or_else(|| (live.len() == 1).then(|| plan_id.clone()));
            membership_file(current, live, selected_plan)
        })
    }

    pub(crate) fn register_fork(
        &self,
        source_plan_id: &PlanId,
        live_plans: &[PlanId],
    ) -> Result<ProjectPlanSelection, MinoError> {
        self.update_membership(live_plans, |current, live| {
            let selected_plan = current
                .and_then(|file| file.selected_plan.clone())
                .filter(|selected| live.contains(selected))
                .or_else(|| {
                    (current.is_none() && live.contains(source_plan_id))
                        .then(|| source_plan_id.clone())
                })
                .or_else(|| (current.is_none() && live.len() == 1).then(|| live[0].clone()));
            membership_file(current, live, selected_plan)
        })
    }

    pub(crate) fn remove_archived(
        &self,
        live_plans: &[PlanId],
    ) -> Result<ProjectPlanSelection, MinoError> {
        self.update_membership(live_plans, |current, live| {
            let selected_plan = current
                .and_then(|file| file.selected_plan.clone())
                .filter(|selected| live.contains(selected));
            membership_file(current, live, selected_plan)
        })
    }

    fn update_membership<F>(
        &self,
        live_plans: &[PlanId],
        update: F,
    ) -> Result<ProjectPlanSelection, MinoError>
    where
        F: FnOnce(Option<&SelectionFile>, Vec<PlanId>) -> SelectionFile,
    {
        let filesystem = self.filesystem()?;
        let _lock = SelectionLock::acquire(&filesystem)?;
        let current = load_file(&filesystem)?;
        let live = normalize_live_plans(live_plans)?;
        let candidate = update(current.as_ref(), live);
        validate_file(&candidate)?;
        let membership_changed = current.as_ref().is_none_or(|file| {
            file.selected_plan != candidate.selected_plan
                || file.alternatives != candidate.alternatives
        });
        if membership_changed
            && current
                .as_ref()
                .is_some_and(|file| file.selection_revision == u64::MAX)
        {
            return Err(revision_error("Project selection revision overflowed"));
        }
        if current.as_ref() == Some(&candidate) {
            return Ok(view(&candidate));
        }
        publish_file(&filesystem, &candidate)?;
        Ok(view(&candidate))
    }

    fn filesystem(&self) -> Result<ProjectFs, MinoError> {
        ProjectFs::open(&self.project_root).map_err(managed_error)
    }
}

fn membership_file(
    current: Option<&SelectionFile>,
    live: Vec<PlanId>,
    selected_plan: Option<PlanId>,
) -> SelectionFile {
    let alternatives = live
        .into_iter()
        .filter(|plan_id| Some(plan_id) != selected_plan.as_ref())
        .collect();
    let current_revision = current.map_or(0, |file| file.selection_revision);
    let unchanged = current.is_some_and(|file| {
        file.selected_plan == selected_plan && file.alternatives == alternatives
    });
    SelectionFile {
        schema_version: SELECTION_SCHEMA_VERSION,
        selection_revision: if unchanged {
            current_revision
        } else {
            current_revision.saturating_add(1).max(1)
        },
        selected_plan,
        alternatives,
        last_selection: current.and_then(|file| file.last_selection.clone()),
    }
}

fn resolve_file(
    file: Option<&SelectionFile>,
    live_plans: &[PlanId],
) -> Result<ProjectPlanSelection, MinoError> {
    let live = normalize_live_plans(live_plans)?;
    let selected_plan = file
        .and_then(|file| file.selected_plan.clone())
        .filter(|selected| live.contains(selected))
        .or_else(|| (file.is_none() && live.len() == 1).then(|| live[0].clone()));
    let alternatives = live
        .into_iter()
        .filter(|plan_id| Some(plan_id) != selected_plan.as_ref())
        .collect();
    Ok(ProjectPlanSelection {
        selection_revision: file.map_or(0, |file| file.selection_revision),
        selected_plan,
        alternatives,
    })
}

fn normalize_live_plans(live_plans: &[PlanId]) -> Result<Vec<PlanId>, MinoError> {
    let mut live = live_plans.to_vec();
    live.sort();
    if !live.windows(2).all(|pair| pair[0] < pair[1]) {
        return Err(MinoError::new(
            ErrorCategory::DriftDetected,
            "Live project plan identifiers are not unique",
        ));
    }
    Ok(live)
}

fn view(file: &SelectionFile) -> ProjectPlanSelection {
    ProjectPlanSelection {
        selection_revision: file.selection_revision,
        selected_plan: file.selected_plan.clone(),
        alternatives: file.alternatives.clone(),
    }
}

fn validate_request(request: &PlanSelectionRequest) -> Result<(), MinoError> {
    if request.actor.trim().is_empty()
        || request.approval_reference.trim().is_empty()
        || request.reason.trim().is_empty()
        || request.command.is_empty()
        || request.command.iter().any(|part| part.trim().is_empty())
    {
        return Err(MinoError::new(
            ErrorCategory::IncompleteOrValidation,
            "Plan selection requires actor, approval reference, reason, and canonical command",
        ));
    }
    Ok(())
}

fn validate_file(file: &SelectionFile) -> Result<(), MinoError> {
    let sorted = file.alternatives.windows(2).all(|pair| pair[0] < pair[1]);
    let selected_is_separate = file
        .selected_plan
        .as_ref()
        .is_none_or(|selected| !file.alternatives.contains(selected));
    let audit_is_valid = file.last_selection.as_ref().is_none_or(|audit| {
        audit
            .expected_selection_revision
            .checked_add(1)
            .is_some_and(|revision| revision <= file.selection_revision)
            && !audit.actor.trim().is_empty()
            && !audit.approval_reference.trim().is_empty()
            && !audit.reason.trim().is_empty()
            && !audit.command.is_empty()
            && audit.command.iter().all(|part| !part.trim().is_empty())
    });
    if file.schema_version != SELECTION_SCHEMA_VERSION
        || file.selection_revision == 0
        || !sorted
        || !selected_is_separate
        || !audit_is_valid
    {
        return Err(MinoError::new(
            ErrorCategory::DriftDetected,
            "Project plan selection state is malformed or unsupported",
        ));
    }
    Ok(())
}

fn same_request(audit: &SelectionAudit, request: &PlanSelectionRequest) -> bool {
    audit.expected_selection_revision == request.expected_selection_revision
        && audit.request_id == request.request_id
        && audit.selected_plan == request.plan_id
        && audit.actor == request.actor
        && audit.approval_reference == request.approval_reference
        && audit.reason == request.reason
        && audit.command == request.command
}

fn load_file(filesystem: &ProjectFs) -> Result<Option<SelectionFile>, MinoError> {
    let path = selection_path();
    if !filesystem.exists(&path).map_err(managed_error)? {
        return Ok(None);
    }
    let bytes = filesystem
        .read_bounded(&path, MAX_SELECTION_BYTES)
        .map_err(managed_error)?;
    let file = serde_json::from_slice::<SelectionFile>(&bytes).map_err(|error| {
        MinoError::new(
            ErrorCategory::DriftDetected,
            format!(
                "Failed to parse project plan selection {}: {error}",
                filesystem.display_path(&path).display()
            ),
        )
    })?;
    validate_file(&file)?;
    if encode_file(&file)? != bytes {
        return Err(drift_error(format!(
            "Project plan selection {} is not canonical",
            filesystem.display_path(&path).display()
        )));
    }
    Ok(Some(file))
}

fn publish_file(filesystem: &ProjectFs, file: &SelectionFile) -> Result<(), MinoError> {
    let path = selection_path();
    let bytes = encode_file(file)?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_SELECTION_BYTES {
        return Err(MinoError::new(
            ErrorCategory::PolicyViolation,
            "Project plan selection exceeds its stable size limit",
        ));
    }
    let parent = path
        .parent()
        .ok_or_else(|| drift_error("Project plan selection path has no parent"))?;
    let sequence = NEXT_SELECTION_FILE.fetch_add(1, Ordering::Relaxed);
    let temporary = parent
        .join(format!(
            ".plan-selection.json.mino-select-{}-{sequence}.tmp",
            std::process::id()
        ))
        .map_err(managed_error)?;
    let backup = parent
        .join(format!(
            ".plan-selection.json.mino-select-{}-{sequence}.bak",
            std::process::id()
        ))
        .map_err(managed_error)?;
    write_new_file(filesystem, &temporary, &bytes)?;
    if !filesystem.exists(&path).map_err(managed_error)? {
        filesystem
            .rename(&temporary, &path)
            .map_err(managed_error)?;
        return filesystem.sync_parent(&path).map_err(managed_error);
    }
    filesystem.rename(&path, &backup).map_err(managed_error)?;
    if let Err(error) = filesystem.rename(&temporary, &path) {
        let restoration = filesystem.rename(&backup, &path);
        let _ = filesystem.remove_file_if_exists(&temporary);
        return match restoration {
            Ok(()) => Err(managed_error(error)),
            Err(restoration_error) => Err(MinoError::new(
                ErrorCategory::EnvironmentUnavailable,
                format!(
                    "Failed to publish {} ({error}) and restore its backup ({restoration_error})",
                    filesystem.display_path(&path).display()
                ),
            )),
        };
    }
    filesystem.remove_file(&backup).map_err(managed_error)?;
    filesystem.sync_parent(&path).map_err(managed_error)
}

fn encode_file(file: &SelectionFile) -> Result<Vec<u8>, MinoError> {
    let mut bytes = serde_json::to_vec_pretty(file).map_err(|error| {
        MinoError::new(
            ErrorCategory::EnvironmentUnavailable,
            format!("Failed to encode project plan selection: {error}"),
        )
    })?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn write_new_file(
    filesystem: &ProjectFs,
    path: &ManagedPath,
    bytes: &[u8],
) -> Result<(), MinoError> {
    let mut file = filesystem.create_new_file(path).map_err(managed_error)?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| {
            MinoError::new(
                ErrorCategory::EnvironmentUnavailable,
                format!(
                    "Failed to write project plan selection {}: {error}",
                    filesystem.display_path(path).display()
                ),
            )
        })
}

struct SelectionLock {
    file: std::fs::File,
}

impl SelectionLock {
    fn acquire(filesystem: &ProjectFs) -> Result<Self, MinoError> {
        let path = selection_lock_path();
        let file = filesystem.open_lock_file(&path).map_err(managed_error)?;
        let started_at = Instant::now();
        loop {
            match FileExt::try_lock(&file) {
                Ok(()) => return Ok(Self { file }),
                Err(TryLockError::WouldBlock) if started_at.elapsed() < LOCK_TIMEOUT => {
                    thread::sleep(LOCK_RETRY);
                }
                Err(TryLockError::WouldBlock) => {
                    return Err(MinoError::new(
                        ErrorCategory::EnvironmentUnavailable,
                        format!(
                            "Timed out acquiring project selection lock {}",
                            filesystem.display_path(&path).display()
                        ),
                    ));
                }
                Err(TryLockError::Error(error)) => {
                    return Err(MinoError::new(
                        ErrorCategory::EnvironmentUnavailable,
                        format!(
                            "Failed to lock project selection {}: {error}",
                            filesystem.display_path(&path).display()
                        ),
                    ));
                }
            }
        }
    }
}

impl Drop for SelectionLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

fn selection_path() -> ManagedPath {
    ManagedPath::new(".mino/plan-selection.json")
        .expect("static project selection path should be valid")
}

fn selection_lock_path() -> ManagedPath {
    ManagedPath::new(".mino/plan-selection.lock")
        .expect("static project selection lock path should be valid")
}

fn managed_error(error: ManagedFsError) -> MinoError {
    let category = match error.kind() {
        ManagedFsErrorKind::InvalidPath | ManagedFsErrorKind::UnsafeComponent => {
            ErrorCategory::DriftDetected
        }
        ManagedFsErrorKind::Io => ErrorCategory::EnvironmentUnavailable,
    };
    MinoError::new(category, error.into_message())
}

fn drift_error(message: impl Into<String>) -> MinoError {
    MinoError::new(ErrorCategory::DriftDetected, message)
}

fn revision_error(message: impl Into<String>) -> MinoError {
    MinoError::new(ErrorCategory::RevisionConflict, message)
}
