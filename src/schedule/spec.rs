//! Versioned inert handoff specifications for external scheduled-task systems.

use std::fs;
use std::path::{Component, Path, PathBuf};

use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::application::monitor::MonitorBounds;
use crate::domain::{CheckId, Plan, PlanId, RequestId, TaskId, Timestamp, VerificationCheck};
use crate::project;
use crate::render::render_plan;
use crate::store::{StorePaths, canonical_json_bytes, sha256_digest};
use crate::{ErrorCategory, MinoError};

/// Stable schema identifier for scheduler-neutral task handoff specifications.
pub const SCHEDULE_SPEC_KIND: &str = "mino.scheduled-task-spec/v1";

const MAX_PLAN_OR_PROJECTION_BYTES: u64 = 16 * 1024 * 1024;
const MAX_EXECUTION_ENVIRONMENT_BYTES: usize = 256;
const MAX_POLICY_TEXT_BYTES: usize = 4 * 1024;
const MAX_RESULT_PATH_BYTES: usize = 1024;
const MAX_DISPATCH_ATTEMPTS: u32 = 100;
const MAX_DISPATCH_RETRY_MILLISECONDS: u64 = 86_400_000;
const MAX_SCHEDULE_WINDOW_MILLISECONDS: u64 = 31 * 86_400_000;

/// Inputs required to generate one bounded, inert external-scheduler handoff.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScheduleSpecRequest {
    /// Exact current plan identifier.
    pub plan_id: PlanId,
    /// Exact current plan revision bound into the scheduled Mino argv.
    pub expected_revision: u64,
    /// Existing uniquely identified planned check.
    pub check_id: CheckId,
    /// Stable idempotency identifier used by the eventual monitor invocation.
    pub execution_request_id: RequestId,
    /// Actor recorded by the eventual monitor attempts.
    pub actor: String,
    /// Human-readable execution-environment identifier.
    pub execution_environment: String,
    /// Finite internal planned-check monitor policy.
    pub monitor_bounds: MonitorBounds,
    /// Earliest RFC3339 instant when an external scheduler may dispatch.
    pub trigger_at: Timestamp,
    /// Hard RFC3339 instant after which an external scheduler must stop.
    pub expires_at: Timestamp,
    /// Maximum number of external dispatch/recovery attempts.
    pub max_dispatch_attempts: u32,
    /// Finite delay between external dispatch/recovery attempts.
    pub dispatch_retry_milliseconds: u64,
    /// Explicit condition defining successful scheduled work.
    pub success_condition: String,
    /// Explicit condition defining when all external observation must stop.
    pub stop_condition: String,
    /// Explicit response when dispatch or execution fails.
    pub failure_handling: String,
    /// Safe project-relative destination for the external scheduler's result.
    pub result_destination: PathBuf,
}

/// Immutable project, plan, revision, and check identity bound into a handoff.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScheduledProjectContext {
    /// Canonical absolute project root used as the external working directory.
    pub project_root: String,
    /// Exact plan identifier.
    pub plan_id: PlanId,
    /// Exact plan revision.
    pub plan_revision: u64,
    /// Digest of canonical current plan bytes and the matching immutable snapshot.
    pub plan_digest: String,
    /// Digest of the verified managed Markdown projection.
    pub projection_digest: String,
    /// Exact planned check identifier.
    pub check_id: CheckId,
    /// Owning task when the check is task-scoped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<TaskId>,
    /// Digest of the authored check command, cwd, expected exit, and requirement.
    pub check_digest: String,
}

/// Finite internal monitor policy embedded in the scheduled Mino command.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScheduledMonitorPolicy {
    /// Maximum planned-check invocations.
    pub max_attempts: u32,
    /// Delay between failed planned-check invocations.
    pub interval_milliseconds: u64,
    /// Complete internal attempt-and-wait deadline.
    pub deadline_milliseconds: u64,
    /// Deterministic timeout allocated to each planned-check process.
    pub check_timeout_milliseconds: u64,
}

