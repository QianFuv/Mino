//! Process execution, bounded stream capture, and immutable run journals.

use std::collections::BTreeMap;
use std::fmt::{self, Debug, Formatter};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::domain::{
    AppliedRedaction, CheckRunCompletion, CheckRunLease, CheckRunOutcome, CheckRunResult,
    RequestId, Timestamp,
};
use crate::managed_fs::{ManagedFsError, ManagedFsErrorKind, ManagedPath, ProjectFs};
use crate::store::{canonical_json_bytes, sha256_digest};

use super::group;
use super::{Redactor, RunnerError, RunnerErrorKind};

const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(10);
const CAPTURE_CHUNK_BYTES: usize = 8 * 1_024;
const TRUNCATION_MARKER: &str = "\n[output truncated by Mino]";
static NEXT_PENDING_FILE: AtomicU64 = AtomicU64::new(1);

/// Runtime environment values passed only through an explicit allowlist.
#[derive(Clone, Default)]
pub struct RunEnvironment {
    variables: BTreeMap<String, String>,
}

impl RunEnvironment {
    /// Creates an empty environment policy.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            variables: BTreeMap::new(),
        }
    }

    /// Captures the available values of Mino's minimal cross-platform safe base.
    #[must_use]
    pub fn minimal() -> Self {
        let names = [
            "CARGO_HOME",
            "HOME",
            "LANG",
            "LC_ALL",
            "PATH",
            "PATHEXT",
            "RUSTUP_HOME",
            "SYSTEMROOT",
            "TEMP",
            "TMP",
            "TMPDIR",
            "USERPROFILE",
            "WINDIR",
        ];
        let variables = names
            .into_iter()
            .filter_map(|name| {
                std::env::var(name)
                    .ok()
                    .map(|value| (name.to_owned(), value))
            })
            .collect();
        Self { variables }
    }

    /// Adds or replaces one validated runtime variable.
    ///
    /// # Errors
    ///
    /// Returns an invalid-request error for an invalid name or a value containing
    /// a NUL character.
    pub fn with_variable(
        mut self,
        name: impl Into<String>,
        value: impl Into<String>,
    ) -> Result<Self, RunnerError> {
        let name = name.into();
        let value = value.into();
        validate_environment_name(&name)?;
        if value.contains('\0') {
            return Err(RunnerError::new(
                RunnerErrorKind::InvalidRequest,
                format!("Environment variable {name} contains a NUL character"),
            ));
        }
        self.variables.insert(name, value);
        Ok(self)
    }

    /// Returns sorted allowlisted variable names for the persisted lease.
    #[must_use]
    pub fn variable_names(&self) -> Vec<String> {
        self.variables.keys().cloned().collect()
    }

    /// Returns a non-secret digest of names and runtime values.
    #[must_use]
    pub fn digest(&self) -> String {
        let mut material = String::new();
        for (name, value) in &self.variables {
            material.push_str(name);
            material.push('\0');
            material.push_str(&sha256_digest(value.as_bytes()));
            material.push('\0');
        }
        sha256_digest(material.as_bytes())
    }

    fn secret_literals(&self) -> Vec<(String, String)> {
        self.variables
            .iter()
            .filter(|(name, value)| is_secret_name(name) && !value.is_empty())
            .map(|(name, value)| {
                (
                    format!("environment-{}", name.to_ascii_lowercase()),
                    value.clone(),
                )
            })
            .collect()
    }
}

impl Debug for RunEnvironment {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RunEnvironment")
            .field("variable_names", &self.variables.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

/// Whether a journaled invocation executed or reused a prior terminal record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RunDisposition {
    /// This invocation started the external process.
    Executed,
    /// A complete immutable result already existed for this request.
    Replayed,
    /// A prior lease lacked a result and was closed as interrupted.
    RecoveredInterrupted,
}

/// A terminal check result together with its recovery disposition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JournaledRun {
    disposition: RunDisposition,
    result: CheckRunResult,
}

impl JournaledRun {
    /// Returns whether execution, replay, or interruption recovery occurred.
    #[must_use]
    pub const fn disposition(&self) -> RunDisposition {
        self.disposition
    }

    /// Returns the immutable terminal record.
    #[must_use]
    pub const fn result(&self) -> &CheckRunResult {
        &self.result
    }

