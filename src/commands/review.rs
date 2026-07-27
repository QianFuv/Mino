//! Classified review and rework CLI adapter.

use std::path::{Path, PathBuf};

use clap::{Args, Subcommand, ValueEnum};
use serde::Serialize;

use crate::application::agent::AgentService;
use crate::application::plan::PlanMutationRequest;
use crate::application::review::ReviewService;
use crate::commands::CommandResponse;
use crate::domain::{
    DraftTaskInput, MaterialReviewDisposition, PlanId, RequestId, ReviewClassification, TaskId,
    Timestamp,
};
use crate::input::read_utf8_file;
use crate::store::sha256_digest;
use crate::{ErrorCategory, MinoError};

#[derive(Debug, Subcommand)]
pub(crate) enum ReviewAction {
    /// Record and classify one reviewer feedback item.
    Record(RecordArguments),
    /// Start an acceptance rerun or materialize a reserved R task.
    Rework(ReworkArguments),
    /// Resolve one fully executed and revalidated rework item.
    Resolve(ResolveArguments),
    /// Decide how one blocked Material review request should proceed.
    Disposition(DispositionArguments),
    /// Record explicit final acceptance and move Review to Done.
    Accept(AcceptArguments),
}

#[derive(Clone, Debug, Args)]
struct MutationArguments {
    /// Target plan identifier.
    #[arg(long)]
    plan: String,
    /// Required optimistic-concurrency revision.
    #[arg(long)]
    expect_revision: u64,
    /// Idempotency UUID for this review request.
    #[arg(long)]
    request_id: String,
    /// Reviewer or actor recorded in the event log.
    #[arg(long, default_value = "user")]
    actor: String,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum ClassificationArgument {
    AcceptanceDefect,
    InScopeRework,
    MaterialChange,
    FollowUp,
}

impl From<ClassificationArgument> for ReviewClassification {
    fn from(value: ClassificationArgument) -> Self {
        match value {
            ClassificationArgument::AcceptanceDefect => Self::AcceptanceDefect,
            ClassificationArgument::InScopeRework => Self::InScopeRework,
            ClassificationArgument::MaterialChange => Self::MaterialChange,
            ClassificationArgument::FollowUp => Self::FollowUp,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum DispositionArgument {
    AcceptChange,
    Decline,
    DeferToFollowUp,
}

impl From<DispositionArgument> for MaterialReviewDisposition {
    fn from(value: DispositionArgument) -> Self {
        match value {
            DispositionArgument::AcceptChange => Self::AcceptChange,
            DispositionArgument::Decline => Self::Decline,
            DispositionArgument::DeferToFollowUp => Self::DeferToFollowUp,
        }
    }
}

#[derive(Debug, Args)]
pub(crate) struct RecordArguments {
    #[command(flatten)]
    mutation: MutationArguments,
    /// Minimum protocol classification for this feedback.
    #[arg(long, value_enum)]
    classification: ClassificationArgument,
    /// Exact reviewer feedback.
    #[arg(long, allow_hyphen_values = true)]
    feedback: String,
    /// Required completed task for acceptance defects and in-scope rework.
    #[arg(long)]
    task: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct ReworkArguments {
    #[command(flatten)]
    mutation: MutationArguments,
    /// Stable review-item identifier such as REV-1.
    #[arg(long)]
    review: String,
    /// Strict YAML definition for the reserved in-scope R task.
    #[arg(long)]
    file: Option<PathBuf>,
    /// Canonical file digest accepted only for normalized replay argv.
    #[arg(long, hide = true)]
    file_digest: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct ResolveArguments {
    #[command(flatten)]
    mutation: MutationArguments,
    /// Stable review-item identifier such as REV-1.
    #[arg(long)]
    review: String,
}

#[derive(Debug, Args)]
pub(crate) struct DispositionArguments {
    #[command(flatten)]
    mutation: MutationArguments,
    /// Stable blocked Material review identifier such as REV-1.
    #[arg(long)]
    review: String,
    /// Product decision for the requested Material change.
    #[arg(long, value_enum)]
    decision: DispositionArgument,
    /// Auditable reference for the explicit product decision.
    #[arg(long)]
    decision_ref: String,
    /// Reason for accepting, declining, or deferring the requested change.
    #[arg(long, allow_hyphen_values = true)]
    reason: String,
}

#[derive(Debug, Args)]
pub(crate) struct AcceptArguments {
    #[command(flatten)]
    mutation: MutationArguments,
    /// Auditable reference for the explicit final acceptance.
    #[arg(long)]
    approval_ref: String,
}

pub(crate) fn execute(start: &Path, action: ReviewAction) -> Result<CommandResponse, MinoError> {
    let service = ReviewService::discover(start)?;
    match action {
        ReviewAction::Record(arguments) => record(start, &service, arguments),
        ReviewAction::Rework(arguments) => rework(start, &service, arguments),
        ReviewAction::Resolve(arguments) => resolve(start, &service, arguments),
        ReviewAction::Disposition(arguments) => disposition(start, &service, arguments),
        ReviewAction::Accept(arguments) => accept(start, &service, arguments),
    }
}

fn record(
    start: &Path,
    service: &ReviewService,
    arguments: RecordArguments,
) -> Result<CommandResponse, MinoError> {
    let classification_name = arguments
        .classification
        .to_possible_value()
        .map_or_else(|| "unknown".to_owned(), |value| value.get_name().to_owned());
    let mut extra = vec![
        "--classification".to_owned(),
        classification_name,
        "--feedback".to_owned(),
        arguments.feedback.clone(),
    ];
    let task_id = arguments.task.as_deref().map(parse_task_id).transpose()?;
    if let Some(task) = &arguments.task {
        extra.extend(["--task".to_owned(), task.clone()]);
    }
    let command = mutation_command(&["record"], &arguments.mutation, extra);
    let request = mutation_request(arguments.mutation, command)?;
    let report = service.record(
        request,
        arguments.classification.into(),
        arguments.feedback,
        task_id,
    )?;
    response_with_guidance(start, "Review feedback recorded.", report)
}

fn rework(
    start: &Path,
    service: &ReviewService,
    arguments: ReworkArguments,
) -> Result<CommandResponse, MinoError> {
    let mut extra = vec!["--review".to_owned(), arguments.review.clone()];
    let task_input = if let Some(path) = &arguments.file {
        let source = read_utf8_file(path)?;
        let digest = sha256_digest(source.as_bytes());
        require_matching_digest(arguments.file_digest.as_deref(), &digest)?;
        extra.extend([
            "--file".to_owned(),
            path.to_string_lossy().into_owned(),
            "--file-digest".to_owned(),
            digest,
        ]);
        Some(parse_task_input(&source)?)
    } else {
        if arguments.file_digest.is_some() {
            return Err(MinoError::new(
                ErrorCategory::IncompleteOrValidation,
                "--file-digest requires --file",
            ));
        }
        None
    };
    let command = mutation_command(&["rework"], &arguments.mutation, extra);
    let request = mutation_request(arguments.mutation, command)?;
    let report = service.rework(request, arguments.review, task_input)?;
    response_with_guidance(start, "Review rework started.", report)
}

fn resolve(
    start: &Path,
    service: &ReviewService,
    arguments: ResolveArguments,
) -> Result<CommandResponse, MinoError> {
    let command = mutation_command(
        &["resolve"],
        &arguments.mutation,
        vec!["--review".to_owned(), arguments.review.clone()],
    );
    let request = mutation_request(arguments.mutation, command)?;
    let report = service.resolve(request, arguments.review)?;
    response_with_guidance(start, "Review rework resolved.", report)
}

fn disposition(
    start: &Path,
    service: &ReviewService,
    arguments: DispositionArguments,
) -> Result<CommandResponse, MinoError> {
    let decision_name = arguments
        .decision
        .to_possible_value()
        .map_or_else(|| "unknown".to_owned(), |value| value.get_name().to_owned());
    let command = mutation_command(
        &["disposition"],
        &arguments.mutation,
        vec![
            "--review".to_owned(),
            arguments.review.clone(),
            "--decision".to_owned(),
            decision_name,
            "--decision-ref".to_owned(),
            arguments.decision_ref.clone(),
            "--reason".to_owned(),
            arguments.reason.clone(),
        ],
    );
    let request = mutation_request(arguments.mutation, command)?;
    let report = service.disposition(
        request,
        arguments.review,
        arguments.decision.into(),
        arguments.decision_ref,
        arguments.reason,
    )?;
    response_with_guidance(start, "Material review disposition recorded.", report)
}

fn accept(
    start: &Path,
    service: &ReviewService,
    arguments: AcceptArguments,
) -> Result<CommandResponse, MinoError> {
    let command = mutation_command(
        &["accept"],
        &arguments.mutation,
        vec!["--approval-ref".to_owned(), arguments.approval_ref.clone()],
    );
    let request = mutation_request(arguments.mutation, command)?;
    let report = service.accept(request, arguments.approval_ref)?;
    response_with_guidance(start, "Reviewed plan accepted and completed.", report)
}

fn parse_task_input(source: &str) -> Result<DraftTaskInput, MinoError> {
    serde_saphyr::from_str(source).map_err(|error| {
        MinoError::new(
            ErrorCategory::IncompleteOrValidation,
            format!("Failed to parse strict review-task YAML: {error}"),
        )
    })
}

fn require_matching_digest(provided: Option<&str>, actual: &str) -> Result<(), MinoError> {
    if provided.is_none_or(|provided| provided == actual) {
        Ok(())
    } else {
        Err(MinoError::new(
            ErrorCategory::RevisionConflict,
            "Review-task input digest does not match the supplied file",
        ))
    }
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
    let mut command = vec!["mino".to_owned(), "review".to_owned()];
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
            format!("Failed to serialize review result: {error}"),
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

fn parse_plan_id(value: &str) -> Result<PlanId, MinoError> {
    PlanId::parse(value).map_err(|error| domain_error(&error))
}

fn parse_task_id(value: &str) -> Result<TaskId, MinoError> {
    TaskId::parse(value).map_err(|error| domain_error(&error))
}

fn parse_request_id(value: &str) -> Result<RequestId, MinoError> {
    RequestId::parse(value).map_err(|error| domain_error(&error))
}

fn domain_error(error: &crate::domain::DomainError) -> MinoError {
    MinoError::new(ErrorCategory::IncompleteOrValidation, error.to_string())
}
