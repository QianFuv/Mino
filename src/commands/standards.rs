//! Standards detection, recommendation, and application CLI adapter.

use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};
use serde::Serialize;

use crate::application::plan::PlanMutationRequest;
use crate::application::standards::{
    StandardsConflictReport, StandardsConflictService, StandardsPlanService,
};
use crate::commands::CommandResponse;
use crate::domain::{PlanId, RequestId, Timestamp};
use crate::project;
use crate::standards::{
    EmbeddedCatalog, SystemToolProbe, apply_recommendation, detected_languages,
    recommend_for_paths, recommend_initial,
};
use crate::{ErrorCategory, MinoError};

#[derive(Clone, Debug, Subcommand)]
pub(crate) enum StandardsAction {
    /// Detect supported languages from project scanner evidence.
    Detect,
    /// Recommend Common and applicable language packages.
    Recommend {
        /// File-map path used for second-stage complete recommendations.
        #[arg(long = "path")]
        paths: Vec<PathBuf>,
    },
    /// Resolve recommended packages, rules, and verification commands.
    Apply {
        /// Apply the deterministic recommendation rather than manual package IDs.
        #[arg(long)]
        recommended: bool,
        /// Include project-resolved verification checks in the result.
        #[arg(long)]
        seed_verification: bool,
        /// File-map path used for second-stage complete recommendations.
        #[arg(long = "path", conflicts_with = "plan")]
        paths: Vec<PathBuf>,
        /// Plan mutated atomically from its current authored File Map.
        #[arg(long, requires_all = ["expect_revision", "request_id"])]
        plan: Option<String>,
        /// Required optimistic-concurrency revision for plan-scoped application.
        #[arg(long, requires = "plan")]
        expect_revision: Option<u64>,
        /// Idempotency UUID for plan-scoped application.
        #[arg(long, requires = "plan")]
        request_id: Option<String>,
        /// Actor recorded for plan-scoped application.
        #[arg(long, default_value = "user")]
        actor: String,
    },
    /// Explicitly download and verify every configured catalog package.
    Sync {
        /// Synchronize every package listed by the configured catalog.
        #[arg(long)]
        all: bool,
    },
    /// Initialize, validate, or build a static team standards catalog.
    Catalog(CatalogArguments),
    /// Inspect, refresh, or explicitly resolve plan-scoped rule conflicts.
    Conflict(ConflictArguments),
}

#[derive(Clone, Debug, Args)]
pub(crate) struct CatalogArguments {
    #[command(subcommand)]
    action: CatalogAction,
}

#[derive(Clone, Debug, Subcommand)]
enum CatalogAction {
    /// Atomically create a valid example source tree.
    Init {
        /// New source-tree path whose parent already exists.
        #[arg(long)]
        source: PathBuf,
        /// Lowercase DNS-like namespace for every team package.
        #[arg(long)]
        namespace: String,
        /// HTTPS base URL where the built tree will be hosted.
        #[arg(long)]
        base_url: String,
    },
    /// Validate and report canonical identities without writing.
    Validate {
        /// Existing team-catalog source-tree path.
        #[arg(long)]
        source: PathBuf,
    },
    /// Atomically produce a static hostable catalog tree.
    Build {
        /// Existing team-catalog source-tree path.
        #[arg(long)]
        source: PathBuf,
        /// Generated output path whose parent already exists.
        #[arg(long)]
        output: PathBuf,
    },
}

#[derive(Clone, Debug, Args)]
pub(crate) struct ConflictArguments {
    #[command(subcommand)]
    action: ConflictAction,
}

#[derive(Clone, Debug, Subcommand)]
enum ConflictAction {
    /// List current candidates, sources, precedence, and decisions.
    List {
        /// Target plan identifier.
        #[arg(long)]
        plan: String,
    },
    /// Persist the exact current conflict candidates without selecting one.
    Refresh(ConflictMutationArguments),
    /// Record one explicit candidate decision and rationale.
    Resolve(ConflictResolveArguments),
}

