//! Stable Agent context, next-action, capability, and active-plan services.

use std::path::Path;

use serde::Serialize;

use crate::application::plan::{
    PlanService, derived_request_id, draft_missing, draft_next_actions,
};
use crate::domain::{
    AmendmentClassification, AmendmentStatus, CURRENT_PROTOCOL_REVISION, CURRENT_PROTOCOL_VERSION,
    CheckId, CheckStatus, MaterialReviewDisposition, Plan, PlanId, PlanStatus,
    ReviewClassification, ReviewStatus, TaskId, TaskStatus,
};
use crate::git::{ActiveBindingStatus, ActiveBindingStore, GitAdapter, GitHeadState};
use crate::project::ProjectPlanSelection;
use crate::validation::validate_plan;
use crate::{ErrorCategory, MinoError, NextAction};

use super::AGENT_EXECUTOR_IDENTITY;

/// Versioned Agent context schema identifier.
pub const AGENT_CONTEXT_KIND: &str = "mino.agent-context/v1";
/// Versioned Agent next-action schema identifier.
pub const AGENT_NEXT_KIND: &str = "mino.agent-next/v1";
/// Versioned Agent capabilities schema identifier.
pub const AGENT_CAPABILITIES_KIND: &str = "mino.agent-capabilities/v1";

const CAPABILITIES: &[(&str, bool, bool)] = &[
    ("agent.capabilities", false, false),
    ("agent.context", false, false),
    ("agent.next", false, false),
    ("evidence.add", true, false),
    ("evidence.list", false, false),
    ("evidence.show", false, false),
    ("exec.block", true, false),
    ("exec.check.monitor", true, false),
    ("exec.check.run", true, false),
    ("exec.checkpoint", true, false),
    ("exec.complete", true, false),
    ("exec.criterion.pass", true, false),
    ("exec.deviation.list", false, false),
    ("exec.deviation.record", true, false),
    ("exec.deviation.reject", true, true),
    ("exec.deviation.resolve", true, false),
    ("exec.deviation.supersede", true, false),
    ("exec.finish", true, false),
    ("exec.resume", true, false),
    ("exec.rework", true, false),
    ("exec.schedule.spec", false, false),
    ("exec.start", true, false),
    ("git.bind", false, false),
    ("git.branch.create", false, true),
    ("git.branch.propose", false, false),
    ("git.commit", false, false),
    ("git.commit.record-manual", true, true),
    ("git.gate.skip", true, true),
    ("git.hook.install", false, true),
    ("git.hook.propose", false, false),
    ("git.hook.run", false, false),
    ("git.hook.status", false, false),
    ("git.inspect", false, false),
    ("plan.alternatives", false, false),
    ("plan.amend.apply", true, false),
    ("plan.amend.approve", true, true),
    ("plan.amend.cancel", true, true),
    ("plan.amend.propose", true, false),
    ("plan.amend.reject", true, true),
    ("plan.amend.withdraw", true, false),
    ("plan.apply", true, false),
    ("plan.approve", true, true),
    ("plan.archive", true, true),
    ("plan.context.add", true, false),
    ("plan.create", true, false),
    ("plan.decision.add", true, false),
    ("plan.decision.remove", true, false),
    ("plan.decision.update", true, false),
    ("plan.diff", false, false),
    ("plan.edge-case.remove", true, false),
    ("plan.edge-case.update", true, false),
    ("plan.file.add", true, false),
    ("plan.file.remove", true, false),
    ("plan.file.update", true, false),
    ("plan.finalize", true, false),
    ("plan.fork", true, false),
    ("plan.metadata.set", true, false),
    ("plan.next", false, false),
    ("plan.outcome.set", true, false),
    ("plan.review", false, false),
    ("plan.scan.accept", true, true),
    ("plan.scope.add", true, false),
    ("plan.scope.set", true, false),
    ("plan.select", true, true),
    ("plan.show", false, false),
    ("plan.summary.set", true, false),
    ("plan.task.add", true, false),
    ("plan.task.criterion.add", true, false),
    ("plan.task.criterion.remove", true, false),
    ("plan.task.criterion.update", true, false),
    ("plan.task.move", true, false),
    ("plan.task.remove", true, false),
    ("plan.task.step.add", true, false),
    ("plan.task.step.remove", true, false),
    ("plan.task.step.update", true, false),
    ("plan.task.update", true, false),
    ("plan.task.verification.add", true, false),
    ("plan.task.verification.remove", true, false),
    ("plan.task.verification.update", true, false),
    ("plan.validate", false, false),
    ("plan.verification.add", true, false),
    ("plan.verification.remove", true, false),
    ("plan.verification.update", true, false),
    ("project.doctor", false, false),
    ("project.import.legacy", true, false),
    ("project.init", false, false),
    ("project.migrate.legacy", false, false),
    ("project.scan", false, false),
    ("project.show", false, false),
    ("protocol.migrate", true, false),
    ("protocol.status", false, false),
    ("review.accept", true, true),
    ("review.disposition", true, true),
    ("review.record", true, false),
    ("review.resolve", true, false),
    ("review.rework", true, false),
    ("standards.apply", true, false),
    ("standards.catalog.build", true, false),
    ("standards.catalog.init", true, false),
    ("standards.catalog.validate", false, false),
    ("standards.conflict.list", false, false),
    ("standards.conflict.refresh", true, false),
    ("standards.conflict.resolve", true, true),
    ("standards.detect", false, false),
    ("standards.recommend", false, false),
    ("standards.sync", false, false),
];

