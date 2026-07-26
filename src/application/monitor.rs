//! Finite foreground monitoring over the existing planned-check execution service.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::application::execution::{CheckExecutionDisposition, ExecutionService};
use crate::application::plan::{PlanMutationRequest, PlanService};
use crate::domain::{
    CheckId, CheckRunLimits, CheckRunOutcome, EvidenceId, Plan, PlanId, RequestId, Timestamp,
};
use crate::store::{canonical_json_bytes, sha256_digest};
use crate::{ErrorCategory, MinoError};

/// Stable schema identifier for terminal monitor summaries.
pub const MONITOR_KIND: &str = "mino.monitor/v1";

const MAX_MONITOR_ATTEMPTS: u32 = 100;
const MAX_MONITOR_INTERVAL_MILLISECONDS: u64 = 60_000;
const MAX_MONITOR_DEADLINE_MILLISECONDS: u64 = 86_400_000;
const MAX_MONITOR_SUMMARY_BYTES: u64 = 4 * 1024 * 1024;
const MONITOR_OUTPUT_LIMIT_BYTES: u64 = 1024 * 1024;
const MAX_SINGLE_CHECK_MILLISECONDS: u64 = 300_000;
static NEXT_SUMMARY_TEMPORARY: AtomicU64 = AtomicU64::new(1);

/// Statically finite attempt, interval, and elapsed-deadline bounds.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MonitorBounds {
    max_attempts: u32,
    interval_milliseconds: u64,
    deadline_milliseconds: u64,
    check_timeout_milliseconds: u64,
}

impl MonitorBounds {
    /// Creates validated finite monitor bounds.
    ///
    /// # Errors
    ///
    /// Returns an input error for zero/excessive attempts, a zero/excessive
    /// interval or deadline, or an interval longer than the complete deadline.
    pub fn new(
        max_attempts: u32,
        interval_milliseconds: u64,
        deadline_milliseconds: u64,
    ) -> Result<Self, MinoError> {
        validate_requested_bounds(max_attempts, interval_milliseconds, deadline_milliseconds)?;
        let check_timeout_milliseconds =
            check_timeout(max_attempts, interval_milliseconds, deadline_milliseconds)?;
        let bounds = Self {
            max_attempts,
            interval_milliseconds,
            deadline_milliseconds,
            check_timeout_milliseconds,
        };
        bounds.validate()?;
        Ok(bounds)
    }

    /// Returns the maximum number of external check invocations.
    #[must_use]
    pub const fn max_attempts(self) -> u32 {
        self.max_attempts
    }

    /// Returns the finite delay between failed attempts.
    #[must_use]
    pub const fn interval_milliseconds(self) -> u64 {
        self.interval_milliseconds
    }

    /// Returns the elapsed-time deadline from observation-loop start.
    #[must_use]
    pub const fn deadline_milliseconds(self) -> u64 {
        self.deadline_milliseconds
    }

    /// Returns the deterministic timeout allocated to every check attempt.
    #[must_use]
    pub const fn check_timeout_milliseconds(self) -> u64 {
        self.check_timeout_milliseconds
    }

    /// Selects a terminal reason before another attempt may start.
    #[must_use]
    pub fn terminal_before_attempt(
        self,
        attempts_completed: u32,
        elapsed: Duration,
        is_cancelled: bool,
    ) -> Option<MonitorTerminalReason> {
        if is_cancelled {
            Some(MonitorTerminalReason::Cancelled)
        } else if elapsed >= Duration::from_millis(self.deadline_milliseconds) {
            Some(MonitorTerminalReason::DeadlineReached)
        } else if attempts_completed >= self.max_attempts {
            Some(MonitorTerminalReason::AttemptsExhausted)
        } else {
            None
        }
    }

    /// Returns the next sleep duration capped by the remaining deadline.
    #[must_use]
    pub fn next_wait(self, elapsed: Duration) -> Duration {
        Duration::from_millis(self.interval_milliseconds)
            .min(Duration::from_millis(self.deadline_milliseconds).saturating_sub(elapsed))
    }

    fn validate(self) -> Result<(), MinoError> {
        validate_requested_bounds(
            self.max_attempts,
            self.interval_milliseconds,
            self.deadline_milliseconds,
        )?;
        if self.check_timeout_milliseconds
            != check_timeout(
                self.max_attempts,
                self.interval_milliseconds,
                self.deadline_milliseconds,
            )?
        {
            return Err(input_error(
                "Monitor check timeout does not match its finite attempt/deadline allocation",
            ));
        }
        Ok(())
    }
}

