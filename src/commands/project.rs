//! Project lifecycle CLI command adapter.

use std::path::{Path, PathBuf};

use clap::{Args, Subcommand, ValueEnum};
use serde::Serialize;

use crate::application::plan::{
    CreatePlanRequest, DraftMutation, PlanMutationRequest, PlanOperationReport, PlanService,
};
use crate::commands::CommandResponse;
use crate::domain::{PlanStatus, RequestId, Timestamp};
use crate::integration::IntegrationOptions;
use crate::project::{
    self, DoctorFinding, LegacyDocumentKind, LegacyInput, PlanningAuthorityApplyRequest,
    PlanningAuthorityDecision, PlanningAuthorityDecisionRequest, PlanningAuthorityService,
};
use crate::store::sha256_digest;
use crate::{ErrorCategory, MinoError, NextAction};

#[derive(Clone, Debug, Subcommand)]
pub(crate) enum ProjectAction {
    /// Initialize missing local Mino project state without network or Git mutation.
    Init(InitArguments),
    /// Show parsed project state and doctor findings.
    Show,
    /// Diagnose configuration, locks, transactions, projections, and integrations.
    Doctor,
    /// Scan workspaces and return evidence-based language rankings.
    Scan,
    /// Analyze legacy planning workflow documents without modifying them.
    Migrate(MigrateArguments),
    /// Import legacy plan authoring into a separate current Draft.
    Import(ImportArguments),
    /// Inspect or explicitly resolve durable-planning authority.
    Authority(AuthorityArguments),
}