/// Stable project identity embedded in every Agent context.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AgentProject {
    /// Discovered project root.
    pub root: String,
    /// Locked protocol version and revision.
    pub protocol: String,
}

/// Current Git worktree and active-binding facts exposed to an Agent.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AgentGitContext {
    /// Canonical worktree root.
    pub worktree: String,
    /// Canonical shared common-directory identity.
    pub common_dir: String,
    /// Current branch, absent for detached HEAD.
    pub branch: Option<String>,
    /// Current full HEAD object ID, absent on an unborn branch.
    pub head: Option<String>,
    /// Explicit branch, unborn, or detached classification.
    pub head_state: GitHeadState,
    /// Whether porcelain v2 reports no index/worktree/untracked changes.
    pub is_clean: bool,
    /// Sorted staged paths.
    pub staged_paths: Vec<String>,
    /// Sorted unstaged and untracked paths.
    pub unstaged_paths: Vec<String>,
    /// Current binding relationship for this worktree.
    pub binding_status: ActiveBindingStatus,
    /// Bound plan when the resolution contains one.
    pub bound_plan: Option<PlanId>,
}

/// Current plan identity and active execution slot exposed to an Agent.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AgentActivePlan {
    /// Stable plan identifier.
    pub id: PlanId,
    /// Current optimistic-concurrency revision.
    pub revision: u64,
    /// Current plan lifecycle state.
    pub status: PlanStatus,
    /// Active or blocked execution task when one owns the slot.
    pub active_task: Option<TaskId>,
}

/// One action that current protocol state forbids.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BlockedAction {
    /// Stable canonical action identifier.
    pub action: String,
    /// Concise protocol reason the action is unavailable.
    pub reason: String,
}

/// Complete dynamic context returned to Coding Agents.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AgentContext {
    /// Versioned context schema identifier.
    pub kind: &'static str,
    /// Stable actor identity for mutations invoked from this Agent context.
    pub executor_identity: &'static str,
    /// Discovered project and protocol identity.
    pub project: AgentProject,
    /// Git worktree facts when the project belongs to a repository.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git: Option<AgentGitContext>,
    /// Only active non-Done plan, when one exists.
    pub active_plan: Option<AgentActivePlan>,
    /// Project-level selected plan and live alternatives when candidates exist.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_selection: Option<ProjectPlanSelection>,
    /// Legal action identifiers in deterministic order.
    pub allowed_actions: Vec<String>,
    /// Important unavailable actions and stable reasons.
    pub blocked_actions: Vec<BlockedAction>,
    /// Exact selected standards package pins.
    pub standards: Vec<String>,
    /// Whether truncated discovery still requires explicit acceptance.
    #[serde(skip_serializing_if = "is_false")]
    pub scan_incomplete: bool,
    /// Whether the Agent must stop for explicit human approval.
    pub approval_required: bool,
    /// Complete canonical commands that may be executed next.
    pub next_actions: Vec<NextAction>,
}

/// Focused next-action view derived from the same current context.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AgentNextReport {
    /// Versioned next-action schema identifier.
    pub kind: &'static str,
    /// Stable actor identity for mutations invoked from this next-action view.
    pub executor_identity: &'static str,
    /// Current active plan identity, when one exists.
    pub active_plan: Option<AgentActivePlan>,
    /// Project-level selected plan and live alternatives when candidates exist.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_selection: Option<ProjectPlanSelection>,
    /// Whether the Agent must stop for explicit human approval.
    pub approval_required: bool,
    /// Important unavailable actions and stable reasons.
    pub blocked_actions: Vec<BlockedAction>,
    /// Complete canonical commands that may be executed next.
    pub next_actions: Vec<NextAction>,
}

/// One stable protocol capability advertised to an Agent.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AgentCapability {
    /// Stable canonical action identifier.
    pub id: String,
    /// Whether the action requires revision and request-ID mutation policy.
    pub mutates: bool,
    /// Whether the Agent must stop for explicit user approval before invocation.
    pub approval_boundary: bool,
}

/// Static machine-use contract for Agent CLI behavior.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AgentCapabilities {
    /// Versioned capabilities schema identifier.
    pub kind: &'static str,
    /// Stable actor identity required in canonical Agent mutation commands.
    pub executor_identity: &'static str,
    /// Locked protocol version and revision.
    pub protocol: String,
    /// Context schema produced by this CLI.
    pub context_kind: &'static str,
    /// Next-action schema produced by this CLI.
    pub next_kind: &'static str,
    /// Required invocation-mode flags for Agent queries.
    pub invocation: AgentInvocationPolicy,
    /// Required optimistic-concurrency fields for mutations.
    pub mutations: AgentMutationPolicy,
    /// Stable protocol actions in canonical identifier order.
    pub actions: Vec<AgentCapability>,
}