/// Stable reason monitoring stopped.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MonitorTerminalReason {
    /// The planned check passed.
    Passed,
    /// Every allowed attempt completed without passing.
    AttemptsExhausted,
    /// The elapsed deadline was reached before another attempt.
    DeadlineReached,
    /// The optional caller-owned cancellation file exists.
    Cancelled,
}

/// Whether one attempt executed a process or reused runner journal state.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MonitorAttemptDisposition {
    /// The external process executed.
    Executed,
    /// Immutable terminal process state replayed.
    Replayed,
    /// An abandoned lease was closed as interrupted.
    RecoveredInterrupted,
}

impl From<CheckExecutionDisposition> for MonitorAttemptDisposition {
    fn from(value: CheckExecutionDisposition) -> Self {
        match value {
            CheckExecutionDisposition::Executed => Self::Executed,
            CheckExecutionDisposition::Replayed => Self::Replayed,
            CheckExecutionDisposition::RecoveredInterrupted => Self::RecoveredInterrupted,
        }
    }
}

/// One durable planned-check attempt included in the terminal summary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MonitorAttempt {
    /// One-based attempt number.
    pub number: u32,
    /// Derived idempotency identifier for this attempt.
    pub request_id: RequestId,
    /// Exact plan revision before the two check phases.
    pub expected_revision: u64,
    /// Exact revision after lease and terminal evidence attachment.
    pub resulting_revision: u64,
    /// Immutable command evidence created or replayed for the attempt.
    pub evidence_id: EvidenceId,
    /// Terminal check process outcome.
    pub outcome: CheckRunOutcome,
    /// Process/journal execution disposition.
    pub disposition: MonitorAttemptDisposition,
    /// Immutable lease start time.
    pub started_at: Timestamp,
    /// Immutable result finish time.
    pub finished_at: Timestamp,
    /// Bounded process duration.
    pub duration_milliseconds: u64,
}

/// Immutable request metadata for one finite monitor invocation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MonitorRequest {
    /// Target plan identifier.
    pub plan_id: PlanId,
    /// Revision before the first possible attempt.
    pub expected_revision: u64,
    /// Base idempotency identifier used to derive attempt IDs and the summary path.
    pub request_id: RequestId,
    /// Actor recorded by every planned-check attempt.
    pub actor: String,
    /// Canonical monitor command vector.
    pub command: Vec<String>,
    /// Existing planned check to invoke.
    pub check_id: CheckId,
    /// Finite monitor policy.
    pub bounds: MonitorBounds,
    /// Optional project-relative caller-owned cancellation file.
    pub cancel_file: Option<PathBuf>,
}

/// Durable terminal summary for one finite monitor request.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MonitorReport {
    /// Stable result discriminator.
    pub monitor_kind: String,
    /// Digest of all immutable monitor request inputs.
    pub request_hash: String,
    /// Target plan identifier.
    pub plan_id: PlanId,
    /// Planned check identifier.
    pub check_id: CheckId,
    /// Base idempotency identifier.
    pub request_id: RequestId,
    /// Exact plan revision before the first possible attempt.
    pub expected_revision: u64,
    /// Finite attempt/deadline policy.
    pub bounds: MonitorBounds,
    /// Normalized optional cancellation path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cancel_file: Option<String>,
    /// Every executed or replayed durable attempt in order.
    pub attempts: Vec<MonitorAttempt>,
    /// First terminal monitor condition.
    pub terminal_reason: MonitorTerminalReason,
    /// Expected plan revision after every recorded attempt.
    pub final_revision: u64,
    /// Total foreground elapsed time for the original completed monitor run.
    pub elapsed_milliseconds: u64,
    /// Timestamp when the terminal summary was published.
    pub finished_at: Timestamp,
}

impl MonitorReport {
    /// Returns whether monitoring stopped because the check passed.
    #[must_use]
    pub const fn is_success(&self) -> bool {
        matches!(self.terminal_reason, MonitorTerminalReason::Passed)
    }
}

#[derive(Serialize)]
struct MonitorIdentity<'a> {
    plan_id: &'a PlanId,
    expected_revision: u64,
    request_id: &'a RequestId,
    actor: &'a str,
    command: &'a [String],
    check_id: &'a CheckId,
    bounds: MonitorBounds,
    cancel_file: Option<&'a str>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MonitorJournalRecord {
    schema_version: String,
    request_hash: String,
    report: MonitorReport,
}