/// Complete command-based execution instruction for an external scheduler.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScheduledExecution {
    /// Stable instruction discriminator; currently always `command`.
    pub instruction_kind: String,
    /// User-declared execution-environment identifier.
    pub environment: String,
    /// Canonical absolute directory in which the external scheduler must run.
    pub working_directory: String,
    /// Original project-relative working directory of the authored check.
    pub check_working_directory: String,
    /// Expected exit code of the authored planned check.
    pub expected_check_exit_code: i32,
    /// Complete argv for one idempotent, internally bounded Mino monitor command.
    pub argv: Vec<String>,
    /// Finite monitor policy encoded by the argv.
    pub monitor: ScheduledMonitorPolicy,
}

/// Finite one-trigger external dispatch policy.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScheduledTrigger {
    /// Stable trigger discriminator; currently always `once`.
    pub trigger_kind: String,
    /// Earliest external dispatch instant.
    pub trigger_at: Timestamp,
    /// Hard external stop instant.
    pub expires_at: Timestamp,
    /// Complete trigger-to-expiry window.
    pub window_milliseconds: u64,
    /// Maximum external dispatch/recovery attempts.
    pub max_dispatch_attempts: u32,
    /// Delay between failed external dispatch/recovery attempts.
    pub dispatch_retry_milliseconds: u64,
    /// Conservative process-and-retry budget required by this specification.
    pub required_budget_milliseconds: u64,
}

/// Explicit outcome, stop, failure, and result-delivery policy.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScheduledOutcomePolicy {
    /// Condition that makes the scheduled task successful.
    pub success_condition: String,
    /// Condition that ends further external observation or dispatch.
    pub stop_condition: String,
    /// Required behavior after dispatch or execution failure.
    pub failure_handling: String,
    /// Normalized project-relative destination for the external result.
    pub result_destination: String,
}

/// Explicit authorization boundary for external scheduled-task creation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScheduledAuthorization {
    /// Whether another explicit authorization is required before creation.
    pub external_creation_required: bool,
    /// Whether this inert emission itself granted creation authority.
    pub authorization_granted: bool,
    /// Stable operator instruction describing the remaining boundary.
    pub instruction: String,
}

/// Side-effect declaration for the specification-emission command.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScheduledSideEffects {
    /// Whether a scheduler task was created or modified.
    pub scheduler_mutated: bool,
    /// Whether emission performed network access.
    pub network_accessed: bool,
    /// Whether emission changed Mino plan, event, evidence, or binding state.
    pub mino_state_mutated: bool,
}

/// Complete versioned scheduler-neutral task handoff.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScheduledTaskSpec {
    /// Stable schema discriminator.
    pub spec_kind: String,
    /// Digest of every specification field other than this digest.
    pub spec_digest: String,
    /// Exact project/plan/check identity.
    pub project: ScheduledProjectContext,
    /// Complete command and execution environment.
    pub execution: ScheduledExecution,
    /// Finite external trigger/expiry/retry policy.
    pub trigger: ScheduledTrigger,
    /// Explicit success, stop, failure, and destination policy.
    pub outcome: ScheduledOutcomePolicy,
    /// Separate external-creation authorization boundary.
    pub authorization: ScheduledAuthorization,
    /// Auditable zero-side-effect declaration for emission.
    pub emission_side_effects: ScheduledSideEffects,
}

impl ScheduledTaskSpec {
    /// Returns a generated JSON Schema for the complete handoff specification.
    #[must_use]
    pub fn schema() -> schemars::Schema {
        schema_for!(Self)
    }

    /// Returns canonical key-sorted JSON bytes for this specification.
    ///
    /// # Errors
    ///
    /// Returns an environment error when serialization unexpectedly fails.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, MinoError> {
        canonical_json_bytes(self)
            .map_err(|error| environment_error(format!("Failed to encode schedule spec: {error}")))
    }

    /// Verifies the embedded digest against every other specification field.
    ///
    /// # Errors
    ///
    /// Returns an environment error when canonical digest input cannot be encoded.
    pub fn verify_digest(&self) -> Result<bool, MinoError> {
        Ok(self.spec_digest == schedule_digest(self)?)
    }
}

#[derive(Serialize)]
struct ScheduleDigestInput<'a> {
    spec_kind: &'a str,
    project: &'a ScheduledProjectContext,
    execution: &'a ScheduledExecution,
    trigger: &'a ScheduledTrigger,
    outcome: &'a ScheduledOutcomePolicy,
    authorization: &'a ScheduledAuthorization,
    emission_side_effects: ScheduledSideEffects,
}