/// Required non-interactive invocation mode for every Agent query.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AgentInvocationPolicy {
    /// Whether every Agent command requires JSON output mode.
    pub requires_json: bool,
    /// Whether every Agent command forbids interactive input.
    pub requires_no_input: bool,
}

/// Required concurrency and idempotency metadata for every mutation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AgentMutationPolicy {
    /// Whether every mutation requires an expected revision.
    pub requires_expected_revision: bool,
    /// Whether every mutation requires an idempotency request identifier.
    pub requires_request_id: bool,
}

/// Application boundary for Agent-specific read-only protocol queries.
#[derive(Clone, Debug)]
pub struct AgentService {
    plans: PlanService,
}

impl AgentService {
    /// Discovers an initialized project and creates its Agent service.
    ///
    /// # Errors
    ///
    /// Returns an environment-unavailable error when no initialized project exists.
    pub fn discover(start: &Path) -> Result<Self, MinoError> {
        Ok(Self {
            plans: PlanService::discover(start)?,
        })
    }

    /// Returns the current one-plan Agent context.
    ///
    /// # Errors
    ///
    /// Returns a typed error for malformed/multiple active state, projection
    /// drift, or repository facts required to validate a Draft.
    pub fn context(&self) -> Result<AgentContext, MinoError> {
        let selection = self.plans.plan_selection()?;
        let active_plan = self.plans.active_plan()?;
        build_agent_context_with_selection(
            self.plans.root(),
            active_plan.as_ref(),
            Some(&selection),
        )
    }

    /// Returns only the current approval boundary and canonical next commands.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::context`].
    pub fn next(&self) -> Result<AgentNextReport, MinoError> {
        let context = self.context()?;
        Ok(AgentNextReport {
            kind: AGENT_NEXT_KIND,
            executor_identity: context.executor_identity,
            active_plan: context.active_plan,
            plan_selection: context.plan_selection,
            approval_required: context.approval_required,
            blocked_actions: context.blocked_actions,
            next_actions: context.next_actions,
        })
    }

    /// Returns the static machine-use contract for this protocol version.
    #[must_use]
    pub fn capabilities() -> AgentCapabilities {
        AgentCapabilities {
            kind: AGENT_CAPABILITIES_KIND,
            executor_identity: AGENT_EXECUTOR_IDENTITY,
            protocol: protocol_name(),
            context_kind: AGENT_CONTEXT_KIND,
            next_kind: AGENT_NEXT_KIND,
            invocation: AgentInvocationPolicy {
                requires_json: true,
                requires_no_input: true,
            },
            mutations: AgentMutationPolicy {
                requires_expected_revision: true,
                requires_request_id: true,
            },
            actions: CAPABILITIES
                .iter()
                .map(|(id, mutates, approval_boundary)| AgentCapability {
                    id: (*id).to_owned(),
                    mutates: *mutates,
                    approval_boundary: *approval_boundary,
                })
                .collect(),
        }
    }
}

/// Builds one deterministic Agent context for a supplied lifecycle state.
///
/// # Errors
///
/// Returns an error when Draft validation requires unavailable project facts.
pub fn build_agent_context(
    root: &Path,
    active_plan: Option<&Plan>,
) -> Result<AgentContext, MinoError> {
    build_agent_context_with_selection(root, active_plan, None)
}

fn build_agent_context_with_selection(
    root: &Path,
    active_plan: Option<&Plan>,
    plan_selection: Option<&ProjectPlanSelection>,
) -> Result<AgentContext, MinoError> {
    let project = AgentProject {
        root: root.to_string_lossy().into_owned(),
        protocol: protocol_name(),
    };
    let git = agent_git_context(root)?;
    let serialized_selection = plan_selection
        .filter(|selection| !selection.is_empty())
        .cloned();
    let Some(plan) = active_plan else {
        if serialized_selection.is_some() {
            return Ok(AgentContext {
                kind: AGENT_CONTEXT_KIND,
                executor_identity: AGENT_EXECUTOR_IDENTITY,
                project,
                git,
                active_plan: None,
                plan_selection: serialized_selection,
                allowed_actions: action_ids(&[
                    "plan.alternatives",
                    "plan.select",
                    "plan.diff",
                    "plan.show",
                    "plan.archive",
                ]),
                blocked_actions: vec![blocked(
                    "plan.create",
                    "Live alternatives require an explicit project plan selection",
                )],
                standards: Vec::new(),
                scan_incomplete: false,
                approval_required: true,
                next_actions: vec![alternatives_action()],
            });
        }
        return Ok(AgentContext {
            kind: AGENT_CONTEXT_KIND,
            executor_identity: AGENT_EXECUTOR_IDENTITY,
            project,
            git,
            active_plan: None,
            plan_selection: None,
            allowed_actions: vec!["plan.create".to_owned(), "project.import.legacy".to_owned()],
            blocked_actions: Vec::new(),
            standards: Vec::new(),
            scan_incomplete: false,
            approval_required: false,
            next_actions: Vec::new(),
        });
    };
    let active_plan = AgentActivePlan {
        id: plan.id().clone(),
        revision: plan.revision(),
        status: plan.status(),
        active_task: active_task(plan),
    };
    let standards = plan
        .standards()
        .iter()
        .map(|standard| format!("{}@{}", standard.package_id(), standard.version()))
        .collect();
    let scan_incomplete = plan.scan_is_incomplete().map_err(|error| {
        MinoError::new(
            ErrorCategory::DriftDetected,
            format!("Project scan state is malformed: {error}"),
        )
    })?;
    let guidance = guidance(root, plan)?;
    let has_alternatives = serialized_selection
        .as_ref()
        .is_some_and(|selection| !selection.alternatives.is_empty());
    let mut allowed_actions = guidance.allowed_actions;
    let mut next_actions = guidance.next_actions;
    if has_alternatives {
        for action in [
            "plan.alternatives",
            "plan.select",
            "plan.diff",
            "plan.archive",
        ] {
            if !allowed_actions.iter().any(|allowed| allowed == action) {
                allowed_actions.push(action.to_owned());
            }
        }
        next_actions = vec![alternatives_action()];
    }
    Ok(AgentContext {
        kind: AGENT_CONTEXT_KIND,
        executor_identity: AGENT_EXECUTOR_IDENTITY,
        project,
        git,
        active_plan: Some(active_plan),
        plan_selection: serialized_selection,
        allowed_actions,
        blocked_actions: guidance.blocked_actions,
        standards,
        scan_incomplete,
        approval_required: guidance.approval_required || has_alternatives,
        next_actions,
    })
}