    /// Consumes the wrapper and returns the immutable terminal record.
    #[must_use]
    pub fn into_result(self) -> CheckRunResult {
        self.result
    }
}

/// Immutable per-request lease and terminal-result persistence.
#[derive(Clone, Debug)]
pub struct CheckRunJournal {
    filesystem: ProjectFs,
    root: ManagedPath,
}

impl CheckRunJournal {
    /// Creates a journal at a project-relative managed directory.
    ///
    /// # Errors
    ///
    /// Returns an error when the project root cannot be opened or the journal
    /// directory is not a normalized project-relative path.
    pub fn new(project_root: &Path, root: &Path) -> Result<Self, RunnerError> {
        Ok(Self {
            filesystem: ProjectFs::open(project_root).map_err(managed_error)?,
            root: ManagedPath::new(root).map_err(managed_error)?,
        })
    }

    /// Publishes a lease or confirms an identical existing lease.
    ///
    /// # Errors
    ///
    /// Returns a conflict for different inputs under the same request ID, or a
    /// corruption/I/O error for invalid persisted state.
    pub fn begin(&self, lease: &CheckRunLease) -> Result<bool, RunnerError> {
        lease
            .validate()
            .map_err(|error| invalid_request(error.to_string()))?;
        let lease_path = self.lease_path(lease.request_id());
        let result_path = self.result_path(lease.request_id());
        let result_exists = self
            .filesystem
            .exists(&result_path)
            .map_err(managed_error)?;
        let lease_exists = self.filesystem.exists(&lease_path).map_err(managed_error)?;
        if result_exists && !lease_exists {
            return Err(corrupt(format!(
                "Check result {} exists without its lease",
                self.filesystem.display_path(&result_path).display()
            )));
        }
        if lease_exists {
            let existing: CheckRunLease = read_canonical(&self.filesystem, &lease_path)?;
            existing
                .validate()
                .map_err(|error| corrupt(error.to_string()))?;
            if existing != *lease {
                return Err(RunnerError::new(
                    RunnerErrorKind::JournalConflict,
                    format!(
                        "Request {} already has different leased inputs",
                        lease.request_id()
                    ),
                ));
            }
            return Ok(false);
        }
        let bytes = canonical_bytes(lease)?;
        publish_immutable(&self.filesystem, &lease_path, &bytes)?;
        Ok(true)
    }

    /// Loads the immutable terminal result for a request when present.
    ///
    /// # Errors
    ///
    /// Returns a corruption or I/O error for malformed persisted state.
    pub fn result(&self, request_id: &RequestId) -> Result<Option<CheckRunResult>, RunnerError> {
        let result_path = self.result_path(request_id);
        if !self
            .filesystem
            .exists(&result_path)
            .map_err(managed_error)?
        {
            return Ok(None);
        }
        let result: CheckRunResult = read_canonical(&self.filesystem, &result_path)?;
        result
            .validate()
            .map_err(|error| corrupt(error.to_string()))?;
        Ok(Some(result))
    }

    /// Publishes a terminal record or confirms identical immutable bytes.
    ///
    /// # Errors
    ///
    /// Returns a conflict when the result does not match the stored lease and a
    /// corruption/I/O error for malformed persisted state.
    pub fn complete(&self, result: &CheckRunResult) -> Result<(), RunnerError> {
        result
            .validate()
            .map_err(|error| invalid_request(error.to_string()))?;
        let lease_path = self.lease_path(result.lease().request_id());
        if !self.filesystem.exists(&lease_path).map_err(managed_error)? {
            return Err(corrupt(format!(
                "Cannot publish result without lease {}",
                self.filesystem.display_path(&lease_path).display()
            )));
        }
        let stored_lease: CheckRunLease = read_canonical(&self.filesystem, &lease_path)?;
        if stored_lease != *result.lease() {
            return Err(RunnerError::new(
                RunnerErrorKind::JournalConflict,
                format!(
                    "Result for request {} does not match its lease",
                    result.lease().request_id()
                ),
            ));
        }
        let result_path = self.result_path(result.lease().request_id());
        let bytes = canonical_bytes(result)?;
        publish_immutable(&self.filesystem, &result_path, &bytes)
    }

    fn request_directory(&self, request_id: &RequestId) -> ManagedPath {
        self.root
            .join(request_id.as_str())
            .expect("validated request ID should form a managed path")
    }

