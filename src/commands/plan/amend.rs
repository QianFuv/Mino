//! Protected plan amendment CLI adapter.

use std::path::{Path, PathBuf};

use clap::{Args, Subcommand, ValueEnum};
use serde::Serialize;

use super::{MutationArguments, parse_plan_id, parse_request_id, require_matching_digest};
use crate::application::agent::AgentService;
use crate::application::amendment::AmendmentService;
use crate::application::plan::PlanMutationRequest;
use crate::commands::CommandResponse;
use crate::domain::{AmendmentClassification, AmendmentPatch, Timestamp};
use crate::input::read_utf8_file;
use crate::store::sha256_digest;
use crate::{ErrorCategory, MinoError};

/// Nested protected-amendment command group.
#[derive(Debug, Args)]
pub(crate) struct AmendArguments {
    #[command(subcommand)]
    action: AmendAction,
}

#[derive(Debug, Subcommand)]
enum AmendAction {
    /// Record typed operations and their minimum protected classification.
    Propose(ProposeArguments),
    /// Record explicit approval for a pending Material change.
    Approve(ApproveArguments),
    /// Reject an unapproved Material change without applying it.
    Reject(DispositionArguments),
    /// Withdraw an unapproved change as its original proposer.
    Withdraw(WithdrawArguments),
    /// Cancel an approved Material change without applying it.
    Cancel(DispositionArguments),
    /// Atomically apply an eligible typed proposal.
    Apply(ApplyArguments),
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum ClassificationArgument {
    Minor,
    Material,
}

impl From<ClassificationArgument> for AmendmentClassification {
    fn from(value: ClassificationArgument) -> Self {
        match value {
            ClassificationArgument::Minor => Self::Minor,
            ClassificationArgument::Material => Self::Material,
        }
    }
}

#[derive(Debug, Args)]
struct ProposeArguments {
    #[command(flatten)]
    mutation: MutationArguments,
    /// Exact reason the approved plan must change.
    #[arg(long, allow_hyphen_values = true)]
    reason: String,
    /// Strict YAML document containing only typed amendment operations.
    #[arg(long)]
    patch_file: PathBuf,
    /// Optional caller-selected class; it may raise but never lower the minimum.
    #[arg(long, value_enum)]
    classification: Option<ClassificationArgument>,
    /// Canonical patch digest accepted only for normalized replay argv.
    #[arg(long, hide = true)]
    patch_digest: Option<String>,
}

#[derive(Debug, Args)]
struct ApproveArguments {
    #[command(flatten)]
    mutation: MutationArguments,
    /// Stable protected-change identifier such as C1.
    #[arg(long)]
    change: String,
    /// Auditable reference for the explicit Material approval.
    #[arg(long)]
    approval_ref: String,
}

#[derive(Debug, Args)]
struct ApplyArguments {
    #[command(flatten)]
    mutation: MutationArguments,
    /// Stable protected-change identifier such as C1.
    #[arg(long)]
    change: String,
}

#[derive(Debug, Args)]
struct DispositionArguments {
    #[command(flatten)]
    mutation: MutationArguments,
    /// Stable protected-change identifier such as C1.
    #[arg(long)]
    change: String,
    /// Auditable reference for the protected terminal decision.
    #[arg(long)]
    decision_ref: String,
    /// Exact reason the proposal will not be applied.
    #[arg(long, allow_hyphen_values = true)]
    reason: String,
}

#[derive(Debug, Args)]
struct WithdrawArguments {
    #[command(flatten)]
    mutation: MutationArguments,
    /// Stable protected-change identifier such as C1.
    #[arg(long)]
    change: String,
    /// Exact reason the original proposer withdrew the proposal.
    #[arg(long, allow_hyphen_values = true)]
    reason: String,
}

pub(crate) fn execute(
    start: &Path,
    arguments: AmendArguments,
) -> Result<CommandResponse, MinoError> {
    let service = AmendmentService::discover(start)?;
    match arguments.action {
        AmendAction::Propose(arguments) => propose(start, &service, arguments),
        AmendAction::Approve(arguments) => approve(start, &service, arguments),
        AmendAction::Reject(arguments) => reject(start, &service, arguments),
        AmendAction::Withdraw(arguments) => withdraw(start, &service, arguments),
        AmendAction::Cancel(arguments) => cancel(start, &service, arguments),
        AmendAction::Apply(arguments) => apply(start, &service, arguments),
    }
}

fn propose(
    start: &Path,
    service: &AmendmentService,
    arguments: ProposeArguments,
) -> Result<CommandResponse, MinoError> {
    let source = read_utf8_file(&arguments.patch_file)?;
    let digest = sha256_digest(source.as_bytes());
    require_matching_digest(arguments.patch_digest.as_deref(), &digest)?;
    let patch: AmendmentPatch = serde_saphyr::from_str(&source).map_err(|error| {
        MinoError::new(
            ErrorCategory::IncompleteOrValidation,
            format!("Failed to parse strict amendment YAML: {error}"),
        )
    })?;
    let mut extra = vec![
        "--reason".to_owned(),
        arguments.reason.clone(),
        "--patch-file".to_owned(),
        arguments.patch_file.to_string_lossy().into_owned(),
        "--patch-digest".to_owned(),
        digest,
    ];
    if let Some(classification) = arguments.classification {
        extra.extend([
            "--classification".to_owned(),
            match classification {
                ClassificationArgument::Minor => "minor",
                ClassificationArgument::Material => "material",
            }
            .to_owned(),
        ]);
    }
    let command = mutation_command(&["propose"], &arguments.mutation, extra);
    let request = mutation_request(arguments.mutation, command)?;
    let report = service.propose(
        request,
        arguments.reason,
        patch,
        arguments.classification.map(Into::into),
    )?;
    response_with_guidance(start, "Protected amendment proposed.", report)
}

fn approve(
    start: &Path,
    service: &AmendmentService,
    arguments: ApproveArguments,
) -> Result<CommandResponse, MinoError> {
    let command = mutation_command(
        &["approve"],
        &arguments.mutation,
        vec![
            "--change".to_owned(),
            arguments.change.clone(),
            "--approval-ref".to_owned(),
            arguments.approval_ref.clone(),
        ],
    );
    let request = mutation_request(arguments.mutation, command)?;
    let report = service.approve(request, arguments.change, arguments.approval_ref)?;
    response_with_guidance(start, "Material amendment approved.", report)
}

fn apply(
    start: &Path,
    service: &AmendmentService,
    arguments: ApplyArguments,
) -> Result<CommandResponse, MinoError> {
    let command = mutation_command(
        &["apply"],
        &arguments.mutation,
        vec!["--change".to_owned(), arguments.change.clone()],
    );
    let request = mutation_request(arguments.mutation, command)?;
    let report = service.apply(request, arguments.change)?;
    response_with_guidance(start, "Protected amendment applied.", report)
}

fn reject(
    start: &Path,
    service: &AmendmentService,
    arguments: DispositionArguments,
) -> Result<CommandResponse, MinoError> {
    let command = disposition_command("reject", &arguments);
    let request = mutation_request(arguments.mutation, command)?;
    let report = service.reject(
        request,
        arguments.change,
        arguments.decision_ref,
        arguments.reason,
    )?;
    response_with_guidance(start, "Protected amendment rejected.", report)
}

fn withdraw(
    start: &Path,
    service: &AmendmentService,
    arguments: WithdrawArguments,
) -> Result<CommandResponse, MinoError> {
    let command = mutation_command(
        &["withdraw"],
        &arguments.mutation,
        vec![
            "--change".to_owned(),
            arguments.change.clone(),
            "--reason".to_owned(),
            arguments.reason.clone(),
        ],
    );
    let request = mutation_request(arguments.mutation, command)?;
    let report = service.withdraw(request, arguments.change, arguments.reason)?;
    response_with_guidance(start, "Protected amendment withdrawn.", report)
}

fn cancel(
    start: &Path,
    service: &AmendmentService,
    arguments: DispositionArguments,
) -> Result<CommandResponse, MinoError> {
    let command = disposition_command("cancel", &arguments);
    let request = mutation_request(arguments.mutation, command)?;
    let report = service.cancel(
        request,
        arguments.change,
        arguments.decision_ref,
        arguments.reason,
    )?;
    response_with_guidance(start, "Protected amendment cancelled.", report)
}

fn disposition_command(action: &str, arguments: &DispositionArguments) -> Vec<String> {
    mutation_command(
        &[action],
        &arguments.mutation,
        vec![
            "--change".to_owned(),
            arguments.change.clone(),
            "--decision-ref".to_owned(),
            arguments.decision_ref.clone(),
            "--reason".to_owned(),
            arguments.reason.clone(),
        ],
    )
}

fn mutation_request(
    arguments: MutationArguments,
    command: Vec<String>,
) -> Result<PlanMutationRequest, MinoError> {
    Ok(PlanMutationRequest {
        plan_id: parse_plan_id(&arguments.plan)?,
        expected_revision: arguments.expect_revision,
        request_id: parse_request_id(&arguments.request_id)?,
        actor: arguments.actor,
        command,
        updated_at: Timestamp::now_utc(),
    })
}

fn mutation_command(
    path: &[&str],
    arguments: &MutationArguments,
    extra: Vec<String>,
) -> Vec<String> {
    let mut command = vec!["mino".to_owned(), "plan".to_owned(), "amend".to_owned()];
    command.extend(path.iter().map(|part| (*part).to_owned()));
    command.extend([
        "--plan".to_owned(),
        arguments.plan.clone(),
        "--expect-revision".to_owned(),
        arguments.expect_revision.to_string(),
        "--request-id".to_owned(),
        arguments.request_id.clone(),
        "--actor".to_owned(),
        arguments.actor.clone(),
    ]);
    command.extend(extra);
    command
}

fn response_with_guidance<T: Serialize>(
    start: &Path,
    message: &str,
    payload: T,
) -> Result<CommandResponse, MinoError> {
    let guidance = AgentService::discover(start)?.context()?;
    let payload = serde_json::to_value(payload).map_err(|error| {
        MinoError::new(
            ErrorCategory::EnvironmentUnavailable,
            format!("Failed to serialize amendment result: {error}"),
        )
    })?;
    Ok(CommandResponse {
        message: message.to_owned(),
        complete: true,
        payload,
        missing: Vec::new(),
        next_actions: guidance.next_actions,
    })
}