#[derive(Clone, Debug, Args)]
struct ConflictMutationArguments {
    /// Target plan identifier.
    #[arg(long)]
    plan: String,
    /// Required optimistic-concurrency revision.
    #[arg(long)]
    expect_revision: u64,
    /// Idempotency UUID for this semantic mutation.
    #[arg(long)]
    request_id: String,
    /// Actor recorded in the plan event log.
    #[arg(long, default_value = "user")]
    actor: String,
    /// Canonical live-source digest accepted only for normalized retry argv.
    #[arg(long, hide = true)]
    source_digest: Option<String>,
}

#[derive(Clone, Debug, Args)]
struct ConflictResolveArguments {
    #[command(flatten)]
    mutation: ConflictMutationArguments,
    /// Stable conflict identifier from `standards conflict list`.
    #[arg(long)]
    conflict: String,
    /// Exact candidate identifier selected by the user.
    #[arg(long)]
    candidate: String,
    /// Non-empty reason for choosing this candidate.
    #[arg(long)]
    rationale: String,
    /// Auditable external reference for the explicit decision.
    #[arg(long)]
    decision_ref: String,
}

pub(crate) fn execute(start: &Path, action: StandardsAction) -> Result<CommandResponse, MinoError> {
    match action {
        StandardsAction::Detect => {
            let scan = project::scan(start)?;
            response(
                "Standards detection completed.",
                serde_json::json!({ "languages": detected_languages(&scan) }),
            )
        }
        StandardsAction::Recommend { paths } => {
            let catalog = EmbeddedCatalog::load()?;
            let scan = project::scan(start)?;
            let recommendation = if paths.is_empty() {
                recommend_initial(&catalog, &scan)?
            } else {
                recommend_for_paths(&catalog, &scan, &paths)?
            };
            response("Standards recommendation completed.", recommendation)
        }
        StandardsAction::Apply {
            recommended,
            seed_verification,
            paths,
            plan,
            expect_revision,
            request_id,
            actor,
        } => {
            if !recommended || !seed_verification {
                return Err(MinoError::new(
                    ErrorCategory::IncompleteOrValidation,
                    "v0.1 standards apply requires --recommended and --seed-verification",
                ));
            }
            if let Some(plan) = plan {
                let expect_revision = expect_revision.ok_or_else(|| {
                    validation_error("Plan-scoped standards apply requires --expect-revision")
                })?;
                let request_id = request_id.ok_or_else(|| {
                    validation_error("Plan-scoped standards apply requires --request-id")
                })?;
                execute_plan_apply(start, plan, expect_revision, request_id, actor)
            } else {
                let catalog = EmbeddedCatalog::load()?;
                let scan = project::scan(start)?;
                let recommendation = if paths.is_empty() {
                    recommend_initial(&catalog, &scan)?
                } else {
                    recommend_for_paths(&catalog, &scan, &paths)?
                };
                let application =
                    apply_recommendation(&scan.root, &catalog, &recommendation, &SystemToolProbe)?;
                response("Standards application resolved.", application)
            }
        }
        StandardsAction::Sync { all } => {
            if !all {
                return Err(MinoError::new(
                    ErrorCategory::IncompleteOrValidation,
                    "v0.1 standards sync requires --all",
                ));
            }
            response(
                "Standards catalog synchronized.",
                crate::standards::synchronize_all(start)?,
            )
        }
        StandardsAction::Catalog(arguments) => execute_catalog(arguments.action),
        StandardsAction::Conflict(arguments) => execute_conflict(start, arguments.action),
    }
}