    fn lease_path(&self, request_id: &RequestId) -> ManagedPath {
        self.request_directory(request_id)
            .join("lease.json")
            .expect("static lease file name should form a managed path")
    }

    fn result_path(&self, request_id: &RequestId) -> ManagedPath {
        self.request_directory(request_id)
            .join("result.json")
            .expect("static result file name should form a managed path")
    }
}

/// Direct process runner with finite polling, time, and output bounds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProcessRunner {
    poll_interval: Duration,
}

impl ProcessRunner {
    /// Creates a runner with a positive bounded status-poll interval.
    ///
    /// # Errors
    ///
    /// Returns an invalid-request error when the interval is zero or exceeds one
    /// second.
    pub fn new(poll_interval: Duration) -> Result<Self, RunnerError> {
        if poll_interval.is_zero() || poll_interval > Duration::from_secs(1) {
            return Err(invalid_request(
                "Runner poll interval must be between one millisecond and one second",
            ));
        }
        Ok(Self { poll_interval })
    }

    /// Runs one leased command without a shell and returns redacted bounded output.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid paths or policies and for internal capture
    /// failures. Process spawn and exit failures are represented as terminal results.
    pub fn run(
        &self,
        project_root: &Path,
        lease: CheckRunLease,
        environment: &RunEnvironment,
        redactor: &Redactor,
    ) -> Result<CheckRunResult, RunnerError> {
        let working_directory = validate_invocation(project_root, &lease, environment, redactor)?;
        execute_process(
            self.poll_interval,
            working_directory,
            lease,
            environment,
            redactor,
        )
    }

    /// Runs or deterministically reconciles one idempotent journaled invocation.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid inputs, journal conflicts/corruption, or
    /// internal process-capture failures.
    pub fn run_journaled(
        &self,
        project_root: &Path,
        journal: &CheckRunJournal,
        lease: CheckRunLease,
        environment: &RunEnvironment,
        redactor: &Redactor,
    ) -> Result<JournaledRun, RunnerError> {
        let working_directory = validate_invocation(project_root, &lease, environment, redactor)?;
        let is_new = journal.begin(&lease)?;
        if let Some(result) = journal.result(lease.request_id())? {
            if result.lease() != &lease {
                return Err(RunnerError::new(
                    RunnerErrorKind::JournalConflict,
                    format!(
                        "Request {} result does not match the supplied lease",
                        lease.request_id()
                    ),
                ));
            }
            return Ok(JournaledRun {
                disposition: RunDisposition::Replayed,
                result,
            });
        }
        if !is_new {
            let result = interrupted_result(lease);
            journal.complete(&result)?;
            return Ok(JournaledRun {
                disposition: RunDisposition::RecoveredInterrupted,
                result,
            });
        }
        let result = execute_process(
            self.poll_interval,
            working_directory,
            lease,
            environment,
            redactor,
        )?;
        journal.complete(&result)?;
        Ok(JournaledRun {
            disposition: RunDisposition::Executed,
            result,
        })
    }
}

impl Default for ProcessRunner {
    fn default() -> Self {
        Self {
            poll_interval: DEFAULT_POLL_INTERVAL,
        }
    }
}

struct StreamCapture {
    bytes: Vec<u8>,
    error: Option<String>,
}

struct ProcessObservation {
    status: Option<ExitStatus>,
    forced_outcome: Option<CheckRunOutcome>,
    process_tree_terminated: bool,
    capture_error: Option<String>,
}

fn validate_invocation(
    project_root: &Path,
    lease: &CheckRunLease,
    environment: &RunEnvironment,
    redactor: &Redactor,
) -> Result<PathBuf, RunnerError> {
    lease
        .validate()
        .map_err(|error| invalid_request(error.to_string()))?;
    if lease.environment_variables() != environment.variable_names()
        || lease.environment_digest() != environment.digest()
    {
        return Err(invalid_request(
            "Runtime environment does not match the immutable check-run lease",
        ));
    }
    if lease.redaction_policy_digest() != redactor.policy_digest() {
        return Err(invalid_request(
            "Redaction policy does not match the immutable check-run lease",
        ));
    }
    reject_shell(&lease.command()[0])?;
    validated_working_directory(project_root, lease.cwd())
}

