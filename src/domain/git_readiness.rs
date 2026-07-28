//! Typed Git readiness and pre-plan decision state stored in plan extensions.

use std::path::{Component, Path};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{DomainError, DomainErrorKind, Timestamp};

pub(crate) const GIT_READINESS_EXTENSION_KEY: &str = "git_readiness_state";
const GIT_READINESS_FORMAT_VERSION: u32 = 1;
const CONVENTIONAL_COMMIT_TYPES: &[&str] = &[
    "build", "chore", "ci", "docs", "feat", "fix", "perf", "refactor", "revert", "style", "test",
];

/// Repository mode represented by one live readiness observation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GitRepositoryMode {
    /// Git explicitly confirmed that the project is not a repository.
    NotRepository,
    /// Git reported a normal repository worktree.
    Worktree,
    /// Git reported a bare repository without a worktree.
    Bare,
}

/// Exact repository identity and status facts captured at one instant.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GitReadinessObservation {
    repository_mode: GitRepositoryMode,
    worktree: Option<String>,
    common_dir: Option<String>,
    branch: Option<String>,
    head: Option<String>,
    status_digest: String,
    is_clean: bool,
    observed_at: Timestamp,
}

impl GitReadinessObservation {
    /// Creates and validates one normalized live observation.
    ///
    /// # Errors
    ///
    /// Returns an invariant error when repository identity, object IDs, paths,
    /// status digest, or mode-specific fields are inconsistent.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        repository_mode: GitRepositoryMode,
        worktree: Option<String>,
        common_dir: Option<String>,
        branch: Option<String>,
        head: Option<String>,
        mut status_digest: String,
        is_clean: bool,
        observed_at: Timestamp,
    ) -> Result<Self, DomainError> {
        status_digest.make_ascii_lowercase();
        let observation = Self {
            repository_mode,
            worktree,
            common_dir,
            branch,
            head: head.map(|value| value.to_ascii_lowercase()),
            status_digest,
            is_clean,
            observed_at,
        };
        observation.validate()?;
        Ok(observation)
    }

    /// Returns the observed repository mode.
    #[must_use]
    pub const fn repository_mode(&self) -> GitRepositoryMode {
        self.repository_mode
    }

    /// Returns the canonical worktree identity when one exists.
    #[must_use]
    pub fn worktree(&self) -> Option<&str> {
        self.worktree.as_deref()
    }

    /// Returns the canonical common-directory identity when one exists.
    #[must_use]
    pub fn common_dir(&self) -> Option<&str> {
        self.common_dir.as_deref()
    }

    /// Returns the current branch when HEAD is not detached.
    #[must_use]
    pub fn branch(&self) -> Option<&str> {
        self.branch.as_deref()
    }

    /// Returns the full current commit object ID when HEAD exists.
    #[must_use]
    pub fn head(&self) -> Option<&str> {
        self.head.as_deref()
    }

    /// Returns the digest of sorted porcelain status entries.
    #[must_use]
    pub fn status_digest(&self) -> &str {
        &self.status_digest
    }

    /// Returns whether the observed worktree had no status entries.
    #[must_use]
    pub const fn is_clean(&self) -> bool {
        self.is_clean
    }

    /// Returns when the live facts were captured.
    #[must_use]
    pub const fn observed_at(&self) -> &Timestamp {
        &self.observed_at
    }

    pub(crate) fn validate(&self) -> Result<(), DomainError> {
        if !is_sha256(&self.status_digest)
            || self.worktree.as_deref().is_some_and(invalid_text)
            || self.common_dir.as_deref().is_some_and(invalid_text)
            || self.branch.as_deref().is_some_and(invalid_text)
            || self
                .head
                .as_deref()
                .is_some_and(|head| !is_full_object_id(head))
        {
            return Err(invariant("Git readiness observation is malformed"));
        }
        let valid_mode = match self.repository_mode {
            GitRepositoryMode::NotRepository => {
                self.worktree.is_none()
                    && self.common_dir.is_none()
                    && self.branch.is_none()
                    && self.head.is_none()
                    && !self.is_clean
            }
            GitRepositoryMode::Worktree => {
                self.worktree.is_some()
                    && self.common_dir.is_some()
                    && (self.head.is_some() || self.branch.is_some())
            }
            GitRepositoryMode::Bare => {
                self.worktree.is_none()
                    && self.common_dir.is_some()
                    && self.branch.is_none()
                    && self.head.is_none()
                    && !self.is_clean
            }
        };
        if valid_mode {
            Ok(())
        } else {
            Err(invariant(
                "Git readiness observation fields do not match the repository mode",
            ))
        }
    }
}

