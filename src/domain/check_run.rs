//! Recoverable leases and immutable terminal records for planned checks.

use std::collections::BTreeSet;
use std::time::Duration;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{
    CheckId, DomainError, DomainErrorKind, PlanId, RequestId, TaskId, Timestamp, VerificationCheck,
    WorkspaceFingerprint,
};

/// Version identifier for persisted check-run leases and terminal records.
pub const CHECK_RUN_SCHEMA_VERSION: &str = "mino.check-run/v1";

const MAX_TIMEOUT_MILLISECONDS: u64 = 3_600_000;
const MAX_OUTPUT_LIMIT_BYTES: u64 = 16 * 1_024 * 1_024;

/// Mandatory elapsed-time and captured-output bounds for one check run.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CheckRunLimits {
    timeout_milliseconds: u64,
    output_limit_bytes: u64,
}

impl CheckRunLimits {
    /// Creates validated finite bounds for one external process.
    ///
    /// # Errors
    ///
    /// Returns an invariant error when either bound is zero or exceeds the
    /// protocol maximum.
    pub fn new(timeout: Duration, output_limit_bytes: u64) -> Result<Self, DomainError> {
        let timeout_milliseconds = u64::try_from(timeout.as_millis()).map_err(|_| {
            DomainError::new(
                DomainErrorKind::InvariantViolation,
                "Check timeout exceeds the supported millisecond range",
            )
        })?;
        let limits = Self {
            timeout_milliseconds,
            output_limit_bytes,
        };
        limits.validate()?;
        Ok(limits)
    }

    /// Returns the finite process timeout.
    #[must_use]
    pub const fn timeout_milliseconds(self) -> u64 {
        self.timeout_milliseconds
    }

    /// Returns the combined stdout and stderr capture limit.
    #[must_use]
    pub const fn output_limit_bytes(self) -> u64 {
        self.output_limit_bytes
    }

    pub(crate) fn validate(self) -> Result<(), DomainError> {
        if self.timeout_milliseconds == 0 || self.timeout_milliseconds > MAX_TIMEOUT_MILLISECONDS {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                format!(
                    "Check timeout must be between 1 and {MAX_TIMEOUT_MILLISECONDS} milliseconds"
                ),
            ));
        }
        if self.output_limit_bytes == 0 || self.output_limit_bytes > MAX_OUTPUT_LIMIT_BYTES {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                format!("Check output limit must be between 1 and {MAX_OUTPUT_LIMIT_BYTES} bytes"),
            ));
        }
        Ok(())
    }
}

/// Revision, actor, and idempotency facts for one planned check invocation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CheckRunContext {
    plan_id: PlanId,
    plan_revision: u64,
    task_id: Option<TaskId>,
    request_id: RequestId,
    actor: String,
    started_at: Timestamp,
}

impl CheckRunContext {
    /// Creates validated invocation context for a task or global check.
    ///
    /// # Errors
    ///
    /// Returns an invariant error for revision zero or an empty actor.
    pub fn new(
        plan_id: PlanId,
        plan_revision: u64,
        task_id: Option<TaskId>,
        request_id: RequestId,
        actor: impl Into<String>,
        started_at: Timestamp,
    ) -> Result<Self, DomainError> {
        let context = Self {
            plan_id,
            plan_revision,
            task_id,
            request_id,
            actor: actor.into(),
            started_at,
        };
        context.validate()?;
        Ok(context)
    }

    fn validate(&self) -> Result<(), DomainError> {
        if self.plan_revision == 0 {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                "A check-run lease requires a positive plan revision",
            ));
        }
        if self.actor.trim().is_empty() {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                "A check-run lease requires an actor",
            ));
        }
        Ok(())
    }
}

/// An immutable pre-execution snapshot used to recover an interrupted invocation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CheckRunLease {
    schema_version: String,
    context: CheckRunContext,
    check_id: CheckId,
    command: Vec<String>,
    cwd: String,
    expected_exit_code: i32,
    limits: CheckRunLimits,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    workspace_fingerprint: Option<WorkspaceFingerprint>,
    environment_variables: Vec<String>,
    environment_digest: String,
    redaction_policy_digest: String,
}