fn execute_process(
    poll_interval: Duration,
    working_directory: PathBuf,
    lease: CheckRunLease,
    environment: &RunEnvironment,
    redactor: &Redactor,
) -> Result<CheckRunResult, RunnerError> {
    let started = Instant::now();
    let mut command = Command::new(&lease.command()[0]);
    command
        .args(&lease.command()[1..])
        .current_dir(working_directory)
        .env_clear()
        .envs(&environment.variables)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = match group::spawn(&mut command) {
        Ok(child) => child,
        Err(error) => {
            return Ok(spawn_failed_result(
                lease,
                redactor,
                environment,
                started,
                &error,
            ));
        }
    };
    let stdout = group::take_stdout(&mut child).ok_or_else(|| {
        RunnerError::new(
            RunnerErrorKind::CaptureFailed,
            "Spawned check did not expose piped stdout",
        )
    })?;
    let stderr = group::take_stderr(&mut child).ok_or_else(|| {
        RunnerError::new(
            RunnerErrorKind::CaptureFailed,
            "Spawned check did not expose piped stderr",
        )
    })?;
    let output_limit = usize::try_from(lease.limits().output_limit_bytes()).map_err(|_| {
        invalid_request("Check output limit cannot be represented on this platform")
    })?;
    let captured_bytes = Arc::new(AtomicUsize::new(0));
    let output_exceeded = Arc::new(AtomicBool::new(false));
    let (stdout_handle, stdout_finished) = spawn_capture(
        stdout,
        output_limit,
        Arc::clone(&captured_bytes),
        Arc::clone(&output_exceeded),
    );
    let (stderr_handle, stderr_finished) = spawn_capture(
        stderr,
        output_limit,
        Arc::clone(&captured_bytes),
        Arc::clone(&output_exceeded),
    );
    let timeout = Duration::from_millis(lease.limits().timeout_milliseconds());
    let mut observation = observe_process(
        &mut child,
        started,
        timeout,
        poll_interval,
        &output_exceeded,
        [stdout_finished.as_ref(), stderr_finished.as_ref()],
    )?;
    let mut stdout_capture = join_capture(stdout_handle, "stdout")?;
    let mut stderr_capture = join_capture(stderr_handle, "stderr")?;
    if output_exceeded.load(Ordering::Acquire) && observation.forced_outcome.is_none() {
        observation.forced_outcome = Some(CheckRunOutcome::OutputLimitExceeded);
    }
    observation.capture_error = stdout_capture.error.take().or(stderr_capture.error.take());
    Ok(build_result(
        lease,
        redactor,
        environment,
        started,
        &mut stdout_capture.bytes,
        &mut stderr_capture.bytes,
        observation,
    ))
}

fn observe_process(
    child: &mut command_group::GroupChild,
    started: Instant,
    timeout: Duration,
    poll_interval: Duration,
    output_exceeded: &AtomicBool,
    captures_finished: [&AtomicBool; 2],
) -> Result<ProcessObservation, RunnerError> {
    let mut observed_status = None;
    loop {
        let forced_outcome = if output_exceeded.load(Ordering::Acquire) {
            Some(CheckRunOutcome::OutputLimitExceeded)
        } else if observed_status.is_some()
            && captures_finished
                .iter()
                .all(|finished| finished.load(Ordering::Acquire))
        {
            return Ok(ProcessObservation {
                status: observed_status,
                forced_outcome: None,
                process_tree_terminated: false,
                capture_error: None,
            });
        } else if started.elapsed() >= timeout {
            Some(CheckRunOutcome::TimedOut)
        } else {
            None
        };
        if let Some(forced_outcome) = forced_outcome {
            let process_tree_terminated = group::terminate(child).map_err(|error| {
                RunnerError::new(
                    RunnerErrorKind::Io,
                    format!("Failed to terminate check process tree: {error}"),
                )
            })?;
            let status = observed_status.or_else(|| group::wait(child).ok());
            return Ok(ProcessObservation {
                status,
                forced_outcome: Some(forced_outcome),
                process_tree_terminated,
                capture_error: None,
            });
        }
        if observed_status.is_none() {
            observed_status = group::try_wait(child).map_err(|error| {
                RunnerError::new(
                    RunnerErrorKind::Io,
                    format!("Failed to observe check process: {error}"),
                )
            })?;
        }
        thread::sleep(poll_interval.min(timeout.saturating_sub(started.elapsed())));
    }
}

