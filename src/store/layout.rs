//! Deterministic project and per-plan storage paths.

use std::path::{Path, PathBuf};

use crate::domain::PlanId;
use crate::managed_fs::ManagedPath;

/// Deterministic paths used by the local Mino store.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StorePaths {
    project_root: PathBuf,
}

impl StorePaths {
    /// Creates a path resolver rooted at a project directory.
    #[must_use]
    pub fn new(project_root: impl Into<PathBuf>) -> Self {
        Self {
            project_root: project_root.into(),
        }
    }

    /// Returns the project root.
    #[must_use]
    pub fn project_root(&self) -> &Path {
        &self.project_root
    }

    /// Returns the project-local Mino directory.
    #[must_use]
    pub fn mino_directory(&self) -> PathBuf {
        self.project_root.join(".mino")
    }

    #[allow(clippy::unused_self)]
    pub(crate) fn mino_managed(&self) -> ManagedPath {
        managed_path(".mino")
    }

    /// Returns the directory containing all plans.
    #[must_use]
    pub fn plans_directory(&self) -> PathBuf {
        self.mino_directory().join("plans")
    }

    #[allow(clippy::unused_self)]
    pub(crate) fn plans_managed(&self) -> ManagedPath {
        managed_path(".mino/plans")
    }

    /// Returns the directory containing one plan and its history.
    #[must_use]
    pub fn plan_directory(&self, plan_id: &PlanId) -> PathBuf {
        self.plans_directory().join(plan_id.as_str())
    }

    pub(crate) fn plan_managed(&self, plan_id: &PlanId) -> ManagedPath {
        self.plans_managed()
            .join(plan_id.as_str())
            .expect("validated plan ID should form a managed path")
    }

    /// Returns the current source-of-truth JSON path.
    #[must_use]
    pub fn current_plan(&self, plan_id: &PlanId) -> PathBuf {
        self.plan_directory(plan_id).join("plan.json")
    }

    pub(crate) fn current_plan_managed(&self, plan_id: &PlanId) -> ManagedPath {
        self.plan_managed(plan_id)
            .join("plan.json")
            .expect("static plan file name should be valid")
    }

    /// Returns the append-only event log path.
    #[must_use]
    pub fn event_log(&self, plan_id: &PlanId) -> PathBuf {
        self.plan_directory(plan_id).join("events.jsonl")
    }

    pub(crate) fn event_log_managed(&self, plan_id: &PlanId) -> ManagedPath {
        self.plan_managed(plan_id)
            .join("events.jsonl")
            .expect("static event file name should be valid")
    }

    /// Returns the immutable snapshot directory.
    #[must_use]
    pub fn snapshots_directory(&self, plan_id: &PlanId) -> PathBuf {
        self.plan_directory(plan_id).join("snapshots")
    }

    pub(crate) fn snapshots_managed(&self, plan_id: &PlanId) -> ManagedPath {
        self.plan_managed(plan_id)
            .join("snapshots")
            .expect("static snapshot directory should be valid")
    }

    /// Returns the immutable snapshot path for a revision.
    #[must_use]
    pub fn snapshot(&self, plan_id: &PlanId, revision: u64) -> PathBuf {
        self.snapshots_directory(plan_id)
            .join(format!("{revision:020}.json"))
    }

    pub(crate) fn snapshot_managed(&self, plan_id: &PlanId, revision: u64) -> ManagedPath {
        self.snapshots_managed(plan_id)
            .join(format!("{revision:020}.json"))
            .expect("numeric snapshot file name should be valid")
    }

    /// Returns the per-plan advisory lock path.
    #[must_use]
    pub fn lock_file(&self, plan_id: &PlanId) -> PathBuf {
        self.plan_directory(plan_id).join("store.lock")
    }

    pub(crate) fn lock_managed(&self, plan_id: &PlanId) -> ManagedPath {
        self.plan_managed(plan_id)
            .join("store.lock")
            .expect("static lock file name should be valid")
    }

    pub(crate) fn transaction_managed(&self, plan_id: &PlanId) -> ManagedPath {
        self.plan_managed(plan_id)
            .join("transaction")
            .expect("static transaction directory should be valid")
    }

    pub(crate) fn journal_managed(&self, plan_id: &PlanId) -> ManagedPath {
        self.transaction_managed(plan_id)
            .join("journal.json")
            .expect("static journal file name should be valid")
    }

    pub(crate) fn pending_journal_managed(&self, plan_id: &PlanId) -> ManagedPath {
        self.transaction_managed(plan_id)
            .join("journal.pending")
            .expect("static pending journal file name should be valid")
    }

    pub(crate) fn next_plan_managed(&self, plan_id: &PlanId) -> ManagedPath {
        self.transaction_managed(plan_id)
            .join("plan.next.json")
            .expect("static next-plan file name should be valid")
    }

    pub(crate) fn previous_plan_managed(&self, plan_id: &PlanId) -> ManagedPath {
        self.transaction_managed(plan_id)
            .join("plan.previous.json")
            .expect("static previous-plan file name should be valid")
    }
}

fn managed_path(path: &str) -> ManagedPath {
    ManagedPath::new(path).expect("static store layout path should be valid")
}
