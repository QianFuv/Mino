//! Protocol bundle status and explicit migration CLI adapter.

use std::path::Path;

use clap::{Args, Subcommand};
use serde::Serialize;

use crate::commands::CommandResponse;
use crate::domain::{PlanId, RequestId};
use crate::project::{self, ProtocolLock};
use crate::protocol::{
    ProtocolError, ProtocolErrorKind, ProtocolManifest, ProtocolMigrationRequest, ProtocolMigrator,
    ProtocolRegistry,
};
use crate::{ErrorCategory, MinoError};

#[derive(Debug, Subcommand)]
pub(crate) enum ProtocolAction {
    /// Report embedded bundle integrity and project-lock compatibility.
    Status,
    /// Run an explicitly registered plan protocol transform.
    Migrate(MigrateArguments),
}

#[derive(Debug, Args)]
pub(crate) struct MigrateArguments {
    /// Target plan identifier.
    #[arg(long)]
    plan: String,
    /// Required current optimistic-concurrency revision.
    #[arg(long)]
    expect_revision: u64,
    /// Idempotency UUID reserved for the migration event.
    #[arg(long)]
    request_id: String,
    /// Exact target calendar protocol version.
    #[arg(long = "to")]
    target_version: String,
}

#[derive(Serialize)]
struct ProtocolStatusReport {
    manifest: ProtocolManifest,
    project_lock: Option<ProtocolLock>,
    compatible: bool,
    findings: Vec<String>,
}

pub(crate) fn execute(start: &Path, action: ProtocolAction) -> Result<CommandResponse, MinoError> {
    match action {
        ProtocolAction::Status => status(start),
        ProtocolAction::Migrate(arguments) => migrate(start, arguments),
    }
}

fn status(start: &Path) -> Result<CommandResponse, MinoError> {
    let manifest = ProtocolRegistry::current()
        .map_err(|error| map_protocol_error(&error))?
        .manifest()
        .clone();
    let project = project::show(start)?;
    let findings = project.project_lock_findings(&manifest);
    let compatible = findings.is_empty();
    response(
        if compatible {
            "Protocol bundle and project lock are compatible."
        } else {
            "Protocol compatibility findings require attention."
        },
        compatible,
        ProtocolStatusReport {
            manifest,
            project_lock: project.protocol_lock,
            compatible,
            findings: findings.clone(),
        },
        findings,
    )
}

fn migrate(start: &Path, arguments: MigrateArguments) -> Result<CommandResponse, MinoError> {
    let report = ProtocolMigrator::discover(start)?.migrate(ProtocolMigrationRequest {
        plan_id: parse_plan_id(&arguments.plan)?,
        expected_revision: arguments.expect_revision,
        request_id: parse_request_id(&arguments.request_id)?,
        target_version: arguments.target_version,
    })?;
    response(
        "Plan already uses the requested protocol; no migration was written.",
        true,
        report,
        Vec::new(),
    )
}

trait ProjectLockCompatibility {
    fn project_lock_findings(&self, manifest: &ProtocolManifest) -> Vec<String>;
}

impl ProjectLockCompatibility for project::ProjectShowReport {
    fn project_lock_findings(&self, manifest: &ProtocolManifest) -> Vec<String> {
        let Some(lock) = &self.protocol_lock else {
            return vec!["protocol_lock_missing".to_owned()];
        };
        let mut findings = Vec::new();
        if lock.lock_version != ProtocolLock::default().lock_version {
            findings.push("protocol_lock_version_mismatch".to_owned());
        }
        if lock.protocol_version != manifest.protocol_version() {
            findings.push("protocol_version_mismatch".to_owned());
        }
        if lock.protocol_revision != manifest.protocol_revision() {
            findings.push("protocol_revision_mismatch".to_owned());
        }
        if lock.schema_version != manifest.schema_version() {
            findings.push("protocol_schema_mismatch".to_owned());
        }
        if lock.renderer_version != manifest.renderer_version() {
            findings.push("protocol_renderer_mismatch".to_owned());
        }
        findings
    }
}

fn response<T: Serialize>(
    message: &str,
    complete: bool,
    payload: T,
    missing: Vec<String>,
) -> Result<CommandResponse, MinoError> {
    let payload = serde_json::to_value(payload).map_err(|error| {
        MinoError::new(
            ErrorCategory::EnvironmentUnavailable,
            format!("Failed to serialize protocol result: {error}"),
        )
    })?;
    Ok(CommandResponse {
        message: message.to_owned(),
        complete,
        payload,
        missing,
        next_actions: Vec::new(),
    })
}

fn parse_plan_id(value: &str) -> Result<PlanId, MinoError> {
    PlanId::parse(value)
        .map_err(|error| MinoError::new(ErrorCategory::IncompleteOrValidation, error.to_string()))
}

fn parse_request_id(value: &str) -> Result<RequestId, MinoError> {
    RequestId::parse(value)
        .map_err(|error| MinoError::new(ErrorCategory::IncompleteOrValidation, error.to_string()))
}

fn map_protocol_error(error: &ProtocolError) -> MinoError {
    let category = match error.kind() {
        ProtocolErrorKind::InvalidBundle => ErrorCategory::DriftDetected,
        ProtocolErrorKind::UnsupportedMigration => ErrorCategory::PolicyViolation,
    };
    MinoError::new(category, error.message())
}