#[derive(Serialize)]
struct CheckDigestInput<'a> {
    id: &'a CheckId,
    command: &'a [String],
    cwd: &'a str,
    expected_exit_code: i32,
    required: bool,
}

struct SelectedCheck<'a> {
    task_id: Option<TaskId>,
    check: &'a VerificationCheck,
}

/// Read-only service that emits complete external scheduled-task handoffs.
#[derive(Clone, Debug)]
pub struct ScheduleSpecService {
    root: PathBuf,
}

impl ScheduleSpecService {
    /// Discovers an existing project without creating scheduler or Mino state.
    ///
    /// # Errors
    ///
    /// Returns an environment error when no project root can be discovered.
    pub fn discover(start: &Path) -> Result<Self, MinoError> {
        Ok(Self {
            root: project::discover(start)?.path().to_path_buf(),
        })
    }

    /// Generates one canonical, bounded, scheduler-neutral task specification.
    ///
    /// The method reads and verifies current plan/snapshot/projection bytes but
    /// never writes Mino state, accesses the network, or calls a scheduler API.
    ///
    /// # Errors
    ///
    /// Returns a typed error for incomplete text, stale revisions, ineligible
    /// or duplicate checks, unbounded trigger/retry policies, unsafe result
    /// paths, corrupt plan/projection bytes, or non-UTF-8 project paths.
    pub fn generate(&self, request: ScheduleSpecRequest) -> Result<ScheduledTaskSpec, MinoError> {
        let validated = validate_request(&self.root, request)?;
        let (plan, plan_bytes, projection_digest) =
            load_plan_read_only(&self.root, &validated.plan_id)?;
        if plan.revision() != validated.expected_revision {
            return Err(revision_error(format!(
                "Plan {} is revision {}, not expected revision {}",
                validated.plan_id,
                plan.revision(),
                validated.expected_revision
            )));
        }
        if plan.is_archived() {
            return Err(policy_error("Archived plans cannot produce scheduled work"));
        }
        if plan.has_pending_amendment() {
            return Err(policy_error(
                "Apply the pending amendment before scheduling a planned check",
            ));
        }
        let selected = select_check(&plan, &validated.check_id)?;
        let mut eligibility = plan.clone();
        eligibility
            .begin_check_run(&validated.check_id, plan.metadata().updated_at().clone())
            .map_err(|error| policy_error(error.to_string()))?;
        let project_root = self
            .root
            .to_str()
            .ok_or_else(|| environment_error("Project root is not valid UTF-8"))?
            .to_owned();
        validate_check_working_directory(&self.root, selected.check.cwd())?;
        let monitor = ScheduledMonitorPolicy {
            max_attempts: validated.monitor_bounds.max_attempts(),
            interval_milliseconds: validated.monitor_bounds.interval_milliseconds(),
            deadline_milliseconds: validated.monitor_bounds.deadline_milliseconds(),
            check_timeout_milliseconds: validated.monitor_bounds.check_timeout_milliseconds(),
        };
        let argv = monitor_argv(&project_root, &validated);
        let execution = ScheduledExecution {
            instruction_kind: "command".to_owned(),
            environment: validated.execution_environment,
            working_directory: project_root.clone(),
            check_working_directory: selected.check.cwd().to_owned(),
            expected_check_exit_code: selected.check.expected_exit_code(),
            argv,
            monitor,
        };
        let project = ScheduledProjectContext {
            project_root,
            plan_id: plan.id().clone(),
            plan_revision: plan.revision(),
            plan_digest: sha256_digest(&plan_bytes),
            projection_digest,
            check_id: selected.check.id().clone(),
            task_id: selected.task_id,
            check_digest: check_digest(selected.check)?,
        };
        let trigger = ScheduledTrigger {
            trigger_kind: "once".to_owned(),
            trigger_at: validated.trigger_at,
            expires_at: validated.expires_at,
            window_milliseconds: validated.window_milliseconds,
            max_dispatch_attempts: validated.max_dispatch_attempts,
            dispatch_retry_milliseconds: validated.dispatch_retry_milliseconds,
            required_budget_milliseconds: validated.required_budget_milliseconds,
        };
        let outcome = ScheduledOutcomePolicy {
            success_condition: validated.success_condition,
            stop_condition: validated.stop_condition,
            failure_handling: validated.failure_handling,
            result_destination: validated.result_destination,
        };
        let mut spec = ScheduledTaskSpec {
            spec_kind: SCHEDULE_SPEC_KIND.to_owned(),
            spec_digest: String::new(),
            project,
            execution,
            trigger,
            outcome,
            authorization: ScheduledAuthorization {
                external_creation_required: true,
                authorization_granted: false,
                instruction: "Obtain separate explicit authorization before creating or updating this task in any external scheduler.".to_owned(),
            },
            emission_side_effects: ScheduledSideEffects {
                scheduler_mutated: false,
                network_accessed: false,
                mino_state_mutated: false,
            },
        };
        spec.spec_digest = schedule_digest(&spec)?;
        Ok(spec)
    }
}