/// Explicit setup choice required when no Git repository exists.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GitSetupDecision {
    /// A repository already exists and no setup choice is required.
    #[default]
    NotRequired,
    /// The user has not selected a setup outcome.
    Pending,
    /// The user approved external Git initialization.
    InitializeApproved,
    /// The user chose to continue with plan-level Git Flow disabled.
    ContinueWithoutGit,
    /// Planning remains blocked until Git is prepared outside Mino.
    BlockedUntilManualSetup,
}

/// Auditable Git setup decision and its optional explicit decision metadata.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GitSetupState {
    decision: GitSetupDecision,
    actor: Option<String>,
    reference: Option<String>,
    decided_at: Option<Timestamp>,
}

impl GitSetupState {
    fn initial(mode: GitRepositoryMode) -> Self {
        Self {
            decision: if mode == GitRepositoryMode::NotRepository {
                GitSetupDecision::Pending
            } else {
                GitSetupDecision::NotRequired
            },
            ..Self::default()
        }
    }

    /// Returns the current setup choice.
    #[must_use]
    pub const fn decision(&self) -> GitSetupDecision {
        self.decision
    }

    /// Returns the actor who recorded an explicit setup choice.
    #[must_use]
    pub fn actor(&self) -> Option<&str> {
        self.actor.as_deref()
    }

    /// Returns the auditable reference for an explicit setup choice.
    #[must_use]
    pub fn reference(&self) -> Option<&str> {
        self.reference.as_deref()
    }

    /// Returns when an explicit setup choice was recorded.
    #[must_use]
    pub const fn decided_at(&self) -> Option<&Timestamp> {
        self.decided_at.as_ref()
    }

    fn decide(
        &mut self,
        decision: GitSetupDecision,
        actor: String,
        reference: String,
        decided_at: Timestamp,
    ) -> Result<(), DomainError> {
        if self.decision == GitSetupDecision::NotRequired
            || matches!(
                decision,
                GitSetupDecision::NotRequired | GitSetupDecision::Pending
            )
            || actor.trim().is_empty()
            || reference.trim().is_empty()
        {
            return Err(invariant(
                "Git setup requires an explicit terminal choice, actor, and reference",
            ));
        }
        self.decision = decision;
        self.actor = Some(actor);
        self.reference = Some(reference);
        self.decided_at = Some(decided_at);
        self.validate()
    }

    fn is_satisfied(&self, mode: GitRepositoryMode) -> bool {
        match self.decision {
            GitSetupDecision::NotRequired => mode != GitRepositoryMode::NotRepository,
            GitSetupDecision::Pending => false,
            GitSetupDecision::InitializeApproved | GitSetupDecision::BlockedUntilManualSetup => {
                mode != GitRepositoryMode::NotRepository
            }
            GitSetupDecision::ContinueWithoutGit => true,
        }
    }

    fn allows_git_flow(&self, mode: GitRepositoryMode) -> bool {
        self.decision != GitSetupDecision::ContinueWithoutGit && self.is_satisfied(mode)
    }

    fn validate(&self) -> Result<(), DomainError> {
        let has_metadata = self
            .actor
            .as_deref()
            .is_some_and(|value| !invalid_text(value))
            && self
                .reference
                .as_deref()
                .is_some_and(|value| !invalid_text(value))
            && self.decided_at.is_some();
        let valid = match self.decision {
            GitSetupDecision::NotRequired | GitSetupDecision::Pending => {
                self.actor.is_none() && self.reference.is_none() && self.decided_at.is_none()
            }
            GitSetupDecision::InitializeApproved
            | GitSetupDecision::ContinueWithoutGit
            | GitSetupDecision::BlockedUntilManualSetup => has_metadata,
        };
        if valid {
            Ok(())
        } else {
            Err(invariant("Git setup decision metadata is inconsistent"))
        }
    }
}