#[derive(Clone, Copy, Debug, Args)]
pub(crate) struct InitArguments {
    /// Apply or refresh only the owned Mino block in AGENTS.md.
    #[arg(long)]
    apply_agents_block: bool,
    /// Apply or refresh only the owned Mino block in .gitignore.
    #[arg(long)]
    apply_gitignore_block: bool,
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
pub(crate) struct ImportArguments {
    #[command(subcommand)]
    action: ImportAction,
}

#[derive(Clone, Debug, Subcommand)]
enum ImportAction {
    /// Conservatively import one legacy managed Markdown plan.
    Legacy(LegacyPlanImportArguments),
}

#[derive(Clone, Debug, Args)]
pub(crate) struct AuthorityArguments {
    #[command(subcommand)]
    action: AuthorityAction,
}

#[derive(Clone, Debug, Subcommand)]
enum AuthorityAction {
    /// Inspect current legacy clauses, decision state, and durable-plan gate.
    Status,
    /// Build the exact Planning Documents replacement without writing.
    Propose,
    /// Record coexistence or decline against exact source bytes.
    Decide(AuthorityDecideArguments),
    /// Apply the exact digest-bound supersession rewrite.
    Apply(AuthorityApplyArguments),
}

#[derive(Clone, Debug, Args)]
struct AuthorityDecideArguments {
    /// Explicit authority outcome.
    #[arg(long, value_enum)]
    decision: AuthorityDecisionArgument,
    /// Exact current AGENTS.md digest returned by status.
    #[arg(long)]
    source_digest: String,
    /// Required authority revision.
    #[arg(long)]
    expect_revision: u64,
    /// Idempotency UUID for this exact decision.
    #[arg(long)]
    request_id: String,
    /// Auditable external approval reference.
    #[arg(long)]
    approval_ref: String,
    /// Actor recorded in authority state.
    #[arg(long, default_value = "user")]
    actor: String,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum AuthorityDecisionArgument {
    CoexistenceApproved,
    Declined,
}

impl From<AuthorityDecisionArgument> for PlanningAuthorityDecision {
    fn from(value: AuthorityDecisionArgument) -> Self {
        match value {
            AuthorityDecisionArgument::CoexistenceApproved => Self::CoexistenceApproved,
            AuthorityDecisionArgument::Declined => Self::Declined,
        }
    }
}

#[derive(Clone, Debug, Args)]
struct AuthorityApplyArguments {
    /// Explicitly confirm replacement of the detected Planning Documents section.
    #[arg(long)]
    apply_rewrite: bool,
    /// Exact current AGENTS.md digest returned by proposal.
    #[arg(long)]
    source_digest: String,
    /// Exact complete replacement digest returned by proposal.
    #[arg(long)]
    replacement_digest: String,
    /// Required authority revision.
    #[arg(long)]
    expect_revision: u64,
    /// Idempotency UUID for this exact apply.
    #[arg(long)]
    request_id: String,
    /// Auditable external approval reference.
    #[arg(long)]
    approval_ref: String,
    /// Actor recorded in authority state.
    #[arg(long, default_value = "user")]
    actor: String,
}

#[derive(Clone, Debug, Args)]
struct LegacyPlanImportArguments {
    /// Legacy Markdown plan to read without modification.
    #[arg(long)]
    source: PathBuf,
    /// ASCII-bearing stable name for the new plan identifier.
    #[arg(long)]
    name: String,
    /// Idempotency UUID for the two-phase import.
    #[arg(long)]
    request_id: String,
    /// Actor recorded in the plan event log.
    #[arg(long, default_value = "user")]
    actor: String,
    /// Canonical source digest accepted only for replayable normalized argv.
    #[arg(long, hide = true)]
    source_digest: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
struct LegacyPlanImportReport {
    source: project::LegacyPlanSource,
    suggested_name: Option<String>,
    mappings: Vec<project::LegacyPlanMapping>,
    warnings: Vec<project::LegacyPlanWarning>,
    #[serde(flatten)]
    imported_plan: PlanOperationReport,
    source_preserved: bool,
    historical_execution_trusted: bool,
    draft_review_required: bool,
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
        ProjectAction::Init(arguments) => {
            let report = project::initialize_with_options(
                start,
                IntegrationOptions {
                    apply_agents_block: arguments.apply_agents_block,
                    apply_gitignore_block: arguments.apply_gitignore_block,
                },
            )?;
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
        ProjectAction::Import(arguments) => match arguments.action {
            ImportAction::Legacy(arguments) => import_legacy_plan(start, arguments),
        },
        ProjectAction::Authority(arguments) => execute_authority(start, arguments.action),
    }
}

fn execute_authority(start: &Path, action: AuthorityAction) -> Result<CommandResponse, MinoError> {
    let service = PlanningAuthorityService::discover(start)?;
    match action {
        AuthorityAction::Status => {
            let status = service.status()?;
            let missing = status.block_reason.iter().cloned().collect();
            let next_actions = status
                .recovery_action
                .clone()
                .or_else(|| status.state_refresh_action.clone())
                .into_iter()
                .collect();
            response(
                "Planning authority inspected without mutation.",
                !status.blocks_durable_planning,
                status,
                missing,
                next_actions,
            )
        }
        AuthorityAction::Propose => response(
            "Planning authority rewrite proposed without mutation.",
            false,
            service.propose()?,
            vec!["planning_authority_decision".to_owned()],
            Vec::new(),
        ),
        AuthorityAction::Decide(arguments) => authority_decide(&service, arguments),
        AuthorityAction::Apply(arguments) => authority_apply(&service, arguments),
    }
}

fn authority_decide(
    service: &PlanningAuthorityService,
    arguments: AuthorityDecideArguments,
) -> Result<CommandResponse, MinoError> {
    let request_id = RequestId::parse(&arguments.request_id)
        .map_err(|error| validation_error(error.to_string()))?;
    let decision: PlanningAuthorityDecision = arguments.decision.into();
    let command = vec![
        "mino".to_owned(),
        "project".to_owned(),
        "authority".to_owned(),
        "decide".to_owned(),
        "--decision".to_owned(),
        authority_decision_name(decision).to_owned(),
        "--source-digest".to_owned(),
        arguments.source_digest.clone(),
        "--expect-revision".to_owned(),
        arguments.expect_revision.to_string(),
        "--request-id".to_owned(),
        request_id.to_string(),
        "--approval-ref".to_owned(),
        arguments.approval_ref.clone(),
        "--actor".to_owned(),
        arguments.actor.clone(),
    ];
    let report = service.decide(PlanningAuthorityDecisionRequest {
        expected_revision: arguments.expect_revision,
        expected_source_digest: arguments.source_digest,
        decision,
        request_id,
        actor: arguments.actor,
        approval_reference: arguments.approval_ref,
        command,
        decided_at: Timestamp::now_utc(),
    })?;
    let complete = !report.status.blocks_durable_planning;
    let missing = report.status.block_reason.iter().cloned().collect();
    response(
        "Planning authority decision recorded.",
        complete,
        report,
        missing,
        Vec::new(),
    )
}

fn authority_apply(
    service: &PlanningAuthorityService,
    arguments: AuthorityApplyArguments,
) -> Result<CommandResponse, MinoError> {
    let request_id = RequestId::parse(&arguments.request_id)
        .map_err(|error| validation_error(error.to_string()))?;
    let command = vec![
        "mino".to_owned(),
        "project".to_owned(),
        "authority".to_owned(),
        "apply".to_owned(),
        "--apply-rewrite".to_owned(),
        "--source-digest".to_owned(),
        arguments.source_digest.clone(),
        "--replacement-digest".to_owned(),
        arguments.replacement_digest.clone(),
        "--expect-revision".to_owned(),
        arguments.expect_revision.to_string(),
        "--request-id".to_owned(),
        request_id.to_string(),
        "--approval-ref".to_owned(),
        arguments.approval_ref.clone(),
        "--actor".to_owned(),
        arguments.actor.clone(),
    ];
    let report = service.apply(&PlanningAuthorityApplyRequest {
        expected_revision: arguments.expect_revision,
        expected_source_digest: arguments.source_digest,
        expected_replacement_digest: arguments.replacement_digest,
        is_confirmed: arguments.apply_rewrite,
        request_id,
        actor: arguments.actor,
        approval_reference: arguments.approval_ref,
        command,
        decided_at: Timestamp::now_utc(),
    })?;
    response(
        "Legacy Planning Documents section superseded through a guarded rewrite.",
        true,
        report,
        Vec::new(),
        Vec::new(),
    )
}

fn authority_decision_name(decision: PlanningAuthorityDecision) -> &'static str {
    match decision {
        PlanningAuthorityDecision::CoexistenceApproved => "coexistence-approved",
        PlanningAuthorityDecision::Declined => "declined",
        PlanningAuthorityDecision::Pending | PlanningAuthorityDecision::Superseded => {
            unreachable!("CLI accepts only explicit decide outcomes")
        }
    }
}

fn import_legacy_plan(
    start: &Path,
    arguments: LegacyPlanImportArguments,
) -> Result<CommandResponse, MinoError> {
    let request_id = RequestId::parse(&arguments.request_id)
        .map_err(|error| validation_error(error.to_string()))?;
    let parsed = project::parse_legacy_plan(&arguments.source)?;
    require_matching_digest(
        arguments.source_digest.as_deref(),
        parsed.source.digest.as_str(),
    )?;
    project::verify_legacy_plan_source(&parsed.source)?;
    let service = PlanService::discover(start)?;
    let command = vec![
        "mino".to_owned(),
        "project".to_owned(),
        "import".to_owned(),
        "legacy".to_owned(),
        "--source".to_owned(),
        arguments.source.to_string_lossy().into_owned(),
        "--name".to_owned(),
        arguments.name.clone(),
        "--source-digest".to_owned(),
        parsed.source.digest.clone(),
        "--request-id".to_owned(),
        request_id.to_string(),
        "--actor".to_owned(),
        arguments.actor.clone(),
    ];
    let created = service.create(CreatePlanRequest {
        name: arguments.name,
        trigger: "legacy-import".to_owned(),
        original_request: parsed.original_request.clone(),
        request_id: request_id.clone(),
        actor: arguments.actor.clone(),
        command: command.clone(),
        created_at: Timestamp::now_utc(),
    })?;
    project::verify_legacy_plan_source(&parsed.source)?;
    let apply_request_id = derived_import_request_id(&request_id, &parsed.source.digest)?;
    let imported_plan = service.mutate(
        PlanMutationRequest {
            plan_id: created.plan_id,
            expected_revision: 1,
            request_id: apply_request_id,
            actor: arguments.actor,
            command,
            updated_at: Timestamp::now_utc(),
        },
        &DraftMutation::Apply(parsed.draft.clone()),
    )?;
    project::verify_legacy_plan_source(&parsed.source)?;
    if imported_plan.status != PlanStatus::Draft {
        return Err(MinoError::new(
            ErrorCategory::DriftDetected,
            "Legacy import produced a non-Draft plan",
        ));
    }
    let guidance = service.next(&imported_plan.plan_id)?;
    let report = LegacyPlanImportReport {
        source: parsed.source,
        suggested_name: parsed.suggested_name,
        mappings: parsed.mappings,
        warnings: parsed.warnings,
        imported_plan,
        source_preserved: true,
        historical_execution_trusted: false,
        draft_review_required: true,
    };
    response(
        "Legacy plan authoring imported into a separate Draft; historical execution remains unverified.",
        false,
        report,
        guidance.missing,
        guidance.next_actions,
    )
}

fn derived_import_request_id(
    request_id: &RequestId,
    source_digest: &str,
) -> Result<RequestId, MinoError> {
    let digest = sha256_digest(
        format!("project.import.legacy.apply:{request_id}:{source_digest}").as_bytes(),
    );
    let value = &digest["sha256:".len().."sha256:".len() + 32];
    RequestId::parse(format!(
        "{}-{}-{}-{}-{}",
        &value[0..8],
        &value[8..12],
        &value[12..16],
        &value[16..20],
        &value[20..32]
    ))
    .map_err(|error| validation_error(error.to_string()))
}

fn require_matching_digest(provided: Option<&str>, actual: &str) -> Result<(), MinoError> {
    if provided.is_none_or(|provided| provided == actual) {
        Ok(())
    } else {
        Err(validation_error(
            "Provided normalized source digest does not match the legacy plan",
        ))
    }
}

fn validation_error(message: impl Into<String>) -> MinoError {
    MinoError::new(ErrorCategory::IncompleteOrValidation, message)
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
    if has_finding(
        findings,
        &[
            "legacy_planning_authority_conflict",
            "mino_durable_planning_declined",
        ],
    ) {
        return vec![project::authority_status_action()];
    }
    let should_install_skill = has_finding(findings, &["mino_skill_missing", "mino_skill_drift"]);
    let should_apply_agents =
        has_finding(findings, &["agents_block_missing", "agents_block_drift"]);
    let should_apply_gitignore = has_finding(
        findings,
        &["gitignore_block_missing", "gitignore_block_drift"],
    );
    if !should_install_skill && !should_apply_agents && !should_apply_gitignore {
        return Vec::new();
    }
    let mut argv = vec!["mino".to_owned(), "project".to_owned(), "init".to_owned()];
    if should_apply_agents {
        argv.push("--apply-agents-block".to_owned());
    }
    if should_apply_gitignore {
        argv.push("--apply-gitignore-block".to_owned());
    }
    argv.extend([
        "--format".to_owned(),
        "json".to_owned(),
        "--no-input".to_owned(),
    ]);
    vec![NextAction {
        id: "project.init".to_owned(),
        argv,
    }]
}

fn has_finding(findings: &[DoctorFinding], codes: &[&str]) -> bool {
    findings
        .iter()
        .any(|finding| codes.contains(&finding.code.as_str()))
}
