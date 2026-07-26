//! Approval-bound advisory hook installation and read-only hook runtime service.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::application::git_binding::map_git_error;
use crate::git::{
    GIT_HOOK_PROPOSAL_KIND, GitHookName, GitHookProposal, GitHookRuntimeReport,
    GitHookStatusReport, hook_proposal, hook_status, install_hooks, observe_hook,
};
use crate::{ErrorCategory, MinoError, NextAction};

/// Stable schema identifier for successful managed-hook installation reports.
pub const GIT_HOOK_INSTALL_KIND: &str = "mino.git-hook-install/v1";

/// Result of installing, repairing, or replaying the two owned advisory hooks.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GitHookInstallReport {
    /// Stable result discriminator.
    pub hook_install_kind: &'static str,
    /// Exact proposal digest covered by explicit approval.
    pub proposal_hash: String,
    /// Auditable external approval reference supplied by the caller.
    pub approval_reference: String,
    /// Whether any managed hook bytes changed.
    pub changed: bool,
    /// Verified current status after installation.
    pub status: GitHookStatusReport,
}

/// Application boundary for optional repository advisory hooks.
#[derive(Clone, Debug)]
pub struct GitHookService {
    root: PathBuf,
}

impl GitHookService {
    /// Discovers an initialized project and creates its hook service.
    ///
    /// # Errors
    ///
    /// Returns an environment error when no initialized project is discoverable.
    pub fn discover(start: &Path) -> Result<Self, MinoError> {
        let project = crate::project::discover(start)?;
        Ok(Self {
            root: project.path().to_path_buf(),
        })
    }

    /// Returns current default-hook ownership and content status without mutation.
    ///
    /// # Errors
    ///
    /// Returns a typed error for unavailable Git, non-worktree repositories,
    /// malformed hook paths, or unreadable repository configuration.
    pub fn status(&self) -> Result<GitHookStatusReport, MinoError> {
        hook_status(&self.root).map_err(|error| map_git_error(&error))
    }

    /// Returns the hash-bound proposal that must be explicitly approved.
    ///
    /// # Errors
    ///
    /// Returns the same read-only inspection failures as [`Self::status`].
    pub fn propose(&self) -> Result<GitHookProposal, MinoError> {
        hook_proposal(&self.root).map_err(|error| map_git_error(&error))
    }

    /// Installs only absent or Mino-owned default hooks after explicit approval.
    ///
    /// # Errors
    ///
    /// Returns an approval error for a missing reference, a drift error for a
    /// stale proposal, or a policy error without writes for any user-owned,
    /// custom, symbolic-link, or otherwise unsupported hook path.
    pub fn install(
        &self,
        proposal_hash: &str,
        approval_reference: &str,
    ) -> Result<GitHookInstallReport, MinoError> {
        if approval_reference.trim().is_empty() {
            return Err(MinoError::new(
                ErrorCategory::ApprovalRequired,
                "Advisory hook installation requires an explicit approval reference",
            )
            .with_remediation(vec!["approval_ref".to_owned()], vec![hook_propose_action()]));
        }
        let proposal = self.propose()?;
        if proposal_hash != proposal.proposal_hash {
            return Err(MinoError::new(
                ErrorCategory::DriftDetected,
                "Hook ownership or template state changed after the supplied proposal",
            )
            .with_remediation(
                vec!["proposal_hash".to_owned()],
                vec![hook_propose_action()],
            ));
        }
        if !proposal.status.installable {
            let details = serde_json::to_value(&proposal).unwrap_or_else(|_| {
                serde_json::json!({
                    "hook_proposal_kind": GIT_HOOK_PROPOSAL_KIND,
                    "proposal_hash": proposal.proposal_hash
                })
            });
            return Err(MinoError::new(
                ErrorCategory::PolicyViolation,
                "Existing repository hook ownership requires manual integration",
            )
            .with_details(details)
            .with_remediation(Vec::new(), vec![hook_status_action()]));
        }
        let (changed, status) = install_hooks(&self.root).map_err(|error| map_git_error(&error))?;
        Ok(GitHookInstallReport {
            hook_install_kind: GIT_HOOK_INSTALL_KIND,
            proposal_hash: proposal.proposal_hash,
            approval_reference: approval_reference.to_owned(),
            changed,
            status,
        })
    }

    /// Observes current staged/commit and active-binding facts without mutation.
    ///
    /// # Errors
    ///
    /// Returns a typed error for unavailable Git or malformed read-only binding state.
    pub fn run(&self, hook: GitHookName) -> Result<GitHookRuntimeReport, MinoError> {
        observe_hook(&self.root, hook).map_err(|error| map_git_error(&error))
    }
}

fn hook_propose_action() -> NextAction {
    NextAction {
        id: "git.hook.propose".to_owned(),
        argv: vec![
            "mino".to_owned(),
            "git".to_owned(),
            "hook".to_owned(),
            "propose".to_owned(),
            "--format".to_owned(),
            "json".to_owned(),
            "--no-input".to_owned(),
        ],
    }
}

fn hook_status_action() -> NextAction {
    NextAction {
        id: "git.hook.status".to_owned(),
        argv: vec![
            "mino".to_owned(),
            "git".to_owned(),
            "hook".to_owned(),
            "status".to_owned(),
            "--format".to_owned(),
            "json".to_owned(),
            "--no-input".to_owned(),
        ],
    }
}