struct ValidatedScheduleRequest {
    plan_id: PlanId,
    expected_revision: u64,
    check_id: CheckId,
    execution_request_id: RequestId,
    actor: String,
    execution_environment: String,
    monitor_bounds: MonitorBounds,
    trigger_at: Timestamp,
    expires_at: Timestamp,
    max_dispatch_attempts: u32,
    dispatch_retry_milliseconds: u64,
    success_condition: String,
    stop_condition: String,
    failure_handling: String,
    result_destination: String,
    window_milliseconds: u64,
    required_budget_milliseconds: u64,
}

fn validate_request(
    root: &Path,
    request: ScheduleSpecRequest,
) -> Result<ValidatedScheduleRequest, MinoError> {
    if request.expected_revision == 0 {
        return Err(input_error(
            "Scheduled work requires a positive plan revision",
        ));
    }
    let actor = normalized_text("actor", &request.actor, MAX_EXECUTION_ENVIRONMENT_BYTES)?;
    let execution_environment = normalized_text(
        "execution environment",
        &request.execution_environment,
        MAX_EXECUTION_ENVIRONMENT_BYTES,
    )?;
    let success_condition = normalized_text(
        "success condition",
        &request.success_condition,
        MAX_POLICY_TEXT_BYTES,
    )?;
    let stop_condition = normalized_text(
        "stop condition",
        &request.stop_condition,
        MAX_POLICY_TEXT_BYTES,
    )?;
    let failure_handling = normalized_text(
        "failure handling",
        &request.failure_handling,
        MAX_POLICY_TEXT_BYTES,
    )?;
    let result_destination = validate_result_destination(root, &request.result_destination)?;
    let (window_milliseconds, required_budget_milliseconds) = validate_trigger_bounds(
        &request.trigger_at,
        &request.expires_at,
        request.max_dispatch_attempts,
        request.dispatch_retry_milliseconds,
        request.monitor_bounds.deadline_milliseconds(),
    )?;
    Ok(ValidatedScheduleRequest {
        plan_id: request.plan_id,
        expected_revision: request.expected_revision,
        check_id: request.check_id,
        execution_request_id: request.execution_request_id,
        actor,
        execution_environment,
        monitor_bounds: request.monitor_bounds,
        trigger_at: request.trigger_at,
        expires_at: request.expires_at,
        max_dispatch_attempts: request.max_dispatch_attempts,
        dispatch_retry_milliseconds: request.dispatch_retry_milliseconds,
        success_condition,
        stop_condition,
        failure_handling,
        result_destination,
        window_milliseconds,
        required_budget_milliseconds,
    })
}