impl CheckRunLease {
    /// Snapshots the exact authored check and runtime policies before execution.
    ///
    /// # Errors
    ///
    /// Returns an invariant error when the check, environment names, bounds, or
    /// policy digests are incomplete.
    pub fn new(
        context: CheckRunContext,
        check: &VerificationCheck,
        limits: CheckRunLimits,
        mut environment_variables: Vec<String>,
        environment_digest: impl Into<String>,
        redaction_policy_digest: impl Into<String>,
    ) -> Result<Self, DomainError> {
        environment_variables.sort();
        environment_variables.dedup();
        let lease = Self {
            schema_version: CHECK_RUN_SCHEMA_VERSION.to_owned(),
            context,
            check_id: check.id().clone(),
            command: check.command().to_vec(),
            cwd: check.cwd().to_owned(),
            expected_exit_code: check.expected_exit_code(),
            limits,
            workspace_fingerprint: None,
            environment_variables,
            environment_digest: environment_digest.into(),
            redaction_policy_digest: redaction_policy_digest.into(),
        };
        lease.validate()?;
        Ok(lease)
    }

    /// Binds the exact workspace content observed before process execution.
    ///
    /// # Errors
    ///
    /// Returns an invariant error when the supplied fingerprint is malformed.
    pub fn bind_workspace_fingerprint(
        mut self,
        fingerprint: WorkspaceFingerprint,
    ) -> Result<Self, DomainError> {
        fingerprint.validate()?;
        self.workspace_fingerprint = Some(fingerprint);
        self.validate()?;
        Ok(self)
    }

    /// Returns the invocation context.
    #[must_use]
    pub const fn context(&self) -> &CheckRunContext {
        &self.context
    }

    /// Returns the plan identifier owning this run.
    #[must_use]
    pub const fn plan_id(&self) -> &PlanId {
        &self.context.plan_id
    }

    /// Returns the exact plan revision owning this run.
    #[must_use]
    pub const fn plan_revision(&self) -> u64 {
        self.context.plan_revision
    }

    /// Returns the optional task identifier for a task-scoped check.
    #[must_use]
    pub const fn task_id(&self) -> Option<&TaskId> {
        self.context.task_id.as_ref()
    }

    /// Returns the invocation request identifier.
    #[must_use]
    pub const fn request_id(&self) -> &RequestId {
        &self.context.request_id
    }

    /// Returns the actor that initiated the check.
    #[must_use]
    pub fn actor(&self) -> &str {
        &self.context.actor
    }

    /// Returns the recorded start timestamp.
    #[must_use]
    pub const fn started_at(&self) -> &Timestamp {
        &self.context.started_at
    }

    /// Returns the stable check identifier.
    #[must_use]
    pub const fn check_id(&self) -> &CheckId {
        &self.check_id
    }

    /// Returns the exact executable and argument vector.
    #[must_use]
    pub fn command(&self) -> &[String] {
        &self.command
    }

    /// Returns the project-relative working directory.
    #[must_use]
    pub fn cwd(&self) -> &str {
        &self.cwd
    }

    /// Returns the expected process exit code.
    #[must_use]
    pub const fn expected_exit_code(&self) -> i32 {
        self.expected_exit_code
    }

    /// Returns the mandatory resource bounds.
    #[must_use]
    pub const fn limits(&self) -> CheckRunLimits {
        self.limits
    }

    /// Returns the exact verified workspace identity when the lease is current-format.
    #[must_use]
    pub const fn workspace_fingerprint(&self) -> Option<&WorkspaceFingerprint> {
        self.workspace_fingerprint.as_ref()
    }

    pub(crate) fn is_legacy_compatible_with(&self, current: &Self) -> bool {
        self.workspace_fingerprint.is_none()
            && current.workspace_fingerprint.is_some()
            && self.schema_version == current.schema_version
            && self.context == current.context
            && self.check_id == current.check_id
            && self.command == current.command
            && self.cwd == current.cwd
            && self.expected_exit_code == current.expected_exit_code
            && self.limits == current.limits
            && self.environment_variables == current.environment_variables
            && self.environment_digest == current.environment_digest
            && self.redaction_policy_digest == current.redaction_policy_digest
    }

    /// Returns the names of environment variables admitted to the child.
    #[must_use]
    pub fn environment_variables(&self) -> &[String] {
        &self.environment_variables
    }

    /// Returns the digest of the runtime environment snapshot.
    #[must_use]
    pub fn environment_digest(&self) -> &str {
        &self.environment_digest
    }

    /// Returns the digest of the redaction policy used for the run.
    #[must_use]
    pub fn redaction_policy_digest(&self) -> &str {
        &self.redaction_policy_digest
    }