fn execute_plan_apply(
    start: &Path,
    plan: String,
    expect_revision: u64,
    request_id: String,
    actor: String,
) -> Result<CommandResponse, MinoError> {
    let plan_id = parse_plan_id(&plan)?;
    let parsed_request_id = parse_request_id(&request_id)?;
    let command = vec![
        "mino".to_owned(),
        "standards".to_owned(),
        "apply".to_owned(),
        "--recommended".to_owned(),
        "--seed-verification".to_owned(),
        "--plan".to_owned(),
        plan,
        "--expect-revision".to_owned(),
        expect_revision.to_string(),
        "--request-id".to_owned(),
        request_id,
        "--actor".to_owned(),
        actor.clone(),
    ];
    let report = StandardsPlanService::discover(start)?.reconcile(PlanMutationRequest {
        plan_id,
        expected_revision: expect_revision,
        request_id: parsed_request_id,
        actor,
        command,
        updated_at: Timestamp::now_utc(),
    })?;
    let complete = !report.project_scan.is_incomplete();
    let missing = if complete {
        Vec::new()
    } else {
        vec!["scan.acceptance".to_owned()]
    };
    let payload = serde_json::to_value(report).map_err(|error| serialization_error(&error))?;
    Ok(CommandResponse {
        message: "Plan standards reconciled.".to_owned(),
        complete,
        payload,
        missing,
        next_actions: Vec::new(),
    })
}

fn execute_catalog(action: CatalogAction) -> Result<CommandResponse, MinoError> {
    match action {
        CatalogAction::Init {
            source,
            namespace,
            base_url,
        } => response(
            "Team standards catalog source initialized.",
            crate::standards::initialize_team_catalog(&source, &namespace, &base_url)?,
        ),
        CatalogAction::Validate { source } => response(
            "Team standards catalog source validated.",
            crate::standards::validate_team_catalog(&source)?,
        ),
        CatalogAction::Build { source, output } => response(
            "Team standards catalog built.",
            crate::standards::build_team_catalog(&source, &output)?,
        ),
    }
}

fn execute_conflict(start: &Path, action: ConflictAction) -> Result<CommandResponse, MinoError> {
    let service = StandardsConflictService::discover(start)?;
    match action {
        ConflictAction::List { plan } => {
            let plan_id = parse_plan_id(&plan)?;
            let report = service.inspect(&plan_id)?;
            conflict_response("Standards conflicts inspected.", report)
        }
        ConflictAction::Refresh(arguments) => {
            let plan_id = parse_plan_id(&arguments.plan)?;
            let request_id = parse_request_id(&arguments.request_id)?;
            let live = service.inspect(&plan_id)?;
            require_matching_digest(arguments.source_digest.as_deref(), &live.source_digest)?;
            let command =
                conflict_mutation_command("refresh", &arguments, &live.source_digest, Vec::new());
            let report = service.refresh(
                PlanMutationRequest {
                    plan_id,
                    expected_revision: arguments.expect_revision,
                    request_id,
                    actor: arguments.actor,
                    command,
                    updated_at: Timestamp::now_utc(),
                },
                &live.source_digest,
            )?;
            conflict_operation_response("Standards conflict snapshots refreshed.", report)
        }
        ConflictAction::Resolve(arguments) => {
            let plan_id = parse_plan_id(&arguments.mutation.plan)?;
            let request_id = parse_request_id(&arguments.mutation.request_id)?;
            let live = service.inspect(&plan_id)?;
            require_matching_digest(
                arguments.mutation.source_digest.as_deref(),
                &live.source_digest,
            )?;
            let command = conflict_mutation_command(
                "resolve",
                &arguments.mutation,
                &live.source_digest,
                vec![
                    "--conflict".to_owned(),
                    arguments.conflict.clone(),
                    "--candidate".to_owned(),
                    arguments.candidate.clone(),
                    "--rationale".to_owned(),
                    arguments.rationale.clone(),
                    "--decision-ref".to_owned(),
                    arguments.decision_ref.clone(),
                ],
            );
            let report = service.resolve(
                PlanMutationRequest {
                    plan_id,
                    expected_revision: arguments.mutation.expect_revision,
                    request_id,
                    actor: arguments.mutation.actor,
                    command,
                    updated_at: Timestamp::now_utc(),
                },
                &live.source_digest,
                &arguments.conflict,
                &arguments.candidate,
                arguments.rationale,
                arguments.decision_ref,
            )?;
            conflict_operation_response("Standards conflict decision recorded.", report)
        }
    }
}