fn alternatives_action() -> NextAction {
    NextAction {
        id: "plan.alternatives".to_owned(),
        argv: vec![
            "mino".to_owned(),
            "plan".to_owned(),
            "alternatives".to_owned(),
            "--format".to_owned(),
            "json".to_owned(),
            "--no-input".to_owned(),
        ],
    }
}

fn agent_git_context(root: &Path) -> Result<Option<AgentGitContext>, MinoError> {
    let Ok(facts) = GitAdapter::new(root).inspect() else {
        return Ok(None);
    };
    if !facts.repository || !facts.is_worktree {
        return Ok(None);
    }
    let resolution = ActiveBindingStore::new(root)
        .resolve(&facts)
        .map_err(|error| crate::application::git_binding::map_git_error(&error))?;
    let worktree = facts
        .worktree
        .as_deref()
        .and_then(Path::to_str)
        .ok_or_else(|| {
            MinoError::new(
                crate::ErrorCategory::DriftDetected,
                "Git worktree path is not valid UTF-8",
            )
        })?
        .replace('\\', "/");
    let common_dir = facts
        .common_dir
        .as_deref()
        .and_then(Path::to_str)
        .ok_or_else(|| {
            MinoError::new(
                crate::ErrorCategory::DriftDetected,
                "Git common-directory path is not valid UTF-8",
            )
        })?
        .replace('\\', "/");
    Ok(Some(AgentGitContext {
        worktree,
        common_dir,
        branch: facts.branch,
        head: facts.head,
        head_state: facts.head_state,
        is_clean: facts.is_clean,
        staged_paths: facts.staged_paths,
        unstaged_paths: facts.unstaged_paths,
        binding_status: resolution.status,
        bound_plan: resolution.binding.map(|binding| binding.plan_id),
    }))
}

struct Guidance {
    allowed_actions: Vec<String>,
    blocked_actions: Vec<BlockedAction>,
    approval_required: bool,
    next_actions: Vec<NextAction>,
}

fn guidance(root: &Path, plan: &Plan) -> Result<Guidance, MinoError> {
    if plan.has_pending_amendment() {
        return Ok(pending_amendment_guidance(plan));
    }
    match plan.status() {
        PlanStatus::Draft => draft_guidance(root, plan),
        PlanStatus::Ready => ready_guidance(root, plan),
        PlanStatus::InProgress => Ok(in_progress_guidance(plan)),
        PlanStatus::Blocked if plan.is_blocked_for_material_review() => {
            Ok(material_review_blocked_guidance(plan))
        }
        PlanStatus::Blocked => Ok(Guidance {
            allowed_actions: action_ids(&["exec.resume"]),
            blocked_actions: vec![blocked(
                "exec.start",
                "The plan must resume from its recorded blocked state",
            )],
            approval_required: false,
            next_actions: vec![resume_action(plan)],
        }),
        PlanStatus::Review => Ok(review_guidance(plan)),
        PlanStatus::Done => Ok(Guidance {
            allowed_actions: action_ids(&["plan.show"]),
            blocked_actions: vec![blocked("exec.start", "The plan is already Done")],
            approval_required: false,
            next_actions: Vec::new(),
        }),
    }
}