/// Aggregate pre-plan cleanup lifecycle.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PrePlanCleanupDecision {
    /// No dirty worktree was observed.
    #[default]
    NotRequired,
    /// A dirty worktree requires an explicit proposal or decline decision.
    Pending,
    /// Every proposed cleanup item has explicit consent.
    Approved,
    /// Cleanup was explicitly declined and Git Flow remains disabled.
    Declined,
    /// Every approved item was recorded and a clean refresh completed.
    Completed,
}

/// Per-item approval status for a proposed cleanup commit.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CleanupConsentStatus {
    /// The item has not received explicit consent.
    #[default]
    Pending,
    /// The exact files and commit message were explicitly approved.
    Approved,
}

/// One ordered, single-responsibility pre-plan cleanup commit.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PrePlanCleanupItem {
    id: String,
    logical_change: String,
    files: Vec<String>,
    planned_commit_message: String,
    consent_status: CleanupConsentStatus,
    approval_actor: Option<String>,
    approval_reference: Option<String>,
    approved_at: Option<Timestamp>,
    actual_commit: Option<String>,
    recorded_at: Option<Timestamp>,
}

impl PrePlanCleanupItem {
    /// Creates one pending cleanup item with a stable ordered identifier.
    ///
    /// # Errors
    ///
    /// Returns an invariant error for malformed identifiers, paths, logical
    /// descriptions, or Conventional Commit messages.
    pub fn new(
        id: impl Into<String>,
        logical_change: impl Into<String>,
        files: Vec<String>,
        planned_commit_message: impl Into<String>,
    ) -> Result<Self, DomainError> {
        let item = Self {
            id: id.into(),
            logical_change: logical_change.into(),
            files: normalized_paths(files)?,
            planned_commit_message: planned_commit_message.into(),
            consent_status: CleanupConsentStatus::Pending,
            approval_actor: None,
            approval_reference: None,
            approved_at: None,
            actual_commit: None,
            recorded_at: None,
        };
        item.validate()?;
        Ok(item)
    }

    /// Returns the stable ordered cleanup item identifier.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns the single logical change represented by this item.
    #[must_use]
    pub fn logical_change(&self) -> &str {
        &self.logical_change
    }

    /// Returns the sorted exact paths assigned to this item.
    #[must_use]
    pub fn files(&self) -> &[String] {
        &self.files
    }

    /// Returns the exact planned Conventional Commit message.
    #[must_use]
    pub fn planned_commit_message(&self) -> &str {
        &self.planned_commit_message
    }

    /// Returns whether the exact item has explicit consent.
    #[must_use]
    pub const fn consent_status(&self) -> CleanupConsentStatus {
        self.consent_status
    }

    /// Returns the auditable approval reference when approved.
    #[must_use]
    pub fn approval_reference(&self) -> Option<&str> {
        self.approval_reference.as_deref()
    }

    /// Returns the verified cleanup commit object ID when recorded.
    #[must_use]
    pub fn actual_commit(&self) -> Option<&str> {
        self.actual_commit.as_deref()
    }

    fn approve(
        &mut self,
        actor: String,
        reference: String,
        approved_at: Timestamp,
    ) -> Result<(), DomainError> {
        if self.consent_status != CleanupConsentStatus::Pending
            || actor.trim().is_empty()
            || reference.trim().is_empty()
        {
            return Err(invariant(
                "Cleanup item approval requires a pending item, actor, and reference",
            ));
        }
        self.consent_status = CleanupConsentStatus::Approved;
        self.approval_actor = Some(actor);
        self.approval_reference = Some(reference);
        self.approved_at = Some(approved_at);
        self.validate()
    }

    fn record(&mut self, commit: &str, recorded_at: Timestamp) -> Result<(), DomainError> {
        if self.consent_status != CleanupConsentStatus::Approved
            || self.actual_commit.is_some()
            || !is_full_object_id(commit)
        {
            return Err(invariant(
                "Cleanup commit recording requires one approved unrecorded item and full object ID",
            ));
        }
        self.actual_commit = Some(commit.to_ascii_lowercase());
        self.recorded_at = Some(recorded_at);
        self.validate()
    }