struct MonitorObservation {
    attempts: Vec<MonitorAttempt>,
    terminal_reason: MonitorTerminalReason,
    elapsed_milliseconds: u64,
}

/// Application boundary for bounded foreground planned-check monitoring.
#[derive(Clone, Debug)]
pub struct MonitorService {
    root: PathBuf,
    plans: PlanService,
}

impl MonitorService {
    /// Discovers an initialized project and creates its monitor service.
    ///
    /// # Errors
    ///
    /// Returns an environment error when no initialized project is discoverable.
    pub fn discover(start: &Path) -> Result<Self, MinoError> {
        let plans = PlanService::discover(start)?;
        let root = plans.root().to_path_buf();
        Ok(Self { root, plans })
    }

    /// Repeatedly invokes one existing check until the first finite terminal condition.
    ///
    /// Every attempt uses the existing journal/evidence/plan transaction path.
    /// A terminal summary is immutable and exact retries return it without
    /// sleeping, executing a process, or mutating plan/evidence state.
    ///
    /// # Errors
    ///
    /// Returns a typed error for invalid bounds/path/request data, stale or
    /// conflicting revisions, a missing/duplicate check, runner/storage failure,
    /// or a corrupt/conflicting terminal summary.
    pub fn run(&self, request: MonitorRequest) -> Result<MonitorReport, MinoError> {
        validate_request(&request)?;
        let cancel_file = normalize_cancel_file(request.cancel_file.as_deref())?;
        let request_hash = request_hash(&request, cancel_file.as_deref())?;
        let summary_path = summary_path(&self.root, &request);
        inspect_summary_directory(&self.root, &request.plan_id, &request.request_id)?;
        match fs::symlink_metadata(&summary_path) {
            Ok(_) => return load_summary(&summary_path, &request_hash),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(environment_error(format!(
                    "Failed to inspect monitor summary {}: {error}",
                    summary_path.display()
                )));
            }
        }
        let initial = self.plans.load_stored(&request.plan_id)?;
        if initial.revision() < request.expected_revision {
            return Err(revision_error(format!(
                "Plan {} is revision {}, below monitor base revision {}",
                request.plan_id,
                initial.revision(),
                request.expected_revision
            )));
        }
        require_unique_check(&initial, &request.check_id)?;
        let recovery_attempts = recovery_attempt_count(
            request.expected_revision,
            initial.revision(),
            request.bounds.max_attempts(),
        )?;
        let cancellation_path = resolve_cancel_file(&self.root, cancel_file.as_deref())?;
        cancellation_requested(cancellation_path.as_deref())?;
        prepare_summary_directory(&self.root, &request.plan_id, &request.request_id)?;

        let check_limits = CheckRunLimits::new(
            Duration::from_millis(request.bounds.check_timeout_milliseconds()),
            MONITOR_OUTPUT_LIMIT_BYTES,
        )
        .map_err(|error| input_error(error.to_string()))?;
        let execution = ExecutionService::discover_with_limits(&self.root, check_limits)?;
        let observation = observe_attempts(
            &execution,
            &request,
            cancellation_path.as_deref(),
            recovery_attempts,
        )?;
        let final_revision =
            monitor_final_revision(request.expected_revision, observation.attempts.len())?;
        let report = MonitorReport {
            monitor_kind: MONITOR_KIND.to_owned(),
            request_hash: request_hash.clone(),
            plan_id: request.plan_id,
            check_id: request.check_id,
            request_id: request.request_id,
            expected_revision: request.expected_revision,
            bounds: request.bounds,
            cancel_file,
            attempts: observation.attempts,
            terminal_reason: observation.terminal_reason,
            final_revision,
            elapsed_milliseconds: observation.elapsed_milliseconds,
            finished_at: Timestamp::now_utc(),
        };
        publish_summary(&self.root, &summary_path, &request_hash, &report)?;
        load_summary(&summary_path, &request_hash)
    }
}

