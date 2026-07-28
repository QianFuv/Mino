//! Git inspection and explicit worktree-aware active-plan binding services.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::application::plan::PlanService;
use crate::domain::{PlanId, PlanStatus, Timestamp};
use crate::git::{
    ActiveBindingResolution, ActiveBindingStatus, ActiveBindingStore, ActivePlanBinding,
    GitAdapter, GitError, GitErrorKind, GitFacts,
};
use crate::{ErrorCategory, MinoError};

/// Complete read-only Git and active-binding inspection result.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GitInspectionReport {
    /// Current repository and worktree facts.
    pub facts: GitFacts,
    /// Current active-binding resolution.
    pub active_binding: ActiveBindingResolution,
    /// Explicitly requested plan when supplied.
    pub requested_plan: Option<PlanId>,
    /// Current revision of the explicitly requested plan.
    pub requested_plan_revision: Option<u64>,
    /// Whether the requested plan is the current worktree binding.
    pub is_requested_plan_bound: bool,
}

/// Result of binding one plan to the current worktree identity.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GitBindReport {
    /// Current repository and worktree facts.
    pub facts: GitFacts,
    /// Persisted binding.
    pub binding: ActivePlanBinding,
    /// Whether an identical binding already existed.
    pub replayed: bool,
}

/// Application boundary for read-only Git facts and active-plan bindings.
#[derive(Clone, Debug)]
pub struct GitBindingService {
    root: PathBuf,
}

impl GitBindingService {
    /// Discovers an initialized project and creates its Git binding service.
    ///
    /// # Errors
    ///
    /// Returns an environment-unavailable error when project discovery fails.
    pub fn discover(start: &Path) -> Result<Self, MinoError> {
        let project = crate::project::discover(start)?;
        Ok(Self {
            root: project.path().to_path_buf(),
        })
    }

    /// Returns current Git facts and active-binding status without mutation.
    ///
    /// # Errors
    ///
    /// Returns a typed error for unavailable Git, malformed binding state, or
    /// a requested plan that cannot be loaded and projection-verified.
    pub fn inspect(
        &self,
        requested_plan: Option<PlanId>,
    ) -> Result<GitInspectionReport, MinoError> {
        let facts = GitAdapter::new(&self.root)
            .inspect()
            .map_err(|error| map_git_error(&error))?;
        let active_binding = ActiveBindingStore::new(&self.root)
            .resolve(&facts)
            .map_err(|error| map_git_error(&error))?;
        let requested = requested_plan
            .as_ref()
            .map(|plan_id| {
                PlanService::discover(&self.root)?
                    .load_verified(plan_id)
                    .map(|plan| plan.revision())
            })
            .transpose()?;
        let is_requested_plan_bound = requested_plan.as_ref().is_some_and(|plan_id| {
            active_binding.status == ActiveBindingStatus::Current
                && active_binding
                    .binding
                    .as_ref()
                    .is_some_and(|binding| &binding.plan_id == plan_id)
        });
        Ok(GitInspectionReport {
            facts,
            active_binding,
            requested_plan,
            requested_plan_revision: requested,
            is_requested_plan_bound,
        })
    }

    /// Binds one current non-Done plan to the exact current worktree identity.
    ///
    /// # Errors
    ///
    /// Returns a typed error for a missing/drifted/Done plan, non-worktree Git
    /// state, malformed binding state, lock timeout, or publication failure.
    pub fn bind_current(&self, plan_id: PlanId) -> Result<GitBindReport, MinoError> {
        let plan = PlanService::discover(&self.root)?.load_verified(&plan_id)?;
        if plan.status() == PlanStatus::Done {
            return Err(MinoError::new(
                ErrorCategory::PolicyViolation,
                format!("Done plan {plan_id} cannot become active"),
            ));
        }
        let facts = GitAdapter::new(&self.root)
            .inspect()
            .map_err(|error| map_git_error(&error))?;
        let report = ActiveBindingStore::new(&self.root)
            .bind(&facts, plan_id, plan.revision(), Timestamp::now_utc())
            .map_err(|error| map_git_error(&error))?;
        Ok(GitBindReport {
            facts,
            binding: report.binding,
            replayed: report.replayed,
        })
    }
}

pub(crate) fn map_git_error(error: &GitError) -> MinoError {
    let (category, message) = match error.kind() {
        GitErrorKind::InvalidOutput => (
            ErrorCategory::DriftDetected,
            "Git returned invalid machine-readable state",
        ),
        GitErrorKind::PolicyViolation => (ErrorCategory::PolicyViolation, error.message()),
        GitErrorKind::Unavailable => (
            ErrorCategory::EnvironmentUnavailable,
            "Git inspection or operation is unavailable",
        ),
    };
    MinoError::new(category, message)
}
