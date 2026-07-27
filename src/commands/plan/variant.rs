//! CLI adapters for historical plan forks, semantic diffs, and archive records.

use std::path::Path;

use clap::Args;

use super::{MutationArguments, mutation_command, parse_plan_id, parse_request_id, response};
use crate::application::plan::{PlanMutationRequest, PlanService};
use crate::application::plan_variant::{ForkPlanRequest, PlanVariantService};
use crate::commands::CommandResponse;
use crate::domain::Timestamp;
use crate::project::PlanSelectionRequest;
use crate::{MinoError, NextAction};

/// Arguments for creating an independent Draft from one retained revision.
#[derive(Debug, Args)]
pub(crate) struct ForkArguments {
    /// Source plan identifier.
    #[arg(long)]
    plan: String,
    /// Exact retained source revision.
    #[arg(long)]
    from_revision: u64,
    /// Human-readable name for the alternative.
    #[arg(long)]
    name: String,
    /// Explicit reason for creating the alternative.
    #[arg(long, allow_hyphen_values = true)]
    reason: String,
    /// Idempotency UUID for the creation event.
    #[arg(long)]
    request_id: String,
    /// Actor recorded in the creation event.
    #[arg(long, default_value = "user")]
    actor: String,
}

/// Arguments for comparing two current or retained plan revisions.
#[derive(Debug, Args)]
pub(crate) struct DiffArguments {
    /// Left plan identifier.
    #[arg(long)]
    left: String,
    /// Optional exact retained left revision; current is used when omitted.
    #[arg(long)]
    left_revision: Option<u64>,
    /// Right plan identifier.
    #[arg(long)]
    right: String,
    /// Optional exact retained right revision; current is used when omitted.
    #[arg(long)]
    right_revision: Option<u64>,
}

/// Arguments for approval-bound project plan selection.
#[derive(Debug, Args)]
pub(crate) struct SelectArguments {
    /// Live plan candidate to select.
    #[arg(long)]
    plan: String,
    /// Required project selection revision from `plan alternatives`.
    #[arg(long)]
    expect_selection_revision: u64,
    /// Idempotency UUID for this exact selection decision.
    #[arg(long)]
    request_id: String,
    /// Actor recorded in the selection audit.
    #[arg(long, default_value = "user")]
    actor: String,
    /// Auditable user approval reference.
    #[arg(long)]
    approval_ref: String,
    /// Non-empty reason for selecting this alternative.
    #[arg(long, allow_hyphen_values = true)]
    reason: String,
}

/// Arguments for approval-bound non-destructive plan deactivation.
#[derive(Debug, Args)]
pub(crate) struct ArchiveArguments {
    #[command(flatten)]
    mutation: MutationArguments,
    /// Explicit reason the plan was not selected.
    #[arg(long, allow_hyphen_values = true)]
    reason: String,
    /// Auditable reference for the user's alternative selection.
    #[arg(long)]
    approval_ref: String,
}

/// Creates or exactly replays a plan alternative.
pub(crate) fn execute_fork(
    start: &Path,
    plans: &PlanService,
    arguments: ForkArguments,
) -> Result<CommandResponse, MinoError> {
    let service = PlanVariantService::discover(start)?;
    let source_plan_id = parse_plan_id(&arguments.plan)?;
    let request_id = parse_request_id(&arguments.request_id)?;
    let command = vec![
        "mino".to_owned(),
        "plan".to_owned(),
        "fork".to_owned(),
        "--plan".to_owned(),
        source_plan_id.to_string(),
        "--from-revision".to_owned(),
        arguments.from_revision.to_string(),
        "--name".to_owned(),
        arguments.name.clone(),
        "--reason".to_owned(),
        arguments.reason.clone(),
        "--request-id".to_owned(),
        request_id.to_string(),
        "--actor".to_owned(),
        arguments.actor.clone(),
    ];
    let report = service.fork(ForkPlanRequest {
        source_plan_id,
        from_revision: arguments.from_revision,
        name: arguments.name,
        reason: arguments.reason,
        request_id,
        actor: arguments.actor,
        command,
        forked_at: Timestamp::now_utc(),
    })?;
    let guidance = plans.next(&report.operation.plan_id)?;
    response(
        "Plan alternative forked from retained source revision.",
        false,
        report,
        guidance.missing,
        guidance.next_actions,
    )
}