fn spawn_capture(
    mut stream: impl Read + Send + 'static,
    output_limit: usize,
    captured_bytes: Arc<AtomicUsize>,
    output_exceeded: Arc<AtomicBool>,
) -> (thread::JoinHandle<StreamCapture>, Arc<AtomicBool>) {
    let finished = Arc::new(AtomicBool::new(false));
    let worker_finished = Arc::clone(&finished);
    let handle = thread::spawn(move || {
        let capture = read_stream(&mut stream, output_limit, &captured_bytes, &output_exceeded);
        worker_finished.store(true, Ordering::Release);
        capture
    });
    (handle, finished)
}

fn read_stream(
    stream: &mut impl Read,
    output_limit: usize,
    captured_bytes: &AtomicUsize,
    output_exceeded: &AtomicBool,
) -> StreamCapture {
    let mut captured = Vec::new();
    let mut chunk = [0_u8; CAPTURE_CHUNK_BYTES];
    loop {
        let read = match stream.read(&mut chunk) {
            Ok(0) => {
                return StreamCapture {
                    bytes: captured,
                    error: None,
                };
            }
            Ok(read) => read,
            Err(error) => {
                return StreamCapture {
                    bytes: captured,
                    error: Some(error.to_string()),
                };
            }
        };
        let previous = captured_bytes.fetch_add(read, Ordering::AcqRel);
        let retained = output_limit.saturating_sub(previous).min(read);
        captured.extend_from_slice(&chunk[..retained]);
        if read > retained {
            output_exceeded.store(true, Ordering::Release);
            return StreamCapture {
                bytes: captured,
                error: None,
            };
        }
    }
}

fn join_capture(
    handle: thread::JoinHandle<StreamCapture>,
    stream_name: &str,
) -> Result<StreamCapture, RunnerError> {
    handle.join().map_err(|_| {
        RunnerError::new(
            RunnerErrorKind::CaptureFailed,
            format!("{stream_name} capture worker panicked"),
        )
    })
}

fn build_result(
    lease: CheckRunLease,
    redactor: &Redactor,
    environment: &RunEnvironment,
    started: Instant,
    stdout_bytes: &mut Vec<u8>,
    stderr_bytes: &mut Vec<u8>,
    observation: ProcessObservation,
) -> CheckRunResult {
    let runtime_literals = environment.secret_literals();
    let (mut stdout_summary, stdout_redactions) =
        redact_bytes(redactor, stdout_bytes, &runtime_literals);
    let (mut stderr_summary, stderr_redactions) =
        redact_bytes(redactor, stderr_bytes, &runtime_literals);
    let output_truncated = observation.forced_outcome == Some(CheckRunOutcome::OutputLimitExceeded);
    if output_truncated {
        stdout_summary.push_str(TRUNCATION_MARKER);
        stderr_summary.push_str(TRUNCATION_MARKER);
    }
    let mut redactions = merge_redactions([stdout_redactions, stderr_redactions]);
    let mut error_summary = observation.capture_error;
    if let Some(error) = error_summary.take() {
        let (redacted_error, error_redactions) =
            redactor.redact_with_literals(&error, &runtime_literals);
        redactions = merge_redactions([redactions, error_redactions]);
        error_summary = Some(redacted_error);
    }
    let exit_code = observation.status.and_then(|status| status.code());
    let outcome = observation.forced_outcome.unwrap_or_else(|| {
        if error_summary.is_some() {
            CheckRunOutcome::CaptureFailed
        } else if exit_code == Some(lease.expected_exit_code()) {
            CheckRunOutcome::Passed
        } else {
            CheckRunOutcome::UnexpectedExit
        }
    });
    let digest_material = format!("stdout\0{stdout_summary}\0stderr\0{stderr_summary}");
    CheckRunResult::completed(
        lease,
        CheckRunCompletion {
            outcome,
            exit_code,
            finished_at: Timestamp::now_utc(),
            duration_milliseconds: elapsed_milliseconds(started),
            stdout_summary,
            stderr_summary,
            output_digest: sha256_digest(digest_material.as_bytes()),
            output_truncated,
            redactions,
            process_tree_terminated: observation.process_tree_terminated,
            error_summary,
        },
    )
}

