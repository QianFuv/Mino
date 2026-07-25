//! CLI adapters for plan finalization, show, revision-bound review, and approval.

use clap::{Args, ValueEnum};

use crate::application::approval::ApprovalService;
use crate::application::plan::{PlanMutationRequest, PlanService};
use crate::commands::CommandResponse;
use crate::domain::{GitFlowConsent, Timestamp};
use crate::{MinoError, NextAction};

use super::{
    MutationArguments, PlanReadArguments, mutation_command, parse_plan_id, parse_request_id,
    response,
};

#[derive(Debug, Args)]
pub(crate) struct FinalizeArguments {
    #[command(flatten)]
    mutation: MutationArguments,
}

#[derive(Debug, Args)]
pub(crate) struct ApproveArguments {
    #[command(flatten)]
    mutation: MutationArguments,
    /// Auditable external reference for the explicit user approval.
    #[arg(long)]
    approval_ref: String,
    /// Explicit consent decision for plan-declared Git Flow commits.
    #[arg(long, value_enum)]
    git_flow_consent: GitFlowConsentArgument,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum GitFlowConsentArgument {
    Approved,
    Disabled,
}

impl From<GitFlowConsentArgument> for GitFlowConsent {
    fn from(value: GitFlowConsentArgument) -> Self {
        match value {
            GitFlowConsentArgument::Approved => Self::Approved,
            GitFlowConsentArgument::Disabled => Self::Disabled,
        }
    }
}

pub(super) fn execute_show(
    service: &PlanService,
    arguments: &PlanReadArguments,
) -> Result<CommandResponse, MinoError> {
    let plan_id = parse_plan_id(&arguments.plan)?;
    let lifecycle = ApprovalService::new(service.clone());
    let plan = lifecycle.show(&plan_id)?;
    response("Plan loaded.", true, plan, Vec::new(), Vec::new())
}

pub(super) fn execute_review(
    service: &PlanService,
    arguments: &PlanReadArguments,
) -> Result<CommandResponse, MinoError> {
    let plan_id = parse_plan_id(&arguments.plan)?;
    let lifecycle = ApprovalService::new(service.clone());
    let report = lifecycle.review(&plan_id)?;
    let is_complete = !report.approval_required;
    let missing = report
        .approval_required
        .then(|| "approval".to_owned())
        .into_iter()
        .collect();
    response(
        "Plan review summary generated.",
        is_complete,
        report,
        missing,
        Vec::new(),
    )
}

pub(super) fn execute_finalize(
    service: &PlanService,
    arguments: FinalizeArguments,
) -> Result<CommandResponse, MinoError> {
    let plan_id = parse_plan_id(&arguments.mutation.plan)?;
    let request_id = parse_request_id(&arguments.mutation.request_id)?;
    let command = mutation_command(&["finalize"], &arguments.mutation, Vec::new());
    let lifecycle = ApprovalService::new(service.clone());
    let report = lifecycle.finalize(PlanMutationRequest {
        plan_id: plan_id.clone(),
        expected_revision: arguments.mutation.expect_revision,
        request_id,
        actor: arguments.mutation.actor,
        command,
        updated_at: Timestamp::now_utc(),
    })?;
    response(
        "Plan created successfully and is ready for review.",
        false,
        report,
        vec!["approval".to_owned()],
        vec![review_action(&plan_id)],
    )
}

pub(super) fn execute_approve(
    service: &PlanService,
    arguments: ApproveArguments,
) -> Result<CommandResponse, MinoError> {
    let plan_id = parse_plan_id(&arguments.mutation.plan)?;
    let request_id = parse_request_id(&arguments.mutation.request_id)?;
    let command = mutation_command(
        &["approve"],
        &arguments.mutation,
        vec![
            "--approval-ref".to_owned(),
            arguments.approval_ref.clone(),
            "--git-flow-consent".to_owned(),
            consent_name(arguments.git_flow_consent).to_owned(),
        ],
    );
    let lifecycle = ApprovalService::new(service.clone());
    let report = lifecycle.approve(
        PlanMutationRequest {
            plan_id,
            expected_revision: arguments.mutation.expect_revision,
            request_id,
            actor: arguments.mutation.actor,
            command,
            updated_at: Timestamp::now_utc(),
        },
        arguments.approval_ref,
        arguments.git_flow_consent.into(),
    )?;
    response(
        "Plan approval recorded.",
        true,
        report,
        Vec::new(),
        Vec::new(),
    )
}

fn review_action(plan_id: &crate::domain::PlanId) -> NextAction {
    NextAction {
        id: "plan.review".to_owned(),
        argv: vec![
            "mino".to_owned(),
            "plan".to_owned(),
            "review".to_owned(),
            "--plan".to_owned(),
            plan_id.to_string(),
            "--format".to_owned(),
            "json".to_owned(),
            "--no-input".to_owned(),
        ],
    }
}

const fn consent_name(consent: GitFlowConsentArgument) -> &'static str {
    match consent {
        GitFlowConsentArgument::Approved => "approved",
        GitFlowConsentArgument::Disabled => "disabled",
    }
}
