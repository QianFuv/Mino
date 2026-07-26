//! Plan-fork provenance and non-destructive archive metadata.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{DomainError, DomainErrorKind, PlanId, Timestamp};

/// Provenance for a plan created from one immutable source revision.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Lineage {
    parent_plan_id: PlanId,
    forked_from_revision: u64,
    fork_reason: String,
    source_state_hash: String,
    forked_at: Timestamp,
}

impl Lineage {
    pub(crate) fn new(
        parent_plan_id: PlanId,
        forked_from_revision: u64,
        fork_reason: String,
        source_state_hash: String,
        forked_at: Timestamp,
    ) -> Result<Self, DomainError> {
        let lineage = Self {
            parent_plan_id,
            forked_from_revision,
            fork_reason,
            source_state_hash,
            forked_at,
        };
        lineage.validate()?;
        Ok(lineage)
    }

    /// Returns the source plan identifier.
    #[must_use]
    pub const fn parent_plan_id(&self) -> &PlanId {
        &self.parent_plan_id
    }

    /// Returns the exact immutable source revision.
    #[must_use]
    pub const fn forked_from_revision(&self) -> u64 {
        self.forked_from_revision
    }

    /// Returns the explicit reason for creating the alternative.
    #[must_use]
    pub fn fork_reason(&self) -> &str {
        &self.fork_reason
    }

    /// Returns the digest of the canonical source snapshot bytes.
    #[must_use]
    pub fn source_state_hash(&self) -> &str {
        &self.source_state_hash
    }

    /// Returns the fork timestamp.
    #[must_use]
    pub const fn forked_at(&self) -> &Timestamp {
        &self.forked_at
    }

    pub(crate) fn validate(&self) -> Result<(), DomainError> {
        if self.forked_from_revision == 0
            || self.fork_reason.trim().is_empty()
            || !is_sha256(&self.source_state_hash)
        {
            return Err(invariant(
                "Plan lineage requires a positive revision, reason, and source digest",
            ));
        }
        Ok(())
    }
}

/// Auditable metadata that deactivates a plan without deleting or restating it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlanArchive {
    reason: String,
    actor: String,
    approval_reference: String,
    archived_at: Timestamp,
}

impl PlanArchive {
    pub(crate) fn new(
        reason: String,
        actor: String,
        approval_reference: String,
        archived_at: Timestamp,
    ) -> Result<Self, DomainError> {
        let archive = Self {
            reason,
            actor,
            approval_reference,
            archived_at,
        };
        archive.validate()?;
        Ok(archive)
    }

    /// Returns the explicit archive reason.
    #[must_use]
    pub fn reason(&self) -> &str {
        &self.reason
    }

    /// Returns the actor that recorded deactivation.
    #[must_use]
    pub fn actor(&self) -> &str {
        &self.actor
    }

    /// Returns the auditable user-selection reference.
    #[must_use]
    pub fn approval_reference(&self) -> &str {
        &self.approval_reference
    }

    /// Returns the archive timestamp.
    #[must_use]
    pub const fn archived_at(&self) -> &Timestamp {
        &self.archived_at
    }

    pub(crate) fn validate(&self) -> Result<(), DomainError> {
        if self.reason.trim().is_empty()
            || self.actor.trim().is_empty()
            || self.approval_reference.trim().is_empty()
        {
            return Err(invariant(
                "Plan archive requires reason, actor, and approval reference",
            ));
        }
        Ok(())
    }
}

fn is_sha256(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

fn invariant(message: impl Into<String>) -> DomainError {
    DomainError::new(DomainErrorKind::InvariantViolation, message)
}