fn spawn_failed_result(
    lease: CheckRunLease,
    redactor: &Redactor,
    environment: &RunEnvironment,
    started: Instant,
    error: &io::Error,
) -> CheckRunResult {
    let runtime_literals = environment.secret_literals();
    let (error_summary, redactions) =
        redactor.redact_with_literals(&error.to_string(), &runtime_literals);
    CheckRunResult::completed(
        lease,
        CheckRunCompletion {
            outcome: CheckRunOutcome::SpawnFailed,
            exit_code: None,
            finished_at: Timestamp::now_utc(),
            duration_milliseconds: elapsed_milliseconds(started),
            stdout_summary: String::new(),
            stderr_summary: String::new(),
            output_digest: sha256_digest(b"stdout\0\0stderr\0"),
            output_truncated: false,
            redactions,
            process_tree_terminated: false,
            error_summary: Some(error_summary),
        },
    )
}

fn interrupted_result(lease: CheckRunLease) -> CheckRunResult {
    CheckRunResult::completed(
        lease,
        CheckRunCompletion {
            outcome: CheckRunOutcome::Interrupted,
            exit_code: None,
            finished_at: Timestamp::now_utc(),
            duration_milliseconds: 0,
            stdout_summary: String::new(),
            stderr_summary: String::new(),
            output_digest: sha256_digest(b"stdout\0\0stderr\0"),
            output_truncated: false,
            redactions: Vec::new(),
            process_tree_terminated: false,
            error_summary: Some(
                "Previous invocation ended before a terminal result was journaled".to_owned(),
            ),
        },
    )
}

fn redact_bytes(
    redactor: &Redactor,
    bytes: &mut Vec<u8>,
    runtime_literals: &[(String, String)],
) -> (String, Vec<AppliedRedaction>) {
    let text = String::from_utf8_lossy(bytes).into_owned();
    bytes.fill(0);
    bytes.clear();
    redactor.redact_with_literals(&text, runtime_literals)
}

fn merge_redactions<const COUNT: usize>(
    groups: [Vec<AppliedRedaction>; COUNT],
) -> Vec<AppliedRedaction> {
    let mut counts = BTreeMap::<String, u32>::new();
    for redaction in groups.into_iter().flatten() {
        counts
            .entry(redaction.rule_id().to_owned())
            .and_modify(|count| *count = count.saturating_add(redaction.replacements()))
            .or_insert(redaction.replacements());
    }
    counts
        .into_iter()
        .map(|(id, count)| AppliedRedaction::new(id, count))
        .collect()
}

fn elapsed_milliseconds(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn validated_working_directory(
    project_root: &Path,
    relative: &str,
) -> Result<PathBuf, RunnerError> {
    let project_root = project_root.canonicalize().map_err(|error| {
        RunnerError::new(
            RunnerErrorKind::Io,
            format!(
                "Failed to resolve project root {}: {error}",
                project_root.display()
            ),
        )
    })?;
    let relative = Path::new(relative);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::CurDir | Component::Normal(_)))
    {
        return Err(invalid_request(
            "Check working directory must be a project-relative path without parent traversal",
        ));
    }
    let working_directory = project_root
        .join(relative)
        .canonicalize()
        .map_err(|error| {
            RunnerError::new(
                RunnerErrorKind::Io,
                format!("Failed to resolve check working directory: {error}"),
            )
        })?;
    if !working_directory.starts_with(&project_root) || !working_directory.is_dir() {
        return Err(invalid_request(
            "Check working directory resolves outside the project or is not a directory",
        ));
    }
    Ok(working_directory)
}

fn reject_shell(program: &str) -> Result<(), RunnerError> {
    let normalized = Path::new(program)
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or(program)
        .to_ascii_lowercase();
    if matches!(
        normalized.as_str(),
        "ash"
            | "bash"
            | "cmd"
            | "csh"
            | "dash"
            | "fish"
            | "ksh"
            | "powershell"
            | "pwsh"
            | "sh"
            | "tcsh"
            | "zsh"
    ) {
        Err(invalid_request(format!(
            "Shell executable {program} is not permitted for planned checks"
        )))
    } else {
        Ok(())
    }
}

fn validate_environment_name(name: &str) -> Result<(), RunnerError> {
    let is_valid = !name.is_empty()
        && name.len() <= 128
        && name.is_ascii()
        && name.bytes().enumerate().all(|(index, byte)| {
            byte == b'_' || byte.is_ascii_alphabetic() || byte.is_ascii_digit() && index != 0
        });
    if is_valid {
        Ok(())
    } else {
        Err(invalid_request(format!(
            "Invalid environment variable name {name}"
        )))
    }
}