fn observe_attempts(
    execution: &ExecutionService,
    request: &MonitorRequest,
    cancellation_path: Option<&Path>,
    recovery_attempts: u32,
) -> Result<MonitorObservation, MinoError> {
    let started = Instant::now();
    let mut attempts = Vec::new();
    let terminal_reason = loop {
        let attempts_completed = u32::try_from(attempts.len())
            .map_err(|_| input_error("Monitor attempt count overflowed"))?;
        if attempts_completed >= recovery_attempts
            && let Some(reason) = request.bounds.terminal_before_attempt(
                attempts_completed,
                started.elapsed(),
                cancellation_requested(cancellation_path)?,
            )
        {
            break reason;
        }
        let number = attempts_completed
            .checked_add(1)
            .ok_or_else(|| input_error("Monitor attempt number overflowed"))?;
        let (attempt, did_pass) = execute_attempt(execution, request, number)?;
        attempts.push(attempt);
        if did_pass {
            if number < recovery_attempts {
                return Err(revision_error(
                    "Plan revisions continue beyond a recovered passing monitor attempt",
                ));
            }
            break MonitorTerminalReason::Passed;
        }
        let attempts_completed = u32::try_from(attempts.len())
            .map_err(|_| input_error("Monitor attempt count overflowed"))?;
        if attempts_completed < recovery_attempts {
            continue;
        }
        if let Some(reason) = request.bounds.terminal_before_attempt(
            attempts_completed,
            started.elapsed(),
            cancellation_requested(cancellation_path)?,
        ) {
            break reason;
        }
        let wait = request.bounds.next_wait(started.elapsed());
        if !wait.is_zero() {
            thread::sleep(wait);
        }
    };
    Ok(MonitorObservation {
        attempts,
        terminal_reason,
        elapsed_milliseconds: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
    })
}

fn execute_attempt(
    execution: &ExecutionService,
    request: &MonitorRequest,
    number: u32,
) -> Result<(MonitorAttempt, bool), MinoError> {
    let expected_revision = attempt_revision(request.expected_revision, number)?;
    let attempt_request_id = attempt_request_id(&request.request_id, number)?;
    let mut command = request.command.clone();
    command.extend(["--mino-monitor-attempt".to_owned(), number.to_string()]);
    let execution = execution.run_check(
        &PlanMutationRequest {
            plan_id: request.plan_id.clone(),
            expected_revision,
            request_id: attempt_request_id.clone(),
            actor: request.actor.clone(),
            command,
            updated_at: Timestamp::now_utc(),
        },
        &request.check_id,
    )?;
    let resulting_revision = expected_revision
        .checked_add(2)
        .ok_or_else(|| revision_error("Monitor resulting revision overflowed"))?;
    let attempt = MonitorAttempt {
        number,
        request_id: attempt_request_id,
        expected_revision,
        resulting_revision,
        evidence_id: execution.evidence().id().clone(),
        outcome: execution.run().outcome(),
        disposition: execution.disposition().into(),
        started_at: execution.run().lease().started_at().clone(),
        finished_at: execution.run().finished_at().clone(),
        duration_milliseconds: execution.run().duration_milliseconds(),
    };
    Ok((attempt, execution.is_success()))
}

fn monitor_final_revision(base_revision: u64, attempt_count: usize) -> Result<u64, MinoError> {
    let attempts = u64::try_from(attempt_count)
        .map_err(|_| revision_error("Monitor attempt count overflowed"))?;
    base_revision
        .checked_add(
            attempts
                .checked_mul(2)
                .ok_or_else(|| revision_error("Monitor revision offset overflowed"))?,
        )
        .ok_or_else(|| revision_error("Monitor final revision overflowed"))
}

fn recovery_attempt_count(
    base_revision: u64,
    current_revision: u64,
    max_attempts: u32,
) -> Result<u32, MinoError> {
    let revision_offset = current_revision
        .checked_sub(base_revision)
        .ok_or_else(|| revision_error("Plan revision is below the monitor base"))?;
    let maximum_offset = u64::from(max_attempts)
        .checked_mul(2)
        .ok_or_else(|| revision_error("Monitor maximum revision offset overflowed"))?;
    if revision_offset > maximum_offset {
        return Err(revision_error(
            "Plan revision is beyond the monitor's finite attempt range",
        ));
    }
    let completed = revision_offset / 2;
    let partial = u64::from(revision_offset % 2 != 0);
    u32::try_from(completed + partial)
        .map_err(|_| revision_error("Monitor recovery attempt count overflowed"))
}

fn validate_request(request: &MonitorRequest) -> Result<(), MinoError> {
    request.bounds.validate()?;
    if request.expected_revision == 0
        || request.actor.trim().is_empty()
        || request.command.is_empty()
        || request.command.iter().any(|part| part.trim().is_empty())
    {
        return Err(input_error(
            "Monitor requires a positive revision, actor, and canonical command",
        ));
    }
    Ok(())
}

