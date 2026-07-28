//! Git inspection and active-binding CLI adapter.

use std::path::{Path, PathBuf};

use clap::{Args, Subcommand, ValueEnum};
use serde::Deserialize;

use crate::application::git_binding::GitBindingService;
use crate::application::git_branch::GitBranchService;
use crate::application::git_commit::GitCommitService;
use crate::application::git_hooks::GitHookService;
use crate::application::git_readiness::{GitReadinessService, PrePlanCleanupItemInput};
use crate::application::plan::PlanMutationRequest;
use crate::commands::CommandResponse;
use crate::domain::{GitSetupDecision, PlanId, RequestId, TaskId, Timestamp};
use crate::git::GitHookName;
use crate::input::read_utf8_file;
use crate::store::sha256_digest;
use crate::{ErrorCategory, MinoError};

#[derive(Debug, Subcommand)]
pub(crate) enum GitAction {
    /// Inspect repository, worktree, HEAD, index, and active binding facts.
    Inspect(InspectArguments),
    /// Bind one plan to the exact current worktree and branch or detached HEAD.
    Bind(BindArguments),
    /// Propose or create the deterministic Git branch for a plan.
    Branch(BranchArguments),
    /// Create or recover one approved task-level commit.
    Commit(CommitArguments),
    /// Manage required task commit-gate exceptions.
    Gate(GateArguments),
    /// Inspect, install, or invoke optional advisory repository hooks.
    Hook(HookArguments),
    /// Inspect or explicitly refresh plan-bound live Git readiness.
    Readiness(ReadinessArguments),
    /// Record the explicit setup decision for a missing Git repository.
    Setup(SetupArguments),
    /// Propose, approve, or verify external pre-plan cleanup commits.
    Cleanup(CleanupArguments),
}

