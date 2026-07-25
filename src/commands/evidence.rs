//! Supplemental evidence CLI adapter.

use std::path::{Path, PathBuf};

use clap::{Args, Subcommand, ValueEnum};
use serde::Serialize;

use crate::commands::CommandResponse;
use crate::domain::{CriterionId, EvidenceId, EvidenceType, PlanId, RequestId, TaskId, Timestamp};
use crate::evidence::{
    AddEvidenceRequest, EvidenceError, EvidenceErrorKind, EvidenceRequestContext, EvidenceSource,
    EvidenceStore,
};
use crate::project;
use crate::runner::Redactor;
use crate::{ErrorCategory, MinoError};

#[derive(Debug, Subcommand)]
pub(crate) enum EvidenceAction {
    /// Add one immutable supplemental evidence record.
    Add(AddArguments),
    /// List evidence in monotonic identifier order.
    List(ListArguments),
    /// Show one immutable evidence record.
    Show(ShowArguments),
}

#[derive(Debug, Args)]
pub(crate) struct AddArguments {
    /// Owning plan identifier.
    #[arg(long)]
    plan: String,
    /// Optional task binding.
    #[arg(long)]
    task: Option<String>,
    /// Optional acceptance-criterion binding.
    #[arg(long)]
    criterion: Option<String>,
    /// Supplemental evidence kind.
    #[arg(long = "type", value_enum)]
    kind: EvidenceKindArgument,
    /// Project-relative artifact path.
    #[arg(long)]
    path: Option<PathBuf>,
    /// URL, commit, or approval reference.
    #[arg(long)]
    reference: Option<String>,
    /// Human-readable observation or artifact description.
    #[arg(long)]
    description: String,
    /// Prior evidence corrected by this immutable record.
    #[arg(long)]
    supersedes: Option<String>,
    /// Required optimistic-concurrency revision.
    #[arg(long)]
    expect_revision: u64,
    /// Idempotency UUID for this evidence mutation.
    #[arg(long)]
    request_id: String,
    /// Actor responsible for this observation.
    #[arg(long, default_value = "user")]
    actor: String,
}

#[derive(Debug, Args)]
pub(crate) struct ListArguments {
    /// Owning plan identifier.
    #[arg(long)]
    plan: String,
    /// Optional exact task filter.
    #[arg(long)]
    task: Option<String>,
    /// Optional exact evidence-kind filter.
    #[arg(long = "type", value_enum)]
    kind: Option<EvidenceKindArgument>,
}

