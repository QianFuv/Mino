//! Acceptance tests for bounded process execution and check-run recovery.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::fs::symlink;
#[cfg(windows)]
use std::os::windows::fs::symlink_dir;

use mino::domain::{
    CheckId, CheckRunContext, CheckRunLease, CheckRunLimits, CheckRunOutcome, PlanId, RequestId,
    TaskId, Timestamp, VerificationCheck,
};
use mino::runner::{
    CheckRunJournal, ProcessRunner, RedactionRule, Redactor, RunDisposition, RunEnvironment,
    RunnerErrorKind,
};

static NEXT_TEMP_DIRECTORY: AtomicU64 = AtomicU64::new(1);

struct TestProject {
    path: PathBuf,
}

impl TestProject {
    fn new(label: &str) -> Self {
        let sequence = NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "mino-runner-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("temporary project should be created");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TestProject {
    fn drop(&mut self) {
        let temporary_root = std::env::temp_dir();
        if self.path.starts_with(&temporary_root)
            && self
                .path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("mino-runner-"))
        {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

fn request_id(sequence: u64) -> RequestId {
    RequestId::parse(format!("00000000-0000-0000-0000-{sequence:012x}"))
        .expect("request ID should be valid")
}

fn fixture_environment(mode: &str, marker: Option<&Path>) -> RunEnvironment {
    let mut environment = RunEnvironment::empty()
        .with_variable("MINO_RUNNER_FIXTURE", mode)
        .expect("fixture selector should be valid");
    if let Some(marker) = marker {
        environment = environment
            .with_variable("MINO_RUNNER_MARKER", marker.to_string_lossy())
            .expect("marker path should be valid");
    }
    environment
}

fn fixture_check() -> VerificationCheck {
    VerificationCheck::new(
        CheckId::parse("T1-V1").expect("check ID should be valid"),
        vec![
            std::env::current_exe()
                .expect("test executable should resolve")
                .to_string_lossy()
                .into_owned(),
            "--exact".to_owned(),
            "runner_fixture_process".to_owned(),
            "--nocapture".to_owned(),
        ],
        ".",
        0,
        true,
    )
}

fn lease(
    sequence: u64,
    check: &VerificationCheck,
    limits: CheckRunLimits,
    environment: &RunEnvironment,
    redactor: &Redactor,
) -> CheckRunLease {
    let context = CheckRunContext::new(
        PlanId::parse("2026-07-25-runner-contract").expect("plan ID should be valid"),
        7,
        Some(TaskId::parse("T1").expect("task ID should be valid")),
        request_id(sequence),
        "codex",
        Timestamp::parse("2026-07-25T12:00:00Z").expect("timestamp should be valid"),
    )
    .expect("run context should be valid");
    CheckRunLease::new(
        context,
        check,
        limits,
        environment.variable_names(),
        environment.digest(),
        redactor.policy_digest(),
    )
    .expect("lease should be valid")
}

fn limits(timeout_milliseconds: u64, output_limit_bytes: u64) -> CheckRunLimits {
    CheckRunLimits::new(
        Duration::from_millis(timeout_milliseconds),
        output_limit_bytes,
    )
    .expect("limits should be valid")
}

fn redactor() -> Redactor {
    Redactor::new(vec![
        RedactionRule::literal("fixture-literal", "fixture-secret"),
        RedactionRule::regex("fixture-regex", r"[0-9]+"),
    ])
    .expect("redaction policy should compile")
}

#[cfg(any(unix, windows))]
#[test]
fn run_journal_rejects_a_symlinked_managed_directory() {
    let project = TestProject::new("journal-symlink");
    let external = TestProject::new("journal-symlink-external");
    fs::create_dir(project.path().join(".mino")).expect("Mino directory should be created");
    let sentinel = external.path().join("sentinel.txt");
    fs::write(&sentinel, b"outside\n").expect("outside sentinel should be written");
    #[cfg(unix)]
    let symlink_result = symlink(external.path(), project.path().join(".mino/runs"));
    #[cfg(windows)]
    let symlink_result = symlink_dir(external.path(), project.path().join(".mino/runs"));
    if symlink_result
        .as_ref()
        .is_err_and(|error| error.kind() == std::io::ErrorKind::PermissionDenied)
    {
        return;
    }
    symlink_result.expect("run journal symlink should be created");
    let environment = RunEnvironment::empty();
    let redactor = redactor();
    let run = lease(
        90,
        &fixture_check(),
        limits(1_000, 1_024),
        &environment,
        &redactor,
    );

    let error = CheckRunJournal::new(project.path(), Path::new(".mino/runs"))
        .expect("journal root should validate lexically")
        .begin(&run)
        .expect_err("symlinked run journal directory must be rejected");
    assert_eq!(error.kind(), RunnerErrorKind::CorruptJournal);
    assert_eq!(
        fs::read(&sentinel).expect("outside sentinel should remain readable"),
        b"outside\n"
    );
    assert_eq!(
        fs::read_dir(external.path())
            .expect("outside directory should remain readable")
            .count(),
        1
    );
}

#[test]
fn runner_fixture_process() {
    let Ok(mode) = std::env::var("MINO_RUNNER_FIXTURE") else {
        return;
    };
    match mode.as_str() {
        "success" => println!("planned success"),
        "nonzero" => {
            eprintln!("planned failure");
            std::process::exit(9);
        }
        "timeout" => thread::sleep(Duration::from_secs(5)),
        "oversized" => {
            io::stdout()
                .write_all(&vec![b'x'; 32 * 1_024])
                .expect("fixture output should write");
            io::stdout().flush().expect("fixture output should flush");
            thread::sleep(Duration::from_secs(1));
        }
        "secret" => println!(
            "fixture-secret regex-123 TOKEN=automatic-secret {}",
            std::env::var("MINO_RUNNER_ACCESS_TOKEN")
                .expect("secret fixture variable should be present")
        ),
        "abort" => std::process::abort(),
        "replay" => {
            write_marker();
            println!("executed once");
        }
        "blocking" => {
            let ready = PathBuf::from(
                std::env::var_os("MINO_RUNNER_READY")
                    .expect("blocking fixture ready path should be supplied"),
            );
            let release = PathBuf::from(
                std::env::var_os("MINO_RUNNER_RELEASE")
                    .expect("blocking fixture release path should be supplied"),
            );
            fs::write(&ready, b"ready").expect("blocking fixture should signal readiness");
            let deadline = Instant::now() + Duration::from_secs(10);
            while !release.exists() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(10));
            }
            if !release.exists() {
                std::process::exit(11);
            }
            write_marker();
            println!("released once");
        }
        "child" => {
            let mut grandchild =
                Command::new(std::env::current_exe().expect("test executable should resolve"))
                    .args(["--exact", "runner_fixture_process", "--nocapture"])
                    .env("MINO_RUNNER_FIXTURE", "grandchild")
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .expect("grandchild fixture should spawn");
            thread::sleep(Duration::from_secs(5));
            grandchild
                .wait()
                .expect("grandchild fixture should be reaped");
        }
        "leader-exit" => {
            let mut grandchild =
                Command::new(std::env::current_exe().expect("test executable should resolve"))
                    .args(["--exact", "runner_fixture_process", "--nocapture"])
                    .env("MINO_RUNNER_FIXTURE", "grandchild")
                    .spawn()
                    .expect("grandchild fixture should spawn");
            assert!(
                grandchild
                    .try_wait()
                    .expect("grandchild status should be observable")
                    .is_none()
            );
        }
        "grandchild" => {
            thread::sleep(Duration::from_millis(700));
            write_marker();
        }
        unexpected => panic!("unexpected fixture mode {unexpected}"),
    }
}

#[test]
fn planned_process_outcomes_are_bounded_and_redacted() {
    let project = TestProject::new("outcomes");
    let journal = CheckRunJournal::new(project.path(), Path::new(".mino/runs"))
        .expect("journal should be valid");
    let runner = ProcessRunner::new(Duration::from_millis(5)).expect("runner should be valid");
    let redactor = redactor();
    let check = fixture_check();

    let base_cases = [
        ("success", CheckRunOutcome::Passed, 2_000, 8_192),
        ("nonzero", CheckRunOutcome::UnexpectedExit, 2_000, 8_192),
        ("timeout", CheckRunOutcome::TimedOut, 100, 8_192),
        (
            "oversized",
            CheckRunOutcome::OutputLimitExceeded,
            2_000,
            1_024,
        ),
        ("secret", CheckRunOutcome::Passed, 2_000, 8_192),
    ];
    #[cfg(unix)]
    let signal_cases = [("abort", CheckRunOutcome::UnexpectedExit, 2_000, 8_192)];
    #[cfg(not(unix))]
    let signal_cases: [(&str, CheckRunOutcome, u64, u64); 0] = [];
    let cases = base_cases.into_iter().chain(signal_cases);
    for (index, (mode, expected, timeout, output_limit)) in cases.into_iter().enumerate() {
        let mut environment = fixture_environment(mode, None);
        if mode == "secret" {
            environment = environment
                .with_variable("MINO_RUNNER_ACCESS_TOKEN", "private-123")
                .expect("secret fixture variable should be valid");
        }
        let lease = lease(
            u64::try_from(index).expect("index should fit") + 1,
            &check,
            limits(timeout, output_limit),
            &environment,
            &redactor,
        );
        assert_eq!(lease.command(), check.command());
        let run = runner
            .run_journaled(project.path(), &journal, lease, &environment, &redactor)
            .expect("planned process should produce a terminal record");
        assert_eq!(run.disposition(), RunDisposition::Executed);
        assert_eq!(run.result().outcome(), expected);
        if mode == "oversized" {
            assert!(run.result().output_truncated());
            assert!(
                run.result()
                    .stdout_summary()
                    .contains("[output truncated by Mino]")
            );
        }
        if mode == "secret" {
            let serialized = serde_json::to_string(run.result()).expect("result should serialize");
            assert!(!serialized.contains("fixture-secret"));
            assert!(!serialized.contains("automatic-secret"));
            assert!(!serialized.contains("regex-123"));
            assert!(!serialized.contains("private"));
            assert!(serialized.contains("[REDACTED]"));
            assert_eq!(run.result().redactions().len(), 4);
        }
    }
}

#[test]
fn timeout_terminates_descendants_before_they_can_escape() {
    let project = TestProject::new("tree");
    let journal = CheckRunJournal::new(project.path(), Path::new(".mino/runs"))
        .expect("journal should be valid");
    let runner = ProcessRunner::new(Duration::from_millis(5)).expect("runner should be valid");
    let redactor = redactor();
    for (index, mode) in ["child", "leader-exit"].into_iter().enumerate() {
        let marker = project.path().join(format!("{mode}-grandchild-finished"));
        let environment = fixture_environment(mode, Some(&marker));
        let lease = lease(
            u64::try_from(index).expect("index should fit") + 20,
            &fixture_check(),
            limits(100, 8_192),
            &environment,
            &redactor,
        );
        let run = runner
            .run_journaled(project.path(), &journal, lease, &environment, &redactor)
            .expect("timed process should produce evidence");
        assert_eq!(run.result().outcome(), CheckRunOutcome::TimedOut);
        assert!(run.result().process_tree_terminated());
        thread::sleep(Duration::from_millis(900));
        assert!(
            !marker.exists(),
            "terminated grandchild must not write its completion marker"
        );
    }
}

#[test]
fn request_retries_replay_and_incomplete_leases_close_once() {
    let project = TestProject::new("recovery");
    let marker = project.path().join("execution-count");
    let journal = CheckRunJournal::new(project.path(), Path::new(".mino/runs"))
        .expect("journal should be valid");
    let runner = ProcessRunner::default();
    let redactor = redactor();
    let environment = fixture_environment("replay", Some(&marker));
    let completed_lease = lease(
        30,
        &fixture_check(),
        limits(2_000, 8_192),
        &environment,
        &redactor,
    );
    let first = runner
        .run_journaled(
            project.path(),
            &journal,
            completed_lease.clone(),
            &environment,
            &redactor,
        )
        .expect("first invocation should execute");
    let replay = runner
        .run_journaled(
            project.path(),
            &journal,
            completed_lease,
            &environment,
            &redactor,
        )
        .expect("retry should replay");
    assert_eq!(first.disposition(), RunDisposition::Executed);
    assert_eq!(replay.disposition(), RunDisposition::Replayed);
    assert_eq!(first.result(), replay.result());
    assert_eq!(
        fs::read_to_string(&marker).expect("marker should exist"),
        "x"
    );

    let interrupted_lease = lease(
        31,
        &fixture_check(),
        limits(2_000, 8_192),
        &environment,
        &redactor,
    );
    assert!(
        journal
            .begin(&interrupted_lease)
            .expect("lease should publish")
    );
    let recovered = runner
        .run_journaled(
            project.path(),
            &journal,
            interrupted_lease.clone(),
            &environment,
            &redactor,
        )
        .expect("incomplete lease should recover");
    let recovered_replay = runner
        .run_journaled(
            project.path(),
            &journal,
            interrupted_lease,
            &environment,
            &redactor,
        )
        .expect("recovered result should replay");
    assert_eq!(
        recovered.disposition(),
        RunDisposition::RecoveredInterrupted
    );
    assert_eq!(recovered.result().outcome(), CheckRunOutcome::Interrupted);
    assert_eq!(recovered_replay.disposition(), RunDisposition::Replayed);
    assert_eq!(recovered.result(), recovered_replay.result());
    assert_eq!(
        fs::read_to_string(&marker).expect("marker should remain"),
        "x"
    );
}

#[test]
fn concurrent_exact_retry_reports_already_running_without_interrupting_the_owner() {
    let project = TestProject::new("concurrent");
    let marker = project.path().join("execution-count");
    let ready = project.path().join("process-ready");
    let release = project.path().join("process-release");
    let journal = CheckRunJournal::new(project.path(), Path::new(".mino/runs"))
        .expect("journal should be valid");
    let runner = ProcessRunner::default();
    let redactor = redactor();
    let environment = fixture_environment("blocking", Some(&marker))
        .with_variable("MINO_RUNNER_READY", ready.to_string_lossy())
        .expect("ready path should be valid")
        .with_variable("MINO_RUNNER_RELEASE", release.to_string_lossy())
        .expect("release path should be valid");
    let run_lease = lease(
        32,
        &fixture_check(),
        limits(10_000, 8_192),
        &environment,
        &redactor,
    );
    let winner_root = project.path().to_path_buf();
    let winner_journal = journal.clone();
    let winner_lease = run_lease.clone();
    let winner_environment = environment.clone();
    let winner_redactor = redactor.clone();
    let winner = thread::spawn(move || {
        runner.run_journaled(
            &winner_root,
            &winner_journal,
            winner_lease,
            &winner_environment,
            &winner_redactor,
        )
    });

    if !wait_for_path(&ready, Duration::from_secs(3)) {
        fs::write(&release, b"release").expect("blocked fixture should be released");
        let _ = winner.join();
        panic!("blocking fixture did not signal readiness");
    }
    let retry = runner
        .run_journaled(
            project.path(),
            &journal,
            run_lease.clone(),
            &environment,
            &redactor,
        )
        .expect_err("live exact retry must not recover interruption");
    assert_eq!(retry.kind(), RunnerErrorKind::AlreadyRunning);
    assert!(journal.result(run_lease.request_id()).unwrap().is_none());
    let request_directory = project
        .path()
        .join(".mino/runs")
        .join(run_lease.request_id().as_str());
    assert!(request_directory.join("owner.lock").is_file());
    assert!(request_directory.join("lease.json").is_file());
    assert!(!request_directory.join("result.json").exists());

    fs::write(&release, b"release").expect("blocked fixture should be released");
    let completed = winner
        .join()
        .expect("winner thread should join")
        .expect("winner should persist its result");
    assert_eq!(completed.disposition(), RunDisposition::Executed);
    assert_eq!(completed.result().outcome(), CheckRunOutcome::Passed);
    assert_eq!(
        fs::read_to_string(&marker).expect("execution marker should exist"),
        "x"
    );
    assert_eq!(
        journal
            .result(run_lease.request_id())
            .expect("terminal result should load")
            .expect("terminal result should exist")
            .outcome(),
        CheckRunOutcome::Passed
    );
    let replay = runner
        .run_journaled(project.path(), &journal, run_lease, &environment, &redactor)
        .expect("completed result should replay");
    assert_eq!(replay.disposition(), RunDisposition::Replayed);
}

#[test]
fn runner_rejects_shells_traversal_and_conflicting_retries() {
    let project = TestProject::new("policy");
    let runner = ProcessRunner::default();
    let redactor = redactor();
    let environment = RunEnvironment::empty();
    let shell_check = VerificationCheck::new(
        CheckId::parse("T1-SHELL").expect("check ID should be valid"),
        vec![if cfg!(windows) {
            "cmd.exe".to_owned()
        } else {
            "sh".to_owned()
        }],
        ".",
        0,
        true,
    );
    let shell_error = runner
        .run(
            project.path(),
            lease(
                40,
                &shell_check,
                limits(1_000, 1_024),
                &environment,
                &redactor,
            ),
            &environment,
            &redactor,
        )
        .expect_err("shell executables must be rejected");
    assert_eq!(shell_error.kind(), RunnerErrorKind::InvalidRequest);

    let traversal_check = VerificationCheck::new(
        CheckId::parse("T1-PATH").expect("check ID should be valid"),
        fixture_check().command().to_vec(),
        "..",
        0,
        true,
    );
    let traversal_error = runner
        .run(
            project.path(),
            lease(
                41,
                &traversal_check,
                limits(1_000, 1_024),
                &environment,
                &redactor,
            ),
            &environment,
            &redactor,
        )
        .expect_err("parent traversal must be rejected");
    assert_eq!(traversal_error.kind(), RunnerErrorKind::InvalidRequest);

    let journal = CheckRunJournal::new(project.path(), Path::new(".mino/runs"))
        .expect("journal should be valid");
    let first = lease(
        42,
        &fixture_check(),
        limits(1_000, 1_024),
        &environment,
        &redactor,
    );
    assert!(journal.begin(&first).expect("first lease should publish"));
    let conflicting_check = VerificationCheck::new(
        CheckId::parse("T1-CONFLICT").expect("check ID should be valid"),
        fixture_check().command().to_vec(),
        ".",
        0,
        true,
    );
    let conflicting = lease(
        42,
        &conflicting_check,
        limits(1_000, 1_024),
        &environment,
        &redactor,
    );
    let conflict = journal
        .begin(&conflicting)
        .expect_err("same request must reject different leased inputs");
    assert_eq!(conflict.kind(), RunnerErrorKind::JournalConflict);
}

fn write_marker() {
    let marker = std::env::var_os("MINO_RUNNER_MARKER")
        .map(PathBuf::from)
        .expect("marker path should be supplied");
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(marker)
        .and_then(|mut file| file.write_all(b"x"))
        .expect("marker should be written");
}

fn wait_for_path(path: &Path, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if path.exists() {
            return true;
        }
        thread::sleep(Duration::from_millis(10));
    }
    path.exists()
}
