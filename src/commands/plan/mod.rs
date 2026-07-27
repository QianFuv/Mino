//! Canonical plan authoring command tree and application adapter.

mod amend;
mod review;
mod variant;

use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};

use clap::{ArgAction, Args, Subcommand, ValueEnum};
use serde::Serialize;

use crate::application::plan::{
    CreatePlanRequest, DraftMutation, DraftMutationRequest, PlanMutationRequest, PlanService,
};
use crate::commands::CommandResponse;
use crate::domain::{
    CheckId, CriterionId, DraftCommitGateInput, DraftContextInput, DraftCriterionInput,
    DraftDecisionInput, DraftEdgeCaseInput, DraftFileInput, DraftMetadataInput, DraftScopeInput,
    DraftTaskInput, DraftTaskUpdateInput, DraftVerificationInput, FileChange, PlanId, RequestId,
    TaskId, Timestamp,
};
use crate::input::{read_utf8_file, read_utf8_stream, wizard, yaml};
use crate::store::sha256_digest;
use crate::{ErrorCategory, MinoError, NextAction};

#[derive(Debug, Subcommand)]
pub(crate) enum PlanAction {
    /// Initialize a revision-one Draft without claiming completion.
    Create(CreateArguments),
    /// Return deterministic missing fields and the next canonical action.
    Next(PlanReadArguments),
    /// Validate the current plan revision without mutating it.
    Validate(PlanReadArguments),
    /// Load the complete current source-of-truth plan.
    Show(PlanReadArguments),
    /// Validate and atomically move a complete Draft to Ready.
    Finalize(review::FinalizeArguments),
    /// Generate the revision-bound summary used at the approval gate.
    Review(PlanReadArguments),
    /// Record explicit plan approval and Git Flow consent.
    Approve(review::ApproveArguments),
    /// Strictly apply authored fields from one YAML document.
    Apply(ApplyArguments),
    /// Propose, approve, and apply typed protected changes after Draft.
    Amend(amend::AmendArguments),
    /// Create an independent Draft from one exact retained revision.
    Fork(variant::ForkArguments),
    /// Compare authored values across current or retained revisions.
    Diff(variant::DiffArguments),
    /// Record approval-bound deactivation without deleting plan history.
    Archive(variant::ArchiveArguments),
    /// Replace human plan metadata while Draft.
    Metadata(MetadataArguments),
    /// Replace the authored plan summary while Draft.
    Summary(SummaryArguments),
    /// Record the verified execution result and residual risk.
    Outcome(OutcomeArguments),
    /// Add current-state references while Draft.
    Context(ContextArguments),
    /// Set or append explicit plan scope boundaries.
    Scope(ScopeArguments),
    /// Add, update, or remove decisions while Draft.
    Decision(DecisionArguments),
    /// Update or remove authored edge cases while Draft.
    EdgeCase(EdgeCaseArguments),
    /// Add, update, remove, or order tasks and their authored details while Draft.
    Task(TaskArguments),
    /// Add, update, or remove task-owned file responsibilities while Draft.
    File(FileArguments),
    /// Add, update, or remove global verification commands while Draft.
    Verification(VerificationArguments),
}

#[derive(Debug, Args)]
pub(crate) struct CreateArguments {
    /// Human-readable requirement name.
    #[arg(long)]
    name: Option<String>,
    /// Planning trigger such as durable.
    #[arg(long)]
    trigger: Option<String>,
    /// UTF-8 file containing the exact original request.
    #[arg(long, conflicts_with = "stdin")]
    request_file: Option<PathBuf>,
    /// Read the exact original request from standard input.
    #[arg(long, conflicts_with = "request_file")]
    stdin: bool,
    /// Run the guided human authoring wizard.
    #[arg(long)]
    interactive: bool,
    /// Idempotency UUID for the creation event.
    #[arg(long)]
    request_id: String,
    /// Actor recorded in the event log.
    #[arg(long, default_value = "user")]
    actor: String,
    /// Canonical request digest accepted only for replayable normalized argv.
    #[arg(long, hide = true)]
    request_digest: Option<String>,
}

#[derive(Clone, Debug, Args)]
struct MutationArguments {
    /// Target plan identifier.
    #[arg(long)]
    plan: String,
    /// Required optimistic-concurrency revision.
    #[arg(long)]
    expect_revision: u64,
    /// Idempotency UUID for this semantic mutation.
    #[arg(long)]
    request_id: String,
    /// Actor recorded in the event log.
    #[arg(long, default_value = "user")]
    actor: String,
}

#[derive(Debug, Args)]
pub(crate) struct PlanReadArguments {
    /// Target plan identifier.
    #[arg(long)]
    plan: String,
}