#[derive(Debug, Args)]
pub(crate) struct ShowArguments {
    /// Owning plan identifier.
    #[arg(long)]
    plan: String,
    /// Evidence identifier to load.
    #[arg(long)]
    evidence: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum EvidenceKindArgument {
    Command,
    File,
    GitDiff,
    Commit,
    Url,
    Log,
    Screenshot,
    ManualObservation,
    AcceptedException,
}

impl From<EvidenceKindArgument> for EvidenceType {
    fn from(value: EvidenceKindArgument) -> Self {
        match value {
            EvidenceKindArgument::Command => Self::Command,
            EvidenceKindArgument::File => Self::File,
            EvidenceKindArgument::GitDiff => Self::GitDiff,
            EvidenceKindArgument::Commit => Self::Commit,
            EvidenceKindArgument::Url => Self::Url,
            EvidenceKindArgument::Log => Self::Log,
            EvidenceKindArgument::Screenshot => Self::Screenshot,
            EvidenceKindArgument::ManualObservation => Self::ManualObservation,
            EvidenceKindArgument::AcceptedException => Self::AcceptedException,
        }
    }
}

pub(crate) fn execute(start: &Path, action: EvidenceAction) -> Result<CommandResponse, MinoError> {
    let root = project::discover(start)?;
    let store = EvidenceStore::new(root.path());
    match action {
        EvidenceAction::Add(arguments) => add(&store, arguments),
        EvidenceAction::List(arguments) => list(&store, &arguments),
        EvidenceAction::Show(arguments) => show(&store, &arguments),
    }
}

fn add(store: &EvidenceStore, arguments: AddArguments) -> Result<CommandResponse, MinoError> {
    let plan_id = parse_plan_id(&arguments.plan)?;
    let request_id = parse_request_id(&arguments.request_id)?;
    let kind = EvidenceType::from(arguments.kind);
    let source = evidence_source(
        kind,
        arguments.path.as_ref(),
        arguments.reference.as_deref(),
    )?;
    let command = add_command(&arguments);
    let context = EvidenceRequestContext::new(
        plan_id,
        arguments.expect_revision,
        request_id,
        arguments.actor,
        command,
        Timestamp::now_utc(),
    )
    .map_err(map_evidence_error)?;
    let mut request = AddEvidenceRequest::new(context, kind, source, arguments.description)
        .map_err(map_evidence_error)?;
    request = match (arguments.task, arguments.criterion) {
        (Some(task), Some(criterion)) => {
            request.with_criterion(parse_task_id(&task)?, parse_criterion_id(&criterion)?)
        }
        (Some(task), None) => request.with_task(parse_task_id(&task)?),
        (None, Some(_)) => {
            return Err(MinoError::new(
                ErrorCategory::IncompleteOrValidation,
                "--criterion requires --task",
            ));
        }
        (None, None) => request,
    };
    if let Some(supersedes) = arguments.supersedes {
        request = request.superseding(parse_evidence_id(&supersedes)?);
    }
    response(
        "Evidence captured.",
        store
            .add(&request, &Redactor::default())
            .map_err(map_evidence_error)?,
    )
}

fn list(store: &EvidenceStore, arguments: &ListArguments) -> Result<CommandResponse, MinoError> {
    let plan_id = parse_plan_id(&arguments.plan)?;
    let task_id = arguments.task.as_deref().map(parse_task_id).transpose()?;
    let kind = arguments.kind.map(EvidenceType::from);
    let evidence = store
        .list(&plan_id)
        .map_err(map_evidence_error)?
        .into_iter()
        .filter(|evidence| {
            task_id
                .as_ref()
                .is_none_or(|task_id| evidence.task_id() == Some(task_id))
                && kind.is_none_or(|kind| evidence.kind() == kind)
        })
        .collect::<Vec<_>>();
    response(
        "Evidence listed.",
        serde_json::json!({ "evidence": evidence }),
    )
}

fn show(store: &EvidenceStore, arguments: &ShowArguments) -> Result<CommandResponse, MinoError> {
    response(
        "Evidence loaded.",
        store
            .show(
                &parse_plan_id(&arguments.plan)?,
                &parse_evidence_id(&arguments.evidence)?,
            )
            .map_err(map_evidence_error)?,
    )
}

fn evidence_source(
    kind: EvidenceType,
    path: Option<&PathBuf>,
    reference: Option<&str>,
) -> Result<EvidenceSource, MinoError> {
    match (kind, path, reference) {
        (
            EvidenceType::File
            | EvidenceType::GitDiff
            | EvidenceType::Log
            | EvidenceType::Screenshot,
            Some(path),
            None,
        ) => Ok(EvidenceSource::Artifact(path.clone())),
        (
            EvidenceType::Commit | EvidenceType::Url | EvidenceType::AcceptedException,
            None,
            Some(reference),
        ) => Ok(EvidenceSource::Reference(reference.to_owned())),
        (EvidenceType::ManualObservation, None, None) => Ok(EvidenceSource::Observation),
        (EvidenceType::Command, _, _) => Err(MinoError::new(
            ErrorCategory::PolicyViolation,
            "Command evidence can only be created by mino exec check run",
        )),
        _ => Err(MinoError::new(
            ErrorCategory::IncompleteOrValidation,
            "Evidence type requires exactly its compatible --path or --reference input",
        )),
    }
}

fn add_command(arguments: &AddArguments) -> Vec<String> {
    let mut command = vec![
        "mino".to_owned(),
        "evidence".to_owned(),
        "add".to_owned(),
        "--plan".to_owned(),
        arguments.plan.clone(),
        "--type".to_owned(),
        arguments
            .kind
            .to_possible_value()
            .map_or_else(|| "unknown".to_owned(), |value| value.get_name().to_owned()),
        "--description".to_owned(),
        arguments.description.clone(),
        "--expect-revision".to_owned(),
        arguments.expect_revision.to_string(),
        "--request-id".to_owned(),
        arguments.request_id.clone(),
        "--actor".to_owned(),
        arguments.actor.clone(),
    ];
    if let Some(task) = &arguments.task {
        command.extend(["--task".to_owned(), task.clone()]);
    }
    if let Some(criterion) = &arguments.criterion {
        command.extend(["--criterion".to_owned(), criterion.clone()]);
    }
    if let Some(path) = &arguments.path {
        command.extend([
            "--path".to_owned(),
            path.to_string_lossy().replace('\\', "/"),
        ]);
    }
    if let Some(reference) = &arguments.reference {
        command.extend(["--reference".to_owned(), reference.clone()]);
    }
    if let Some(supersedes) = &arguments.supersedes {
        command.extend(["--supersedes".to_owned(), supersedes.clone()]);
    }
    command
}

fn response<T: Serialize>(
    message: impl Into<String>,
    payload: T,
) -> Result<CommandResponse, MinoError> {
    let payload = serde_json::to_value(payload).map_err(|error| {
        MinoError::new(
            ErrorCategory::EnvironmentUnavailable,
            format!("Failed to serialize evidence result: {error}"),
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

fn parse_plan_id(value: &str) -> Result<PlanId, MinoError> {
    PlanId::parse(value).map_err(|error| domain_error(&error))
}

fn parse_task_id(value: &str) -> Result<TaskId, MinoError> {
    TaskId::parse(value).map_err(|error| domain_error(&error))
}

fn parse_criterion_id(value: &str) -> Result<CriterionId, MinoError> {
    CriterionId::parse(value).map_err(|error| domain_error(&error))
}

fn parse_evidence_id(value: &str) -> Result<EvidenceId, MinoError> {
    EvidenceId::parse(value).map_err(|error| domain_error(&error))
}

fn parse_request_id(value: &str) -> Result<RequestId, MinoError> {
    RequestId::parse(value).map_err(|error| domain_error(&error))
}

fn domain_error(error: &crate::domain::DomainError) -> MinoError {
    MinoError::new(ErrorCategory::IncompleteOrValidation, error.to_string())
}

fn map_evidence_error(error: EvidenceError) -> MinoError {
    let category = match error.kind() {
        EvidenceErrorKind::InvalidRequest
        | EvidenceErrorKind::PlanNotFound
        | EvidenceErrorKind::EvidenceNotFound => ErrorCategory::IncompleteOrValidation,
        EvidenceErrorKind::RevisionConflict | EvidenceErrorKind::RequestConflict => {
            ErrorCategory::RevisionConflict
        }
        EvidenceErrorKind::CorruptStore => ErrorCategory::DriftDetected,
        EvidenceErrorKind::Io
        | EvidenceErrorKind::Serialization
        | EvidenceErrorKind::LockTimeout => ErrorCategory::EnvironmentUnavailable,
    };
    MinoError::new(category, error.into_message())
}