fn material_review_blocked_guidance(plan: &Plan) -> Guidance {
    let accepted_change = plan.review_items().iter().any(|item| {
        item.classification() == ReviewClassification::MaterialChange
            && item.status() == ReviewStatus::Blocked
            && item.disposition() == Some(MaterialReviewDisposition::AcceptChange)
    });
    if accepted_change {
        Guidance {
            allowed_actions: action_ids(&["plan.show", "plan.amend.propose"]),
            blocked_actions: vec![
                blocked(
                    "exec.resume",
                    "Accepted Material review feedback requires a protected amendment",
                ),
                blocked("review.accept", "Material review feedback remains blocked"),
            ],
            approval_required: false,
            next_actions: Vec::new(),
        }
    } else {
        Guidance {
            allowed_actions: action_ids(&["plan.show", "review.disposition"]),
            blocked_actions: vec![
                blocked(
                    "plan.amend.propose",
                    "The Material review request needs an explicit disposition first",
                ),
                blocked(
                    "exec.resume",
                    "Material review feedback requires an explicit product decision",
                ),
                blocked("review.accept", "Material review feedback remains blocked"),
            ],
            approval_required: true,
            next_actions: Vec::new(),
        }
    }
}

fn pending_amendment_guidance(plan: &Plan) -> Guidance {
    let amendment = plan
        .pending_amendment()
        .expect("caller established a pending amendment");
    let blocked_actions = vec![
        blocked(
            "exec.start",
            "The pending protected amendment must be applied first",
        ),
        blocked(
            "evidence.add",
            "Evidence cannot be captured against unapplied plan inputs",
        ),
        blocked(
            "git.commit",
            "Git commits cannot cross an unapplied plan change",
        ),
    ];
    match (amendment.classification(), amendment.status()) {
        (AmendmentClassification::Material, AmendmentStatus::ApprovalRequired) => Guidance {
            allowed_actions: action_ids(&["plan.show", "plan.amend.reject", "plan.amend.withdraw"]),
            blocked_actions: [
                blocked_actions,
                vec![blocked(
                    "plan.amend.apply",
                    "Material amendments require explicit approval before apply",
                )],
            ]
            .concat(),
            approval_required: true,
            next_actions: Vec::new(),
        },
        (AmendmentClassification::Material, AmendmentStatus::Approved) => Guidance {
            allowed_actions: action_ids(&["plan.show", "plan.amend.apply", "plan.amend.cancel"]),
            blocked_actions,
            approval_required: false,
            next_actions: vec![amendment_apply_action(plan, amendment.id())],
        },
        (AmendmentClassification::Minor, AmendmentStatus::Proposed) => Guidance {
            allowed_actions: action_ids(&["plan.show", "plan.amend.apply", "plan.amend.withdraw"]),
            blocked_actions,
            approval_required: false,
            next_actions: vec![amendment_apply_action(plan, amendment.id())],
        },
        _ => Guidance {
            allowed_actions: action_ids(&["plan.show"]),
            blocked_actions,
            approval_required: true,
            next_actions: Vec::new(),
        },
    }
}

fn review_guidance(plan: &Plan) -> Guidance {
    let unresolved = plan
        .review_items()
        .iter()
        .find(|item| matches!(item.status(), ReviewStatus::Open | ReviewStatus::InProgress));
    let Some(item) = unresolved else {
        if !plan.final_outcome().is_complete() {
            return Guidance {
                allowed_actions: action_ids(&["plan.show", "plan.outcome.set"]),
                blocked_actions: vec![blocked(
                    "review.accept",
                    "A complete Final Outcome is required before acceptance",
                )],
                approval_required: false,
                next_actions: Vec::new(),
            };
        }
        return Guidance {
            allowed_actions: action_ids(&["plan.show", "review.record"]),
            blocked_actions: vec![
                blocked(
                    "review.accept",
                    "Explicit user review acceptance is required; the Agent must stop",
                ),
                blocked(
                    "exec.start",
                    "Implementation is complete and awaiting review",
                ),
            ],
            approval_required: true,
            next_actions: Vec::new(),
        };
    };
    let mut allowed_actions = action_ids(&["plan.show", "review.record"]);
    if !plan.final_outcome().is_complete() {
        allowed_actions.push("plan.outcome.set".to_owned());
    }
    let next_actions = match item.status() {
        ReviewStatus::Open => {
            allowed_actions.push("review.rework".to_owned());
            if item.classification() == ReviewClassification::AcceptanceDefect {
                vec![review_item_action(
                    plan,
                    "review.rework",
                    "rework",
                    item.id(),
                )]
            } else {
                Vec::new()
            }
        }
        ReviewStatus::InProgress => {
            allowed_actions.push("review.resolve".to_owned());
            vec![review_item_action(
                plan,
                "review.resolve",
                "resolve",
                item.id(),
            )]
        }
        ReviewStatus::Resolved | ReviewStatus::Blocked | ReviewStatus::Deferred => Vec::new(),
    };
    Guidance {
        allowed_actions,
        blocked_actions: vec![
            blocked(
                "review.accept",
                "Every blocking review item must be resolved before acceptance",
            ),
            blocked(
                "exec.start",
                "Review rework has not been selected or resolved",
            ),
        ],
        approval_required: false,
        next_actions,
    }
}

