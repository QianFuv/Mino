//! Regression tests for ambient Git environment isolation.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use mino::domain::PlanId;
use mino::git::inspect_changes;
use mino::project::initialize;
use mino::store::PlanStore;
use serde_json::Value;

static NEXT_TEMP_DIRECTORY: AtomicU64 = AtomicU64::new(1);

const HELPER_ENVIRONMENT: &str = "MINO_GIT_ENVIRONMENT_HELPER";
const HELPER_ROOT: &str = "MINO_GIT_ENVIRONMENT_ROOT";
const GIT_ENVIRONMENT_NAMES: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_CONFIG_COUNT",
    "GIT_CONFIG_KEY_0",
    "GIT_CONFIG_VALUE_0",
];

struct TestArea {
    root: PathBuf,
}

impl TestArea {
    fn new(label: &str) -> Self {
        let sequence = NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "mino-git-environment-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&root).expect("temporary Git environment area should be created");
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
                .is_some_and(|name| name.to_string_lossy().starts_with("mino-git-environment-"))
        {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

#[test]
fn cli_root_inspection_and_readiness_ignore_all_ambient_git_controls() {
    let area = TestArea::new("cli");
    let primary = committed_repository(&area.path("primary"), "primary", true);
    let foreign = committed_repository(&area.path("foreign"), "foreign", false);
    fs::write(primary.join("primary-change.txt"), "primary\n")
        .expect("primary change should be written");
    fs::write(foreign.join("foreign-change.txt"), "foreign\n")
        .expect("foreign change should be written");

    let inspected = json_success(&run_poisoned_mino(&primary, &foreign, &["git", "inspect"]));
    assert_eq!(
        PathBuf::from(
            inspected["facts"]["worktree"]
                .as_str()
                .expect("Git inspection should report a worktree")
        ),
        primary
    );
    assert_eq!(
        inspected["facts"]["untracked_paths"],
        Value::from(vec!["primary-change.txt"])
    );

    fs::remove_file(primary.join("primary-change.txt")).expect("primary change should be removed");
    let base_commit = git_text(&primary, &["rev-parse", "--short", "HEAD"]);
    let request_file = primary.join("request.md").to_string_lossy().into_owned();
    let created = json_success(&run_poisoned_mino(
        &primary,
        &foreign,
        &[
            "plan",
            "create",
            "--name",
            "Git environment isolation",
            "--trigger",
            "durable",
            "--request-file",
            &request_file,
            "--request-id",
            "81000000-0000-0000-0000-000000000001",
            "--actor",
            "codex",
        ],
    ));
    let plan_id = created["plan_id"]
        .as_str()
        .expect("plan creation should report an identifier");
    let plan_id = PlanId::parse(plan_id).expect("plan identifier should parse");
    let plan = PlanStore::new(&primary)
        .load_plan(&plan_id)
        .expect("created plan should load");
    assert_eq!(plan.git_readiness().repository(), "Present");
    assert_eq!(plan.git_readiness().working_tree(), "Clean");
    assert_eq!(plan.git_readiness().branch(), Some("main"));
    assert_eq!(
        plan.git_readiness().base_commit(),
        Some(base_commit.as_str())
    );
    assert!(plan.git_readiness().git_flow_enabled());
}

#[test]
fn discovery_and_readiness_probes_enforce_timeout_and_output_limits() {
    let area = TestArea::new("bounded");
    let shim_directory = area.path("shim");
    fs::create_dir(&shim_directory).expect("Git shim directory should be created");
    compile_git_shim(&shim_directory);

    let missing = initialized_non_git_project(&area.path("missing"));
    let missing_plan = create_plan(&missing, None, 1);
    assert_eq!(missing_plan.git_readiness().repository(), "Missing");
    assert_eq!(
        missing_plan.git_readiness().working_tree(),
        "Not Applicable"
    );
    assert!(!missing_plan.git_readiness().git_flow_enabled());

    let root_timeout = initialized_non_git_project(&area.path("root-timeout"));
    fs::write(root_timeout.join("root-timeout.marker"), "timeout\n")
        .expect("root timeout marker should be written");
    let started = Instant::now();
    let mut command = Command::new(std::env::current_exe().expect("test binary should resolve"));
    command
        .args(["--exact", "root_discovery_helper", "--nocapture"])
        .env(HELPER_ENVIRONMENT, "discover")
        .env(HELPER_ROOT, &root_timeout)
        .env("PATH", &shim_directory)
        .stdin(Stdio::null());
    clear_git_environment(&mut command);
    let output = command.output().expect("root discovery helper should run");
    assert!(
        output.status.success(),
        "root discovery helper failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        started.elapsed() < Duration::from_secs(8),
        "Git root discovery exceeded its bounded timeout"
    );

    let output_limited = initialized_non_git_project(&area.path("output-limited"));
    fs::write(output_limited.join("output-limit.marker"), "limit\n")
        .expect("output limit marker should be written");
    let output_plan = create_plan(&output_limited, Some(&shim_directory), 2);
    assert_eq!(output_plan.git_readiness().repository(), "Unknown");
    assert_eq!(output_plan.git_readiness().working_tree(), "Unknown");
    assert!(!output_plan.git_readiness().git_flow_enabled());
    assert!(
        output_plan
            .git_readiness()
            .base_status()
            .contains("65536-byte limit")
    );

    let probe_timeout = initialized_non_git_project(&area.path("probe-timeout"));
    fs::write(probe_timeout.join("probe-timeout.marker"), "timeout\n")
        .expect("readiness timeout marker should be written");
    let started = Instant::now();
    let timeout_plan = create_plan(&probe_timeout, Some(&shim_directory), 3);
    assert!(
        started.elapsed() < Duration::from_secs(8),
        "Git readiness exceeded its bounded timeout"
    );
    assert_eq!(timeout_plan.git_readiness().repository(), "Unknown");
    assert!(!timeout_plan.git_readiness().git_flow_enabled());
    assert!(
        timeout_plan
            .git_readiness()
            .base_status()
            .contains("Command exceeded its timeout")
    );
}

#[test]
fn agent_context_only_downgrades_an_explicit_non_repository() {
    let area = TestArea::new("agent-inspection");
    let shim_directory = area.path("shim");
    fs::create_dir(&shim_directory).expect("Git shim directory should be created");
    compile_git_shim(&shim_directory);

    let non_repository = initialized_non_git_project(&area.path("non-repository"));
    let context = json_success(&run_with_git_shim(
        &non_repository,
        &shim_directory,
        &["agent", "context"],
    ));
    assert_eq!(context["git"], Value::Null);

    for (label, marker, exit_code, error_code) in [
        (
            "command-failure",
            "command-failure.marker",
            7,
            "environment_unavailable",
        ),
        ("malformed", "malformed.marker", 8, "drift_detected"),
        ("output-limit", "output-limit.marker", 8, "drift_detected"),
    ] {
        let project = initialized_non_git_project(&area.path(label));
        fs::write(project.join(marker), "trigger\n").expect("Git failure marker should be written");
        let output = run_with_git_shim(&project, &shim_directory, &["agent", "context"]);
        let failure = json_failure(&output, exit_code, error_code);
        assert_eq!(failure["active_plan"], Value::Null);
        assert!(!String::from_utf8_lossy(&output.stdout).contains("gitcommandcredentialvalue"));
    }

    let timeout = initialized_non_git_project(&area.path("timeout"));
    fs::write(timeout.join("probe-timeout.marker"), "timeout\n")
        .expect("Git timeout marker should be written");
    let started = Instant::now();
    let failure = run_with_git_shim(&timeout, &shim_directory, &["agent", "context"]);
    assert!(
        started.elapsed() < Duration::from_secs(8),
        "Agent Git inspection exceeded its bounded timeout"
    );
    json_failure(&failure, 7, "environment_unavailable");

    let empty_path = area.path("missing-git");
    fs::create_dir(&empty_path).expect("empty PATH directory should be created");
    let missing = initialized_non_git_project(&area.path("missing-executable"));
    let failure = run_with_git_shim(&missing, &empty_path, &["agent", "context"]);
    json_failure(&failure, 7, "environment_unavailable");
}

#[test]
fn recorded_git_plan_blocks_agent_context_when_repository_metadata_breaks() {
    let area = TestArea::new("recorded-git-failure");
    let repository = committed_repository(&area.path("repository"), "recorded-git", true);
    let plan = create_plan(&repository, None, 20);
    assert_eq!(plan.git_readiness().repository(), "Present");
    assert!(plan.git_readiness().git_flow_enabled());

    let secret = "gitmetadatacredentialvalue";
    fs::write(repository.join(".git/HEAD"), format!("invalid {secret}\n"))
        .expect("corrupt HEAD should be written");
    let output = run_mino(&repository, &["agent", "context"]);
    json_failure(&output, 7, "environment_unavailable");
    assert!(!String::from_utf8_lossy(&output.stdout).contains(secret));
}

#[test]
fn inspect_changes_uses_project_root_and_default_index_under_each_git_override() {
    let area = TestArea::new("changes");
    let primary = committed_repository(&area.path("primary"), "primary", false);
    let foreign = committed_repository(&area.path("foreign"), "foreign", false);
    fs::write(primary.join("inside-change.txt"), "inside\n")
        .expect("inside change should be written");
    fs::write(foreign.join("foreign-change.txt"), "foreign\n")
        .expect("foreign change should be written");

    for override_name in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_CONFIG_COUNT",
    ] {
        let mut command =
            Command::new(std::env::current_exe().expect("test binary should resolve"));
        command
            .args(["--exact", "inspect_changes_helper", "--nocapture"])
            .env(HELPER_ENVIRONMENT, "inspect_changes")
            .env(HELPER_ROOT, &primary)
            .stdin(Stdio::null());
        clear_git_environment(&mut command);
        apply_single_override(&mut command, override_name, &foreign);
        let output = command.output().expect("inspection helper should run");
        assert!(
            output.status.success(),
            "override {override_name} escaped the project Git boundary\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn inspect_changes_helper() {
    if std::env::var(HELPER_ENVIRONMENT).as_deref() != Ok("inspect_changes") {
        return;
    }
    let root = PathBuf::from(
        std::env::var_os(HELPER_ROOT).expect("inspection helper root should be supplied"),
    );
    let changes = inspect_changes(&root).expect("project changes should inspect");
    assert!(changes.is_repository());
    assert_eq!(changes.files().len(), 1);
    assert_eq!(changes.files()[0].path(), "inside-change.txt");
}

#[test]
fn root_discovery_helper() {
    if std::env::var(HELPER_ENVIRONMENT).as_deref() != Ok("discover") {
        return;
    }
    let root = PathBuf::from(
        std::env::var_os(HELPER_ROOT).expect("root discovery helper path should be supplied"),
    );
    let discovered = mino::project::discover(&root).expect("Mino marker fallback should succeed");
    assert_eq!(discovered.path(), root);
    assert_eq!(
        discovered.source(),
        mino::project::RootSource::MinoDirectory
    );
}

fn committed_repository(path: &Path, contents: &str, initialize_mino: bool) -> PathBuf {
    fs::create_dir_all(path.join("src")).expect("repository source directory should be created");
    fs::write(
        path.join("Cargo.toml"),
        format!("[package]\nname = \"{contents}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n"),
    )
    .expect("repository manifest should be written");
    fs::write(
        path.join("src/lib.rs"),
        format!("pub const VALUE: &str = \"{contents}\";\n"),
    )
    .expect("repository source should be written");
    fs::write(
        path.join("request.md"),
        "Preserve the Git project boundary.\n",
    )
    .expect("plan request should be written");
    fs::write(path.join(".gitignore"), "/.mino/\n/docs/plan/\n")
        .expect("ignore file should be written");
    if initialize_mino {
        initialize(path).expect("Mino project should initialize");
    }
    git(path, &["init", "--quiet", "--initial-branch", "main"]);
    git(path, &["add", "."]);
    git(
        path,
        &[
            "-c",
            "user.name=Mino Tests",
            "-c",
            "user.email=mino-tests@example.invalid",
            "commit",
            "--quiet",
            "-m",
            contents,
        ],
    );
    path.canonicalize().expect("repository should resolve")
}

fn initialized_non_git_project(path: &Path) -> PathBuf {
    fs::create_dir_all(path).expect("non-Git project should be created");
    fs::write(
        path.join("Cargo.toml"),
        "[package]\nname = \"bounded-probe\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("non-Git manifest should be written");
    fs::write(path.join("request.md"), "Bound Git probes.\n")
        .expect("bounded probe request should be written");
    initialize(path).expect("non-Git Mino project should initialize");
    path.canonicalize().expect("non-Git project should resolve")
}

fn create_plan(
    root: &Path,
    shim_directory: Option<&Path>,
    request_number: u64,
) -> mino::domain::Plan {
    let request_file = root.join("request.md").to_string_lossy().into_owned();
    let request_id = format!("81000000-0000-0000-0000-{request_number:012}");
    let arguments = [
        "plan",
        "create",
        "--name",
        "Bounded Git probe",
        "--trigger",
        "durable",
        "--request-file",
        &request_file,
        "--request-id",
        &request_id,
        "--actor",
        "codex",
    ];
    let output = if let Some(shim_directory) = shim_directory {
        run_with_git_shim(root, shim_directory, &arguments)
    } else {
        run_mino(root, &arguments)
    };
    let created = json_success(&output);
    let plan_id = created["plan_id"]
        .as_str()
        .expect("plan creation should report an identifier");
    let plan_id = PlanId::parse(plan_id).expect("plan identifier should parse");
    PlanStore::new(root)
        .load_plan(&plan_id)
        .expect("bounded-probe plan should load")
}

fn compile_git_shim(directory: &Path) {
    let source = directory.join("git-shim.rs");
    let executable = directory.join(if cfg!(windows) { "git.exe" } else { "git" });
    fs::write(
        &source,
        r#"use std::env;
use std::io::{self, Write};
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

fn main() {
    let arguments = env::args_os().collect::<Vec<_>>();
    let root = arguments
        .windows(2)
        .find(|pair| pair[0] == "-C")
        .map(|pair| PathBuf::from(&pair[1]))
        .expect("Git shim should receive -C root");
    let is_root_probe = arguments.iter().any(|argument| argument == "--show-toplevel");
    if is_root_probe && root.join("root-timeout.marker").exists() {
        thread::sleep(Duration::from_secs(30));
        return;
    }
    if is_root_probe {
        println!("{}", root.display());
        return;
    }
    if arguments
        .iter()
        .any(|argument| argument == "--is-bare-repository")
    {
        println!("false");
        return;
    }
    if arguments
        .iter()
        .any(|argument| argument == "--is-inside-work-tree")
    {
        if root.join("command-failure.marker").exists() {
            eprintln!("permission denied: gitcommandcredentialvalue");
            std::process::exit(1);
        }
        if root.join("malformed.marker").exists() {
            println!("invalid-worktree-value");
            return;
        }
        if root.join("probe-timeout.marker").exists() {
            thread::sleep(Duration::from_secs(30));
            return;
        }
        if root.join("output-limit.marker").exists() {
            io::stdout()
                .write_all(&vec![b'x'; 70 * 1024])
                .expect("Git shim output should write");
            return;
        }
        println!("false");
        return;
    }
}
"#,
    )
    .expect("Git shim source should be written");
    let output = Command::new("rustc")
        .args([source.as_os_str(), "-o".as_ref(), executable.as_os_str()])
        .output()
        .expect("rustc should compile the Git shim");
    assert!(
        output.status.success(),
        "Git shim compilation failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn run_with_git_shim(root: &Path, shim_directory: &Path, arguments: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mino"));
    command
        .arg("--root")
        .arg(root)
        .args(["--format", "json", "--no-input"])
        .args(arguments)
        .env("PATH", shim_directory)
        .stdin(Stdio::null());
    clear_git_environment(&mut command);
    command
        .output()
        .expect("Mino binary should run with Git shim")
}

fn run_mino(root: &Path, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mino"))
        .arg("--root")
        .arg(root)
        .args(["--format", "json", "--no-input"])
        .args(arguments)
        .stdin(Stdio::null())
        .output()
        .expect("Mino binary should run")
}

fn run_poisoned_mino(root: &Path, foreign: &Path, arguments: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mino"));
    command
        .arg("--root")
        .arg(root)
        .args(["--format", "json", "--no-input"])
        .args(arguments)
        .stdin(Stdio::null());
    clear_git_environment(&mut command);
    command
        .env("GIT_DIR", foreign.join(".git"))
        .env("GIT_WORK_TREE", foreign)
        .env("GIT_INDEX_FILE", foreign.join(".git/index"))
        .env("GIT_OBJECT_DIRECTORY", foreign.join(".git/objects"))
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "core.worktree")
        .env("GIT_CONFIG_VALUE_0", foreign)
        .output()
        .expect("Mino binary should run")
}

fn apply_single_override(command: &mut Command, name: &str, foreign: &Path) {
    match name {
        "GIT_DIR" => {
            command.env(name, foreign.join(".git"));
        }
        "GIT_WORK_TREE" => {
            command.env(name, foreign);
        }
        "GIT_INDEX_FILE" => {
            command.env(name, foreign.join(".git/index"));
        }
        "GIT_OBJECT_DIRECTORY" => {
            command.env(name, foreign.join(".git/objects"));
        }
        "GIT_CONFIG_COUNT" => {
            command
                .env(name, "1")
                .env("GIT_CONFIG_KEY_0", "core.worktree")
                .env("GIT_CONFIG_VALUE_0", foreign);
        }
        _ => unreachable!("override names are fixed by the test"),
    }
}

fn clear_git_environment(command: &mut Command) {
    for name in GIT_ENVIRONMENT_NAMES {
        command.env_remove(name);
    }
}

fn json_success(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    serde_json::from_slice(&output.stdout).expect("Mino stdout should be JSON")
}

fn json_failure(output: &Output, exit_code: i32, error_code: &str) -> Value {
    assert_eq!(
        output.status.code(),
        Some(exit_code),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let value: Value = serde_json::from_slice(&output.stdout).expect("Mino failure should be JSON");
    assert_eq!(value["error"]["code"], error_code);
    value
}

fn git(path: &Path, arguments: &[&str]) {
    let mut command = Command::new("git");
    command.args(arguments).current_dir(path);
    clear_git_environment(&mut command);
    let output = command.output().expect("Git should run");
    assert!(
        output.status.success(),
        "git {arguments:?} failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_text(path: &Path, arguments: &[&str]) -> String {
    let mut command = Command::new("git");
    command.args(arguments).current_dir(path);
    clear_git_environment(&mut command);
    let output = command.output().expect("Git should run");
    assert!(output.status.success(), "Git text query should succeed");
    String::from_utf8(output.stdout)
        .expect("Git text should be UTF-8")
        .trim()
        .to_owned()
}
