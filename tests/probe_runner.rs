//! Contract tests for bounded automatic tool availability probes.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use mino::project::initialize;
use mino::standards::{SystemToolProbe, ToolProbe, ToolProbeOutcome};
use serde_json::Value;

static NEXT_TEMP_DIRECTORY: AtomicU64 = AtomicU64::new(1);

const HELPER_MODE: &str = "MINO_PROBE_TEST_HELPER";
const HELPER_ROOT: &str = "MINO_PROBE_TEST_ROOT";
const HELPER_EXPECTED: &str = "MINO_PROBE_TEST_EXPECTED";

struct TestArea {
    root: PathBuf,
}

impl TestArea {
    fn new(label: &str) -> Self {
        let sequence = NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "mino-probe-runner-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&root).expect("probe test area should be created");
        Self { root }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }
}

impl Drop for TestArea {
    fn drop(&mut self) {
        let temporary_root = std::env::temp_dir();
        if self.root.starts_with(&temporary_root)
            && self
                .root
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("mino-probe-runner-"))
        {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

#[test]
fn system_tool_probe_bounds_io_environment_output_time_and_descendants() {
    let area = TestArea::new("lifecycle");
    let shim_directory = area.path("shim");
    fs::create_dir(&shim_directory).expect("shim directory should be created");
    compile_probe_tool(&shim_directory);

    for (mode, expected) in [
        ("available", "available"),
        ("unavailable", "unavailable"),
        ("stdin", "available"),
        ("output", "output_limit_exceeded"),
        ("timeout", "timed_out"),
        ("descendant", "timed_out"),
    ] {
        let root = area.path(mode);
        fs::create_dir(&root).expect("probe working directory should be created");
        fs::write(root.join(format!("{mode}.mode")), b"mode")
            .expect("probe mode marker should be written");
        let started = Instant::now();
        let output = Command::new(std::env::current_exe().expect("test binary should resolve"))
            .args(["--exact", "system_tool_probe_helper", "--nocapture"])
            .env(HELPER_MODE, "run")
            .env(HELPER_ROOT, &root)
            .env(HELPER_EXPECTED, expected)
            .env("MINO_PROBE_SECRET", "must-not-leak")
            .env("PATH", &shim_directory)
            .stdin(Stdio::null())
            .output()
            .expect("probe helper should run");
        assert!(
            output.status.success(),
            "probe mode {mode} failed\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            started.elapsed() < Duration::from_secs(6),
            "probe mode {mode} exceeded its bounded deadline"
        );
    }

    thread::sleep(Duration::from_millis(900));
    assert!(
        !area.path("descendant/descendant-finished").exists(),
        "timed-out probe descendants must not escape termination"
    );
}

#[test]
fn system_tool_probe_helper() {
    if std::env::var(HELPER_MODE).as_deref() != Ok("run") {
        return;
    }
    let root =
        PathBuf::from(std::env::var_os(HELPER_ROOT).expect("probe helper root should be supplied"));
    let expected = std::env::var(HELPER_EXPECTED).expect("expected outcome should be supplied");
    let outcome = SystemToolProbe.probe("probe-tool", &root);
    assert_eq!(outcome_label(outcome), expected);
}

#[test]
fn standards_resolution_and_plan_creation_survive_bounded_probe_failure() {
    let area = TestArea::new("application");
    let shim_directory = area.path("shim");
    fs::create_dir(&shim_directory).expect("shim directory should be created");
    compile_probe_tool(&shim_directory);
    let project = area.path("project");
    fs::create_dir_all(project.join("src")).expect("project source directory should be created");
    fs::write(
        project.join("Cargo.toml"),
        "[package]\nname='probe-application'\nversion='0.1.0'\nedition='2024'\n",
    )
    .expect("project manifest should be written");
    fs::write(project.join("src/lib.rs"), "pub fn value() {}\n")
        .expect("project source should be written");
    fs::write(project.join("request.md"), "Bound automatic tool probes.\n")
        .expect("plan request should be written");
    fs::write(project.join("output.mode"), b"mode").expect("output mode should be selected");
    initialize(&project).expect("Mino project should initialize");
    let path = probe_path(&shim_directory);

    let application = json_success(&run_mino_with_path(
        &path,
        &[
            "--root".to_owned(),
            project.to_string_lossy().into_owned(),
            "--format".to_owned(),
            "json".to_owned(),
            "--no-input".to_owned(),
            "standards".to_owned(),
            "apply".to_owned(),
            "--recommended".to_owned(),
            "--seed-verification".to_owned(),
        ],
    ));
    let cargo_checks = application["checks"]
        .as_array()
        .expect("standards result should contain checks")
        .iter()
        .filter(|check| check["tool"] == "cargo")
        .collect::<Vec<_>>();
    assert!(!cargo_checks.is_empty());
    assert!(cargo_checks.iter().all(|check| {
        check["status"] == "unresolved"
            && check["unresolved_reason"] == "Tool probe output exceeded 65536 bytes"
    }));

    let created = json_success(&run_mino_with_path(
        &path,
        &[
            "--root".to_owned(),
            project.to_string_lossy().into_owned(),
            "--format".to_owned(),
            "json".to_owned(),
            "--no-input".to_owned(),
            "plan".to_owned(),
            "create".to_owned(),
            "--name".to_owned(),
            "Bounded probe plan".to_owned(),
            "--trigger".to_owned(),
            "durable".to_owned(),
            "--request-file".to_owned(),
            project.join("request.md").to_string_lossy().into_owned(),
            "--request-id".to_owned(),
            "82000000-0000-0000-0000-000000000001".to_owned(),
            "--actor".to_owned(),
            "codex".to_owned(),
        ],
    ));
    assert_eq!(created["status"], "Draft");
    assert!(created["plan_id"].is_string());
}

fn outcome_label(outcome: ToolProbeOutcome) -> &'static str {
    match outcome {
        ToolProbeOutcome::Available => "available",
        ToolProbeOutcome::Unavailable => "unavailable",
        ToolProbeOutcome::TimedOut => "timed_out",
        ToolProbeOutcome::OutputLimitExceeded => "output_limit_exceeded",
        ToolProbeOutcome::Failed => "failed",
    }
}

fn compile_probe_tool(directory: &Path) {
    let source = directory.join("probe-tool.rs");
    fs::write(
        &source,
        r#"use std::env;
use std::fs;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

fn main() {
    let mut arguments = env::args_os().skip(1);
    if arguments.next().as_deref() == Some("--grandchild".as_ref()) {
        let marker = PathBuf::from(arguments.next().expect("grandchild marker is required"));
        thread::sleep(Duration::from_secs(5));
        fs::write(marker, b"escaped").expect("grandchild marker should write");
        return;
    }
    let mut input = Vec::new();
    io::stdin().read_to_end(&mut input).expect("stdin should reach EOF");
    if env::var_os("MINO_PROBE_SECRET").is_some() {
        std::process::exit(12);
    }
    let root = env::current_dir().expect("probe cwd should resolve");
    if root.join("unavailable.mode").exists() {
        std::process::exit(9);
    }
    if root.join("output.mode").exists() {
        io::stdout().write_all(&vec![b'x'; 128 * 1024]).expect("output should write");
        io::stdout().flush().expect("output should flush");
        thread::sleep(Duration::from_secs(30));
        return;
    }
    if root.join("timeout.mode").exists() {
        thread::sleep(Duration::from_secs(30));
        return;
    }
    if root.join("descendant.mode").exists() {
        let marker = root.join("descendant-finished");
        let _child = Command::new(env::current_exe().expect("probe executable should resolve"))
            .arg("--grandchild")
            .arg(marker)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("probe descendant should start");
        thread::sleep(Duration::from_secs(30));
        return;
    }
    println!("probe tool 1.0.0");
}
"#,
    )
    .expect("probe tool source should be written");
    for name in ["probe-tool", "cargo"] {
        let executable = directory.join(if cfg!(windows) {
            format!("{name}.exe")
        } else {
            name.to_owned()
        });
        let output = Command::new("rustc")
            .args([source.as_os_str(), "-o".as_ref(), executable.as_os_str()])
            .output()
            .expect("rustc should compile the probe tool");
        assert!(
            output.status.success(),
            "probe tool compilation failed\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

fn probe_path(shim_directory: &Path) -> std::ffi::OsString {
    let mut paths = vec![shim_directory.to_path_buf()];
    if let Some(existing) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&existing));
    }
    std::env::join_paths(paths).expect("probe PATH should join")
}

fn run_mino_with_path(path: &std::ffi::OsStr, arguments: &[String]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_mino"))
        .args(arguments)
        .env("PATH", path)
        .env("MINO_PROBE_SECRET", "must-not-leak")
        .stdin(Stdio::null())
        .output()
        .expect("Mino binary should run")
}

fn json_success(output: &std::process::Output) -> Value {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    serde_json::from_slice(&output.stdout).expect("Mino stdout should be JSON")
}