fn draft_guidance(root: &Path, plan: &Plan) -> Result<Guidance, MinoError> {
    let requires_scan_acceptance = plan.scan_is_incomplete().map_err(|error| {
        MinoError::new(
            ErrorCategory::DriftDetected,
            format!("Project scan state is malformed: {error}"),
        )
    })?;
    let missing = draft_missing(plan);
    let (next_actions, is_valid, blocking_count, requires_conflict_decision) = if missing.is_empty()
    {
        let report = validate_plan(root, plan)?;
        let requires_conflict_decision = report
            .findings
            .iter()
            .any(|finding| finding.id == "POLICY-STANDARD-CONFLICT-UNRESOLVED");
        (
            report.next_actions,
            report.valid,
            report
                .findings
                .iter()
                .filter(|finding| finding.blocking)
                .count(),
            requires_conflict_decision,
        )
    } else {
        (
            draft_next_actions(plan, &missing),
            false,
            missing.len(),
            false,
        )
    };
    let mut allowed_actions = action_ids(&[
        "plan.apply",
        "plan.metadata.set",
        "plan.summary.set",
        "plan.context.add",
        "plan.scope.set",
        "plan.scope.add",
        "plan.decision.add",
        "plan.decision.remove",
        "plan.decision.update",
        "plan.edge-case.remove",
        "plan.edge-case.update",
        "plan.task.add",
        "plan.task.update",
        "plan.task.remove",
        "plan.task.move",
        "plan.task.step.add",
        "plan.task.step.update",
        "plan.task.step.remove",
        "plan.task.criterion.add",
        "plan.task.criterion.update",
        "plan.task.criterion.remove",
        "plan.task.verification.add",
        "plan.task.verification.update",
        "plan.task.verification.remove",
        "plan.file.add",
        "plan.file.update",
        "plan.file.remove",
        "plan.verification.add",
        "plan.verification.update",
        "plan.verification.remove",
        "plan.validate",
        "plan.show",
        "standards.apply",
        "standards.conflict.list",
        "standards.conflict.refresh",
        "standards.conflict.resolve",
    ]);
    if requires_scan_acceptance {
        allowed_actions.push("plan.scan.accept".to_owned());
    }
    let mut blocked_actions = vec![
        blocked("plan.approve", "The plan must be Ready before approval"),
        blocked(
            "exec.start",
            "The plan must be Ready and explicitly approved before execution",
        ),
    ];
    if is_valid {
        allowed_actions.push("plan.finalize".to_owned());
    } else {
        blocked_actions.insert(
            0,
            blocked(
                "plan.finalize",
                &format!("Plan validation has {blocking_count} blocking item(s)"),
            ),
        );
    }
    Ok(Guidance {
        allowed_actions,
        blocked_actions,
        approval_required: requires_conflict_decision || requires_scan_acceptance,
        next_actions,
    })
}

#[allow(clippy::trivially_copy_pass_by_ref)]
const fn is_false(value: &bool) -> bool {
    !*value
}

fn ready_guidance(root: &Path, plan: &Plan) -> Result<Guidance, MinoError> {
    let has_open_deviation = plan
        .execution_state()
        .map_err(|error| {
            MinoError::new(
                ErrorCategory::DriftDetected,
                format!("Execution state is malformed: {error}"),
            )
        })?
        .deviations()
        .iter()
        .any(crate::domain::Deviation::is_open);
    let validation = validate_plan(root, plan)?;
    if !validation.valid {
        let requires_conflict_decision = validation
            .findings
            .iter()
            .any(|finding| finding.id == "POLICY-STANDARD-CONFLICT-UNRESOLVED");
        let mut allowed_actions = action_ids(&[
            "plan.show",
            "plan.validate",
            "plan.amend.propose",
            "standards.conflict.list",
            "standards.conflict.refresh",
            "standards.conflict.resolve",
        ]);
        append_ready_deviation_actions(&mut allowed_actions, has_open_deviation);
        return Ok(Guidance {
            allowed_actions,
            blocked_actions: vec![
                blocked(
                    "plan.approve",
                    "Current repository or standards validation is blocking",
                ),
                blocked(
                    "exec.start",
                    "Current repository or standards validation is blocking",
                ),
            ],
            approval_required: requires_conflict_decision,
            next_actions: validation.next_actions,
        });
    }
    if !plan.has_plan_approval() {
        let mut allowed_actions = action_ids(&[
            "plan.show",
            "plan.validate",
            "plan.review",
            "plan.amend.propose",
            "standards.conflict.list",
            "standards.conflict.refresh",
            "standards.conflict.resolve",
        ]);
        append_ready_deviation_actions(&mut allowed_actions, has_open_deviation);
        return Ok(Guidance {
            allowed_actions,
            blocked_actions: vec![
                blocked(
                    "plan.approve",
                    "Explicit user approval is required; the Agent must stop",
                ),
                blocked(
                    "exec.start",
                    "Plan execution requires explicit user approval",
                ),
            ],
            approval_required: true,
            next_actions: Vec::new(),
        });
    }
    let next_actions = first_incomplete_task(plan)
        .map(|task_id| vec![start_action(plan, task_id)])
        .unwrap_or_default();
    let mut allowed_actions = action_ids(&[
        "plan.show",
        "plan.validate",
        "plan.review",
        "plan.amend.propose",
        "exec.start",
        "standards.conflict.list",
        "standards.conflict.refresh",
        "standards.conflict.resolve",
    ]);
    append_ready_deviation_actions(&mut allowed_actions, has_open_deviation);
    Ok(Guidance {
        allowed_actions,
        blocked_actions: vec![blocked(
            "git.commit",
            "No task has completed its verification and commit gate",
        )],
        approval_required: false,
        next_actions,
    })
}