/// Returns one non-mutating semantic comparison.
pub(crate) fn execute_diff(
    start: &Path,
    arguments: &DiffArguments,
) -> Result<CommandResponse, MinoError> {
    let service = PlanVariantService::discover(start)?;
    let left = parse_plan_id(&arguments.left)?;
    let right = parse_plan_id(&arguments.right)?;
    let report = service.diff(
        &left,
        arguments.left_revision,
        &right,
        arguments.right_revision,
    )?;
    let message = report.render_human();
    response(message, true, report, Vec::new(), Vec::new())
}

/// Returns the selected project plan and every live alternative.
pub(crate) fn execute_alternatives(plans: &PlanService) -> Result<CommandResponse, MinoError> {
    let selection = plans.plan_selection()?;
    let complete = selection.selected_plan.is_some() || selection.is_empty();
    let missing = if complete {
        Vec::new()
    } else {
        vec!["plan_selection".to_owned()]
    };
    response(
        "Project plan alternatives inspected.",
        complete,
        selection,
        missing,
        Vec::<NextAction>::new(),
    )
}

/// Records an explicit project-level alternative selection.
pub(crate) fn execute_select(
    plans: &PlanService,
    arguments: SelectArguments,
) -> Result<CommandResponse, MinoError> {
    let plan_id = parse_plan_id(&arguments.plan)?;
    let request_id = parse_request_id(&arguments.request_id)?;
    let command = vec![
        "mino".to_owned(),
        "plan".to_owned(),
        "select".to_owned(),
        "--plan".to_owned(),
        plan_id.to_string(),
        "--expect-selection-revision".to_owned(),
        arguments.expect_selection_revision.to_string(),
        "--request-id".to_owned(),
        request_id.to_string(),
        "--actor".to_owned(),
        arguments.actor.clone(),
        "--approval-ref".to_owned(),
        arguments.approval_ref.clone(),
        "--reason".to_owned(),
        arguments.reason.clone(),
    ];
    let report = plans.select_plan(PlanSelectionRequest {
        plan_id,
        expected_selection_revision: arguments.expect_selection_revision,
        request_id,
        actor: arguments.actor,
        approval_reference: arguments.approval_ref,
        reason: arguments.reason,
        command,
        selected_at: Timestamp::now_utc(),
    })?;
    response(
        "Project plan alternative selected.",
        true,
        report,
        Vec::new(),
        Vec::<NextAction>::new(),
    )
}

/// Records approval-bound semantic deactivation without deleting plan history.
pub(crate) fn execute_archive(
    start: &Path,
    arguments: ArchiveArguments,
) -> Result<CommandResponse, MinoError> {
    let service = PlanVariantService::discover(start)?;
    let command = mutation_command(
        &["archive"],
        &arguments.mutation,
        vec![
            "--reason".to_owned(),
            arguments.reason.clone(),
            "--approval-ref".to_owned(),
            arguments.approval_ref.clone(),
        ],
    );
    let request = PlanMutationRequest {
        plan_id: parse_plan_id(&arguments.mutation.plan)?,
        expected_revision: arguments.mutation.expect_revision,
        request_id: parse_request_id(&arguments.mutation.request_id)?,
        actor: arguments.mutation.actor,
        command,
        updated_at: Timestamp::now_utc(),
    };
    let report = service.archive(request, arguments.reason, arguments.approval_ref)?;
    response(
        "Plan archived without deleting its state or history.",
        true,
        report,
        Vec::new(),
        Vec::<NextAction>::new(),
    )
}