fn validate_trigger_bounds(
    trigger_at: &Timestamp,
    expires_at: &Timestamp,
    max_dispatch_attempts: u32,
    dispatch_retry_milliseconds: u64,
    monitor_deadline_milliseconds: u64,
) -> Result<(u64, u64), MinoError> {
    if max_dispatch_attempts == 0 || max_dispatch_attempts > MAX_DISPATCH_ATTEMPTS {
        return Err(input_error(format!(
            "Dispatch attempts must be between 1 and {MAX_DISPATCH_ATTEMPTS}"
        )));
    }
    if dispatch_retry_milliseconds == 0
        || dispatch_retry_milliseconds > MAX_DISPATCH_RETRY_MILLISECONDS
    {
        return Err(input_error(format!(
            "Dispatch retry interval must be between 1 and {MAX_DISPATCH_RETRY_MILLISECONDS} milliseconds"
        )));
    }
    let trigger = parse_timestamp(trigger_at)?;
    let expiry = parse_timestamp(expires_at)?;
    let window = (expiry - trigger).whole_milliseconds();
    let window_milliseconds = u64::try_from(window).map_err(|_| {
        input_error("Schedule expiry must be after its trigger by at least one millisecond")
    })?;
    if window_milliseconds == 0 || window_milliseconds > MAX_SCHEDULE_WINDOW_MILLISECONDS {
        return Err(input_error(format!(
            "Schedule window must be between 1 and {MAX_SCHEDULE_WINDOW_MILLISECONDS} milliseconds"
        )));
    }
    let dispatches = u64::from(max_dispatch_attempts);
    let execution_budget = monitor_deadline_milliseconds
        .checked_mul(dispatches)
        .ok_or_else(|| input_error("Scheduled execution budget overflowed"))?;
    let retry_budget = dispatch_retry_milliseconds
        .checked_mul(dispatches.saturating_sub(1))
        .ok_or_else(|| input_error("Scheduled retry budget overflowed"))?;
    let required_budget_milliseconds = execution_budget
        .checked_add(retry_budget)
        .ok_or_else(|| input_error("Complete scheduled-task budget overflowed"))?;
    if required_budget_milliseconds > window_milliseconds {
        return Err(input_error(format!(
            "Schedule window {window_milliseconds} ms is smaller than the required bounded budget {required_budget_milliseconds} ms"
        )));
    }
    Ok((window_milliseconds, required_budget_milliseconds))
}

fn parse_timestamp(value: &Timestamp) -> Result<OffsetDateTime, MinoError> {
    OffsetDateTime::parse(value.as_str(), &Rfc3339)
        .map_err(|error| input_error(format!("Failed to parse normalized timestamp: {error}")))
}

fn normalized_text(label: &str, value: &str, max_bytes: usize) -> Result<String, MinoError> {
    let value = value.trim().to_owned();
    if value.is_empty() || value.len() > max_bytes || value.chars().any(char::is_control) {
        return Err(input_error(format!(
            "Scheduled-task {label} must be non-empty, control-free, and at most {max_bytes} UTF-8 bytes"
        )));
    }
    Ok(value)
}

fn validate_result_destination(root: &Path, relative: &Path) -> Result<String, MinoError> {
    if relative.as_os_str().is_empty()
        || relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(input_error(
            "Result destination must be a safe project-relative file path",
        ));
    }
    let components = relative
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => value.to_str(),
            _ => None,
        })
        .collect::<Vec<_>>();
    if components.len() != relative.components().count() {
        return Err(input_error("Result destination is not valid UTF-8"));
    }
    if components
        .first()
        .is_some_and(|value| value.eq_ignore_ascii_case(".mino"))
        || components.len() >= 2
            && components[0].eq_ignore_ascii_case("docs")
            && components[1].eq_ignore_ascii_case("plan")
    {
        return Err(policy_error(
            "Scheduled results cannot target Mino state or managed plan projections",
        ));
    }
    let normalized = components.join("/");
    if normalized.len() > MAX_RESULT_PATH_BYTES {
        return Err(input_error(format!(
            "Result destination must be at most {MAX_RESULT_PATH_BYTES} UTF-8 bytes"
        )));
    }
    let target = root.join(relative);
    validate_regular_parent_chain(
        root,
        target.parent().ok_or_else(|| {
            input_error("Result destination must have an existing project parent")
        })?,
    )?;
    match fs::symlink_metadata(&target) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => Err(
            input_error("Existing result destination must be a regular file"),
        ),
        Ok(_) => Ok(normalized),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(normalized),
        Err(error) => Err(environment_error(format!(
            "Failed to inspect result destination {}: {error}",
            target.display()
        ))),
    }
}