fn check_timeout(
    max_attempts: u32,
    interval_milliseconds: u64,
    deadline_milliseconds: u64,
) -> Result<u64, MinoError> {
    if max_attempts == 0 {
        return Err(input_error("Monitor attempts must be positive"));
    }
    let attempts = u64::from(max_attempts);
    let waits = attempts
        .checked_sub(1)
        .ok_or_else(|| input_error("Monitor wait count underflowed"))?;
    let wait_budget = interval_milliseconds
        .checked_mul(waits)
        .ok_or_else(|| input_error("Monitor interval budget overflowed"))?;
    let execution_budget = deadline_milliseconds
        .checked_sub(wait_budget)
        .ok_or_else(|| input_error("Monitor intervals consume the complete deadline"))?;
    let per_attempt = execution_budget / attempts;
    if per_attempt == 0 {
        return Err(input_error(
            "Monitor deadline must allocate at least one millisecond per attempt",
        ));
    }
    Ok(per_attempt.min(MAX_SINGLE_CHECK_MILLISECONDS))
}

fn validate_requested_bounds(
    max_attempts: u32,
    interval_milliseconds: u64,
    deadline_milliseconds: u64,
) -> Result<(), MinoError> {
    if max_attempts == 0 || max_attempts > MAX_MONITOR_ATTEMPTS {
        return Err(input_error(format!(
            "Monitor attempts must be between 1 and {MAX_MONITOR_ATTEMPTS}"
        )));
    }
    if interval_milliseconds == 0 || interval_milliseconds > MAX_MONITOR_INTERVAL_MILLISECONDS {
        return Err(input_error(format!(
            "Monitor interval must be between 1 and {MAX_MONITOR_INTERVAL_MILLISECONDS} milliseconds"
        )));
    }
    if deadline_milliseconds == 0 || deadline_milliseconds > MAX_MONITOR_DEADLINE_MILLISECONDS {
        return Err(input_error(format!(
            "Monitor deadline must be between 1 and {MAX_MONITOR_DEADLINE_MILLISECONDS} milliseconds"
        )));
    }
    if interval_milliseconds > deadline_milliseconds {
        return Err(input_error(
            "Monitor interval cannot exceed the complete elapsed deadline",
        ));
    }
    Ok(())
}

fn normalize_cancel_file(path: Option<&Path>) -> Result<Option<String>, MinoError> {
    let Some(path) = path else {
        return Ok(None);
    };
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(input_error(
            "Monitor cancellation file must be a safe project-relative path",
        ));
    }
    path.to_str()
        .map(|value| Some(value.replace('\\', "/")))
        .ok_or_else(|| input_error("Monitor cancellation path is not valid UTF-8"))
}

fn resolve_cancel_file(root: &Path, relative: Option<&str>) -> Result<Option<PathBuf>, MinoError> {
    let Some(relative) = relative else {
        return Ok(None);
    };
    let relative_path = Path::new(relative);
    let parent_relative = relative_path
        .parent()
        .ok_or_else(|| input_error("Monitor cancellation path has no parent"))?;
    let mut parent_path = root.to_path_buf();
    for component in parent_relative.components() {
        let Component::Normal(name) = component else {
            return Err(input_error(
                "Monitor cancellation parent contains an unsafe component",
            ));
        };
        parent_path.push(name);
        let metadata = fs::symlink_metadata(&parent_path).map_err(|error| {
            input_error(format!(
                "Monitor cancellation parent {} is unavailable: {error}",
                parent_path.display()
            ))
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(input_error(
                "Monitor cancellation parent must contain only regular directories",
            ));
        }
    }
    let canonical_parent = parent_path.canonicalize().map_err(|error| {
        environment_error(format!(
            "Failed to resolve cancellation parent {}: {error}",
            parent_path.display()
        ))
    })?;
    if !canonical_parent.starts_with(root) || !canonical_parent.is_dir() {
        return Err(input_error(
            "Monitor cancellation path resolves outside the project",
        ));
    }
    Ok(Some(root.join(relative_path)))
}

fn cancellation_requested(path: Option<&Path>) -> Result<bool, MinoError> {
    let Some(path) = path else {
        return Ok(false);
    };
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => Err(
            input_error("Monitor cancellation path must be a regular file when present"),
        ),
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(environment_error(format!(
            "Failed to inspect cancellation path {}: {error}",
            path.display()
        ))),
    }
}