    fn validate(&self) -> Result<(), DomainError> {
        if !is_cleanup_item_id(&self.id)
            || invalid_text(&self.logical_change)
            || self.files.is_empty()
            || self.files.iter().any(|path| !is_repository_path(path))
            || self.files.windows(2).any(|pair| pair[0] >= pair[1])
            || !is_conventional_commit(&self.planned_commit_message)
        {
            return Err(invariant("Pre-plan cleanup item is malformed"));
        }
        let approval_metadata = self
            .approval_actor
            .as_deref()
            .is_some_and(|value| !invalid_text(value))
            && self
                .approval_reference
                .as_deref()
                .is_some_and(|value| !invalid_text(value))
            && self.approved_at.is_some();
        let consent_valid = match self.consent_status {
            CleanupConsentStatus::Pending => {
                self.approval_actor.is_none()
                    && self.approval_reference.is_none()
                    && self.approved_at.is_none()
                    && self.actual_commit.is_none()
                    && self.recorded_at.is_none()
            }
            CleanupConsentStatus::Approved => {
                approval_metadata
                    && (self.actual_commit.is_none() && self.recorded_at.is_none()
                        || self.actual_commit.as_deref().is_some_and(is_full_object_id)
                            && self.recorded_at.is_some())
            }
        };
        if consent_valid {
            Ok(())
        } else {
            Err(invariant(
                "Pre-plan cleanup item consent or commit metadata is inconsistent",
            ))
        }
    }
}

/// Auditable pre-plan cleanup decision, live dirty paths, and ordered items.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PrePlanCleanupState {
    decision: PrePlanCleanupDecision,
    observed_paths: Vec<String>,
    blockers: Vec<String>,
    items: Vec<PrePlanCleanupItem>,
    decision_actor: Option<String>,
    decision_reference: Option<String>,
    decided_at: Option<Timestamp>,
}

impl PrePlanCleanupState {
    fn initial(observed_paths: Vec<String>, blockers: Vec<String>) -> Result<Self, DomainError> {
        let observed_paths = normalized_paths(observed_paths)?;
        let mut blockers = blockers;
        blockers.sort();
        if blockers.iter().any(|blocker| invalid_text(blocker))
            || blockers.windows(2).any(|pair| pair[0] == pair[1])
        {
            return Err(invariant("Pre-plan cleanup blockers are malformed"));
        }
        let state = Self {
            decision: if observed_paths.is_empty() && blockers.is_empty() {
                PrePlanCleanupDecision::NotRequired
            } else {
                PrePlanCleanupDecision::Pending
            },
            observed_paths,
            blockers,
            ..Self::default()
        };
        state.validate()?;
        Ok(state)
    }

    /// Returns the aggregate cleanup decision.
    #[must_use]
    pub const fn decision(&self) -> PrePlanCleanupDecision {
        self.decision
    }

    /// Returns the sorted dirty paths bound to the current decision state.
    #[must_use]
    pub fn observed_paths(&self) -> &[String] {
        &self.observed_paths
    }

    /// Returns deterministic blockers that prevent safe cleanup separation.
    #[must_use]
    pub fn blockers(&self) -> &[String] {
        &self.blockers
    }

    /// Returns ordered cleanup items.
    #[must_use]
    pub fn items(&self) -> &[PrePlanCleanupItem] {
        &self.items
    }

    /// Returns the auditable aggregate decline reference when one exists.
    #[must_use]
    pub fn decision_reference(&self) -> Option<&str> {
        self.decision_reference.as_deref()
    }

    fn propose(&mut self, items: Vec<PrePlanCleanupItem>) -> Result<(), DomainError> {
        if self.decision != PrePlanCleanupDecision::Pending
            || self.observed_paths.is_empty()
            || !self.blockers.is_empty()
            || items.is_empty()
            || self
                .items
                .iter()
                .any(|item| item.consent_status == CleanupConsentStatus::Approved)
        {
            return Err(invariant(
                "Cleanup proposal requires a safe pending dirty-path observation",
            ));
        }
        let proposed_paths = items
            .iter()
            .flat_map(|item| item.files.iter().cloned())
            .collect::<Vec<_>>();
        let proposed_paths = normalized_paths(proposed_paths)?;
        if proposed_paths != self.observed_paths {
            return Err(invariant(
                "Cleanup proposal items must cover every observed dirty path exactly once",
            ));
        }
        self.items = items;
        self.validate()
    }