fn validate_regular_parent_chain(root: &Path, parent: &Path) -> Result<(), MinoError> {
    let relative = parent
        .strip_prefix(root)
        .map_err(|_| input_error("Result destination parent is outside the project"))?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err(input_error(
                "Result destination parent contains an unsafe component",
            ));
        };
        current.push(name);
        let metadata = fs::symlink_metadata(&current).map_err(|error| {
            input_error(format!(
                "Result destination parent {} is unavailable: {error}",
                current.display()
            ))
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(input_error(
                "Result destination parent must contain only regular directories",
            ));
        }
    }
    let canonical = parent.canonicalize().map_err(|error| {
        environment_error(format!(
            "Failed to resolve result destination parent {}: {error}",
            parent.display()
        ))
    })?;
    if !canonical.starts_with(root) {
        return Err(input_error(
            "Result destination parent resolves outside the project",
        ));
    }
    Ok(())
}

fn load_plan_read_only(
    root: &Path,
    plan_id: &PlanId,
) -> Result<(Plan, Vec<u8>, String), MinoError> {
    let paths = StorePaths::new(root);
    let current_path = paths.current_plan(plan_id);
    let current_bytes = read_bounded_regular(root, &current_path, "current plan")?;
    let plan: Plan = serde_json::from_slice(&current_bytes)
        .map_err(|error| drift_error(format!("Failed to parse current plan: {error}")))?;
    if plan.id() != plan_id
        || canonical_json_bytes(&plan)
            .map_err(|error| drift_error(format!("Failed to canonicalize plan: {error}")))?
            != current_bytes
    {
        return Err(drift_error(
            "Current plan bytes are non-canonical or have the wrong identity",
        ));
    }
    plan.validate_invariants()
        .map_err(|error| drift_error(error.to_string()))?;
    let snapshot = read_bounded_regular(
        root,
        &paths.snapshot(plan_id, plan.revision()),
        "current plan snapshot",
    )?;
    if snapshot != current_bytes {
        return Err(drift_error(
            "Current plan does not match its immutable revision snapshot",
        ));
    }
    let projection_relative = plan
        .metadata()
        .markdown_path()
        .ok_or_else(|| drift_error("Current plan has no managed Markdown projection path"))?;
    let projection_relative = normalize_project_relative(projection_relative, "projection")?;
    let projection_path = root.join(projection_relative);
    let projection = read_bounded_regular(root, &projection_path, "plan projection")?;
    let rendered = render_plan(&plan)
        .map_err(|error| drift_error(format!("Failed to render current plan: {error}")))?;
    if projection != rendered.as_bytes() {
        return Err(drift_error(
            "Managed plan projection does not match current canonical state",
        ));
    }
    Ok((plan, current_bytes, rendered.projection_digest().to_owned()))
}

fn read_bounded_regular(root: &Path, path: &Path, label: &str) -> Result<Vec<u8>, MinoError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        environment_error(format!(
            "Failed to inspect {label} {}: {error}",
            path.display()
        ))
    })?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() > MAX_PLAN_OR_PROJECTION_BYTES
    {
        return Err(drift_error(format!(
            "Scheduled-task {label} must be a bounded regular file"
        )));
    }
    let canonical = path.canonicalize().map_err(|error| {
        environment_error(format!(
            "Failed to resolve {label} {}: {error}",
            path.display()
        ))
    })?;
    if !canonical.starts_with(root) {
        return Err(drift_error(format!(
            "Scheduled-task {label} resolves outside the project"
        )));
    }
    let bytes = fs::read(path).map_err(|error| {
        environment_error(format!(
            "Failed to read {label} {}: {error}",
            path.display()
        ))
    })?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_PLAN_OR_PROJECTION_BYTES {
        return Err(drift_error(format!(
            "Scheduled-task {label} exceeds its byte bound"
        )));
    }
    Ok(bytes)
}

fn normalize_project_relative(value: &str, label: &str) -> Result<PathBuf, MinoError> {
    let path = Path::new(value);
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(drift_error(format!(
            "Scheduled-task {label} path is not safe and project-relative"
        )));
    }
    Ok(path.to_path_buf())
}