fn append_ready_deviation_actions(actions: &mut Vec<String>, has_open_deviation: bool) {
    if has_open_deviation {
        actions.extend(action_ids(&[
            "exec.deviation.list",
            "exec.deviation.supersede",
        ]));
    }
}

fn in_progress_guidance(plan: &Plan) -> Guidance {
    let active = plan
        .tasks()
        .iter()
        .find(|task| task.status() == TaskStatus::InProgress);
    let can_commit = pending_commit_task(plan).is_some();
    let has_automatic_commit_consent = plan.git_readiness().git_flow_enabled()
        && plan.git_readiness().git_flow_consent() == crate::domain::GitFlowConsent::Approved;
    let (mut allowed_actions, next_actions) = if let Some(task) = active {
        let next_actions = next_execution_check(plan).map_or_else(
            || {
                if task.acceptance_criteria().iter().any(|criterion| {
                    !matches!(
                        criterion.status(),
                        crate::domain::CriterionStatus::Passed
                            | crate::domain::CriterionStatus::AcceptedException
                    )
                }) {
                    vec![evidence_list_action(plan, task.id())]
                } else {
                    vec![complete_action(plan, task.id())]
                }
            },
            |check_id| vec![check_action(plan, check_id)],
        );
        (in_progress_task_actions(), next_actions)
    } else if let Some(task_id) = pending_commit_task(plan) {
        if has_automatic_commit_consent {
            (
                action_ids(&[
                    "git.commit",
                    "git.commit.record-manual",
                    "git.gate.skip",
                    "exec.block",
                ]),
                vec![commit_action(plan, task_id)],
            )
        } else {
            (
                action_ids(&["git.commit.record-manual", "git.gate.skip", "exec.block"]),
                Vec::new(),
            )
        }
    } else if let Some(task_id) = first_incomplete_task(plan) {
        (
            action_ids(&["exec.start", "exec.block"]),
            vec![start_action(plan, task_id)],
        )
    } else if let Some(check_id) = failed_required_global_check(plan) {
        (
            action_ids(&["exec.check.run", "exec.rework", "exec.block"]),
            plan.task_order()
                .iter()
                .filter_map(|task_id| plan.task(task_id))
                .filter(|task| task.status() == TaskStatus::Done)
                .map(|task| rework_action(plan, task.id(), check_id))
                .collect(),
        )
    } else if let Some(check_id) = next_execution_check(plan) {
        (
            action_ids(&["exec.check.run", "exec.block"]),
            vec![check_action(plan, check_id)],
        )
    } else if !plan.final_outcome().is_complete() {
        (action_ids(&["plan.outcome.set", "exec.block"]), Vec::new())
    } else {
        (
            action_ids(&["exec.finish", "exec.block"]),
            vec![finish_action(plan)],
        )
    };
    allowed_actions.push("plan.amend.propose".to_owned());
    let mut blocked_actions = if can_commit && has_automatic_commit_consent {
        Vec::new()
    } else if can_commit {
        vec![blocked(
            "git.commit",
            "Automatic commit requires Approved Git Flow consent; record a manually approved commit or approved skip",
        )]
    } else {
        vec![blocked(
            "git.commit",
            "Task verification or completion is incomplete",
        )]
    };
    if !plan.final_outcome().is_complete() {
        blocked_actions.push(blocked(
            "exec.finish",
            "A complete Final Outcome is required before Review",
        ));
    }
    Guidance {
        allowed_actions,
        blocked_actions,
        approval_required: can_commit && !has_automatic_commit_consent,
        next_actions,
    }
}

fn in_progress_task_actions() -> Vec<String> {
    action_ids(&[
        "exec.check.run",
        "exec.checkpoint",
        "exec.criterion.pass",
        "exec.complete",
        "exec.block",
        "exec.deviation.list",
        "exec.deviation.record",
        "exec.deviation.reject",
        "exec.deviation.resolve",
        "exec.deviation.supersede",
    ])
}

fn active_task(plan: &Plan) -> Option<TaskId> {
    plan.tasks()
        .iter()
        .find(|task| matches!(task.status(), TaskStatus::InProgress | TaskStatus::Blocked))
        .map(|task| task.id().clone())
}

fn first_incomplete_task(plan: &Plan) -> Option<&TaskId> {
    plan.task_order().iter().find(|task_id| {
        plan.task(task_id)
            .is_some_and(|task| task.status() != TaskStatus::Done)
    })
}

fn pending_commit_task(plan: &Plan) -> Option<&TaskId> {
    plan.task_order().iter().find(|task_id| {
        plan.task(task_id).is_some_and(|task| {
            task.status() == TaskStatus::Done
                && task.commit_gate().is_some_and(|gate| {
                    gate.is_required()
                        && matches!(
                            gate.status(),
                            crate::domain::CommitStatus::Pending
                                | crate::domain::CommitStatus::Blocked
                        )
                })
        })
    })
}