fn require_unique_check(plan: &Plan, check_id: &CheckId) -> Result<(), MinoError> {
    let count = plan
        .tasks()
        .iter()
        .flat_map(crate::domain::Task::verification_checks)
        .chain(plan.global_verification())
        .filter(|check| check.id() == check_id)
        .count();
    match count {
        0 => Err(input_error(format!("Check {check_id} does not exist"))),
        1 => Ok(()),
        _ => Err(MinoError::new(
            ErrorCategory::DriftDetected,
            format!("Check {check_id} is not globally unique"),
        )),
    }
}

fn request_hash(request: &MonitorRequest, cancel_file: Option<&str>) -> Result<String, MinoError> {
    canonical_json_bytes(&MonitorIdentity {
        plan_id: &request.plan_id,
        expected_revision: request.expected_revision,
        request_id: &request.request_id,
        actor: &request.actor,
        command: &request.command,
        check_id: &request.check_id,
        bounds: request.bounds,
        cancel_file,
    })
    .map(|bytes| sha256_digest(&bytes))
    .map_err(|error| environment_error(format!("Failed to encode monitor request: {error}")))
}

fn attempt_request_id(base: &RequestId, number: u32) -> Result<RequestId, MinoError> {
    let digest = sha256_digest(format!("{base}:monitor-attempt:{number}").as_bytes());
    let value = &digest["sha256:".len().."sha256:".len() + 32];
    RequestId::parse(format!(
        "{}-{}-{}-{}-{}",
        &value[0..8],
        &value[8..12],
        &value[12..16],
        &value[16..20],
        &value[20..32]
    ))
    .map_err(|error| input_error(error.to_string()))
}

fn attempt_revision(base: u64, number: u32) -> Result<u64, MinoError> {
    let prior_attempts = u64::from(
        number
            .checked_sub(1)
            .ok_or_else(|| input_error("Monitor attempt number must be positive"))?,
    );
    base.checked_add(
        prior_attempts
            .checked_mul(2)
            .ok_or_else(|| revision_error("Monitor revision offset overflowed"))?,
    )
    .ok_or_else(|| revision_error("Monitor expected revision overflowed"))
}

fn summary_path(root: &Path, request: &MonitorRequest) -> PathBuf {
    root.join(".mino")
        .join("plans")
        .join(request.plan_id.as_str())
        .join("monitors")
        .join(request.request_id.to_string())
        .join("summary.json")
}

fn load_summary(path: &Path, request_hash: &str) -> Result<MonitorReport, MinoError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        environment_error(format!(
            "Failed to inspect monitor summary {}: {error}",
            path.display()
        ))
    })?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() > MAX_MONITOR_SUMMARY_BYTES
    {
        return Err(drift_error("Monitor summary is not a bounded regular file"));
    }
    let bytes = fs::read(path).map_err(|error| {
        environment_error(format!(
            "Failed to read monitor summary {}: {error}",
            path.display()
        ))
    })?;
    let record: MonitorJournalRecord = serde_json::from_slice(&bytes)
        .map_err(|error| drift_error(format!("Failed to parse monitor summary: {error}")))?;
    if record.schema_version != MONITOR_KIND
        || record.request_hash != request_hash
        || record.report.request_hash != request_hash
    {
        return Err(revision_error(
            "Monitor request ID was reused with different inputs or schema",
        ));
    }
    record.report.bounds.validate()?;
    let canonical = canonical_json_bytes(&record)
        .map_err(|error| drift_error(format!("Failed to canonicalize monitor summary: {error}")))?;
    if canonical != bytes {
        return Err(drift_error("Monitor summary bytes are not canonical"));
    }
    validate_report(&record.report)?;
    Ok(record.report)
}

