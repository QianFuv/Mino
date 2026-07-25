//! Deterministic project and per-plan storage paths.

use std::path::{Path, PathBuf};

use crate::domain::PlanId;

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

    /// Returns the directory containing all plans.
    #[must_use]
    pub fn plans_directory(&self) -> PathBuf {
        self.mino_directory().join("plans")
    }

    /// Returns the directory containing one plan and its history.
    #[must_use]
    pub fn plan_directory(&self, plan_id: &PlanId) -> PathBuf {
        self.plans_directory().join(plan_id.as_str())
    }

    /// Returns the current source-of-truth JSON path.
    #[must_use]
    pub fn current_plan(&self, plan_id: &PlanId) -> PathBuf {
        self.plan_directory(plan_id).join("plan.json")
    }

    /// Returns the append-only event log path.
    #[must_use]
    pub fn event_log(&self, plan_id: &PlanId) -> PathBuf {
        self.plan_directory(plan_id).join("events.jsonl")
    }

    /// Returns the immutable snapshot directory.
    #[must_use]
    pub fn snapshots_directory(&self, plan_id: &PlanId) -> PathBuf {
        self.plan_directory(plan_id).join("snapshots")
    }

    /// Returns the immutable snapshot path for a revision.
    #[must_use]
    pub fn snapshot(&self, plan_id: &PlanId, revision: u64) -> PathBuf {
        self.snapshots_directory(plan_id)
            .join(format!("{revision:020}.json"))
    }

    /// Returns the per-plan advisory lock path.
    #[must_use]
    pub fn lock_file(&self, plan_id: &PlanId) -> PathBuf {
        self.plan_directory(plan_id).join("store.lock")
    }

    pub(crate) fn transaction_directory(&self, plan_id: &PlanId) -> PathBuf {
        self.plan_directory(plan_id).join("transaction")
    }

    pub(crate) fn journal(&self, plan_id: &PlanId) -> PathBuf {
        self.transaction_directory(plan_id).join("journal.json")
    }

    pub(crate) fn pending_journal(&self, plan_id: &PlanId) -> PathBuf {
        self.transaction_directory(plan_id).join("journal.pending")
    }

    pub(crate) fn next_plan(&self, plan_id: &PlanId) -> PathBuf {
        self.transaction_directory(plan_id).join("plan.next.json")
    }

    pub(crate) fn previous_plan(&self, plan_id: &PlanId) -> PathBuf {
        self.transaction_directory(plan_id)
            .join("plan.previous.json")
    }
}