fn start_action(plan: &Plan, task_id: &TaskId) -> NextAction {
    mutation_action(
        plan,
        "exec.start",
        &["exec", "start"],
        vec!["--task".to_owned(), task_id.to_string()],
    )
}

fn resume_action(plan: &Plan) -> NextAction {
    mutation_action(plan, "exec.resume", &["exec", "resume"], Vec::new())
}

fn check_action(plan: &Plan, check_id: &CheckId) -> NextAction {
    mutation_action(
        plan,
        "exec.check.run",
        &["exec", "check", "run"],
        vec!["--check".to_owned(), check_id.to_string()],
    )
}

fn complete_action(plan: &Plan, task_id: &TaskId) -> NextAction {
    mutation_action(
        plan,
        "exec.complete",
        &["exec", "complete"],
        vec!["--task".to_owned(), task_id.to_string()],
    )
}

fn finish_action(plan: &Plan) -> NextAction {
    mutation_action(plan, "exec.finish", &["exec", "finish"], Vec::new())
}

fn rework_action(plan: &Plan, task_id: &TaskId, check_id: &CheckId) -> NextAction {
    mutation_action(
        plan,
        "exec.rework",
        &["exec", "rework"],
        vec![
            "--task".to_owned(),
            task_id.to_string(),
            "--reason".to_owned(),
            format!("Required global check {check_id} failed"),
        ],
    )
}

fn amendment_apply_action(plan: &Plan, change_id: &str) -> NextAction {
    mutation_action(
        plan,
        "plan.amend.apply",
        &["plan", "amend", "apply"],
        vec!["--change".to_owned(), change_id.to_owned()],
    )
}

fn review_item_action(plan: &Plan, id: &str, command: &str, review_id: &str) -> NextAction {
    mutation_action(
        plan,
        id,
        &["review", command],
        vec!["--review".to_owned(), review_id.to_owned()],
    )
}

fn commit_action(plan: &Plan, task_id: &TaskId) -> NextAction {
    NextAction {
        id: "git.commit".to_owned(),
        argv: vec![
            "mino".to_owned(),
            "git".to_owned(),
            "commit".to_owned(),
            "--plan".to_owned(),
            plan.id().to_string(),
            "--task".to_owned(),
            task_id.to_string(),
            "--format".to_owned(),
            "json".to_owned(),
            "--no-input".to_owned(),
        ],
    }
}

fn evidence_list_action(plan: &Plan, task_id: &TaskId) -> NextAction {
    NextAction {
        id: "evidence.list".to_owned(),
        argv: vec![
            "mino".to_owned(),
            "evidence".to_owned(),
            "list".to_owned(),
            "--plan".to_owned(),
            plan.id().to_string(),
            "--task".to_owned(),
            task_id.to_string(),
            "--format".to_owned(),
            "json".to_owned(),
            "--no-input".to_owned(),
        ],
    }
}

fn next_execution_check(plan: &Plan) -> Option<&CheckId> {
    if let Some(task) = plan
        .tasks()
        .iter()
        .find(|task| task.status() == TaskStatus::InProgress)
    {
        return task
            .verification_checks()
            .iter()
            .find(|check| {
                matches!(
                    check.status(),
                    CheckStatus::Pending | CheckStatus::Failed | CheckStatus::Stale
                )
            })
            .map(crate::domain::VerificationCheck::id);
    }
    if plan
        .tasks()
        .iter()
        .all(|task| task.status() == TaskStatus::Done)
    {
        return plan
            .global_verification()
            .iter()
            .find(|check| {
                matches!(
                    check.status(),
                    CheckStatus::Pending | CheckStatus::Failed | CheckStatus::Stale
                )
            })
            .map(crate::domain::VerificationCheck::id);
    }
    None
}

fn failed_required_global_check(plan: &Plan) -> Option<&CheckId> {
    plan.global_verification()
        .iter()
        .find(|check| check.is_required() && check.status() == CheckStatus::Failed)
        .map(crate::domain::VerificationCheck::id)
}

fn mutation_action(plan: &Plan, id: &str, command: &[&str], extra: Vec<String>) -> NextAction {
    let mut argv = vec!["mino".to_owned()];
    argv.extend(command.iter().map(|part| (*part).to_owned()));
    argv.extend(["--plan".to_owned(), plan.id().to_string()]);
    argv.extend(extra);
    argv.extend([
        "--expect-revision".to_owned(),
        plan.revision().to_string(),
        "--request-id".to_owned(),
        derived_request_id(plan, id),
        "--actor".to_owned(),
        AGENT_EXECUTOR_IDENTITY.to_owned(),
    ]);
    argv.extend([
        "--format".to_owned(),
        "json".to_owned(),
        "--no-input".to_owned(),
    ]);
    NextAction {
        id: id.to_owned(),
        argv,
    }
}

fn action_ids(actions: &[&str]) -> Vec<String> {
    actions.iter().map(|action| (*action).to_owned()).collect()
}

fn blocked(action: &str, reason: &str) -> BlockedAction {
    BlockedAction {
        action: action.to_owned(),
        reason: reason.to_owned(),
    }
}

fn protocol_name() -> String {
    format!("{CURRENT_PROTOCOL_VERSION}.{CURRENT_PROTOCOL_REVISION}")
}