fn publish_summary(
    root: &Path,
    path: &Path,
    request_hash: &str,
    report: &MonitorReport,
) -> Result<(), MinoError> {
    let record = MonitorJournalRecord {
        schema_version: MONITOR_KIND.to_owned(),
        request_hash: request_hash.to_owned(),
        report: report.clone(),
    };
    let bytes = canonical_json_bytes(&record)
        .map_err(|error| environment_error(format!("Failed to encode monitor summary: {error}")))?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_MONITOR_SUMMARY_BYTES {
        return Err(environment_error(
            "Monitor summary exceeds its storage bound",
        ));
    }
    let directory = prepare_summary_directory(root, &report.plan_id, &report.request_id)?;
    if path.parent() != Some(directory.as_path()) {
        return Err(drift_error(
            "Monitor summary path does not match its request identity",
        ));
    }
    let temporary_sequence = NEXT_SUMMARY_TEMPORARY.fetch_add(1, Ordering::Relaxed);
    let temporary = directory.join(format!(
        "summary.{}.{temporary_sequence}.tmp",
        std::process::id()
    ));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .map_err(|error| {
            environment_error(format!(
                "Failed to create monitor summary {}: {error}",
                temporary.display()
            ))
        })?;
    let result = file.write_all(&bytes).and_then(|()| file.sync_all());
    drop(file);
    if let Err(error) = result {
        let _ = fs::remove_file(&temporary);
        return Err(environment_error(format!(
            "Failed to write monitor summary {}: {error}",
            temporary.display()
        )));
    }
    if let Err(error) = fs::hard_link(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        if fs::symlink_metadata(path).is_ok() {
            load_summary(path, request_hash)?;
            return Ok(());
        }
        return Err(environment_error(format!(
            "Failed to publish monitor summary {}: {error}",
            path.display()
        )));
    }
    fs::remove_file(&temporary).map_err(|error| {
        environment_error(format!(
            "Failed to remove published monitor temporary {}: {error}",
            temporary.display()
        ))
    })?;
    Ok(())
}

fn inspect_summary_directory(
    root: &Path,
    plan_id: &PlanId,
    request_id: &RequestId,
) -> Result<(), MinoError> {
    let canonical_plan = canonical_plan_directory(root, plan_id)?;
    let monitors_directory = root
        .join(".mino")
        .join("plans")
        .join(plan_id.as_str())
        .join("monitors");
    if !inspect_optional_safe_directory(&monitors_directory, &canonical_plan)? {
        return Ok(());
    }
    let request_directory = monitors_directory.join(request_id.to_string());
    inspect_optional_safe_directory(&request_directory, &canonical_plan)?;
    Ok(())
}

fn prepare_summary_directory(
    root: &Path,
    plan_id: &PlanId,
    request_id: &RequestId,
) -> Result<PathBuf, MinoError> {
    let canonical_plan = canonical_plan_directory(root, plan_id)?;
    let plan_directory = root.join(".mino").join("plans").join(plan_id.as_str());
    let monitors_directory = plan_directory.join("monitors");
    ensure_safe_directory(&monitors_directory)?;
    let request_directory = monitors_directory.join(request_id.to_string());
    ensure_safe_directory(&request_directory)?;
    for directory in [&monitors_directory, &request_directory] {
        require_directory_within(directory, &canonical_plan)?;
    }
    Ok(request_directory)
}

fn canonical_plan_directory(root: &Path, plan_id: &PlanId) -> Result<PathBuf, MinoError> {
    let canonical_root = root.canonicalize().map_err(|error| {
        environment_error(format!(
            "Failed to resolve monitor project root {}: {error}",
            root.display()
        ))
    })?;
    let mino_directory = root.join(".mino");
    let plans_root = mino_directory.join("plans");
    let target_plan_directory = plans_root.join(plan_id.as_str());
    for directory in [&mino_directory, &plans_root, &target_plan_directory] {
        require_safe_directory(directory)?;
    }
    let canonical_plan = target_plan_directory.canonicalize().map_err(|error| {
        environment_error(format!(
            "Failed to resolve monitor plan directory {}: {error}",
            target_plan_directory.display()
        ))
    })?;
    if !canonical_plan.starts_with(&canonical_root) {
        return Err(drift_error(
            "Monitor plan directory resolves outside the project",
        ));
    }
    Ok(canonical_plan)
}

fn inspect_optional_safe_directory(path: &Path, canonical_plan: &Path) -> Result<bool, MinoError> {
    match fs::symlink_metadata(path) {
        Ok(_) => {
            require_safe_directory(path)?;
            require_directory_within(path, canonical_plan)?;
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(environment_error(format!(
            "Failed to inspect monitor directory {}: {error}",
            path.display()
        ))),
    }
}

fn require_directory_within(path: &Path, canonical_plan: &Path) -> Result<(), MinoError> {
    let canonical = path.canonicalize().map_err(|error| {
        environment_error(format!(
            "Failed to resolve monitor directory {}: {error}",
            path.display()
        ))
    })?;
    if !canonical.starts_with(canonical_plan) {
        return Err(drift_error(
            "Monitor summary directory resolves outside its plan",
        ));
    }
    Ok(())
}

fn require_safe_directory(path: &Path) -> Result<(), MinoError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        environment_error(format!(
            "Failed to inspect monitor directory {}: {error}",
            path.display()
        ))
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(drift_error(format!(
            "Monitor directory {} is not a regular directory",
            path.display()
        )));
    }
    Ok(())
}

