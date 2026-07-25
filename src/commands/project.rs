//! Project lifecycle CLI command adapter.

use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};
use serde::Serialize;

use crate::commands::CommandResponse;
use crate::project::{self, DoctorFinding, LegacyDocumentKind, LegacyInput};
use crate::{ErrorCategory, MinoError, NextAction};

#[derive(Clone, Debug, Subcommand)]
pub(crate) enum ProjectAction {
    /// Initialize missing local Mino project state without network or Git mutation.
    Init,
    /// Show parsed project state and doctor findings.
    Show,
    /// Diagnose configuration, locks, transactions, projections, and integrations.
    Doctor,
    /// Scan workspaces and return evidence-based language rankings.
    Scan,
    /// Analyze legacy planning workflow documents without modifying them.
    Migrate(MigrateArguments),
}

#[derive(Clone, Debug, Args)]
pub(crate) struct MigrateArguments {
    #[command(subcommand)]
    action: MigrateAction,
}

#[derive(Clone, Debug, Subcommand)]
enum MigrateAction {
    /// Map legacy AGENTS, plan template, and execution guide content.
    Legacy(LegacyArguments),
}

#[derive(Clone, Debug, Args)]
struct LegacyArguments {
    /// Legacy repository AGENTS document.
    #[arg(long)]
    agents: Option<PathBuf>,
    /// Legacy planning template document.
    #[arg(long)]
    template: Option<PathBuf>,
    /// Legacy plan execution guide document.
    #[arg(long)]
    execution: Option<PathBuf>,
}

pub(crate) fn execute(start: &Path, action: ProjectAction) -> Result<CommandResponse, MinoError> {
    match action {
        ProjectAction::Init => {
            let report = project::initialize(start)?;
            let complete = report.findings.is_empty();
            let missing = finding_codes(&report.findings);
            let next_actions = integration_next_actions(&report.findings);
            let message = format!(
                "Project initialized with {} new and {} existing Mino files.",
                report.created_files.len(),
                report.existing_files.len()
            );
            response(message, complete, report, missing, next_actions)
        }
        ProjectAction::Show => {
            let report = project::show(start)?;
            let complete = report.doctor.is_complete();
            let missing = finding_codes(&report.doctor.findings);
            let next_actions = integration_next_actions(&report.doctor.findings);
            response(
                "Project state loaded.",
                complete,
                report,
                missing,
                next_actions,
            )
        }
        ProjectAction::Doctor => {
            let report = project::doctor(start)?;
            let complete = report.is_complete();
            let message = if report.is_healthy() {
                format!(
                    "Project doctor completed with {} finding(s).",
                    report.findings.len()
                )
            } else {
                format!("Project doctor found {} issue(s).", report.findings.len())
            };
            let missing = finding_codes(&report.findings);
            let next_actions = integration_next_actions(&report.findings);
            response(message, complete, report, missing, next_actions)
        }
        ProjectAction::Scan => response(
            "Project scan completed.",
            true,
            project::scan(start)?,
            Vec::new(),
            Vec::new(),
        ),
        ProjectAction::Migrate(arguments) => match arguments.action {
            MigrateAction::Legacy(arguments) => migrate_legacy(arguments),
        },
    }
}

fn migrate_legacy(arguments: LegacyArguments) -> Result<CommandResponse, MinoError> {
    let mut inputs = Vec::new();
    if let Some(path) = arguments.agents {
        inputs.push(LegacyInput {
            kind: LegacyDocumentKind::Agents,
            path,
        });
    }
    if let Some(path) = arguments.template {
        inputs.push(LegacyInput {
            kind: LegacyDocumentKind::PlanTemplate,
            path,
        });
    }
    if let Some(path) = arguments.execution {
        inputs.push(LegacyInput {
            kind: LegacyDocumentKind::PlanExecution,
            path,
        });
    }
    let report = project::analyze_legacy(&inputs)?;
    let complete = report.findings.is_empty();
    let missing = report
        .findings
        .iter()
        .map(|finding| finding.code.clone())
        .collect();
    response(
        "Legacy planning workflow analyzed without modifying source files.",
        complete,
        report,
        missing,
        Vec::new(),
    )
}

fn response<T: Serialize>(
    message: impl Into<String>,
    complete: bool,
    payload: T,
    missing: Vec<String>,
    next_actions: Vec<NextAction>,
) -> Result<CommandResponse, MinoError> {
    let payload = serde_json::to_value(payload).map_err(|error| {
        MinoError::new(
            ErrorCategory::EnvironmentUnavailable,
            format!("Failed to serialize project result: {error}"),
        )
    })?;
    Ok(CommandResponse {
        message: message.into(),
        complete,
        payload,
        missing,
        next_actions,
    })
}

fn finding_codes(findings: &[DoctorFinding]) -> Vec<String> {
    findings
        .iter()
        .map(|finding| finding.code.clone())
        .collect()
}

fn integration_next_actions(findings: &[DoctorFinding]) -> Vec<NextAction> {
    if findings.iter().any(|finding| {
        matches!(
            finding.code.as_str(),
            "mino_skill_missing" | "agents_file_missing" | "agents_standards_block_missing"
        )
    }) {
        vec![NextAction {
            id: "project.doctor".to_owned(),
            argv: vec![
                "mino".to_owned(),
                "project".to_owned(),
                "doctor".to_owned(),
                "--format".to_owned(),
                "json".to_owned(),
                "--no-input".to_owned(),
            ],
        }]
    } else {
        Vec::new()
    }
}