    fn approve_item(
        &mut self,
        item_id: &str,
        actor: String,
        reference: String,
        approved_at: Timestamp,
    ) -> Result<(), DomainError> {
        if self.decision != PrePlanCleanupDecision::Pending
            || self.items.is_empty()
            || !self.blockers.is_empty()
        {
            return Err(invariant(
                "Cleanup item approval requires a complete pending proposal",
            ));
        }
        let item = self
            .items
            .iter_mut()
            .find(|item| item.id == item_id)
            .ok_or_else(|| invariant(format!("Cleanup item {item_id} does not exist")))?;
        item.approve(actor, reference, approved_at)?;
        if self
            .items
            .iter()
            .all(|item| item.consent_status == CleanupConsentStatus::Approved)
        {
            self.decision = PrePlanCleanupDecision::Approved;
        }
        self.validate()
    }

    fn decline(
        &mut self,
        actor: String,
        reference: String,
        decided_at: Timestamp,
    ) -> Result<(), DomainError> {
        if self.decision != PrePlanCleanupDecision::Pending
            || actor.trim().is_empty()
            || reference.trim().is_empty()
            || self
                .items
                .iter()
                .any(|item| item.consent_status == CleanupConsentStatus::Approved)
        {
            return Err(invariant(
                "Cleanup decline requires a pending unapproved decision, actor, and reference",
            ));
        }
        self.decision = PrePlanCleanupDecision::Declined;
        self.decision_actor = Some(actor);
        self.decision_reference = Some(reference);
        self.decided_at = Some(decided_at);
        self.validate()
    }

    fn record_commit(
        &mut self,
        item_id: &str,
        commit: &str,
        recorded_at: Timestamp,
    ) -> Result<(), DomainError> {
        if self.decision != PrePlanCleanupDecision::Approved {
            return Err(invariant(
                "Cleanup commit recording requires an approved proposal",
            ));
        }
        let next_index = self
            .items
            .iter()
            .position(|item| item.actual_commit.is_none())
            .ok_or_else(|| invariant("Every approved cleanup item is already recorded"))?;
        if self.items[next_index].id != item_id {
            return Err(invariant(format!(
                "Cleanup commits must be recorded in order; {} is next",
                self.items[next_index].id
            )));
        }
        self.items[next_index].record(commit, recorded_at)?;
        self.validate()
    }

    fn refreshed(
        &self,
        observed_paths: Vec<String>,
        blockers: Vec<String>,
    ) -> Result<Self, DomainError> {
        let live = Self::initial(observed_paths, blockers)?;
        if live.observed_paths.is_empty() {
            return self.refreshed_clean(live);
        }
        if matches!(
            self.decision,
            PrePlanCleanupDecision::NotRequired | PrePlanCleanupDecision::Completed
        ) {
            return Ok(live);
        }
        let mut next = self.clone();
        if matches!(
            next.decision,
            PrePlanCleanupDecision::Pending | PrePlanCleanupDecision::Approved
        ) && next.observed_paths != live.observed_paths
            && next.items.iter().all(|item| {
                item.consent_status == CleanupConsentStatus::Pending && item.actual_commit.is_none()
            })
        {
            next.decision = PrePlanCleanupDecision::Pending;
            next.items.clear();
        }
        next.observed_paths = live.observed_paths;
        next.blockers = live.blockers;
        next.validate()?;
        Ok(next)
    }

    fn refreshed_clean(&self, mut live: Self) -> Result<Self, DomainError> {
        match self.decision {
            PrePlanCleanupDecision::Approved
                if !self.items.is_empty()
                    && self.items.iter().all(|item| item.actual_commit.is_some()) =>
            {
                live.decision = PrePlanCleanupDecision::Completed;
                live.items.clone_from(&self.items);
            }
            PrePlanCleanupDecision::Pending
                if self.items.iter().all(|item| {
                    item.consent_status == CleanupConsentStatus::Pending
                        && item.actual_commit.is_none()
                }) => {}
            PrePlanCleanupDecision::NotRequired => {}
            _ => {
                live = self.clone();
                live.observed_paths.clear();
                live.blockers.clear();
            }
        }
        live.validate()?;
        Ok(live)
    }