fn validate_check_working_directory(root: &Path, value: &str) -> Result<(), MinoError> {
    let path = Path::new(value);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::CurDir | Component::Normal(_)))
    {
        return Err(input_error(
            "Scheduled check working directory is not safe and project-relative",
        ));
    }
    let canonical = root.join(path).canonicalize().map_err(|error| {
        input_error(format!(
            "Scheduled check working directory is unavailable: {error}"
        ))
    })?;
    if !canonical.starts_with(root) || !canonical.is_dir() {
        return Err(input_error(
            "Scheduled check working directory resolves outside the project",
        ));
    }
    Ok(())
}

fn select_check<'a>(plan: &'a Plan, check_id: &CheckId) -> Result<SelectedCheck<'a>, MinoError> {
    let mut matches = plan
        .tasks()
        .iter()
        .flat_map(|task| {
            task.verification_checks()
                .iter()
                .filter(move |check| check.id() == check_id)
                .map(move |check| SelectedCheck {
                    task_id: Some(task.id().clone()),
                    check,
                })
        })
        .chain(
            plan.global_verification()
                .iter()
                .filter(|check| check.id() == check_id)
                .map(|check| SelectedCheck {
                    task_id: None,
                    check,
                }),
        );
    let selected = matches
        .next()
        .ok_or_else(|| input_error(format!("Check {check_id} does not exist")))?;
    if matches.next().is_some() {
        return Err(drift_error(format!(
            "Check {check_id} is not globally unique"
        )));
    }
    Ok(selected)
}

fn check_digest(check: &VerificationCheck) -> Result<String, MinoError> {
    canonical_json_bytes(&CheckDigestInput {
        id: check.id(),
        command: check.command(),
        cwd: check.cwd(),
        expected_exit_code: check.expected_exit_code(),
        required: check.is_required(),
    })
    .map(|bytes| sha256_digest(&bytes))
    .map_err(|error| environment_error(format!("Failed to encode check identity: {error}")))
}

fn monitor_argv(root: &str, request: &ValidatedScheduleRequest) -> Vec<String> {
    vec![
        "mino".to_owned(),
        "--root".to_owned(),
        root.to_owned(),
        "--format".to_owned(),
        "json".to_owned(),
        "--no-input".to_owned(),
        "exec".to_owned(),
        "check".to_owned(),
        "monitor".to_owned(),
        "--plan".to_owned(),
        request.plan_id.to_string(),
        "--check".to_owned(),
        request.check_id.to_string(),
        "--expect-revision".to_owned(),
        request.expected_revision.to_string(),
        "--request-id".to_owned(),
        request.execution_request_id.to_string(),
        "--actor".to_owned(),
        request.actor.clone(),
        "--max-attempts".to_owned(),
        request.monitor_bounds.max_attempts().to_string(),
        "--interval-milliseconds".to_owned(),
        request.monitor_bounds.interval_milliseconds().to_string(),
        "--deadline-milliseconds".to_owned(),
        request.monitor_bounds.deadline_milliseconds().to_string(),
    ]
}

fn schedule_digest(spec: &ScheduledTaskSpec) -> Result<String, MinoError> {
    canonical_json_bytes(&ScheduleDigestInput {
        spec_kind: &spec.spec_kind,
        project: &spec.project,
        execution: &spec.execution,
        trigger: &spec.trigger,
        outcome: &spec.outcome,
        authorization: &spec.authorization,
        emission_side_effects: spec.emission_side_effects,
    })
    .map(|bytes| sha256_digest(&bytes))
    .map_err(|error| environment_error(format!("Failed to encode schedule digest: {error}")))
}

fn input_error(message: impl Into<String>) -> MinoError {
    MinoError::new(ErrorCategory::IncompleteOrValidation, message)
}

fn revision_error(message: impl Into<String>) -> MinoError {
    MinoError::new(ErrorCategory::RevisionConflict, message)
}

fn policy_error(message: impl Into<String>) -> MinoError {
    MinoError::new(ErrorCategory::PolicyViolation, message)
}

fn environment_error(message: impl Into<String>) -> MinoError {
    MinoError::new(ErrorCategory::EnvironmentUnavailable, message)
}

fn drift_error(message: impl Into<String>) -> MinoError {
    MinoError::new(ErrorCategory::DriftDetected, message)
}