fn is_secret_name(name: &str) -> bool {
    let normalized = name.to_ascii_uppercase();
    ["API_KEY", "AUTH", "PASSWORD", "SECRET", "TOKEN"]
        .iter()
        .any(|fragment| normalized.contains(fragment))
}

fn canonical_bytes(value: &impl Serialize) -> Result<Vec<u8>, RunnerError> {
    canonical_json_bytes(value).map_err(|error| {
        RunnerError::new(
            RunnerErrorKind::Serialization,
            format!("Failed to encode check journal: {error}"),
        )
    })
}

fn read_canonical<T>(filesystem: &ProjectFs, path: &ManagedPath) -> Result<T, RunnerError>
where
    T: DeserializeOwned + Serialize,
{
    let bytes = filesystem.read(path).map_err(managed_error)?;
    let value = serde_json::from_slice(&bytes).map_err(|error| {
        corrupt(format!(
            "Failed to decode check journal {}: {error}",
            filesystem.display_path(path).display()
        ))
    })?;
    let canonical = canonical_bytes(&value)?;
    if canonical != bytes {
        return Err(corrupt(format!(
            "Check journal {} is not canonical",
            filesystem.display_path(path).display()
        )));
    }
    Ok(value)
}

fn publish_immutable(
    filesystem: &ProjectFs,
    path: &ManagedPath,
    bytes: &[u8],
) -> Result<(), RunnerError> {
    if filesystem.exists(path).map_err(managed_error)? {
        let existing = filesystem.read(path).map_err(managed_error)?;
        if existing == bytes {
            return Ok(());
        }
        return Err(RunnerError::new(
            RunnerErrorKind::JournalConflict,
            format!(
                "Immutable check journal {} has conflicting bytes",
                filesystem.display_path(path).display()
            ),
        ));
    }
    let parent = path.parent().ok_or_else(|| {
        RunnerError::new(
            RunnerErrorKind::Io,
            format!(
                "Check journal path {} has no parent",
                filesystem.display_path(path).display()
            ),
        )
    })?;
    filesystem
        .ensure_directory(&parent)
        .map_err(managed_error)?;
    let sequence = NEXT_PENDING_FILE.fetch_add(1, Ordering::Relaxed);
    let file_name = path
        .as_path()
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("journal");
    let pending = parent
        .join(format!(
            ".{file_name}.{}.{}.pending",
            std::process::id(),
            sequence
        ))
        .map_err(managed_error)?;
    let mut file = filesystem
        .create_new_file(&pending)
        .map_err(managed_error)?;
    if let Err(error) = file.write_all(bytes).and_then(|()| file.sync_all()) {
        drop(file);
        let _ = filesystem.remove_file_if_exists(&pending);
        return Err(io_error(
            "write",
            &filesystem.display_path(&pending),
            &error,
        ));
    }
    drop(file);
    if let Err(error) = filesystem.rename(&pending, path) {
        let _ = filesystem.remove_file_if_exists(&pending);
        if filesystem.exists(path).map_err(managed_error)? {
            let existing = filesystem.read(path).map_err(managed_error)?;
            if existing == bytes {
                return Ok(());
            }
            return Err(RunnerError::new(
                RunnerErrorKind::JournalConflict,
                format!(
                    "Immutable check journal {} was published with different bytes",
                    filesystem.display_path(path).display()
                ),
            ));
        }
        return Err(managed_error(error));
    }
    filesystem.sync_parent(path).map_err(managed_error)
}

fn invalid_request(message: impl Into<String>) -> RunnerError {
    RunnerError::new(RunnerErrorKind::InvalidRequest, message)
}

fn corrupt(message: impl Into<String>) -> RunnerError {
    RunnerError::new(RunnerErrorKind::CorruptJournal, message)
}

fn io_error(action: &str, path: &Path, error: &io::Error) -> RunnerError {
    RunnerError::new(
        RunnerErrorKind::Io,
        format!("Failed to {action} {}: {error}", path.display()),
    )
}

fn managed_error(error: ManagedFsError) -> RunnerError {
    let kind = match error.kind() {
        ManagedFsErrorKind::InvalidPath | ManagedFsErrorKind::UnsafeComponent => {
            RunnerErrorKind::CorruptJournal
        }
        ManagedFsErrorKind::Io => RunnerErrorKind::Io,
    };
    RunnerError::new(kind, error.into_message())
}