    fn is_satisfied(&self) -> bool {
        matches!(
            self.decision,
            PrePlanCleanupDecision::NotRequired
                | PrePlanCleanupDecision::Declined
                | PrePlanCleanupDecision::Completed
        )
    }

    fn allows_git_flow(&self) -> bool {
        matches!(
            self.decision,
            PrePlanCleanupDecision::NotRequired | PrePlanCleanupDecision::Completed
        )
    }

    fn validate(&self) -> Result<(), DomainError> {
        if self
            .observed_paths
            .iter()
            .any(|path| !is_repository_path(path))
            || self
                .observed_paths
                .windows(2)
                .any(|pair| pair[0] >= pair[1])
            || self.blockers.iter().any(|blocker| invalid_text(blocker))
            || self.blockers.windows(2).any(|pair| pair[0] >= pair[1])
        {
            return Err(invariant("Pre-plan cleanup live facts are malformed"));
        }
        for (index, item) in self.items.iter().enumerate() {
            item.validate()?;
            if item.id != format!("C{}", index + 1) {
                return Err(invariant(
                    "Cleanup item identifiers must follow stored order",
                ));
            }
        }
        let aggregate = self
            .items
            .iter()
            .flat_map(|item| item.files.iter())
            .collect::<Vec<_>>();
        if aggregate
            .iter()
            .enumerate()
            .any(|(index, path)| aggregate.iter().skip(index + 1).any(|other| path == other))
        {
            return Err(invariant("Cleanup item file scopes overlap"));
        }
        self.validate_decision()
    }

    fn validate_decision(&self) -> Result<(), DomainError> {
        let decline_metadata = self
            .decision_actor
            .as_deref()
            .is_some_and(|value| !invalid_text(value))
            && self
                .decision_reference
                .as_deref()
                .is_some_and(|value| !invalid_text(value))
            && self.decided_at.is_some();
        let valid = match self.decision {
            PrePlanCleanupDecision::NotRequired => {
                self.observed_paths.is_empty()
                    && self.items.is_empty()
                    && !decline_metadata
                    && self.decision_actor.is_none()
                    && self.decision_reference.is_none()
                    && self.decided_at.is_none()
            }
            PrePlanCleanupDecision::Pending => {
                self.items.iter().all(|item| item.actual_commit.is_none())
                    && (self.items.is_empty()
                        || self
                            .items
                            .iter()
                            .any(|item| item.consent_status == CleanupConsentStatus::Pending))
                    && self.decision_actor.is_none()
                    && self.decision_reference.is_none()
                    && self.decided_at.is_none()
            }
            PrePlanCleanupDecision::Approved => {
                !self.items.is_empty()
                    && self
                        .items
                        .iter()
                        .all(|item| item.consent_status == CleanupConsentStatus::Approved)
                    && self.decision_actor.is_none()
                    && self.decision_reference.is_none()
                    && self.decided_at.is_none()
            }
            PrePlanCleanupDecision::Declined => decline_metadata,
            PrePlanCleanupDecision::Completed => {
                self.observed_paths.is_empty()
                    && !self.items.is_empty()
                    && self.items.iter().all(|item| {
                        item.consent_status == CleanupConsentStatus::Approved
                            && item.actual_commit.is_some()
                    })
                    && self.decision_actor.is_none()
                    && self.decision_reference.is_none()
                    && self.decided_at.is_none()
            }
        };
        if valid {
            Ok(())
        } else {
            Err(invariant("Pre-plan cleanup decision state is inconsistent"))
        }
    }
}

/// Versioned typed Git readiness extension stored on one plan revision.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GitReadinessState {
    format_version: u32,
    observation: GitReadinessObservation,
    #[serde(default)]
    setup: GitSetupState,
    #[serde(default)]
    cleanup: PrePlanCleanupState,
}