    pub(crate) fn validate(&self) -> Result<(), DomainError> {
        self.context.validate()?;
        self.limits.validate()?;
        if let Some(fingerprint) = &self.workspace_fingerprint {
            fingerprint.validate()?;
        }
        if self.schema_version != CHECK_RUN_SCHEMA_VERSION {
            return Err(DomainError::new(
                DomainErrorKind::UnsupportedSchemaVersion,
                format!("Unsupported check-run schema {}", self.schema_version),
            ));
        }
        if self.command.is_empty()
            || self.command.iter().any(|part| part.trim().is_empty())
            || self.cwd.trim().is_empty()
        {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                "A check-run lease requires a complete command and working directory",
            ));
        }
        validate_environment_variables(&self.environment_variables)?;
        validate_digest("environment", &self.environment_digest)?;
        validate_digest("redaction policy", &self.redaction_policy_digest)
    }
}

/// Stable terminal classifications for one check invocation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CheckRunOutcome {
    /// The process exited with the planned status.
    Passed,
    /// The process exited with a status other than the planned status.
    UnexpectedExit,
    /// The process exceeded its elapsed-time bound.
    TimedOut,
    /// Combined output exceeded its capture bound.
    OutputLimitExceeded,
    /// The executable could not be started.
    SpawnFailed,
    /// The process ended but one of its output streams could not be captured.
    CaptureFailed,
    /// Captured text still resembled a credential after all redaction rules ran.
    CaptureBlocked,
    /// A prior invocation stopped after its lease but before its result journal.
    Interrupted,
}

/// Count of replacements made by one redaction rule.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AppliedRedaction {
    rule_id: String,
    replacements: u32,
}

impl AppliedRedaction {
    pub(crate) fn new(rule_id: impl Into<String>, replacements: u32) -> Self {
        Self {
            rule_id: rule_id.into(),
            replacements,
        }
    }

    /// Returns the stable policy rule identifier.
    #[must_use]
    pub fn rule_id(&self) -> &str {
        &self.rule_id
    }

    /// Returns how many matches were replaced.
    #[must_use]
    pub const fn replacements(&self) -> u32 {
        self.replacements
    }
}

pub(crate) struct CheckRunCompletion {
    pub outcome: CheckRunOutcome,
    pub exit_code: Option<i32>,
    pub finished_at: Timestamp,
    pub duration_milliseconds: u64,
    pub stdout_summary: String,
    pub stderr_summary: String,
    pub output_digest: String,
    pub output_truncated: bool,
    pub redactions: Vec<AppliedRedaction>,
    pub process_tree_terminated: bool,
    pub error_summary: Option<String>,
}

/// Immutable redacted terminal record for one leased check invocation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CheckRunResult {
    schema_version: String,
    lease: CheckRunLease,
    outcome: CheckRunOutcome,
    exit_code: Option<i32>,
    finished_at: Timestamp,
    duration_milliseconds: u64,
    stdout_summary: String,
    stderr_summary: String,
    output_digest: String,
    output_truncated: bool,
    redactions: Vec<AppliedRedaction>,
    process_tree_terminated: bool,
    error_summary: Option<String>,
}

impl CheckRunResult {
    pub(crate) fn completed(lease: CheckRunLease, completion: CheckRunCompletion) -> Self {
        Self {
            schema_version: CHECK_RUN_SCHEMA_VERSION.to_owned(),
            lease,
            outcome: completion.outcome,
            exit_code: completion.exit_code,
            finished_at: completion.finished_at,
            duration_milliseconds: completion.duration_milliseconds,
            stdout_summary: completion.stdout_summary,
            stderr_summary: completion.stderr_summary,
            output_digest: completion.output_digest,
            output_truncated: completion.output_truncated,
            redactions: completion.redactions,
            process_tree_terminated: completion.process_tree_terminated,
            error_summary: completion.error_summary,
        }
    }

    /// Returns the immutable invocation lease.
    #[must_use]
    pub const fn lease(&self) -> &CheckRunLease {
        &self.lease
    }

    /// Returns the stable terminal classification.
    #[must_use]
    pub const fn outcome(&self) -> CheckRunOutcome {
        self.outcome
    }

    /// Returns the process exit code when one was observed.
    #[must_use]
    pub const fn exit_code(&self) -> Option<i32> {
        self.exit_code
    }

    /// Returns the terminal timestamp.
    #[must_use]
    pub const fn finished_at(&self) -> &Timestamp {
        &self.finished_at
    }

    /// Returns measured elapsed time in milliseconds.
    #[must_use]
    pub const fn duration_milliseconds(&self) -> u64 {
        self.duration_milliseconds
    }

