//! Explicit compatibility checks and future-safe migration registry.

use serde::Serialize;

use crate::application::plan::PlanService;
use crate::domain::{PlanId, RequestId};
use crate::{ErrorCategory, MinoError};

use super::{ProtocolError, ProtocolErrorKind, ProtocolRegistry};

/// Stable outcomes for an explicit protocol migration request.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MigrationDisposition {
    /// The plan already uses the requested exact protocol bundle.
    AlreadyCurrent,
}

/// Inputs required to check or execute one explicit migration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProtocolMigrationRequest {
    /// Target plan identifier.
    pub plan_id: PlanId,
    /// Required current optimistic-concurrency revision.
    pub expected_revision: u64,
    /// Idempotency identifier reserved for a supported transform event.
    pub request_id: RequestId,
    /// Requested calendar protocol version.
    pub target_version: String,
}

/// Non-mutating result for an already-current explicit migration request.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProtocolMigrationReport {
    /// Target plan identifier.
    pub plan_id: PlanId,
    /// Unchanged plan revision.
    pub revision: u64,
    /// Stable explicit migration outcome.
    pub disposition: MigrationDisposition,
    /// Verified target calendar version.
    pub protocol_version: String,
    /// Verified target named revision.
    pub protocol_revision: String,
    /// Request identity reserved for future transformed events.
    pub request_id: RequestId,
}

/// Registry-backed explicit migration service.
#[derive(Clone, Debug)]
pub struct ProtocolMigrator {
    plans: PlanService,
}

impl ProtocolMigrator {
    /// Discovers an initialized project and creates its migration service.
    ///
    /// # Errors
    ///
    /// Returns an environment error when no initialized project is discoverable.
    pub fn discover(start: &std::path::Path) -> Result<Self, MinoError> {
        Ok(Self {
            plans: PlanService::discover(start)?,
        })
    }

    /// Executes a supported explicit transform or confirms an exact current plan.
    ///
    /// # Errors
    ///
    /// Returns a revision conflict for stale input and an unsupported-migration
    /// error when no registered transform exists. Unsupported requests do not write.
    pub fn migrate(
        &self,
        request: ProtocolMigrationRequest,
    ) -> Result<ProtocolMigrationReport, MinoError> {
        let bundle = ProtocolRegistry::current().map_err(|error| map_protocol_error(&error))?;
        let plan = self.plans.load_verified(&request.plan_id)?;
        if plan.revision() != request.expected_revision {
            return Err(MinoError::new(
                ErrorCategory::RevisionConflict,
                format!(
                    "Plan {} is revision {}, not expected revision {}",
                    plan.id(),
                    plan.revision(),
                    request.expected_revision
                ),
            ));
        }
        let manifest = bundle.manifest();
        if request.target_version == manifest.protocol_version()
            && plan.protocol_version().version() == manifest.protocol_version()
            && plan.protocol_version().revision() == manifest.protocol_revision()
        {
            return Ok(ProtocolMigrationReport {
                plan_id: plan.id().clone(),
                revision: plan.revision(),
                disposition: MigrationDisposition::AlreadyCurrent,
                protocol_version: manifest.protocol_version().to_owned(),
                protocol_revision: manifest.protocol_revision().to_owned(),
                request_id: request.request_id,
            });
        }
        Err(map_protocol_error(&ProtocolError::new(
            ProtocolErrorKind::UnsupportedMigration,
            format!(
                "No explicit protocol transform is registered from {}/{} to {}",
                plan.protocol_version().version(),
                plan.protocol_version().revision(),
                request.target_version
            ),
        )))
    }
}

fn map_protocol_error(error: &ProtocolError) -> MinoError {
    let category = match error.kind() {
        ProtocolErrorKind::InvalidBundle => ErrorCategory::DriftDetected,
        ProtocolErrorKind::UnsupportedMigration => ErrorCategory::PolicyViolation,
    };
    MinoError::new(category, error.message())
}