impl GitReadinessState {
    /// Creates the current version of the Git readiness extension.
    ///
    /// Dirty observations created without detailed status paths remain safely
    /// blocked until a complete live refresh supplies those paths.
    ///
    /// # Errors
    ///
    /// Returns an invariant error when the observation is malformed.
    pub fn new(observation: GitReadinessObservation) -> Result<Self, DomainError> {
        let blockers = (observation.repository_mode == GitRepositoryMode::Worktree
            && !observation.is_clean)
            .then(|| "status_paths_unavailable".to_owned())
            .into_iter()
            .collect();
        Self::captured(observation, Vec::new(), blockers)
    }

    pub(crate) fn captured(
        observation: GitReadinessObservation,
        observed_paths: Vec<String>,
        blockers: Vec<String>,
    ) -> Result<Self, DomainError> {
        observation.validate()?;
        let state = Self {
            format_version: GIT_READINESS_FORMAT_VERSION,
            setup: GitSetupState::initial(observation.repository_mode),
            cleanup: PrePlanCleanupState::initial(observed_paths, blockers)?,
            observation,
        };
        state.validate()?;
        Ok(state)
    }

    /// Returns the typed extension format version.
    #[must_use]
    pub const fn format_version(&self) -> u32 {
        self.format_version
    }

    /// Returns the current persisted observation.
    #[must_use]
    pub const fn observation(&self) -> &GitReadinessObservation {
        &self.observation
    }

    /// Returns the current Git setup decision state.
    #[must_use]
    pub const fn setup(&self) -> &GitSetupState {
        &self.setup
    }

    /// Returns the current pre-plan cleanup state.
    #[must_use]
    pub const fn cleanup(&self) -> &PrePlanCleanupState {
        &self.cleanup
    }

    /// Records one explicit setup decision in this aggregate.
    ///
    /// Persistence callers must still commit the enclosing plan revision.
    ///
    /// # Errors
    ///
    /// Returns an invariant error for a non-terminal choice, missing audit
    /// metadata, or a setup decision that was not required.
    pub fn decide_setup(
        &mut self,
        decision: GitSetupDecision,
        actor: String,
        reference: String,
        decided_at: Timestamp,
    ) -> Result<(), DomainError> {
        self.setup.decide(decision, actor, reference, decided_at)?;
        self.validate()
    }

    pub(crate) fn propose_cleanup(
        &mut self,
        items: Vec<PrePlanCleanupItem>,
    ) -> Result<(), DomainError> {
        self.cleanup.propose(items)?;
        self.validate()
    }

    pub(crate) fn approve_cleanup_item(
        &mut self,
        item_id: &str,
        actor: String,
        reference: String,
        approved_at: Timestamp,
    ) -> Result<(), DomainError> {
        self.cleanup
            .approve_item(item_id, actor, reference, approved_at)?;
        self.validate()
    }

    pub(crate) fn decline_cleanup(
        &mut self,
        actor: String,
        reference: String,
        decided_at: Timestamp,
    ) -> Result<(), DomainError> {
        self.cleanup.decline(actor, reference, decided_at)?;
        self.validate()
    }

    pub(crate) fn record_cleanup_commit(
        &mut self,
        item_id: &str,
        commit: &str,
        recorded_at: Timestamp,
    ) -> Result<(), DomainError> {
        self.cleanup.record_commit(item_id, commit, recorded_at)?;
        self.validate()
    }

    pub(crate) fn refreshed_from(&self, captured: &Self) -> Result<Self, DomainError> {
        captured.validate()?;
        let mut refreshed = Self {
            format_version: GIT_READINESS_FORMAT_VERSION,
            observation: captured.observation.clone(),
            setup: self.setup.clone(),
            cleanup: self.cleanup.refreshed(
                captured.cleanup.observed_paths.clone(),
                captured.cleanup.blockers.clone(),
            )?,
        };
        if refreshed.setup.decision == GitSetupDecision::NotRequired
            && refreshed.observation.repository_mode == GitRepositoryMode::NotRepository
        {
            refreshed.setup = GitSetupState::initial(GitRepositoryMode::NotRepository);
        }
        refreshed.validate()?;
        Ok(refreshed)
    }