#[derive(Debug, Args)]
pub(crate) struct InspectArguments {
    /// Optional plan whose binding relationship should be reported.
    #[arg(long)]
    plan: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct BindArguments {
    /// Plan to bind to the current worktree identity.
    #[arg(long)]
    plan: String,
    /// Explicitly select the current worktree; no other target is supported.
    #[arg(long)]
    current: bool,
}

#[derive(Debug, Args)]
pub(crate) struct BranchArguments {
    #[command(subcommand)]
    action: GitBranchAction,
}

#[derive(Debug, Subcommand)]
enum GitBranchAction {
    /// Return a deterministic branch proposal without mutation.
    Propose(BranchProposeArguments),
    /// Create the proposed branch after an explicit approval reference.
    Create(BranchCreateArguments),
}

#[derive(Debug, Args)]
struct BranchProposeArguments {
    /// Plan for which a Git branch should be proposed.
    #[arg(long)]
    plan: String,
}

#[derive(Debug, Args)]
struct BranchCreateArguments {
    /// Plan for which the approved Git branch should be created.
    #[arg(long)]
    plan: String,
    /// Auditable external reference for explicit branch approval.
    #[arg(long)]
    approval_ref: String,
    /// Optional explicit branch name, which must equal the proposal.
    #[arg(long)]
    branch: Option<String>,
}

#[derive(Debug, Args)]
pub(crate) struct CommitArguments {
    #[command(subcommand)]
    action: Option<GitCommitAction>,
    /// Approved plan containing the task commit gate.
    #[arg(long)]
    plan: Option<String>,
    /// Done task whose exact commit gate should execute.
    #[arg(long)]
    task: Option<String>,
}

#[derive(Debug, Subcommand)]
enum GitCommitAction {
    /// Verify and record a commit created outside Mino.
    RecordManual(RecordManualArguments),
}

#[derive(Debug, Args)]
struct RecordManualArguments {
    /// Approved plan containing the task commit gate.
    #[arg(long)]
    plan: String,
    /// Done task whose required gate should record the commit.
    #[arg(long)]
    task: String,
    /// Full commit object ID already at current branch HEAD.
    #[arg(long)]
    commit: String,
    /// Auditable external reference approving this manual commit.
    #[arg(long)]
    approval_ref: String,
    /// Required optimistic-concurrency revision.
    #[arg(long)]
    expect_revision: u64,
    /// Idempotency UUID for this record request.
    #[arg(long)]
    request_id: String,
    /// Actor recorded in the event and evidence logs.
    #[arg(long, default_value = "user")]
    actor: String,
}

#[derive(Debug, Args)]
pub(crate) struct GateArguments {
    #[command(subcommand)]
    action: GitGateAction,
}

#[derive(Debug, Subcommand)]
enum GitGateAction {
    /// Satisfy a required gate with an explicitly approved exception.
    Skip(SkipGateArguments),
}

#[derive(Debug, Args)]
struct SkipGateArguments {
    /// Approved plan containing the task commit gate.
    #[arg(long)]
    plan: String,
    /// Done task whose required commit gate should be skipped.
    #[arg(long)]
    task: String,
    /// Auditable external reference approving the exception.
    #[arg(long)]
    approval_ref: String,
    /// Human-readable reason for the exception.
    #[arg(long)]
    reason: String,
    /// Required optimistic-concurrency revision.
    #[arg(long)]
    expect_revision: u64,
    /// Idempotency UUID for this skip request.
    #[arg(long)]
    request_id: String,
    /// Actor recorded in the event and evidence logs.
    #[arg(long, default_value = "user")]
    actor: String,
}

#[derive(Debug, Args)]
pub(crate) struct HookArguments {
    #[command(subcommand)]
    action: GitHookAction,
}

#[derive(Debug, Args)]
pub(crate) struct ReadinessArguments {
    #[command(subcommand)]
    action: GitReadinessAction,
}

#[derive(Debug, Subcommand)]
enum GitReadinessAction {
    /// Capture current Git facts as one revisioned plan mutation.
    Refresh(ReadinessRefreshArguments),
}

#[derive(Debug, Args)]
struct ReadinessRefreshArguments {
    /// Draft or Ready plan whose readiness should be refreshed.
    #[arg(long)]
    plan: String,
    /// Required optimistic-concurrency revision.
    #[arg(long)]
    expect_revision: u64,
    /// Idempotency UUID for this refresh request.
    #[arg(long)]
    request_id: String,
    /// Actor recorded in the event log.
    #[arg(long, default_value = "user")]
    actor: String,
}

#[derive(Debug, Args)]
pub(crate) struct SetupArguments {
    #[command(subcommand)]
    action: GitSetupAction,
}

#[derive(Debug, Subcommand)]
enum GitSetupAction {
    /// Record one explicit missing-repository decision without mutating Git.
    Decide(SetupDecideArguments),
}

#[derive(Debug, Args)]
struct SetupDecideArguments {
    /// Draft or Ready plan whose setup decision should be recorded.
    #[arg(long)]
    plan: String,
    /// Explicit setup decision.
    #[arg(long, value_enum)]
    decision: SetupDecisionArgument,
    /// Auditable external reference for the setup decision.
    #[arg(long)]
    approval_ref: String,
    /// Required optimistic-concurrency revision.
    #[arg(long)]
    expect_revision: u64,
    /// Idempotency UUID for this decision request.
    #[arg(long)]
    request_id: String,
    /// Actor recorded in the event log.
    #[arg(long, default_value = "user")]
    actor: String,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum SetupDecisionArgument {
    InitializeApproved,
    ContinueWithoutGit,
    BlockedUntilManualSetup,
}

impl From<SetupDecisionArgument> for GitSetupDecision {
    fn from(value: SetupDecisionArgument) -> Self {
        match value {
            SetupDecisionArgument::InitializeApproved => Self::InitializeApproved,
            SetupDecisionArgument::ContinueWithoutGit => Self::ContinueWithoutGit,
            SetupDecisionArgument::BlockedUntilManualSetup => Self::BlockedUntilManualSetup,
        }
    }
}

#[derive(Debug, Args)]
pub(crate) struct CleanupArguments {
    #[command(subcommand)]
    action: GitCleanupAction,
}

#[derive(Debug, Subcommand)]
enum GitCleanupAction {
    /// Record an exact proposal covering every observed dirty path.
    Propose(CleanupProposeArguments),
    /// Approve one proposal item or explicitly decline cleanup.
    Approve(CleanupApproveArguments),
    /// Verify and record one already-created cleanup commit.
    Record(CleanupRecordArguments),
}

#[derive(Debug, Args)]
struct CleanupProposeArguments {
    /// Draft or Ready plan whose dirty paths should be proposed for cleanup.
    #[arg(long)]
    plan: String,
    /// YAML document containing the ordered cleanup items.
    #[arg(long)]
    file: PathBuf,
    /// Optional digest that must match the normalized proposal bytes.
    #[arg(long)]
    file_digest: Option<String>,
    /// Required optimistic-concurrency revision.
    #[arg(long)]
    expect_revision: u64,
    /// Idempotency UUID for this proposal.
    #[arg(long)]
    request_id: String,
    /// Actor recorded in the event log.
    #[arg(long, default_value = "user")]
    actor: String,
}

#[derive(Debug, Args)]
struct CleanupApproveArguments {
    /// Draft or Ready plan containing the cleanup proposal.
    #[arg(long)]
    plan: String,
    /// Exact cleanup item to approve.
    #[arg(long, conflicts_with = "decline")]
    item: Option<String>,
    /// Explicitly decline cleanup and disable Git Flow.
    #[arg(long, conflicts_with = "item")]
    decline: bool,
    /// Auditable external reference for the approval or decline decision.
    #[arg(long)]
    approval_ref: String,
    /// Required optimistic-concurrency revision.
    #[arg(long)]
    expect_revision: u64,
    /// Idempotency UUID for this decision.
    #[arg(long)]
    request_id: String,
    /// Actor recorded in the event log.
    #[arg(long, default_value = "user")]
    actor: String,
}

#[derive(Debug, Args)]
struct CleanupRecordArguments {
    /// Draft or Ready plan containing the approved cleanup item.
    #[arg(long)]
    plan: String,
    /// Exact cleanup item being recorded.
    #[arg(long)]
    item: String,
    /// Full existing cleanup commit object ID at current HEAD.
    #[arg(long)]
    commit: String,
    /// Required optimistic-concurrency revision.
    #[arg(long)]
    expect_revision: u64,
    /// Idempotency UUID for this record request.
    #[arg(long)]
    request_id: String,
    /// Actor recorded in the event log.
    #[arg(long, default_value = "user")]
    actor: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CleanupProposalDocument {
    items: Vec<PrePlanCleanupItemInput>,
}

#[derive(Debug, Subcommand)]
enum GitHookAction {
    /// Return the hash-bound installation proposal without mutation.
    Propose,
    /// Return current hook ownership and content status without mutation.
    Status,
    /// Install or repair only Mino-owned hooks after explicit approval.
    Install(HookInstallArguments),
    /// Emit advisory staged/commit observations without mutation.
    Run(HookRunArguments),
}

#[derive(Debug, Args)]
struct HookInstallArguments {
    /// Exact current digest returned by `git hook propose`.
    #[arg(long)]
    proposal_hash: String,
    /// Auditable external reference for explicit hook installation approval.
    #[arg(long)]
    approval_ref: String,
}

#[derive(Debug, Args)]
struct HookRunArguments {
    /// Advisory Git hook being invoked.
    #[arg(long, value_enum)]
    hook: HookNameArgument,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum HookNameArgument {
    PreCommit,
    PostCommit,
}

impl From<HookNameArgument> for GitHookName {
    fn from(value: HookNameArgument) -> Self {
        match value {
            HookNameArgument::PreCommit => Self::PreCommit,
            HookNameArgument::PostCommit => Self::PostCommit,
        }
    }
}

pub(crate) fn execute(start: &Path, action: GitAction) -> Result<CommandResponse, MinoError> {
    let service = GitBindingService::discover(start)?;
    let (message, payload) = match action {
        GitAction::Inspect(arguments) => (
            "Git repository and active binding inspected.",
            serde_json::to_value(
                service.inspect(arguments.plan.as_deref().map(parse_plan_id).transpose()?)?,
            ),
        ),
        GitAction::Bind(arguments) => {
            if !arguments.current {
                return Err(MinoError::new(
                    ErrorCategory::IncompleteOrValidation,
                    "Git binding requires --current",
                ));
            }
            (
                "Plan bound to the current Git worktree.",
                serde_json::to_value(service.bind_current(parse_plan_id(&arguments.plan)?)?),
            )
        }
        GitAction::Branch(arguments) => execute_branch(start, arguments.action)?,
        GitAction::Commit(arguments) => execute_commit(start, arguments)?,
        GitAction::Gate(arguments) => execute_gate(start, arguments.action)?,
        GitAction::Hook(arguments) => return execute_hook(start, arguments.action),
        GitAction::Readiness(arguments) => execute_readiness(start, arguments.action)?,
        GitAction::Setup(arguments) => execute_setup(start, arguments.action)?,
        GitAction::Cleanup(arguments) => execute_cleanup(start, arguments.action)?,
    };
    let payload = payload.map_err(|error| {
        MinoError::new(
            ErrorCategory::EnvironmentUnavailable,
            format!("Failed to serialize Git result: {error}"),
        )
    })?;
    Ok(CommandResponse {
        message: message.to_owned(),
        complete: true,
        payload,
        missing: Vec::new(),
        next_actions: Vec::new(),
    })
}

fn execute_setup(
    start: &Path,
    action: GitSetupAction,
) -> Result<(&'static str, Result<serde_json::Value, serde_json::Error>), MinoError> {
    let service = GitReadinessService::discover(start)?;
    match action {
        GitSetupAction::Decide(arguments) => {
            let request = readiness_mutation_request(
                &arguments.plan,
                arguments.expect_revision,
                &arguments.request_id,
                &arguments.actor,
                vec![
                    "mino".to_owned(),
                    "git".to_owned(),
                    "setup".to_owned(),
                    "decide".to_owned(),
                    "--plan".to_owned(),
                    arguments.plan.clone(),
                    "--decision".to_owned(),
                    setup_decision_name(arguments.decision).to_owned(),
                    "--approval-ref".to_owned(),
                    arguments.approval_ref.clone(),
                    "--expect-revision".to_owned(),
                    arguments.expect_revision.to_string(),
                    "--request-id".to_owned(),
                    arguments.request_id.clone(),
                    "--actor".to_owned(),
                    arguments.actor.clone(),
                ],
            )?;
            Ok((
                "Git setup decision recorded without mutating Git.",
                serde_json::to_value(service.decide_setup(
                    request,
                    arguments.decision.into(),
                    arguments.approval_ref,
                )?),
            ))
        }
    }
}

fn execute_cleanup(
    start: &Path,
    action: GitCleanupAction,
) -> Result<(&'static str, Result<serde_json::Value, serde_json::Error>), MinoError> {
    let service = GitReadinessService::discover(start)?;
    match action {
        GitCleanupAction::Propose(arguments) => execute_cleanup_proposal(&service, &arguments),
        GitCleanupAction::Approve(arguments) => execute_cleanup_approval(&service, arguments),
        GitCleanupAction::Record(arguments) => execute_cleanup_record(&service, &arguments),
    }
}

fn execute_cleanup_proposal(
    service: &GitReadinessService,
    arguments: &CleanupProposeArguments,
) -> Result<(&'static str, Result<serde_json::Value, serde_json::Error>), MinoError> {
    let source = read_utf8_file(&arguments.file)?;
    let digest = sha256_digest(source.as_bytes());
    require_matching_digest(arguments.file_digest.as_deref(), &digest)?;
    let document: CleanupProposalDocument = serde_saphyr::from_str(&source).map_err(|error| {
        MinoError::new(
            ErrorCategory::IncompleteOrValidation,
            format!("Failed to parse cleanup proposal YAML: {error}"),
        )
    })?;
    let request = readiness_mutation_request(
        &arguments.plan,
        arguments.expect_revision,
        &arguments.request_id,
        &arguments.actor,
        vec![
            "mino".to_owned(),
            "git".to_owned(),
            "cleanup".to_owned(),
            "propose".to_owned(),
            "--plan".to_owned(),
            arguments.plan.clone(),
            "--file".to_owned(),
            arguments.file.to_string_lossy().into_owned(),
            "--file-digest".to_owned(),
            digest,
            "--expect-revision".to_owned(),
            arguments.expect_revision.to_string(),
            "--request-id".to_owned(),
            arguments.request_id.clone(),
            "--actor".to_owned(),
            arguments.actor.clone(),
        ],
    )?;
    Ok((
        "Pre-plan cleanup proposal recorded without mutating Git.",
        serde_json::to_value(service.propose_cleanup(request, document.items)?),
    ))
}

fn execute_cleanup_approval(
    service: &GitReadinessService,
    arguments: CleanupApproveArguments,
) -> Result<(&'static str, Result<serde_json::Value, serde_json::Error>), MinoError> {
    if arguments.item.is_none() != arguments.decline {
        return Err(MinoError::new(
            ErrorCategory::IncompleteOrValidation,
            "Cleanup approval requires exactly one of --item or --decline",
        ));
    }
    let mut command = vec![
        "mino".to_owned(),
        "git".to_owned(),
        "cleanup".to_owned(),
        "approve".to_owned(),
        "--plan".to_owned(),
        arguments.plan.clone(),
    ];
    if let Some(item) = &arguments.item {
        command.extend(["--item".to_owned(), item.clone()]);
    } else {
        command.push("--decline".to_owned());
    }
    command.extend([
        "--approval-ref".to_owned(),
        arguments.approval_ref.clone(),
        "--expect-revision".to_owned(),
        arguments.expect_revision.to_string(),
        "--request-id".to_owned(),
        arguments.request_id.clone(),
        "--actor".to_owned(),
        arguments.actor.clone(),
    ]);
    let request = readiness_mutation_request(
        &arguments.plan,
        arguments.expect_revision,
        &arguments.request_id,
        &arguments.actor,
        command,
    )?;
    let report = if let Some(item) = arguments.item {
        service.approve_cleanup_item(request, &item, arguments.approval_ref)?
    } else {
        service.decline_cleanup(request, arguments.approval_ref)?
    };
    Ok((
        "Pre-plan cleanup decision recorded without mutating Git.",
        serde_json::to_value(report),
    ))
}

fn execute_cleanup_record(
    service: &GitReadinessService,
    arguments: &CleanupRecordArguments,
) -> Result<(&'static str, Result<serde_json::Value, serde_json::Error>), MinoError> {
    let command = vec![
        "mino".to_owned(),
        "git".to_owned(),
        "cleanup".to_owned(),
        "record".to_owned(),
        "--plan".to_owned(),
        arguments.plan.clone(),
        "--item".to_owned(),
        arguments.item.clone(),
        "--commit".to_owned(),
        arguments.commit.clone(),
        "--expect-revision".to_owned(),
        arguments.expect_revision.to_string(),
        "--request-id".to_owned(),
        arguments.request_id.clone(),
        "--actor".to_owned(),
        arguments.actor.clone(),
    ];
    let request = readiness_mutation_request(
        &arguments.plan,
        arguments.expect_revision,
        &arguments.request_id,
        &arguments.actor,
        command,
    )?;
    Ok((
        "Existing pre-plan cleanup commit verified and recorded.",
        serde_json::to_value(service.record_cleanup_commit(
            request,
            &arguments.item,
            &arguments.commit,
        )?),
    ))
}

fn execute_readiness(
    start: &Path,
    action: GitReadinessAction,
) -> Result<(&'static str, Result<serde_json::Value, serde_json::Error>), MinoError> {
    let service = GitReadinessService::discover(start)?;
    match action {
        GitReadinessAction::Refresh(arguments) => {
            let request = PlanMutationRequest {
                plan_id: parse_plan_id(&arguments.plan)?,
                expected_revision: arguments.expect_revision,
                request_id: parse_request_id(&arguments.request_id)?,
                actor: arguments.actor.clone(),
                command: vec![
                    "mino".to_owned(),
                    "git".to_owned(),
                    "readiness".to_owned(),
                    "refresh".to_owned(),
                    "--plan".to_owned(),
                    arguments.plan,
                    "--expect-revision".to_owned(),
                    arguments.expect_revision.to_string(),
                    "--request-id".to_owned(),
                    arguments.request_id,
                    "--actor".to_owned(),
                    arguments.actor,
                ],
                updated_at: Timestamp::now_utc(),
            };
            Ok((
                "Git readiness refreshed as a plan revision.",
                serde_json::to_value(service.refresh(request)?),
            ))
        }
    }
}

fn execute_commit(
    start: &Path,
    arguments: CommitArguments,
) -> Result<(&'static str, Result<serde_json::Value, serde_json::Error>), MinoError> {
    let service = GitCommitService::discover(start)?;
    match arguments.action {
        Some(GitCommitAction::RecordManual(arguments)) => {
            let request = manual_commit_request(&arguments)?;
            Ok((
                "Manual task commit verified, evidenced, and recorded.",
                serde_json::to_value(service.record_manual(
                    request,
                    &parse_task_id(&arguments.task)?,
                    &arguments.commit,
                    &arguments.approval_ref,
                )?),
            ))
        }
        None => {
            let plan = arguments.plan.as_deref().ok_or_else(|| {
                MinoError::new(
                    ErrorCategory::IncompleteOrValidation,
                    "Automatic git commit requires --plan",
                )
            })?;
            let task = arguments.task.as_deref().ok_or_else(|| {
                MinoError::new(
                    ErrorCategory::IncompleteOrValidation,
                    "Automatic git commit requires --task",
                )
            })?;
            Ok((
                "Task commit created, evidenced, and recorded.",
                serde_json::to_value(service.commit(&parse_plan_id(plan)?, &parse_task_id(task)?)?),
            ))
        }
    }
}

fn execute_gate(
    start: &Path,
    action: GitGateAction,
) -> Result<(&'static str, Result<serde_json::Value, serde_json::Error>), MinoError> {
    let service = GitCommitService::discover(start)?;
    match action {
        GitGateAction::Skip(arguments) => {
            let request = skip_gate_request(&arguments)?;
            Ok((
                "Required task commit gate skipped with approved evidence.",
                serde_json::to_value(service.skip_gate(
                    request,
                    &parse_task_id(&arguments.task)?,
                    &arguments.approval_ref,
                    &arguments.reason,
                )?),
            ))
        }
    }
}

fn manual_commit_request(
    arguments: &RecordManualArguments,
) -> Result<PlanMutationRequest, MinoError> {
    Ok(PlanMutationRequest {
        plan_id: parse_plan_id(&arguments.plan)?,
        expected_revision: arguments.expect_revision,
        request_id: parse_request_id(&arguments.request_id)?,
        actor: arguments.actor.clone(),
        command: vec![
            "mino".to_owned(),
            "git".to_owned(),
            "commit".to_owned(),
            "record-manual".to_owned(),
            "--plan".to_owned(),
            arguments.plan.clone(),
            "--task".to_owned(),
            arguments.task.clone(),
            "--commit".to_owned(),
            arguments.commit.clone(),
            "--approval-ref".to_owned(),
            arguments.approval_ref.clone(),
            "--expect-revision".to_owned(),
            arguments.expect_revision.to_string(),
            "--request-id".to_owned(),
            arguments.request_id.clone(),
            "--actor".to_owned(),
            arguments.actor.clone(),
        ],
        updated_at: Timestamp::now_utc(),
    })
}

fn skip_gate_request(arguments: &SkipGateArguments) -> Result<PlanMutationRequest, MinoError> {
    Ok(PlanMutationRequest {
        plan_id: parse_plan_id(&arguments.plan)?,
        expected_revision: arguments.expect_revision,
        request_id: parse_request_id(&arguments.request_id)?,
        actor: arguments.actor.clone(),
        command: vec![
            "mino".to_owned(),
            "git".to_owned(),
            "gate".to_owned(),
            "skip".to_owned(),
            "--plan".to_owned(),
            arguments.plan.clone(),
            "--task".to_owned(),
            arguments.task.clone(),
            "--approval-ref".to_owned(),
            arguments.approval_ref.clone(),
            "--reason".to_owned(),
            arguments.reason.clone(),
            "--expect-revision".to_owned(),
            arguments.expect_revision.to_string(),
            "--request-id".to_owned(),
            arguments.request_id.clone(),
            "--actor".to_owned(),
            arguments.actor.clone(),
        ],
        updated_at: Timestamp::now_utc(),
    })
}

fn execute_hook(start: &Path, action: GitHookAction) -> Result<CommandResponse, MinoError> {
    let service = GitHookService::discover(start)?;
    let (message, payload, next_actions) = match action {
        GitHookAction::Propose => (
            "Advisory Git hook proposal generated without mutation.".to_owned(),
            serde_json::to_value(service.propose()?),
            Vec::new(),
        ),
        GitHookAction::Status => (
            "Advisory Git hook status inspected without mutation.".to_owned(),
            serde_json::to_value(service.status()?),
            Vec::new(),
        ),
        GitHookAction::Install(arguments) => (
            "Approved advisory Git hooks installed and verified.".to_owned(),
            serde_json::to_value(
                service.install(&arguments.proposal_hash, &arguments.approval_ref)?,
            ),
            Vec::new(),
        ),
        GitHookAction::Run(arguments) => {
            let report = service.run(arguments.hook.into())?;
            let message = format!(
                "Advisory {} observation: {}",
                report.hook.as_str(),
                report
                    .diagnostics
                    .iter()
                    .map(|diagnostic| diagnostic.message.as_str())
                    .collect::<Vec<_>>()
                    .join("; ")
            );
            let next_actions = report.next_actions.clone();
            (message, serde_json::to_value(report), next_actions)
        }
    };
    let payload = payload.map_err(|error| {
        MinoError::new(
            ErrorCategory::EnvironmentUnavailable,
            format!("Failed to serialize Git hook result: {error}"),
        )
    })?;
    Ok(CommandResponse {
        message,
        complete: true,
        payload,
        missing: Vec::new(),
        next_actions,
    })
}

fn execute_branch(
    start: &Path,
    action: GitBranchAction,
) -> Result<(&'static str, Result<serde_json::Value, serde_json::Error>), MinoError> {
    let service = GitBranchService::discover(start)?;
    Ok(match action {
        GitBranchAction::Propose(arguments) => (
            "Git branch proposal generated without mutation.",
            serde_json::to_value(service.propose(&parse_plan_id(&arguments.plan)?)?),
        ),
        GitBranchAction::Create(arguments) => (
            "Approved Git branch created and bound.",
            serde_json::to_value(service.create(
                &parse_plan_id(&arguments.plan)?,
                &arguments.approval_ref,
                arguments.branch.as_deref(),
            )?),
        ),
    })
}

fn readiness_mutation_request(
    plan: &str,
    expected_revision: u64,
    request_id: &str,
    actor: &str,
    command: Vec<String>,
) -> Result<PlanMutationRequest, MinoError> {
    Ok(PlanMutationRequest {
        plan_id: parse_plan_id(plan)?,
        expected_revision,
        request_id: parse_request_id(request_id)?,
        actor: actor.to_owned(),
        command,
        updated_at: Timestamp::now_utc(),
    })
}

const fn setup_decision_name(decision: SetupDecisionArgument) -> &'static str {
    match decision {
        SetupDecisionArgument::InitializeApproved => "initialize-approved",
        SetupDecisionArgument::ContinueWithoutGit => "continue-without-git",
        SetupDecisionArgument::BlockedUntilManualSetup => "blocked-until-manual-setup",
    }
}

fn require_matching_digest(provided: Option<&str>, actual: &str) -> Result<(), MinoError> {
    if provided.is_none_or(|provided| provided == actual) {
        Ok(())
    } else {
        Err(MinoError::new(
            ErrorCategory::IncompleteOrValidation,
            "Provided cleanup proposal digest does not match the supplied content",
        ))
    }
}

fn parse_plan_id(value: &str) -> Result<PlanId, MinoError> {
    PlanId::parse(value)
        .map_err(|error| MinoError::new(ErrorCategory::IncompleteOrValidation, error.to_string()))
}

fn parse_task_id(value: &str) -> Result<TaskId, MinoError> {
    TaskId::parse(value)
        .map_err(|error| MinoError::new(ErrorCategory::IncompleteOrValidation, error.to_string()))
}

fn parse_request_id(value: &str) -> Result<RequestId, MinoError> {
    RequestId::parse(value)
        .map_err(|error| MinoError::new(ErrorCategory::IncompleteOrValidation, error.to_string()))
}