    /// Returns bounded redacted stdout text.
    #[must_use]
    pub fn stdout_summary(&self) -> &str {
        &self.stdout_summary
    }

    /// Returns bounded redacted stderr text.
    #[must_use]
    pub fn stderr_summary(&self) -> &str {
        &self.stderr_summary
    }

    /// Returns the digest of the complete redacted output summary.
    #[must_use]
    pub fn output_digest(&self) -> &str {
        &self.output_digest
    }

    /// Returns whether raw output exceeded the configured capture limit.
    #[must_use]
    pub const fn output_truncated(&self) -> bool {
        self.output_truncated
    }

    /// Returns redaction rule counts without exposing matched values.
    #[must_use]
    pub fn redactions(&self) -> &[AppliedRedaction] {
        &self.redactions
    }

    /// Returns whether process-tree termination was requested and succeeded.
    #[must_use]
    pub const fn process_tree_terminated(&self) -> bool {
        self.process_tree_terminated
    }

    /// Returns a redacted execution error summary when present.
    #[must_use]
    pub fn error_summary(&self) -> Option<&str> {
        self.error_summary.as_deref()
    }

    /// Returns whether the check observed its planned exit status.
    #[must_use]
    pub const fn is_success(&self) -> bool {
        matches!(self.outcome, CheckRunOutcome::Passed)
    }

    pub(crate) fn validate(&self) -> Result<(), DomainError> {
        self.lease.validate()?;
        if self.schema_version != CHECK_RUN_SCHEMA_VERSION {
            return Err(DomainError::new(
                DomainErrorKind::UnsupportedSchemaVersion,
                format!("Unsupported check-run schema {}", self.schema_version),
            ));
        }
        validate_digest("output", &self.output_digest)?;
        let mut rule_ids = BTreeSet::new();
        if self.redactions.iter().any(|redaction| {
            redaction.rule_id.trim().is_empty()
                || redaction.replacements == 0
                || !rule_ids.insert(redaction.rule_id.as_str())
        }) {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                "Check-run redaction records must have unique IDs and positive counts",
            ));
        }
        match self.outcome {
            CheckRunOutcome::Passed
                if self.exit_code != Some(self.lease.expected_exit_code)
                    || self.error_summary.is_some() =>
            {
                Err(DomainError::new(
                    DomainErrorKind::InvariantViolation,
                    "A passing check-run result must contain the expected exit code",
                ))
            }
            CheckRunOutcome::UnexpectedExit
                if self.exit_code == Some(self.lease.expected_exit_code) =>
            {
                Err(DomainError::new(
                    DomainErrorKind::InvariantViolation,
                    "An unexpected-exit result cannot contain the expected exit code",
                ))
            }
            CheckRunOutcome::SpawnFailed
            | CheckRunOutcome::CaptureFailed
            | CheckRunOutcome::CaptureBlocked
            | CheckRunOutcome::Interrupted
                if self.error_summary.as_deref().is_none_or(str::is_empty) =>
            {
                Err(DomainError::new(
                    DomainErrorKind::InvariantViolation,
                    "Failed, blocked, and interrupted results require an error summary",
                ))
            }
            CheckRunOutcome::CaptureBlocked
                if !self.stdout_summary.is_empty() || !self.stderr_summary.is_empty() =>
            {
                Err(DomainError::new(
                    DomainErrorKind::InvariantViolation,
                    "A capture-blocked result cannot retain stdout or stderr text",
                ))
            }
            _ => Ok(()),
        }
    }
}

fn validate_environment_variables(environment_variables: &[String]) -> Result<(), DomainError> {
    let mut previous: Option<&str> = None;
    for variable in environment_variables {
        let is_valid = !variable.is_empty()
            && variable.len() <= 128
            && variable.is_ascii()
            && variable.bytes().enumerate().all(|(index, byte)| {
                byte == b'_' || byte.is_ascii_alphabetic() || byte.is_ascii_digit() && index != 0
            });
        if !is_valid || previous.is_some_and(|value| value >= variable.as_str()) {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                "Check-run environment names must be sorted, unique ASCII identifiers",
            ));
        }
        previous = Some(variable);
    }
    Ok(())
}

fn validate_digest(label: &str, digest: &str) -> Result<(), DomainError> {
    let hexadecimal = digest.strip_prefix("sha256:").unwrap_or_default();
    if hexadecimal.len() == 64
        && hexadecimal
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err(DomainError::new(
            DomainErrorKind::InvariantViolation,
            format!("Check-run {label} digest must be a lowercase SHA-256 digest"),
        ))
    }
}