    pub(crate) fn git_flow_allowed(&self) -> bool {
        self.observation.repository_mode == GitRepositoryMode::Worktree
            && self.observation.is_clean
            && self.observation.branch.is_some()
            && self.observation.head.is_some()
            && self.setup.allows_git_flow(self.observation.repository_mode)
            && self.cleanup.allows_git_flow()
    }

    pub(crate) fn setup_is_satisfied(&self) -> bool {
        self.setup.is_satisfied(self.observation.repository_mode)
    }

    pub(crate) fn cleanup_is_satisfied(&self) -> bool {
        self.cleanup.is_satisfied()
    }

    pub(crate) fn validate(&self) -> Result<(), DomainError> {
        if self.format_version != GIT_READINESS_FORMAT_VERSION {
            return Err(invariant("Git readiness extension version is unsupported"));
        }
        self.observation.validate()?;
        self.setup.validate()?;
        self.cleanup.validate()?;
        if self.observation.repository_mode == GitRepositoryMode::NotRepository
            && self.setup.decision == GitSetupDecision::NotRequired
        {
            return Err(invariant(
                "A missing repository requires an explicit setup decision",
            ));
        }
        if self.observation.repository_mode != GitRepositoryMode::Worktree
            && (!self.cleanup.observed_paths.is_empty()
                || !self.cleanup.blockers.is_empty()
                || self.cleanup.decision != PrePlanCleanupDecision::NotRequired)
        {
            return Err(invariant(
                "Pre-plan cleanup state requires a normal Git worktree",
            ));
        }
        Ok(())
    }

    pub(crate) fn materialize_legacy(&mut self) -> Result<(), DomainError> {
        if self.observation.repository_mode == GitRepositoryMode::NotRepository
            && self.setup == GitSetupState::default()
        {
            self.setup = GitSetupState::initial(GitRepositoryMode::NotRepository);
        }
        if self.observation.repository_mode == GitRepositoryMode::Worktree
            && !self.observation.is_clean
            && self.cleanup == PrePlanCleanupState::default()
        {
            self.cleanup = PrePlanCleanupState::initial(
                Vec::new(),
                vec!["status_paths_unavailable".to_owned()],
            )?;
        }
        self.validate()
    }
}

pub(crate) fn is_conventional_commit(message: &str) -> bool {
    if message.contains(['\r', '\n']) || message.ends_with('.') {
        return false;
    }
    let Some((prefix, description)) = message.split_once(": ") else {
        return false;
    };
    if description.trim().is_empty() {
        return false;
    }
    let (type_, scope) = if let Some((type_, scope)) = prefix.split_once('(') {
        let Some(scope) = scope.strip_suffix(')') else {
            return false;
        };
        (type_, Some(scope))
    } else {
        (prefix, None)
    };
    CONVENTIONAL_COMMIT_TYPES.contains(&type_)
        && scope.is_none_or(|scope| {
            !scope.is_empty()
                && scope.bytes().all(|byte| {
                    byte.is_ascii_lowercase()
                        || byte.is_ascii_digit()
                        || matches!(byte, b'-' | b'_')
                })
        })
}

fn normalized_paths(mut paths: Vec<String>) -> Result<Vec<String>, DomainError> {
    if paths.iter().any(|path| !is_repository_path(path)) {
        return Err(invariant(
            "Cleanup paths must be safe repository-relative paths",
        ));
    }
    paths.sort();
    if paths.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(invariant("Cleanup paths must be unique"));
    }
    Ok(paths)
}

fn is_repository_path(value: &str) -> bool {
    let path = Path::new(value);
    !value.is_empty()
        && !value.contains('\\')
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn is_cleanup_item_id(value: &str) -> bool {
    value.strip_prefix('C').is_some_and(|suffix| {
        !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
    })
}

fn invalid_text(value: &str) -> bool {
    value.trim().is_empty() || value.contains('\0')
}

fn is_sha256(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("sha256:")
        && value["sha256:".len()..]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
}

fn is_full_object_id(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn invariant(message: impl Into<String>) -> DomainError {
    DomainError::new(DomainErrorKind::InvariantViolation, message)
}