fn ensure_safe_directory(path: &Path) -> Result<(), MinoError> {
    match fs::symlink_metadata(path) {
        Ok(_) => require_safe_directory(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => match fs::create_dir(path) {
            Ok(()) => require_safe_directory(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                require_safe_directory(path)
            }
            Err(error) => Err(environment_error(format!(
                "Failed to create monitor directory {}: {error}",
                path.display()
            ))),
        },
        Err(error) => Err(environment_error(format!(
            "Failed to inspect monitor directory {}: {error}",
            path.display()
        ))),
    }
}

fn validate_report(report: &MonitorReport) -> Result<(), MinoError> {
    report.bounds.validate()?;
    if report.monitor_kind != MONITOR_KIND || report.expected_revision == 0 {
        return Err(drift_error("Monitor summary identity is invalid"));
    }
    if report.attempts.len() > usize::try_from(report.bounds.max_attempts()).unwrap_or(usize::MAX) {
        return Err(drift_error("Monitor summary exceeds its attempt bound"));
    }
    for (index, attempt) in report.attempts.iter().enumerate() {
        let number = u32::try_from(index)
            .ok()
            .and_then(|value| value.checked_add(1))
            .ok_or_else(|| drift_error("Monitor summary attempt number overflowed"))?;
        let expected_revision = attempt_revision(report.expected_revision, number)?;
        let expected_request_id = attempt_request_id(&report.request_id, number)?;
        let resulting_revision = expected_revision
            .checked_add(2)
            .ok_or_else(|| drift_error("Monitor summary resulting revision overflowed"))?;
        if attempt.number != number
            || attempt.request_id != expected_request_id
            || attempt.expected_revision != expected_revision
            || attempt.resulting_revision != resulting_revision
        {
            return Err(drift_error(
                "Monitor summary attempt sequence or identity is invalid",
            ));
        }
        if matches!(attempt.outcome, CheckRunOutcome::Passed)
            && index.checked_add(1) != Some(report.attempts.len())
        {
            return Err(drift_error(
                "Monitor summary continued after a passing attempt",
            ));
        }
    }
    let attempt_count = u64::try_from(report.attempts.len())
        .map_err(|_| drift_error("Monitor summary attempt count overflowed"))?;
    let expected_final_revision = report
        .expected_revision
        .checked_add(
            attempt_count
                .checked_mul(2)
                .ok_or_else(|| drift_error("Monitor summary revision offset overflowed"))?,
        )
        .ok_or_else(|| drift_error("Monitor summary final revision overflowed"))?;
    if report.final_revision != expected_final_revision {
        return Err(drift_error(
            "Monitor summary final revision does not match its attempts",
        ));
    }
    let did_pass = report
        .attempts
        .last()
        .is_some_and(|attempt| matches!(attempt.outcome, CheckRunOutcome::Passed));
    let terminal_is_valid = match report.terminal_reason {
        MonitorTerminalReason::Passed => did_pass,
        MonitorTerminalReason::AttemptsExhausted => {
            !did_pass
                && report.attempts.len()
                    == usize::try_from(report.bounds.max_attempts()).unwrap_or(usize::MAX)
        }
        MonitorTerminalReason::DeadlineReached => {
            !did_pass && report.elapsed_milliseconds >= report.bounds.deadline_milliseconds()
        }
        MonitorTerminalReason::Cancelled => !did_pass,
    };
    if !terminal_is_valid {
        return Err(drift_error(
            "Monitor summary terminal reason does not match its attempts",
        ));
    }
    Ok(())
}

fn input_error(message: impl Into<String>) -> MinoError {
    MinoError::new(ErrorCategory::IncompleteOrValidation, message)
}

fn revision_error(message: impl Into<String>) -> MinoError {
    MinoError::new(ErrorCategory::RevisionConflict, message)
}

fn environment_error(message: impl Into<String>) -> MinoError {
    MinoError::new(ErrorCategory::EnvironmentUnavailable, message)
}

fn drift_error(message: impl Into<String>) -> MinoError {
    MinoError::new(ErrorCategory::DriftDetected, message)
}