#[derive(Debug, Args)]
pub(crate) struct ApplyArguments {
    #[command(flatten)]
    mutation: MutationArguments,
    /// Strict single-document authored Draft YAML file.
    #[arg(long)]
    file: PathBuf,
    /// Canonical file digest accepted only for normalized replay argv.
    #[arg(long, hide = true)]
    file_digest: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct MetadataArguments {
    #[command(subcommand)]
    action: MetadataAction,
}

#[derive(Debug, Subcommand)]
enum MetadataAction {
    /// Replace supplied metadata fields.
    Set(MetadataSetArguments),
}

#[derive(Debug, Args)]
struct MetadataSetArguments {
    #[command(flatten)]
    mutation: MutationArguments,
    #[arg(long)]
    name: Option<String>,
    #[arg(long)]
    priority: Option<String>,
    #[arg(long = "type")]
    plan_type: Option<String>,
    #[arg(long)]
    area: Option<String>,
    #[arg(long)]
    owner: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct SummaryArguments {
    #[command(subcommand)]
    action: SummaryAction,
}

#[derive(Debug, Subcommand)]
enum SummaryAction {
    /// Replace the complete plan summary.
    Set(SummarySetArguments),
}

#[derive(Debug, Args)]
struct SummarySetArguments {
    #[command(flatten)]
    mutation: MutationArguments,
    /// Summary supplied directly as one argument.
    #[arg(long, conflicts_with = "stdin", allow_hyphen_values = true)]
    value: Option<String>,
    /// Read the summary from standard input.
    #[arg(long, conflicts_with = "value")]
    stdin: bool,
    /// Canonical stdin digest accepted only for normalized replay argv.
    #[arg(long, hide = true)]
    input_digest: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct OutcomeArguments {
    #[command(subcommand)]
    action: OutcomeAction,
}

#[derive(Debug, Subcommand)]
enum OutcomeAction {
    /// Set the complete Final Outcome after global verification passes.
    Set(OutcomeSetArguments),
}

#[derive(Debug, Args)]
struct OutcomeSetArguments {
    #[command(flatten)]
    mutation: MutationArguments,
    /// User-visible result of the completed execution.
    #[arg(long, allow_hyphen_values = true)]
    summary: String,
    /// Explicit residual risk statement, including N/A when none remains.
    #[arg(long, allow_hyphen_values = true)]
    remaining_risk: String,
    /// Optional non-blocking follow-up task; repeat for multiple tasks.
    #[arg(long = "follow-up", allow_hyphen_values = true)]
    follow_ups: Vec<String>,
}

#[derive(Debug, Args)]
pub(crate) struct ContextArguments {
    #[command(subcommand)]
    action: ContextAction,
}

#[derive(Debug, Subcommand)]
enum ContextAction {
    /// Append one current-state reference.
    Add(ContextAddArguments),
}

#[derive(Debug, Args)]
struct ContextAddArguments {
    #[command(flatten)]
    mutation: MutationArguments,
    #[arg(long)]
    reference: String,
    #[arg(long)]
    fact: String,
    #[arg(long)]
    implication: String,
}

#[derive(Debug, Args)]
pub(crate) struct ScopeArguments {
    #[command(subcommand)]
    action: ScopeAction,
}

#[derive(Debug, Subcommand)]
enum ScopeAction {
    /// Replace one or more complete scope fields.
    Set(ScopeSetArguments),
    /// Append one value to a scope list.
    Add(ScopeAddArguments),
}

#[derive(Debug, Args)]
struct ScopeSetArguments {
    #[command(flatten)]
    mutation: MutationArguments,
    #[arg(long)]
    goal: Option<String>,
    #[arg(long = "deliverable")]
    deliverables: Vec<String>,
    #[arg(long = "in-scope")]
    in_scope: Vec<String>,
    #[arg(long = "out-of-scope")]
    out_of_scope: Vec<String>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum ScopeSection {
    Deliverable,
    InScope,
    OutOfScope,
}

#[derive(Debug, Args)]
struct ScopeAddArguments {
    #[command(flatten)]
    mutation: MutationArguments,
    #[arg(long, value_enum)]
    section: ScopeSection,
    #[arg(long)]
    value: String,
}

#[derive(Debug, Args)]
pub(crate) struct DecisionArguments {
    #[command(subcommand)]
    action: DecisionAction,
}

#[derive(Debug, Subcommand)]
enum DecisionAction {
    /// Append one decision, assumption, or question.
    Add(DecisionAddArguments),
    /// Replace one decision, assumption, or question by one-based position.
    Update(DecisionUpdateArguments),
    /// Remove one decision, assumption, or question by one-based position.
    Remove(PositionMutationArguments),
}

#[derive(Debug, Args)]
struct DecisionAddArguments {
    #[command(flatten)]
    mutation: MutationArguments,
    #[arg(long)]
    item: String,
    #[arg(long = "type")]
    kind: String,
    #[arg(long = "decision")]
    value: String,
    #[arg(long)]
    reason: String,
    #[arg(long)]
    status: String,
}

#[derive(Debug, Args)]
struct DecisionUpdateArguments {
    #[command(flatten)]
    mutation: MutationArguments,
    #[arg(long)]
    position: usize,
    #[arg(long)]
    item: String,
    #[arg(long = "type")]
    kind: String,
    #[arg(long = "decision")]
    value: String,
    #[arg(long)]
    reason: String,
    #[arg(long)]
    status: String,
}

#[derive(Debug, Args)]
pub(crate) struct EdgeCaseArguments {
    #[command(subcommand)]
    action: EdgeCaseAction,
}

#[derive(Debug, Subcommand)]
enum EdgeCaseAction {
    /// Replace one edge case by one-based position.
    Update(EdgeCaseUpdateArguments),
    /// Remove one edge case by one-based position.
    Remove(PositionMutationArguments),
}

#[derive(Debug, Args)]
struct EdgeCaseUpdateArguments {
    #[command(flatten)]
    mutation: MutationArguments,
    #[arg(long)]
    position: usize,
    #[arg(long = "case")]
    case_: String,
    #[arg(long)]
    expected_behavior: String,
    #[arg(long = "covered-by")]
    covered_by: Vec<String>,
}

#[derive(Debug, Args)]
struct PositionMutationArguments {
    #[command(flatten)]
    mutation: MutationArguments,
    #[arg(long)]
    position: usize,
}

#[derive(Debug, Args)]
pub(crate) struct TaskArguments {
    #[command(subcommand)]
    action: TaskAction,
}

#[derive(Debug, Subcommand)]
enum TaskAction {
    /// Append one deterministically identified task.
    Add(TaskAddArguments),
    /// Replace supplied fields on one existing task.
    Update(TaskUpdateArguments),
    /// Remove one unreferenced task.
    Remove(TaskMutationArguments),
    /// Move one task to a one-based implementation position.
    Move(TaskMoveArguments),
    /// Author ordered task steps.
    Step(TaskStepArguments),
    /// Author task acceptance criteria.
    Criterion(TaskCriterionArguments),
    /// Author task verification commands.
    Verification(TaskVerificationArguments),
}

#[derive(Debug, Args)]
struct TaskAddArguments {
    #[command(flatten)]
    mutation: MutationArguments,
    #[arg(long)]
    title: String,
    #[arg(long = "depends-on")]
    depends_on: Vec<String>,
    #[arg(long)]
    commit_required: bool,
    #[arg(long)]
    planned_commit_message: Option<String>,
    #[arg(long = "commit-scope")]
    commit_scope: Vec<String>,
}

#[derive(Debug, Args)]
struct TaskUpdateArguments {
    #[command(flatten)]
    mutation: MutationArguments,
    #[arg(long)]
    task: String,
    #[arg(long)]
    title: Option<String>,
    #[arg(long = "depends-on", conflicts_with = "clear_dependencies")]
    depends_on: Vec<String>,
    #[arg(long, conflicts_with = "depends_on")]
    clear_dependencies: bool,
    #[arg(long, action = ArgAction::Set, conflicts_with = "clear_commit_gate")]
    commit_required: Option<bool>,
    #[arg(long, conflicts_with = "clear_commit_gate")]
    planned_commit_message: Option<String>,
    #[arg(long = "commit-scope", conflicts_with = "clear_commit_gate")]
    commit_scope: Vec<String>,
    #[arg(
        long,
        conflicts_with_all = ["commit_required", "planned_commit_message", "commit_scope"]
    )]
    clear_commit_gate: bool,
}

#[derive(Debug, Args)]
struct TaskMutationArguments {
    #[command(flatten)]
    mutation: MutationArguments,
    #[arg(long)]
    task: String,
}

#[derive(Debug, Args)]
struct TaskMoveArguments {
    #[command(flatten)]
    mutation: MutationArguments,
    #[arg(long)]
    task: String,
    #[arg(long)]
    position: usize,
}

#[derive(Debug, Args)]
struct TaskStepArguments {
    #[command(subcommand)]
    action: TaskStepAction,
}

#[derive(Debug, Subcommand)]
enum TaskStepAction {
    /// Append one ordered implementation step.
    Add(TaskStepAddArguments),
    /// Replace one ordered implementation step by one-based position.
    Update(TaskStepUpdateArguments),
    /// Remove one ordered implementation step by one-based position.
    Remove(TaskPositionMutationArguments),
}

#[derive(Debug, Args)]
struct TaskStepAddArguments {
    #[command(flatten)]
    mutation: MutationArguments,
    #[arg(long)]
    task: String,
    #[arg(long)]
    value: String,
}

#[derive(Debug, Args)]
struct TaskStepUpdateArguments {
    #[command(flatten)]
    mutation: MutationArguments,
    #[arg(long)]
    task: String,
    #[arg(long)]
    position: usize,
    #[arg(long)]
    value: String,
}

#[derive(Debug, Args)]
struct TaskPositionMutationArguments {
    #[command(flatten)]
    mutation: MutationArguments,
    #[arg(long)]
    task: String,
    #[arg(long)]
    position: usize,
}

#[derive(Debug, Args)]
struct TaskCriterionArguments {
    #[command(subcommand)]
    action: TaskCriterionAction,
}

#[derive(Debug, Subcommand)]
enum TaskCriterionAction {
    /// Append the next deterministic acceptance criterion.
    Add(TaskCriterionAddArguments),
    /// Replace one acceptance criterion by stable identifier.
    Update(TaskCriterionUpdateArguments),
    /// Remove one acceptance criterion by stable identifier.
    Remove(TaskCriterionMutationArguments),
}

#[derive(Debug, Args)]
struct TaskCriterionAddArguments {
    #[command(flatten)]
    mutation: MutationArguments,
    #[arg(long)]
    task: String,
    #[arg(long)]
    description: String,
}

#[derive(Debug, Args)]
struct TaskCriterionUpdateArguments {
    #[command(flatten)]
    mutation: MutationArguments,
    #[arg(long)]
    task: String,
    #[arg(long)]
    criterion: String,
    #[arg(long)]
    description: String,
}

#[derive(Debug, Args)]
struct TaskCriterionMutationArguments {
    #[command(flatten)]
    mutation: MutationArguments,
    #[arg(long)]
    task: String,
    #[arg(long)]
    criterion: String,
}

#[derive(Debug, Args)]
struct TaskVerificationArguments {
    #[command(subcommand)]
    action: TaskVerificationAction,
}

#[derive(Debug, Subcommand)]
enum TaskVerificationAction {
    /// Append one task-scoped verification command.
    Add(TaskVerificationAddArguments),
    /// Replace one task-scoped verification command by stable identifier.
    Update(TaskVerificationUpdateArguments),
    /// Remove one task-scoped verification command by stable identifier.
    Remove(TaskVerificationMutationArguments),
}

#[derive(Debug, Args)]
struct TaskVerificationAddArguments {
    #[command(flatten)]
    mutation: MutationArguments,
    #[arg(long)]
    task: String,
    #[command(flatten)]
    verification: VerificationInputArguments,
}

#[derive(Debug, Args)]
struct TaskVerificationUpdateArguments {
    #[command(flatten)]
    mutation: MutationArguments,
    #[arg(long)]
    task: String,
    #[arg(long)]
    check: String,
    #[command(flatten)]
    definition: VerificationDefinitionArguments,
}

#[derive(Debug, Args)]
struct TaskVerificationMutationArguments {
    #[command(flatten)]
    mutation: MutationArguments,
    #[arg(long)]
    task: String,
    #[arg(long)]
    check: String,
}

#[derive(Debug, Args)]
pub(crate) struct FileArguments {
    #[command(subcommand)]
    action: FileAction,
}

#[derive(Debug, Subcommand)]
enum FileAction {
    /// Append one task-owned file responsibility.
    Add(FileAddArguments),
    /// Replace one task-owned file responsibility by one-based position.
    Update(FileUpdateArguments),
    /// Remove one task-owned file responsibility by one-based position.
    Remove(TaskPositionMutationArguments),
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum FileChangeArgument {
    Create,
    Modify,
    Delete,
    Test,
    NotApplicable,
}

impl From<FileChangeArgument> for FileChange {
    fn from(value: FileChangeArgument) -> Self {
        match value {
            FileChangeArgument::Create => Self::Create,
            FileChangeArgument::Modify => Self::Modify,
            FileChangeArgument::Delete => Self::Delete,
            FileChangeArgument::Test => Self::Test,
            FileChangeArgument::NotApplicable => Self::NotApplicable,
        }
    }
}

#[derive(Debug, Args)]
struct FileAddArguments {
    #[command(flatten)]
    mutation: MutationArguments,
    #[arg(long)]
    task: String,
    #[arg(long)]
    path: String,
    #[arg(long, value_enum)]
    change: FileChangeArgument,
    #[arg(long)]
    reason: String,
}

#[derive(Debug, Args)]
struct FileUpdateArguments {
    #[command(flatten)]
    mutation: MutationArguments,
    #[arg(long)]
    task: String,
    #[arg(long)]
    position: usize,
    #[arg(long)]
    path: String,
    #[arg(long, value_enum)]
    change: FileChangeArgument,
    #[arg(long)]
    reason: String,
}

#[derive(Debug, Args)]
pub(crate) struct VerificationArguments {
    #[command(subcommand)]
    action: VerificationAction,
}

#[derive(Debug, Subcommand)]
enum VerificationAction {
    /// Append one global verification command.
    Add(VerificationAddArguments),
    /// Replace one global verification command by stable identifier.
    Update(VerificationUpdateArguments),
    /// Remove one global verification command by stable identifier.
    Remove(VerificationMutationArguments),
}

#[derive(Debug, Args)]
struct VerificationAddArguments {
    #[command(flatten)]
    mutation: MutationArguments,
    #[command(flatten)]
    verification: VerificationInputArguments,
}

#[derive(Debug, Args)]
struct VerificationUpdateArguments {
    #[command(flatten)]
    mutation: MutationArguments,
    #[arg(long)]
    check: String,
    #[command(flatten)]
    definition: VerificationDefinitionArguments,
}

#[derive(Debug, Args)]
struct VerificationMutationArguments {
    #[command(flatten)]
    mutation: MutationArguments,
    #[arg(long)]
    check: String,
}

#[derive(Debug, Args)]
struct VerificationInputArguments {
    #[arg(long)]
    id: String,
    #[command(flatten)]
    definition: VerificationDefinitionArguments,
}

#[derive(Debug, Args)]
struct VerificationDefinitionArguments {
    #[arg(long = "command", required = true, allow_hyphen_values = true)]
    command: Vec<String>,
    #[arg(long, default_value = ".")]
    cwd: String,
    #[arg(long, default_value_t = 0)]
    expected_exit_code: i32,
    #[arg(long, default_value_t = true, action = ArgAction::Set)]
    required: bool,
}

pub(crate) fn execute(
    start: &Path,
    action: PlanAction,
    no_input: bool,
) -> Result<CommandResponse, MinoError> {
    let service = PlanService::discover(start)?;
    match action {
        PlanAction::Create(arguments) => execute_create(&service, arguments, no_input),
        PlanAction::Next(arguments) => execute_next(&service, &arguments),
        PlanAction::Validate(arguments) => execute_validate(&service, &arguments),
        PlanAction::Show(arguments) => review::execute_show(&service, &arguments),
        PlanAction::Finalize(arguments) => review::execute_finalize(&service, arguments),
        PlanAction::Review(arguments) => review::execute_review(&service, &arguments),
        PlanAction::Approve(arguments) => review::execute_approve(&service, arguments),
        PlanAction::Apply(arguments) => execute_apply(&service, arguments),
        PlanAction::Amend(arguments) => amend::execute(start, arguments),
        PlanAction::Fork(arguments) => variant::execute_fork(start, &service, arguments),
        PlanAction::Diff(arguments) => variant::execute_diff(start, &arguments),
        PlanAction::Archive(arguments) => variant::execute_archive(start, arguments),
        PlanAction::Metadata(arguments) => match arguments.action {
            MetadataAction::Set(arguments) => execute_metadata(&service, arguments),
        },
        PlanAction::Summary(arguments) => match arguments.action {
            SummaryAction::Set(arguments) => execute_summary(&service, arguments),
        },
        PlanAction::Outcome(arguments) => match arguments.action {
            OutcomeAction::Set(arguments) => execute_outcome(&service, arguments),
        },
        PlanAction::Context(arguments) => match arguments.action {
            ContextAction::Add(arguments) => execute_context(&service, arguments),
        },
        PlanAction::Scope(arguments) => match arguments.action {
            ScopeAction::Set(arguments) => execute_scope_set(&service, arguments),
            ScopeAction::Add(arguments) => execute_scope_add(&service, arguments),
        },
        PlanAction::Decision(arguments) => match arguments.action {
            DecisionAction::Add(arguments) => execute_decision(&service, arguments),
            DecisionAction::Update(arguments) => execute_decision_update(&service, arguments),
            DecisionAction::Remove(arguments) => execute_decision_remove(&service, arguments),
        },
        PlanAction::EdgeCase(arguments) => match arguments.action {
            EdgeCaseAction::Update(arguments) => execute_edge_case_update(&service, arguments),
            EdgeCaseAction::Remove(arguments) => execute_edge_case_remove(&service, arguments),
        },
        PlanAction::Task(arguments) => execute_task(&service, arguments.action),
        PlanAction::File(arguments) => match arguments.action {
            FileAction::Add(arguments) => execute_file(&service, arguments),
            FileAction::Update(arguments) => execute_file_update(&service, arguments),
            FileAction::Remove(arguments) => execute_file_remove(&service, arguments),
        },
        PlanAction::Verification(arguments) => match arguments.action {
            VerificationAction::Add(arguments) => execute_global_verification(&service, arguments),
            VerificationAction::Update(arguments) => {
                execute_global_verification_update(&service, arguments)
            }
            VerificationAction::Remove(arguments) => {
                execute_global_verification_remove(&service, arguments)
            }
        },
    }
}

fn execute_create(
    service: &PlanService,
    arguments: CreateArguments,
    no_input: bool,
) -> Result<CommandResponse, MinoError> {
    let request_id = parse_request_id(&arguments.request_id)?;
    let (name, trigger, original_request, source_marker) = if arguments.interactive {
        if no_input {
            return Err(MinoError::new(
                ErrorCategory::PolicyViolation,
                "--interactive cannot be combined with --no-input",
            ));
        }
        if arguments.name.is_some()
            || arguments.trigger.is_some()
            || arguments.request_file.is_some()
            || arguments.stdin
        {
            return Err(validation_error(
                "Interactive create cannot be combined with direct authored inputs",
            ));
        }
        if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
            return Err(MinoError::new(
                ErrorCategory::EnvironmentUnavailable,
                "Interactive plan creation requires a terminal; use direct arguments in agent mode",
            ));
        }
        let mut stdin = io::stdin().lock();
        let mut stderr = io::stderr().lock();
        let collected = wizard::collect(&mut stdin, &mut stderr)?.ok_or_else(|| {
            validation_error("Interactive plan creation was cancelled before writing")
        })?;
        (
            collected.name,
            collected.trigger,
            collected.original_request,
            "--stdin".to_owned(),
        )
    } else {
        let name = arguments
            .name
            .ok_or_else(|| validation_error("Plan create requires --name"))?;
        let trigger = arguments
            .trigger
            .ok_or_else(|| validation_error("Plan create requires --trigger"))?;
        let (request, marker) = match (arguments.request_file, arguments.stdin) {
            (Some(path), false) => (
                read_utf8_file(&path)?,
                format!("--request-file={}", path.display()),
            ),
            (None, true) => {
                let mut stdin = io::stdin().lock();
                (read_utf8_stream(&mut stdin)?, "--stdin".to_owned())
            }
            _ => {
                return Err(validation_error(
                    "Plan create requires exactly one of --request-file or --stdin",
                ));
            }
        };
        (name, trigger, request, marker)
    };
    let request_digest = sha256_digest(original_request.as_bytes());
    require_matching_digest(arguments.request_digest.as_deref(), &request_digest)?;
    let command = vec![
        "mino".to_owned(),
        "plan".to_owned(),
        "create".to_owned(),
        "--name".to_owned(),
        name.clone(),
        "--trigger".to_owned(),
        trigger.clone(),
        source_marker,
        "--request-digest".to_owned(),
        request_digest,
        "--request-id".to_owned(),
        request_id.to_string(),
        "--actor".to_owned(),
        arguments.actor.clone(),
    ];
    let report = service.create(CreatePlanRequest {
        name,
        trigger,
        original_request,
        request_id,
        actor: arguments.actor,
        command,
        created_at: Timestamp::now_utc(),
    })?;
    operation_response(service, "Plan draft initialized.", report)
}

fn execute_next(
    service: &PlanService,
    arguments: &PlanReadArguments,
) -> Result<CommandResponse, MinoError> {
    let plan_id = parse_plan_id(&arguments.plan)?;
    let report = service.next(&plan_id)?;
    let missing = report.missing.clone();
    let next_actions = report.next_actions.clone();
    response(
        "Next plan action resolved.",
        false,
        report,
        missing,
        next_actions,
    )
}

fn execute_validate(
    service: &PlanService,
    arguments: &PlanReadArguments,
) -> Result<CommandResponse, MinoError> {
    let plan_id = parse_plan_id(&arguments.plan)?;
    let report = service.validate(&plan_id)?;
    if !report.valid {
        return Err(crate::validation::validation_failure(&report));
    }
    let next_actions = report.next_actions.clone();
    response(
        "Plan validation passed.",
        false,
        report,
        Vec::new(),
        next_actions,
    )
}

fn execute_apply(
    service: &PlanService,
    arguments: ApplyArguments,
) -> Result<CommandResponse, MinoError> {
    let source = read_utf8_file(&arguments.file)?;
    let digest = sha256_digest(source.as_bytes());
    require_matching_digest(arguments.file_digest.as_deref(), &digest)?;
    let input = yaml::parse_draft(&source)?;
    let command = mutation_command(
        &["apply"],
        &arguments.mutation,
        vec![
            "--file".to_owned(),
            arguments.file.to_string_lossy().into_owned(),
            "--file-digest".to_owned(),
            digest,
        ],
    );
    execute_mutation(
        service,
        arguments.mutation,
        &DraftMutation::Apply(input),
        command,
        "Draft fields applied.",
    )
}

fn execute_metadata(
    service: &PlanService,
    arguments: MetadataSetArguments,
) -> Result<CommandResponse, MinoError> {
    let input = DraftMetadataInput {
        name: arguments.name,
        priority: arguments.priority,
        plan_type: arguments.plan_type,
        area: arguments.area,
        owner: arguments.owner,
    };
    let command = mutation_command(
        &["metadata", "set"],
        &arguments.mutation,
        metadata_argv(&input),
    );
    execute_mutation(
        service,
        arguments.mutation,
        &DraftMutation::Metadata(input),
        command,
        "Plan metadata updated.",
    )
}

fn execute_summary(
    service: &PlanService,
    arguments: SummarySetArguments,
) -> Result<CommandResponse, MinoError> {
    let summary = match (arguments.value, arguments.stdin) {
        (Some(value), false) => value,
        (None, true) => {
            let mut stdin = io::stdin().lock();
            trim_one_line_ending(read_utf8_stream(&mut stdin)?)
        }
        _ => {
            return Err(validation_error(
                "Summary set requires exactly one of --value or --stdin",
            ));
        }
    };
    let digest = sha256_digest(summary.as_bytes());
    require_matching_digest(arguments.input_digest.as_deref(), &digest)?;
    let command = mutation_command(
        &["summary", "set"],
        &arguments.mutation,
        vec![
            "--value".to_owned(),
            summary.clone(),
            "--input-digest".to_owned(),
            digest,
        ],
    );
    execute_mutation(
        service,
        arguments.mutation,
        &DraftMutation::Summary(summary),
        command,
        "Plan summary updated.",
    )
}

fn execute_outcome(
    service: &PlanService,
    arguments: OutcomeSetArguments,
) -> Result<CommandResponse, MinoError> {
    let mut extra = vec![
        "--summary".to_owned(),
        arguments.summary.clone(),
        "--remaining-risk".to_owned(),
        arguments.remaining_risk.clone(),
    ];
    append_repeated(&mut extra, "--follow-up", &arguments.follow_ups);
    let command = mutation_command(&["outcome", "set"], &arguments.mutation, extra);
    let request = PlanMutationRequest {
        plan_id: parse_plan_id(&arguments.mutation.plan)?,
        expected_revision: arguments.mutation.expect_revision,
        request_id: parse_request_id(&arguments.mutation.request_id)?,
        actor: arguments.mutation.actor,
        command,
        updated_at: Timestamp::now_utc(),
    };
    let report = service.set_outcome(
        request,
        arguments.summary,
        arguments.remaining_risk,
        arguments.follow_ups,
    )?;
    operation_response(service, "Final Outcome recorded.", report)
}

fn execute_context(
    service: &PlanService,
    arguments: ContextAddArguments,
) -> Result<CommandResponse, MinoError> {
    let input = DraftContextInput {
        reference: arguments.reference,
        fact: arguments.fact,
        implication: arguments.implication,
    };
    let command = mutation_command(
        &["context", "add"],
        &arguments.mutation,
        vec![
            "--reference".to_owned(),
            input.reference.clone(),
            "--fact".to_owned(),
            input.fact.clone(),
            "--implication".to_owned(),
            input.implication.clone(),
        ],
    );
    execute_mutation(
        service,
        arguments.mutation,
        &DraftMutation::Context(input),
        command,
        "Plan context reference added.",
    )
}

fn execute_scope_set(
    service: &PlanService,
    arguments: ScopeSetArguments,
) -> Result<CommandResponse, MinoError> {
    let input = DraftScopeInput {
        goal: arguments.goal,
        deliverables: (!arguments.deliverables.is_empty()).then_some(arguments.deliverables),
        in_scope: (!arguments.in_scope.is_empty()).then_some(arguments.in_scope),
        out_of_scope: (!arguments.out_of_scope.is_empty()).then_some(arguments.out_of_scope),
    };
    let command = mutation_command(&["scope", "set"], &arguments.mutation, scope_argv(&input));
    execute_mutation(
        service,
        arguments.mutation,
        &DraftMutation::Scope(input),
        command,
        "Plan scope updated.",
    )
}

fn execute_scope_add(
    service: &PlanService,
    arguments: ScopeAddArguments,
) -> Result<CommandResponse, MinoError> {
    let mutation = match arguments.section {
        ScopeSection::Deliverable => DraftMutation::AddDeliverable(arguments.value.clone()),
        ScopeSection::InScope => DraftMutation::AddInScope(arguments.value.clone()),
        ScopeSection::OutOfScope => DraftMutation::AddOutOfScope(arguments.value.clone()),
    };
    let section = match arguments.section {
        ScopeSection::Deliverable => "deliverable",
        ScopeSection::InScope => "in-scope",
        ScopeSection::OutOfScope => "out-of-scope",
    };
    let command = mutation_command(
        &["scope", "add"],
        &arguments.mutation,
        vec![
            "--section".to_owned(),
            section.to_owned(),
            "--value".to_owned(),
            arguments.value,
        ],
    );
    execute_mutation(
        service,
        arguments.mutation,
        &mutation,
        command,
        "Plan scope item added.",
    )
}

fn execute_decision(
    service: &PlanService,
    arguments: DecisionAddArguments,
) -> Result<CommandResponse, MinoError> {
    let input = DraftDecisionInput {
        item: arguments.item,
        kind: arguments.kind,
        value: arguments.value,
        reason: arguments.reason,
        status: arguments.status,
    };
    let command = mutation_command(
        &["decision", "add"],
        &arguments.mutation,
        vec![
            "--item".to_owned(),
            input.item.clone(),
            "--type".to_owned(),
            input.kind.clone(),
            "--decision".to_owned(),
            input.value.clone(),
            "--reason".to_owned(),
            input.reason.clone(),
            "--status".to_owned(),
            input.status.clone(),
        ],
    );
    execute_mutation(
        service,
        arguments.mutation,
        &DraftMutation::Decision(input),
        command,
        "Plan decision added.",
    )
}

fn execute_decision_update(
    service: &PlanService,
    arguments: DecisionUpdateArguments,
) -> Result<CommandResponse, MinoError> {
    let input = DraftDecisionInput {
        item: arguments.item,
        kind: arguments.kind,
        value: arguments.value,
        reason: arguments.reason,
        status: arguments.status,
    };
    let command = mutation_command(
        &["decision", "update"],
        &arguments.mutation,
        vec![
            "--position".to_owned(),
            arguments.position.to_string(),
            "--item".to_owned(),
            input.item.clone(),
            "--type".to_owned(),
            input.kind.clone(),
            "--decision".to_owned(),
            input.value.clone(),
            "--reason".to_owned(),
            input.reason.clone(),
            "--status".to_owned(),
            input.status.clone(),
        ],
    );
    execute_mutation(
        service,
        arguments.mutation,
        &DraftMutation::DecisionUpdate {
            position: arguments.position,
            decision: input,
        },
        command,
        "Plan decision updated.",
    )
}

fn execute_decision_remove(
    service: &PlanService,
    arguments: PositionMutationArguments,
) -> Result<CommandResponse, MinoError> {
    let command = mutation_command(
        &["decision", "remove"],
        &arguments.mutation,
        vec!["--position".to_owned(), arguments.position.to_string()],
    );
    execute_mutation(
        service,
        arguments.mutation,
        &DraftMutation::DecisionRemove {
            position: arguments.position,
        },
        command,
        "Plan decision removed.",
    )
}

fn execute_edge_case_update(
    service: &PlanService,
    arguments: EdgeCaseUpdateArguments,
) -> Result<CommandResponse, MinoError> {
    let input = DraftEdgeCaseInput {
        case_: arguments.case_,
        expected_behavior: arguments.expected_behavior,
        covered_by: arguments.covered_by,
    };
    let mut extra = vec![
        "--position".to_owned(),
        arguments.position.to_string(),
        "--case".to_owned(),
        input.case_.clone(),
        "--expected-behavior".to_owned(),
        input.expected_behavior.clone(),
    ];
    append_repeated(&mut extra, "--covered-by", &input.covered_by);
    let command = mutation_command(&["edge-case", "update"], &arguments.mutation, extra);
    execute_mutation(
        service,
        arguments.mutation,
        &DraftMutation::EdgeCaseUpdate {
            position: arguments.position,
            edge_case: input,
        },
        command,
        "Plan edge case updated.",
    )
}

fn execute_edge_case_remove(
    service: &PlanService,
    arguments: PositionMutationArguments,
) -> Result<CommandResponse, MinoError> {
    let command = mutation_command(
        &["edge-case", "remove"],
        &arguments.mutation,
        vec!["--position".to_owned(), arguments.position.to_string()],
    );
    execute_mutation(
        service,
        arguments.mutation,
        &DraftMutation::EdgeCaseRemove {
            position: arguments.position,
        },
        command,
        "Plan edge case removed.",
    )
}

fn execute_task(service: &PlanService, action: TaskAction) -> Result<CommandResponse, MinoError> {
    match action {
        TaskAction::Add(arguments) => execute_task_add(service, arguments),
        TaskAction::Update(arguments) => execute_task_update(service, arguments),
        TaskAction::Remove(arguments) => execute_task_remove(service, arguments),
        TaskAction::Move(arguments) => execute_task_move(service, arguments),
        TaskAction::Step(arguments) => match arguments.action {
            TaskStepAction::Add(arguments) => execute_task_step(service, arguments),
            TaskStepAction::Update(arguments) => execute_task_step_update(service, arguments),
            TaskStepAction::Remove(arguments) => execute_task_step_remove(service, arguments),
        },
        TaskAction::Criterion(arguments) => match arguments.action {
            TaskCriterionAction::Add(arguments) => execute_task_criterion(service, arguments),
            TaskCriterionAction::Update(arguments) => {
                execute_task_criterion_update(service, arguments)
            }
            TaskCriterionAction::Remove(arguments) => {
                execute_task_criterion_remove(service, arguments)
            }
        },
        TaskAction::Verification(arguments) => match arguments.action {
            TaskVerificationAction::Add(arguments) => execute_task_verification(service, arguments),
            TaskVerificationAction::Update(arguments) => {
                execute_task_verification_update(service, arguments)
            }
            TaskVerificationAction::Remove(arguments) => {
                execute_task_verification_remove(service, arguments)
            }
        },
    }
}

fn execute_task_add(
    service: &PlanService,
    arguments: TaskAddArguments,
) -> Result<CommandResponse, MinoError> {
    let depends_on = arguments
        .depends_on
        .iter()
        .map(|value| TaskId::parse(value).map_err(|error| domain_error(&error)))
        .collect::<Result<Vec<_>, _>>()?;
    let commit_gate = (arguments.commit_required
        || arguments.planned_commit_message.is_some()
        || !arguments.commit_scope.is_empty())
    .then(|| DraftCommitGateInput {
        required: arguments.commit_required,
        planned_message: arguments.planned_commit_message.unwrap_or_default(),
        scope: arguments.commit_scope,
    });
    let input = DraftTaskInput {
        id: None,
        title: arguments.title,
        depends_on,
        steps: Vec::new(),
        files: Vec::new(),
        acceptance_criteria: Vec::new(),
        verification: Vec::new(),
        commit_gate,
    };
    let mut extra = vec!["--title".to_owned(), input.title.clone()];
    for dependency in &input.depends_on {
        extra.extend(["--depends-on".to_owned(), dependency.to_string()]);
    }
    if let Some(gate) = &input.commit_gate {
        if gate.required {
            extra.push("--commit-required".to_owned());
        }
        extra.extend([
            "--planned-commit-message".to_owned(),
            gate.planned_message.clone(),
        ]);
        for scope in &gate.scope {
            extra.extend(["--commit-scope".to_owned(), scope.clone()]);
        }
    }
    let command = mutation_command(&["task", "add"], &arguments.mutation, extra);
    execute_mutation(
        service,
        arguments.mutation,
        &DraftMutation::Task(input),
        command,
        "Plan task added.",
    )
}

fn execute_task_update(
    service: &PlanService,
    arguments: TaskUpdateArguments,
) -> Result<CommandResponse, MinoError> {
    let task_id = parse_task_id(&arguments.task)?;
    let depends_on = if arguments.clear_dependencies {
        Some(Vec::new())
    } else if arguments.depends_on.is_empty() {
        None
    } else {
        Some(
            arguments
                .depends_on
                .iter()
                .map(|value| TaskId::parse(value).map_err(|error| domain_error(&error)))
                .collect::<Result<Vec<_>, _>>()?,
        )
    };
    let has_commit_gate = arguments.commit_required.is_some()
        || arguments.planned_commit_message.is_some()
        || !arguments.commit_scope.is_empty();
    let commit_gate = has_commit_gate.then(|| DraftCommitGateInput {
        required: arguments.commit_required.unwrap_or(false),
        planned_message: arguments.planned_commit_message.unwrap_or_default(),
        scope: arguments.commit_scope,
    });
    let update = DraftTaskUpdateInput {
        title: arguments.title,
        depends_on,
        commit_gate,
        clear_commit_gate: arguments.clear_commit_gate,
    };
    let mut extra = vec!["--task".to_owned(), task_id.to_string()];
    append_optional(&mut extra, "--title", update.title.as_deref());
    if arguments.clear_dependencies {
        extra.push("--clear-dependencies".to_owned());
    } else if let Some(dependencies) = &update.depends_on {
        for dependency in dependencies {
            extra.extend(["--depends-on".to_owned(), dependency.to_string()]);
        }
    }
    if update.clear_commit_gate {
        extra.push("--clear-commit-gate".to_owned());
    } else if let Some(gate) = &update.commit_gate {
        extra.extend([
            "--commit-required".to_owned(),
            gate.required.to_string(),
            "--planned-commit-message".to_owned(),
            gate.planned_message.clone(),
        ]);
        append_repeated(&mut extra, "--commit-scope", &gate.scope);
    }
    let command = mutation_command(&["task", "update"], &arguments.mutation, extra);
    execute_mutation(
        service,
        arguments.mutation,
        &DraftMutation::TaskUpdate { task_id, update },
        command,
        "Plan task updated.",
    )
}

fn execute_task_remove(
    service: &PlanService,
    arguments: TaskMutationArguments,
) -> Result<CommandResponse, MinoError> {
    let task_id = parse_task_id(&arguments.task)?;
    let command = mutation_command(
        &["task", "remove"],
        &arguments.mutation,
        vec!["--task".to_owned(), task_id.to_string()],
    );
    execute_mutation(
        service,
        arguments.mutation,
        &DraftMutation::TaskRemove { task_id },
        command,
        "Plan task removed.",
    )
}

fn execute_task_move(
    service: &PlanService,
    arguments: TaskMoveArguments,
) -> Result<CommandResponse, MinoError> {
    let task_id = parse_task_id(&arguments.task)?;
    let command = mutation_command(
        &["task", "move"],
        &arguments.mutation,
        vec![
            "--task".to_owned(),
            task_id.to_string(),
            "--position".to_owned(),
            arguments.position.to_string(),
        ],
    );
    execute_mutation(
        service,
        arguments.mutation,
        &DraftMutation::TaskMove {
            task_id,
            position: arguments.position,
        },
        command,
        "Plan task moved.",
    )
}

fn execute_task_step(
    service: &PlanService,
    arguments: TaskStepAddArguments,
) -> Result<CommandResponse, MinoError> {
    let task_id = parse_task_id(&arguments.task)?;
    let command = mutation_command(
        &["task", "step", "add"],
        &arguments.mutation,
        vec![
            "--task".to_owned(),
            task_id.to_string(),
            "--value".to_owned(),
            arguments.value.clone(),
        ],
    );
    execute_mutation(
        service,
        arguments.mutation,
        &DraftMutation::TaskStep {
            task_id,
            step: arguments.value,
        },
        command,
        "Task step added.",
    )
}

fn execute_task_step_update(
    service: &PlanService,
    arguments: TaskStepUpdateArguments,
) -> Result<CommandResponse, MinoError> {
    let task_id = parse_task_id(&arguments.task)?;
    let command = mutation_command(
        &["task", "step", "update"],
        &arguments.mutation,
        vec![
            "--task".to_owned(),
            task_id.to_string(),
            "--position".to_owned(),
            arguments.position.to_string(),
            "--value".to_owned(),
            arguments.value.clone(),
        ],
    );
    execute_mutation(
        service,
        arguments.mutation,
        &DraftMutation::TaskStepUpdate {
            task_id,
            position: arguments.position,
            value: arguments.value,
        },
        command,
        "Task step updated.",
    )
}

fn execute_task_step_remove(
    service: &PlanService,
    arguments: TaskPositionMutationArguments,
) -> Result<CommandResponse, MinoError> {
    let task_id = parse_task_id(&arguments.task)?;
    let command = mutation_command(
        &["task", "step", "remove"],
        &arguments.mutation,
        vec![
            "--task".to_owned(),
            task_id.to_string(),
            "--position".to_owned(),
            arguments.position.to_string(),
        ],
    );
    execute_mutation(
        service,
        arguments.mutation,
        &DraftMutation::TaskStepRemove {
            task_id,
            position: arguments.position,
        },
        command,
        "Task step removed.",
    )
}

fn execute_task_criterion(
    service: &PlanService,
    arguments: TaskCriterionAddArguments,
) -> Result<CommandResponse, MinoError> {
    let task_id = parse_task_id(&arguments.task)?;
    let command = mutation_command(
        &["task", "criterion", "add"],
        &arguments.mutation,
        vec![
            "--task".to_owned(),
            task_id.to_string(),
            "--description".to_owned(),
            arguments.description.clone(),
        ],
    );
    execute_mutation(
        service,
        arguments.mutation,
        &DraftMutation::TaskCriterion {
            task_id,
            criterion: DraftCriterionInput {
                id: None,
                description: arguments.description,
            },
        },
        command,
        "Task acceptance criterion added.",
    )
}

fn execute_task_criterion_update(
    service: &PlanService,
    arguments: TaskCriterionUpdateArguments,
) -> Result<CommandResponse, MinoError> {
    let task_id = parse_task_id(&arguments.task)?;
    let criterion_id = parse_criterion_id(&arguments.criterion)?;
    let command = mutation_command(
        &["task", "criterion", "update"],
        &arguments.mutation,
        vec![
            "--task".to_owned(),
            task_id.to_string(),
            "--criterion".to_owned(),
            criterion_id.to_string(),
            "--description".to_owned(),
            arguments.description.clone(),
        ],
    );
    execute_mutation(
        service,
        arguments.mutation,
        &DraftMutation::TaskCriterionUpdate {
            task_id,
            criterion_id,
            description: arguments.description,
        },
        command,
        "Task acceptance criterion updated.",
    )
}

fn execute_task_criterion_remove(
    service: &PlanService,
    arguments: TaskCriterionMutationArguments,
) -> Result<CommandResponse, MinoError> {
    let task_id = parse_task_id(&arguments.task)?;
    let criterion_id = parse_criterion_id(&arguments.criterion)?;
    let command = mutation_command(
        &["task", "criterion", "remove"],
        &arguments.mutation,
        vec![
            "--task".to_owned(),
            task_id.to_string(),
            "--criterion".to_owned(),
            criterion_id.to_string(),
        ],
    );
    execute_mutation(
        service,
        arguments.mutation,
        &DraftMutation::TaskCriterionRemove {
            task_id,
            criterion_id,
        },
        command,
        "Task acceptance criterion removed.",
    )
}

fn execute_task_verification(
    service: &PlanService,
    arguments: TaskVerificationAddArguments,
) -> Result<CommandResponse, MinoError> {
    let task_id = parse_task_id(&arguments.task)?;
    let verification = verification_input(&arguments.verification)?;
    let mut extra = vec!["--task".to_owned(), task_id.to_string()];
    extra.extend(verification_argv(&verification));
    let command = mutation_command(&["task", "verification", "add"], &arguments.mutation, extra);
    execute_mutation(
        service,
        arguments.mutation,
        &DraftMutation::TaskVerification {
            task_id,
            verification,
        },
        command,
        "Task verification added.",
    )
}

fn execute_task_verification_update(
    service: &PlanService,
    arguments: TaskVerificationUpdateArguments,
) -> Result<CommandResponse, MinoError> {
    let task_id = parse_task_id(&arguments.task)?;
    let check_id = parse_check_id(&arguments.check)?;
    let verification = verification_definition_input(check_id.clone(), &arguments.definition);
    let mut extra = vec![
        "--task".to_owned(),
        task_id.to_string(),
        "--check".to_owned(),
        check_id.to_string(),
    ];
    extra.extend(verification_definition_argv(&verification));
    let command = mutation_command(
        &["task", "verification", "update"],
        &arguments.mutation,
        extra,
    );
    execute_mutation(
        service,
        arguments.mutation,
        &DraftMutation::TaskVerificationUpdate {
            task_id,
            check_id,
            verification,
        },
        command,
        "Task verification updated.",
    )
}

fn execute_task_verification_remove(
    service: &PlanService,
    arguments: TaskVerificationMutationArguments,
) -> Result<CommandResponse, MinoError> {
    let task_id = parse_task_id(&arguments.task)?;
    let check_id = parse_check_id(&arguments.check)?;
    let command = mutation_command(
        &["task", "verification", "remove"],
        &arguments.mutation,
        vec![
            "--task".to_owned(),
            task_id.to_string(),
            "--check".to_owned(),
            check_id.to_string(),
        ],
    );
    execute_mutation(
        service,
        arguments.mutation,
        &DraftMutation::TaskVerificationRemove { task_id, check_id },
        command,
        "Task verification removed.",
    )
}

fn execute_file(
    service: &PlanService,
    arguments: FileAddArguments,
) -> Result<CommandResponse, MinoError> {
    let task_id = parse_task_id(&arguments.task)?;
    let file = DraftFileInput {
        path: arguments.path,
        change: arguments.change.into(),
        reason: arguments.reason,
    };
    let command = mutation_command(
        &["file", "add"],
        &arguments.mutation,
        vec![
            "--task".to_owned(),
            task_id.to_string(),
            "--path".to_owned(),
            file.path.clone(),
            "--change".to_owned(),
            file_change_name(file.change).to_owned(),
            "--reason".to_owned(),
            file.reason.clone(),
        ],
    );
    execute_mutation(
        service,
        arguments.mutation,
        &DraftMutation::File { task_id, file },
        command,
        "Task file responsibility added.",
    )
}

fn execute_file_update(
    service: &PlanService,
    arguments: FileUpdateArguments,
) -> Result<CommandResponse, MinoError> {
    let task_id = parse_task_id(&arguments.task)?;
    let file = DraftFileInput {
        path: arguments.path,
        change: arguments.change.into(),
        reason: arguments.reason,
    };
    let command = mutation_command(
        &["file", "update"],
        &arguments.mutation,
        vec![
            "--task".to_owned(),
            task_id.to_string(),
            "--position".to_owned(),
            arguments.position.to_string(),
            "--path".to_owned(),
            file.path.clone(),
            "--change".to_owned(),
            file_change_name(file.change).to_owned(),
            "--reason".to_owned(),
            file.reason.clone(),
        ],
    );
    execute_mutation(
        service,
        arguments.mutation,
        &DraftMutation::FileUpdate {
            task_id,
            position: arguments.position,
            file,
        },
        command,
        "Task file responsibility updated.",
    )
}

fn execute_file_remove(
    service: &PlanService,
    arguments: TaskPositionMutationArguments,
) -> Result<CommandResponse, MinoError> {
    let task_id = parse_task_id(&arguments.task)?;
    let command = mutation_command(
        &["file", "remove"],
        &arguments.mutation,
        vec![
            "--task".to_owned(),
            task_id.to_string(),
            "--position".to_owned(),
            arguments.position.to_string(),
        ],
    );
    execute_mutation(
        service,
        arguments.mutation,
        &DraftMutation::FileRemove {
            task_id,
            position: arguments.position,
        },
        command,
        "Task file responsibility removed.",
    )
}

fn execute_global_verification(
    service: &PlanService,
    arguments: VerificationAddArguments,
) -> Result<CommandResponse, MinoError> {
    let verification = verification_input(&arguments.verification)?;
    let command = mutation_command(
        &["verification", "add"],
        &arguments.mutation,
        verification_argv(&verification),
    );
    execute_mutation(
        service,
        arguments.mutation,
        &DraftMutation::GlobalVerification(verification),
        command,
        "Global verification added.",
    )
}

fn execute_global_verification_update(
    service: &PlanService,
    arguments: VerificationUpdateArguments,
) -> Result<CommandResponse, MinoError> {
    let check_id = parse_check_id(&arguments.check)?;
    let verification = verification_definition_input(check_id.clone(), &arguments.definition);
    let mut extra = vec!["--check".to_owned(), check_id.to_string()];
    extra.extend(verification_definition_argv(&verification));
    let command = mutation_command(&["verification", "update"], &arguments.mutation, extra);
    execute_mutation(
        service,
        arguments.mutation,
        &DraftMutation::GlobalVerificationUpdate {
            check_id,
            verification,
        },
        command,
        "Global verification updated.",
    )
}

fn execute_global_verification_remove(
    service: &PlanService,
    arguments: VerificationMutationArguments,
) -> Result<CommandResponse, MinoError> {
    let check_id = parse_check_id(&arguments.check)?;
    let command = mutation_command(
        &["verification", "remove"],
        &arguments.mutation,
        vec!["--check".to_owned(), check_id.to_string()],
    );
    execute_mutation(
        service,
        arguments.mutation,
        &DraftMutation::GlobalVerificationRemove { check_id },
        command,
        "Global verification removed.",
    )
}

fn execute_mutation(
    service: &PlanService,
    arguments: MutationArguments,
    mutation: &DraftMutation,
    command: Vec<String>,
    message: &str,
) -> Result<CommandResponse, MinoError> {
    let plan_id = parse_plan_id(&arguments.plan)?;
    let request_id = parse_request_id(&arguments.request_id)?;
    let report = service.mutate(
        DraftMutationRequest {
            plan_id,
            expected_revision: arguments.expect_revision,
            request_id,
            actor: arguments.actor,
            command,
            updated_at: Timestamp::now_utc(),
        },
        mutation,
    )?;
    operation_response(service, message, report)
}

fn operation_response(
    service: &PlanService,
    message: &str,
    report: crate::application::plan::PlanOperationReport,
) -> Result<CommandResponse, MinoError> {
    let guidance = service.next(&report.plan_id)?;
    response(
        message,
        false,
        report,
        guidance.missing,
        guidance.next_actions,
    )
}

fn mutation_command(
    path: &[&str],
    arguments: &MutationArguments,
    extra: Vec<String>,
) -> Vec<String> {
    let mut command = vec!["mino".to_owned(), "plan".to_owned()];
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

fn metadata_argv(input: &DraftMetadataInput) -> Vec<String> {
    let mut arguments = Vec::new();
    append_optional(&mut arguments, "--name", input.name.as_deref());
    append_optional(&mut arguments, "--priority", input.priority.as_deref());
    append_optional(&mut arguments, "--type", input.plan_type.as_deref());
    append_optional(&mut arguments, "--area", input.area.as_deref());
    append_optional(&mut arguments, "--owner", input.owner.as_deref());
    arguments
}

fn scope_argv(input: &DraftScopeInput) -> Vec<String> {
    let mut arguments = Vec::new();
    append_optional(&mut arguments, "--goal", input.goal.as_deref());
    append_repeated(
        &mut arguments,
        "--deliverable",
        input.deliverables.as_deref().unwrap_or_default(),
    );
    append_repeated(
        &mut arguments,
        "--in-scope",
        input.in_scope.as_deref().unwrap_or_default(),
    );
    append_repeated(
        &mut arguments,
        "--out-of-scope",
        input.out_of_scope.as_deref().unwrap_or_default(),
    );
    arguments
}

fn verification_input(
    arguments: &VerificationInputArguments,
) -> Result<DraftVerificationInput, MinoError> {
    Ok(verification_definition_input(
        parse_check_id(&arguments.id)?,
        &arguments.definition,
    ))
}

fn verification_definition_input(
    check_id: CheckId,
    arguments: &VerificationDefinitionArguments,
) -> DraftVerificationInput {
    DraftVerificationInput {
        id: check_id,
        command: arguments.command.clone(),
        cwd: arguments.cwd.clone(),
        expected_exit_code: arguments.expected_exit_code,
        required: arguments.required,
    }
}

fn verification_argv(input: &DraftVerificationInput) -> Vec<String> {
    let mut arguments = vec!["--id".to_owned(), input.id.to_string()];
    arguments.extend(verification_definition_argv(input));
    arguments
}

fn verification_definition_argv(input: &DraftVerificationInput) -> Vec<String> {
    let mut arguments = Vec::new();
    append_repeated(&mut arguments, "--command", &input.command);
    arguments.extend([
        "--cwd".to_owned(),
        input.cwd.clone(),
        "--expected-exit-code".to_owned(),
        input.expected_exit_code.to_string(),
        "--required".to_owned(),
        input.required.to_string(),
    ]);
    arguments
}

fn append_optional(arguments: &mut Vec<String>, name: &str, value: Option<&str>) {
    if let Some(value) = value {
        arguments.extend([name.to_owned(), value.to_owned()]);
    }
}

fn append_repeated(arguments: &mut Vec<String>, name: &str, values: &[String]) {
    for value in values {
        arguments.extend([name.to_owned(), value.clone()]);
    }
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

fn parse_check_id(value: &str) -> Result<CheckId, MinoError> {
    CheckId::parse(value).map_err(|error| domain_error(&error))
}

fn parse_request_id(value: &str) -> Result<RequestId, MinoError> {
    RequestId::parse(value).map_err(|error| domain_error(&error))
}

fn domain_error(error: &crate::domain::DomainError) -> MinoError {
    MinoError::new(ErrorCategory::IncompleteOrValidation, error.to_string())
}

fn validation_error(message: impl Into<String>) -> MinoError {
    MinoError::new(ErrorCategory::IncompleteOrValidation, message)
}

fn require_matching_digest(provided: Option<&str>, actual: &str) -> Result<(), MinoError> {
    if provided.is_none_or(|provided| provided == actual) {
        Ok(())
    } else {
        Err(validation_error(
            "Provided normalized input digest does not match the supplied content",
        ))
    }
}

fn trim_one_line_ending(mut value: String) -> String {
    if value.ends_with('\n') {
        value.pop();
        if value.ends_with('\r') {
            value.pop();
        }
    }
    value
}

fn file_change_name(change: FileChange) -> &'static str {
    match change {
        FileChange::Create => "create",
        FileChange::Modify => "modify",
        FileChange::Delete => "delete",
        FileChange::Test => "test",
        FileChange::NotApplicable => "not-applicable",
    }
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
            format!("Failed to serialize plan result: {error}"),
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
