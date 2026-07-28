//! Typed live Git readiness observations stored in the plan extension namespace.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{DomainError, DomainErrorKind, Timestamp};

pub(crate) const GIT_READINESS_EXTENSION_KEY: &str = "git_readiness_state";
const GIT_READINESS_FORMAT_VERSION: u32 = 1;

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

/// Versioned typed Git readiness extension stored on one plan revision.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GitReadinessState {
    format_version: u32,
    observation: GitReadinessObservation,
}

impl GitReadinessState {
    /// Creates the current version of the Git readiness extension.
    ///
    /// # Errors
    ///
    /// Returns an invariant error when the observation is malformed.
    pub fn new(observation: GitReadinessObservation) -> Result<Self, DomainError> {
        observation.validate()?;
        Ok(Self {
            format_version: GIT_READINESS_FORMAT_VERSION,
            observation,
        })
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

    pub(crate) fn validate(&self) -> Result<(), DomainError> {
        if self.format_version != GIT_READINESS_FORMAT_VERSION {
            return Err(invariant("Git readiness extension version is unsupported"));
        }
        self.observation.validate()
    }
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