fn conflict_mutation_command(
    action: &str,
    arguments: &ConflictMutationArguments,
    source_digest: &str,
    extra: Vec<String>,
) -> Vec<String> {
    let mut command = vec![
        "mino".to_owned(),
        "standards".to_owned(),
        "conflict".to_owned(),
        action.to_owned(),
        "--plan".to_owned(),
        arguments.plan.clone(),
        "--expect-revision".to_owned(),
        arguments.expect_revision.to_string(),
        "--request-id".to_owned(),
        arguments.request_id.clone(),
        "--actor".to_owned(),
        arguments.actor.clone(),
        "--source-digest".to_owned(),
        source_digest.to_owned(),
    ];
    command.extend(extra);
    command
}

fn conflict_operation_response(
    message: &str,
    report: crate::application::standards::StandardsConflictOperationReport,
) -> Result<CommandResponse, MinoError> {
    let complete = report.standards_conflicts.resolved;
    let missing = conflict_missing(&report.standards_conflicts);
    let payload = serde_json::to_value(report).map_err(|error| serialization_error(&error))?;
    Ok(CommandResponse {
        message: message.to_owned(),
        complete,
        payload,
        missing,
        next_actions: Vec::new(),
    })
}

fn conflict_response(
    message: &str,
    report: StandardsConflictReport,
) -> Result<CommandResponse, MinoError> {
    let complete = report.resolved;
    let missing = conflict_missing(&report);
    let payload = serde_json::to_value(report).map_err(|error| serialization_error(&error))?;
    Ok(CommandResponse {
        message: message.to_owned(),
        complete,
        payload,
        missing,
        next_actions: Vec::new(),
    })
}

fn conflict_missing(report: &StandardsConflictReport) -> Vec<String> {
    let mut missing = report
        .conflicts
        .iter()
        .filter(|conflict| conflict.status != crate::standards::StandardConflictStatus::Resolved)
        .map(|conflict| format!("standards.conflicts.{}", conflict.conflict.id()))
        .collect::<Vec<_>>();
    missing.extend(
        report
            .stale_conflict_ids
            .iter()
            .map(|id| format!("standards.conflicts.{id}")),
    );
    missing.sort();
    missing.dedup();
    missing
}

fn parse_plan_id(value: &str) -> Result<PlanId, MinoError> {
    PlanId::parse(value).map_err(|error| validation_error(error.to_string()))
}

fn parse_request_id(value: &str) -> Result<RequestId, MinoError> {
    RequestId::parse(value).map_err(|error| validation_error(error.to_string()))
}

fn require_matching_digest(provided: Option<&str>, actual: &str) -> Result<(), MinoError> {
    if provided.is_none_or(|provided| provided == actual) {
        Ok(())
    } else {
        Err(MinoError::new(
            ErrorCategory::DriftDetected,
            "Provided standards source digest does not match current candidates",
        ))
    }
}

fn serialization_error(error: &serde_json::Error) -> MinoError {
    MinoError::new(
        ErrorCategory::EnvironmentUnavailable,
        format!("Failed to serialize standards conflict result: {error}"),
    )
}

fn validation_error(message: impl Into<String>) -> MinoError {
    MinoError::new(ErrorCategory::IncompleteOrValidation, message)
}

fn response<T: Serialize>(
    message: impl Into<String>,
    payload: T,
) -> Result<CommandResponse, MinoError> {
    let payload = serde_json::to_value(payload).map_err(|error| {
        MinoError::new(
            ErrorCategory::EnvironmentUnavailable,
            format!("Failed to serialize standards result: {error}"),
        )
    })?;
    Ok(CommandResponse {
        message: message.into(),
        complete: true,
        payload,
        missing: Vec::new(),
        next_actions: Vec::new(),
    })
}
