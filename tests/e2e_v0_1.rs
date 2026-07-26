//! Installed-binary release proof for the complete Mino v0.1 lifecycle.

use std::collections::BTreeMap;
use std::env;
use std::fmt::Write as FmtWrite;
use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use mino::domain::{
    CURRENT_PROTOCOL_VERSION, CheckpointKind, PlanId, RequestId, TaskId, Timestamp,
};
use mino::evidence::EvidenceStore;
use mino::project::{ProjectConfig, ProjectLayout};
use mino::standards::{SourcePolicy, SyncLimits, SyncOptions, synchronize_all_with_options};
use mino::store::{
    CommitOptions, FailurePoint, MutationRequest, PlanStore, StoreErrorKind, sha256_digest,
};
use serde_json::Value;

static NEXT_TEMP_DIRECTORY: AtomicU64 = AtomicU64::new(1);

struct TestWorkspace {
    root: PathBuf,
    project: PathBuf,
    install_root: PathBuf,
}

impl TestWorkspace {
    fn new() -> Self {
        let sequence = NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let root = env::temp_dir().join(format!("mino e2e 空间-{}-{sequence}", std::process::id()));
        let project = root.join("repository with spaces");
        fs::create_dir_all(&project).expect("E2E project should be created");
        let root = root.canonicalize().expect("E2E root should resolve");
        Self {
            project: root.join("repository with spaces"),
            install_root: root.join("installed binary"),
            root,
        }
    }
}

impl Drop for TestWorkspace {
    fn drop(&mut self) {
        let temporary_root = env::temp_dir();
        if self.root.starts_with(&temporary_root)
            && self
                .root
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("mino e2e 空间-"))
        {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

struct TestServer {
    address: SocketAddr,
    stop: Arc<AtomicBool>,
    requests: Arc<Mutex<Vec<String>>>,
    thread: Option<JoinHandle<()>>,
}

impl TestServer {
    fn new(build_routes: impl FnOnce(&str) -> BTreeMap<String, Vec<u8>>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("test server should bind");
        listener
            .set_nonblocking(true)
            .expect("test listener should be nonblocking");
        let address = listener
            .local_addr()
            .expect("test server address should resolve");
        let routes = Arc::new(build_routes(&format!("http://{address}")));
        let stop = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let server_stop = Arc::clone(&stop);
        let server_requests = Arc::clone(&requests);
        let thread = thread::spawn(move || {
            while !server_stop.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let routes = Arc::clone(&routes);
                        let requests = Arc::clone(&server_requests);
                        thread::spawn(move || handle_connection(stream, &routes, &requests));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(_) if !server_stop.load(Ordering::Acquire) => {
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            address,
            stop,
            requests,
            thread: Some(thread),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.address, path)
    }

    fn request_count(&self) -> usize {
        self.requests
            .lock()
            .expect("request log should be available")
            .len()
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = TcpStream::connect(self.address);
        if let Some(thread) = self.thread.take() {
            thread.join().expect("test server should stop");
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
struct PlanBytes {
    state: Vec<u8>,
    events: Vec<u8>,
    projection: Vec<u8>,
}

#[test]
fn installed_binary_completes_the_v0_1_lifecycle_without_source_leakage() {
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let source_status = git_status(source_root);
    let workspace = TestWorkspace::new();
    let binary = install_binary(&workspace, source_root);
    verify_version(&binary);
    prepare_project_inputs(&workspace.project, &binary);
    let server = verify_project_and_standards_surface(&binary, &workspace.project);
    let plan_id = author_and_approve_plan(&binary, &workspace.project);
    execute_plan_through_review(&binary, &workspace.project, &plan_id);
    verify_final_state(&binary, &workspace.project, &plan_id);
    assert!(server.request_count() >= 4);
    assert_eq!(git_status(source_root), source_status);
}

#[test]
fn catalog_package_digest_normalizes_line_endings() {
    let lf = package_digest("manifest\n", "rules\n", "checks\n");
    let crlf = package_digest("manifest\r\n", "rules\r\n", "checks\r\n");
    let cr = package_digest("manifest\r", "rules\r", "checks\r");

    assert_eq!(lf, crlf);
    assert_eq!(lf, cr);
}

fn install_binary(workspace: &TestWorkspace, source_root: &Path) -> PathBuf {
    if let Some(configured) = env::var_os("MINO_E2E_BINARY") {
        let binary = PathBuf::from(configured);
        assert!(binary.is_file(), "configured E2E binary must exist");
        return binary;
    }
    let target_directory = workspace.root.join("cargo install target");
    let output = Command::new(env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .arg("install")
        .arg("--path")
        .arg(source_root)
        .arg("--root")
        .arg(&workspace.install_root)
        .args(["--locked", "--offline", "--debug"])
        .env("CARGO_TARGET_DIR", target_directory)
        .stdin(Stdio::null())
        .output()
        .expect("cargo install should start");
    assert_successful_process(&output, "cargo install");
    workspace
        .install_root
        .join("bin")
        .join(if cfg!(windows) { "mino.exe" } else { "mino" })
}

fn verify_version(binary: &Path) {
    let output = run_binary(binary, &["--version".to_owned()]);
    assert_successful_process(&output, "mino --version");
    assert_eq!(output.stdout, b"mino 0.1.0\n");
    assert!(output.stderr.is_empty());
}

fn prepare_project_inputs(project: &Path, binary: &Path) {
    fs::write(project.join(".gitignore"), "/target/\n/runner-ready/\n")
        .expect("user ignore rules should be written");
    let probe = project.join("language-probe");
    fs::create_dir_all(probe.join("src")).expect("language probe should be created");
    fs::write(
        probe.join("Cargo.toml"),
        "[package]\nname = \"language-probe\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .expect("language probe manifest should be written");
    fs::write(
        probe.join("src/lib.rs"),
        "//! Language detection probe.\n\n/// Returns a probe value.\npub const fn probe() -> u8 { 1 }\n",
    )
    .expect("language probe source should be written");
    let fixtures = project.join("fixtures");
    let legacy = fixtures.join("legacy files");
    fs::create_dir_all(&legacy).expect("fixture directories should be created");
    let plan_template = fixture_path("e2e/plan.yaml");
    let child_root = project.join("runner-ready");
    let rendered_plan = fs::read_to_string(plan_template)
        .expect("E2E plan template should be readable")
        .replace(
            "{{MINO_BINARY}}",
            &serde_json::to_string(&binary.to_string_lossy()).expect("binary path should encode"),
        )
        .replace(
            "{{CHILD_ROOT}}",
            &serde_json::to_string(&child_root.to_string_lossy())
                .expect("child path should encode"),
        );
    fs::write(fixtures.join("plan.yaml"), rendered_plan)
        .expect("rendered plan fixture should be written");
    for name in ["AGENTS.md", "PLAN_TEMPLATE.md", "PLAN_EXECUTION.md"] {
        fs::copy(fixture_path(&format!("legacy/{name}")), legacy.join(name))
            .expect("legacy fixture should be copied");
    }
    fs::write(
        project.join("请求.md"),
        "Exercise the complete Mino v0.1 lifecycle through Review.\n",
    )
    .expect("request fixture should be written");
}

fn verify_project_and_standards_surface(binary: &Path, project: &Path) -> TestServer {
    let init = assert_success(&run_json(
        binary,
        project,
        &strings(&[
            "project",
            "init",
            "--apply-agents-block",
            "--apply-gitignore-block",
        ]),
    ));
    assert_eq!(init["complete"], true);
    assert!(project.join(".agents/skills/mino/SKILL.md").is_file());
    assert!(
        project
            .join(".agents/skills/mino/agents/openai.yaml")
            .is_file()
    );
    let repeated = assert_success(&run_json(
        binary,
        project,
        &strings(&[
            "project",
            "init",
            "--apply-agents-block",
            "--apply-gitignore-block",
        ]),
    ));
    assert_eq!(repeated["complete"], true);
    assert_eq!(repeated["findings"], Value::Array(Vec::new()));

    let scan = assert_success(&run_json(binary, project, &strings(&["project", "scan"])));
    assert_eq!(scan["languages"][0]["language"], "rust");
    let detected = assert_success(&run_json(
        binary,
        project,
        &strings(&["standards", "detect"]),
    ));
    assert!(
        detected["languages"]
            .as_array()
            .is_some_and(|languages| languages.contains(&Value::from("rust")))
    );
    let recommendation = assert_success(&run_json(
        binary,
        project,
        &strings(&["standards", "recommend"]),
    ));
    assert_eq!(recommendation["packages"][0]["package_id"], "common");
    assert_eq!(recommendation["packages"][1]["package_id"], "rust");
    let applied = assert_success(&run_json(
        binary,
        project,
        &strings(&["standards", "apply", "--recommended", "--seed-verification"]),
    ));
    assert_eq!(applied["standards"][1]["package_id"], "rust");
    assert!(
        applied["checks"]
            .as_array()
            .is_some_and(|checks| !checks.is_empty())
    );
    assert_failure(
        &run_json(binary, project, &strings(&["standards", "apply"])),
        2,
        "incomplete_or_validation",
    );

    fs::remove_dir_all(project.join("language-probe"))
        .expect("language probe should be removed before the Git baseline");
    establish_git_baseline(project);
    verify_synchronized_project_surface(binary, project)
}

fn verify_synchronized_project_surface(binary: &Path, project: &Path) -> TestServer {
    let server = synchronize_local_catalog(project);
    let requests_before_policy_check = server.request_count();
    assert_failure(
        &run_json(binary, project, &strings(&["standards", "sync", "--all"])),
        7,
        "environment_unavailable",
    );
    assert_eq!(server.request_count(), requests_before_policy_check);

    let shown = assert_success(&run_json(binary, project, &strings(&["project", "show"])));
    assert_eq!(shown["complete"], true);
    assert_eq!(
        shown["standards_lock"]["packages"][0]["package_id"],
        "common"
    );
    assert_eq!(shown["standards_lock"]["packages"][0]["version"], "9.9.9");
    verify_legacy_analysis(binary, project);

    let protocol = assert_success(&run_json(
        binary,
        project,
        &strings(&["protocol", "status"]),
    ));
    assert_eq!(protocol["compatible"], true);
    assert_eq!(
        protocol["manifest"]["protocol_version"],
        CURRENT_PROTOCOL_VERSION
    );
    let capabilities = assert_agent_success(&run_json(
        binary,
        project,
        &strings(&["agent", "capabilities"]),
    ));
    assert_eq!(capabilities["kind"], "mino.agent-capabilities/v1");
    assert!(
        capabilities["actions"]
            .as_array()
            .is_some_and(|actions| actions.iter().any(|action| action["id"] == "exec.finish"))
    );
    let context = assert_agent_success(&run_json(binary, project, &strings(&["agent", "context"])));
    assert_eq!(context["active_plan"], Value::Null);
    let policy_output = run_with_global_flags(
        binary,
        project,
        &["--format", "json"],
        &strings(&["agent", "context"]),
    );
    assert_failure(&policy_output, 5, "policy_violation");
    let human = run_with_global_flags(binary, project, &[], &strings(&["project", "doctor"]));
    assert_successful_process(&human, "human project doctor");
    assert!(String::from_utf8_lossy(&human.stdout).contains("0 finding(s)"));
    server
}

fn author_and_approve_plan(binary: &Path, project: &Path) -> String {
    let plan_id = create_and_prepare_draft(binary, project);
    finalize_and_approve_plan(binary, project, &plan_id);
    plan_id
}

fn create_and_prepare_draft(binary: &Path, project: &Path) -> String {
    let create = assert_success(&run_json(
        binary,
        project,
        &[
            strings(&[
                "plan",
                "create",
                "--name",
                "Complete E2E lifecycle",
                "--trigger",
                "durable",
            ]),
            vec![
                "--request-file".to_owned(),
                project.join("请求.md").to_string_lossy().into_owned(),
                "--request-id".to_owned(),
                request_id(1),
                "--actor".to_owned(),
                "codex".to_owned(),
            ],
        ]
        .concat(),
    ));
    assert_eq!(create["status"], "Draft");
    assert_eq!(create["revision"], 1);
    assert_eq!(create["complete"], false);
    let plan_id = create["plan_id"]
        .as_str()
        .expect("create should return a plan ID")
        .to_owned();
    let next = assert_success(&run_json(
        binary,
        project,
        &read_arguments("next", &plan_id),
    ));
    assert!(
        !next["missing"]
            .as_array()
            .expect("missing should be an array")
            .is_empty()
    );

    let mut apply = mutation_arguments(&["plan", "apply"], &plan_id, 1, 2);
    apply.extend([
        "--file".to_owned(),
        project
            .join("fixtures/plan.yaml")
            .to_string_lossy()
            .into_owned(),
    ]);
    let applied = assert_success(&run_json(binary, project, &apply));
    assert_eq!(applied["revision"], 2);
    let mut stale = mutation_arguments(&["plan", "apply"], &plan_id, 1, 3);
    stale.extend([
        "--file".to_owned(),
        project
            .join("fixtures/plan.yaml")
            .to_string_lossy()
            .into_owned(),
    ]);
    assert_failure(&run_json(binary, project, &stale), 3, "revision_conflict");

    let validation = assert_success(&run_json(
        binary,
        project,
        &read_arguments("validate", &plan_id),
    ));
    assert_eq!(validation["valid"], true);
    let draft = assert_success(&run_json(
        binary,
        project,
        &read_arguments("show", &plan_id),
    ));
    assert_eq!(draft["standards"][0]["package_id"], "common");
    assert_eq!(draft["standards"].as_array().map(Vec::len), Some(1));
    assert_eq!(draft["verification_plan"].as_array().map(Vec::len), Some(1));
    verify_projection_drift(binary, project, &plan_id);
    verify_protocol_migrations(binary, project, &plan_id, 2);
    plan_id
}

fn finalize_and_approve_plan(binary: &Path, project: &Path, plan_id: &str) {
    let finalized = assert_success(&run_json(
        binary,
        project,
        &mutation_arguments(&["plan", "finalize"], plan_id, 2, 6),
    ));
    assert_eq!(finalized["status"], "Ready");
    assert_eq!(finalized["revision"], 3);
    let review = assert_success(&run_json(
        binary,
        project,
        &read_arguments("review", plan_id),
    ));
    assert_eq!(review["review_kind"], "mino.plan-review/v1");
    assert_eq!(review["approval_required"], true);
    let unapproved_start = exec_arguments(&["start"], plan_id, 3, 7, &["--task", "T1"]);
    assert_failure(
        &run_json(binary, project, &unapproved_start),
        4,
        "approval_required",
    );

    let mut approve = mutation_arguments(&["plan", "approve"], plan_id, 3, 8);
    approve.extend(strings(&[
        "--approval-ref",
        "chat:e2e-explicit-approval",
        "--git-flow-consent",
        "approved",
    ]));
    let approved = assert_success(&run_json(binary, project, &approve));
    assert_eq!(approved["revision"], 4);
    let next = assert_agent_success(&run_json(binary, project, &strings(&["agent", "next"])));
    assert_eq!(next["approval_required"], false);
    assert_eq!(next["next_actions"][0]["id"], "exec.start");
}

fn execute_plan_through_review(binary: &Path, project: &Path, plan_id: &str) {
    start_recover_and_resume(binary, project, plan_id);
    run_checks_and_finish(binary, project, plan_id);
}

fn start_recover_and_resume(binary: &Path, project: &Path, plan_id: &str) {
    let started = assert_success(&run_json(
        binary,
        project,
        &exec_arguments(&["start"], plan_id, 4, 9, &["--task", "T1"]),
    ));
    assert_eq!(started["revision"], 5);
    let checkpoint = exec_arguments(
        &["checkpoint"],
        plan_id,
        5,
        10,
        &[
            "--task",
            "T1",
            "--kind",
            "inspection",
            "--summary",
            "Inspected the disposable execution boundary",
        ],
    );
    inject_checkpoint_transaction(project, plan_id, &checkpoint, 5, 10);
    let recovered = assert_success(&run_json(binary, project, &checkpoint));
    assert_eq!(recovered["revision"], 6);
    assert_eq!(recovered["replayed"], true);

    let blocked = assert_success(&run_json(
        binary,
        project,
        &exec_arguments(
            &["block"],
            plan_id,
            6,
            11,
            &["--reason", "Exercise resumable blocking"],
        ),
    ));
    assert_eq!(blocked["status"], "Blocked");
    assert_eq!(blocked["revision"], 7);
    let blocked_context =
        assert_agent_success(&run_json(binary, project, &strings(&["agent", "context"])));
    assert_eq!(blocked_context["active_plan"]["status"], "Blocked");
    assert_eq!(blocked_context["next_actions"][0]["id"], "exec.resume");
    let resumed = assert_success(&run_json(
        binary,
        project,
        &exec_arguments(&["resume"], plan_id, 7, 12, &[]),
    ));
    assert_eq!(resumed["status"], "In Progress");
    assert_eq!(resumed["revision"], 8);
}

fn run_checks_and_finish(binary: &Path, project: &Path, plan_id: &str) {
    let failed_check = exec_arguments(&["check", "run"], plan_id, 8, 13, &["--check", "T1-CHECK"]);
    let failed = assert_failure(&run_json(binary, project, &failed_check), 6, "check_failed");
    assert_eq!(failed["execution"]["plan"]["revision"], 10);
    assert_eq!(failed["execution"]["run"]["outcome"], "unexpected_exit");
    assert_eq!(failed["execution"]["evidence"]["exit_code"], 7);
    assert_eq!(failed["execution"]["evidence"]["id"], "E0001");

    prepare_passing_child(binary, project);
    fs::write(project.join("feature.txt"), "verified feature\n")
        .expect("planned feature should be created");
    let passing_check =
        exec_arguments(&["check", "run"], plan_id, 10, 14, &["--check", "T1-CHECK"]);
    let passed = assert_success(&run_json(binary, project, &passing_check));
    assert_eq!(passed["plan"]["revision"], 12);
    assert_eq!(passed["run"]["outcome"], "passed");
    assert_eq!(passed["evidence"]["id"], "E0002");
    assert_eq!(passed["disposition"], "executed");
    let replayed = assert_success(&run_json(binary, project, &passing_check));
    assert_eq!(replayed["plan"]["revision"], 12);
    assert_eq!(replayed["disposition"], "replayed");

    verify_and_add_evidence(binary, project, plan_id);
    let criterion = assert_success(&run_json(
        binary,
        project,
        &exec_arguments(
            &["criterion", "pass"],
            plan_id,
            12,
            16,
            &["--criterion", "T1-A1", "--evidence", "E0002"],
        ),
    ));
    assert_eq!(criterion["revision"], 13);
    let completed = assert_success(&run_json(
        binary,
        project,
        &exec_arguments(&["complete"], plan_id, 13, 17, &["--task", "T1"]),
    ));
    assert_eq!(completed["revision"], 14);

    let next = assert_agent_success(&run_json(binary, project, &strings(&["agent", "next"])));
    assert_eq!(next["next_actions"][0]["id"], "git.commit");
    assert!(next["blocked_actions"].as_array().is_some_and(|actions| {
        actions
            .iter()
            .all(|action| action["action"] != "git.commit")
    }));
    let bound = assert_success(&run_json(
        binary,
        project,
        &strings(&["git", "bind", "--plan", plan_id, "--current"]),
    ));
    assert_eq!(bound["binding"]["plan_id"], plan_id);
    let committed = assert_success(&run_json(
        binary,
        project,
        &strings(&["git", "commit", "--plan", plan_id, "--task", "T1"]),
    ));
    assert_eq!(committed["plan_revision"], 15);
    assert_eq!(
        committed["completion"]["message"],
        "feat(fixture): add verified feature"
    );
    assert_eq!(
        committed["completion"]["files"],
        serde_json::json!(["feature.txt"])
    );

    let global = assert_success(&run_json(
        binary,
        project,
        &exec_arguments(
            &["check", "run"],
            plan_id,
            15,
            18,
            &["--check", "GLOBAL-SMOKE"],
        ),
    ));
    assert_eq!(global["plan"]["revision"], 17);
    assert_eq!(global["evidence"]["id"], "E0005");
    let finish = exec_arguments(&["finish"], plan_id, 17, 19, &[]);
    let finished = assert_success(&run_json(binary, project, &finish));
    assert_eq!(finished["status"], "Review");
    assert_eq!(finished["revision"], 18);
    let projection = projection_path(project, plan_id);
    fs::remove_file(&projection).expect("projection-loss fixture should be injected");
    let recovered_projection = assert_success(&run_json(binary, project, &finish));
    assert_eq!(recovered_projection["revision"], 18);
    assert_eq!(recovered_projection["replayed"], true);
    assert!(projection.is_file());
}

fn verify_final_state(binary: &Path, project: &Path, plan_id: &str) {
    let shown = assert_success(&run_json(binary, project, &read_arguments("show", plan_id)));
    assert_eq!(shown["status"], "Review");
    assert_eq!(shown["revision"], 18);
    assert_eq!(shown["tasks"][0]["status"], "Done");
    assert_eq!(
        shown["tasks"][0]["acceptance_criteria"][0]["status"],
        "Passed"
    );
    assert_eq!(
        shown["tasks"][0]["verification_checks"][0]["status"],
        "Passed"
    );
    assert_eq!(shown["verification_plan"][0]["status"], "Passed");

    let context = assert_agent_success(&run_json(binary, project, &strings(&["agent", "context"])));
    assert_eq!(context["active_plan"]["status"], "Review");
    assert_eq!(context["approval_required"], true);
    assert_eq!(context["next_actions"], Value::Array(Vec::new()));
    let next = assert_agent_success(&run_json(binary, project, &strings(&["agent", "next"])));
    assert_eq!(next["approval_required"], true);
    assert_eq!(next["next_actions"], Value::Array(Vec::new()));

    let doctor = assert_success(&run_json(binary, project, &strings(&["project", "doctor"])));
    assert_eq!(doctor["complete"], true);
    assert_eq!(doctor["findings"], Value::Array(Vec::new()));
    let project_show = assert_success(&run_json(binary, project, &strings(&["project", "show"])));
    assert_eq!(project_show["doctor"]["findings"], Value::Array(Vec::new()));
    let protocol = assert_success(&run_json(
        binary,
        project,
        &strings(&["protocol", "status"]),
    ));
    assert_eq!(protocol["compatible"], true);

    let typed_plan = PlanId::parse(plan_id).expect("plan ID should parse");
    let audit = PlanStore::new(project)
        .audit(&typed_plan)
        .expect("plan store should audit");
    assert_eq!(audit.revision(), 18);
    assert_eq!(audit.event_count(), 18);
    assert_eq!(audit.snapshot_count(), 18);
    let evidence = EvidenceStore::new(project);
    let records = evidence.list(&typed_plan).expect("evidence should list");
    assert_eq!(records.len(), 5);
    assert!(
        evidence
            .audit(&typed_plan)
            .expect("evidence should audit")
            .is_healthy()
    );
    assert!(git_status(project).is_empty());
}

fn verify_legacy_analysis(binary: &Path, project: &Path) {
    let legacy = project.join("fixtures/legacy files");
    let paths = [
        legacy.join("AGENTS.md"),
        legacy.join("PLAN_TEMPLATE.md"),
        legacy.join("PLAN_EXECUTION.md"),
    ];
    let before = paths
        .iter()
        .map(|path| fs::read(path).expect("legacy source should be readable"))
        .collect::<Vec<_>>();
    let arguments = [
        strings(&["project", "migrate", "legacy"]),
        vec![
            "--agents".to_owned(),
            paths[0].to_string_lossy().into_owned(),
            "--template".to_owned(),
            paths[1].to_string_lossy().into_owned(),
            "--execution".to_owned(),
            paths[2].to_string_lossy().into_owned(),
        ],
    ]
    .concat();
    let report = assert_success(&run_json(binary, project, &arguments));
    assert_eq!(report["applied"], false);
    assert_eq!(report["sources"].as_array().map(Vec::len), Some(3));
    assert!(
        report["mappings"]
            .as_array()
            .is_some_and(|mappings| !mappings.is_empty())
    );
    assert_eq!(report["deleted_sources"], Value::Array(Vec::new()));
    for (path, expected) in paths.iter().zip(before) {
        assert_eq!(
            fs::read(path).expect("legacy source should remain"),
            expected
        );
    }
}

fn verify_projection_drift(binary: &Path, project: &Path, plan_id: &str) {
    let path = projection_path(project, plan_id);
    let original = fs::read(&path).expect("projection should be readable");
    fs::write(&path, "manual drift\n").expect("projection drift should be injected");
    assert_failure(
        &run_json(binary, project, &read_arguments("show", plan_id)),
        8,
        "drift_detected",
    );
    fs::write(path, original).expect("projection should be restored exactly");
}

fn verify_protocol_migrations(binary: &Path, project: &Path, plan_id: &str, revision: u64) {
    let before = plan_bytes(project, plan_id);
    let current = protocol_migration_arguments(plan_id, revision, 4, CURRENT_PROTOCOL_VERSION);
    let report = assert_success(&run_json(binary, project, &current));
    assert_eq!(report["disposition"], "already_current");
    assert_eq!(report["revision"], revision);
    assert_eq!(plan_bytes(project, plan_id), before);
    let unsupported = protocol_migration_arguments(plan_id, revision, 5, "2099-01-01");
    assert_failure(
        &run_json(binary, project, &unsupported),
        5,
        "policy_violation",
    );
    assert_eq!(plan_bytes(project, plan_id), before);
}

fn inject_checkpoint_transaction(
    project: &Path,
    plan_id: &str,
    arguments: &[String],
    expected_revision: u64,
    request_number: u64,
) {
    let typed_plan = PlanId::parse(plan_id).expect("plan ID should parse");
    let typed_task = TaskId::parse("T1").expect("task ID should parse");
    let command = [vec!["mino".to_owned()], arguments.to_vec()].concat();
    let request = MutationRequest::new(
        expected_revision,
        RequestId::parse(request_id(request_number)).expect("request ID should parse"),
        "codex",
        command,
        vec!["extensions.execution".to_owned()],
    )
    .expect("transaction request should be valid");
    let store = PlanStore::new(project);
    let error = store
        .commit_with_options(
            &typed_plan,
            request,
            CommitOptions::fail_at(FailurePoint::AfterJournal),
            |plan| {
                plan.record_checkpoint(
                    &typed_task,
                    CheckpointKind::Inspection,
                    "Inspected the disposable execution boundary",
                    "codex",
                    Timestamp::now_utc(),
                )
            },
        )
        .expect_err("transaction should stop after its journal");
    assert_eq!(error.kind(), StoreErrorKind::InjectedFailure);
    assert!(
        store
            .paths()
            .plan_directory(&typed_plan)
            .join("transaction")
            .is_dir()
    );
}

fn prepare_passing_child(binary: &Path, project: &Path) {
    let child = project.join("runner-ready");
    fs::create_dir(&child).expect("child project should be created");
    let initialized = assert_success(&run_json(
        binary,
        &child,
        &strings(&[
            "project",
            "init",
            "--apply-agents-block",
            "--apply-gitignore-block",
        ]),
    ));
    assert_eq!(initialized["complete"], true);
}

fn verify_and_add_evidence(binary: &Path, project: &Path, plan_id: &str) {
    let command_records = assert_success(&run_json(
        binary,
        project,
        &[
            strings(&["evidence", "list", "--plan", plan_id]),
            strings(&["--task", "T1", "--type", "command"]),
        ]
        .concat(),
    ));
    assert_eq!(
        command_records["evidence"].as_array().map(Vec::len),
        Some(2)
    );
    let failed = assert_success(&run_json(
        binary,
        project,
        &strings(&["evidence", "show", "--plan", plan_id, "--evidence", "E0001"]),
    ));
    assert_eq!(failed["exit_code"], 7);

    let supplemental = [
        strings(&[
            "evidence",
            "add",
            "--plan",
            plan_id,
            "--task",
            "T1",
            "--type",
            "manual-observation",
            "--description",
            "The disposable feature path was inspected",
        ]),
        strings(&[
            "--expect-revision",
            "12",
            "--request-id",
            &request_id(15),
            "--actor",
            "codex",
        ]),
    ]
    .concat();
    let added = assert_success(&run_json(binary, project, &supplemental));
    assert_eq!(added["evidence"]["id"], "E0003");
    assert_eq!(added["replayed"], false);
    let shown = assert_success(&run_json(
        binary,
        project,
        &strings(&["evidence", "show", "--plan", plan_id, "--evidence", "E0003"]),
    ));
    assert_eq!(shown["type"], "manual-observation");
}

fn establish_git_baseline(project: &Path) {
    assert_successful_process(
        &Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(project)
            .output()
            .expect("git init should run"),
        "git init",
    );
    assert_successful_process(
        &Command::new("git")
            .args([
                "add",
                "--",
                ".gitignore",
                ".agents",
                "AGENTS.md",
                "fixtures",
                "请求.md",
            ])
            .current_dir(project)
            .output()
            .expect("git add should run"),
        "git add",
    );
    assert_successful_process(
        &Command::new("git")
            .args([
                "-c",
                "user.name=Mino E2E",
                "-c",
                "user.email=mino-e2e@example.invalid",
                "commit",
                "--quiet",
                "-m",
                "chore: establish e2e baseline",
            ])
            .current_dir(project)
            .output()
            .expect("git commit should run"),
        "git commit",
    );
    assert!(git_status(project).is_empty());
}

fn synchronize_local_catalog(project: &Path) -> TestServer {
    let manifest = fs::read_to_string(fixture_path("e2e/catalog/common/manifest.toml"))
        .expect("catalog manifest should be readable");
    let rules = fs::read_to_string(fixture_path("e2e/catalog/common/rules.toml"))
        .expect("catalog rules should be readable");
    let checks = fs::read_to_string(fixture_path("e2e/catalog/common/checks.toml"))
        .expect("catalog checks should be readable");
    let digest = package_digest(&manifest, &rules, &checks);
    let server = TestServer::new(|base_url| {
        let mut catalog = "catalog_version = 1\n".to_owned();
        write!(
            catalog,
            "\n[[packages]]\npackage_id = \"common\"\nversion = \"9.9.9\"\ndigest = \"{digest}\"\nmanifest_url = \"{base_url}/common/manifest.toml\"\nrules_url = \"{base_url}/common/rules.toml\"\nchecks_url = \"{base_url}/common/checks.toml\"\n"
        )
        .expect("catalog should render");
        BTreeMap::from([
            ("/catalog.toml".to_owned(), catalog.into_bytes()),
            ("/common/manifest.toml".to_owned(), manifest.into_bytes()),
            ("/common/rules.toml".to_owned(), rules.into_bytes()),
            ("/common/checks.toml".to_owned(), checks.into_bytes()),
        ])
    });
    configure_catalog(project, &server.url("/catalog.toml"));
    let report = synchronize_all_with_options(
        project,
        SyncOptions::new(SyncLimits::default(), SourcePolicy::HttpsOrLoopbackHttp),
    )
    .expect("loopback catalog should synchronize");
    assert_eq!(report.packages.len(), 1);
    assert_eq!(report.packages[0].package_id, "common");
    assert!(!report.reused_generation);
    server
}

fn configure_catalog(project: &Path, url: &str) {
    let layout = ProjectLayout::new(project);
    let mut config: ProjectConfig = toml::from_str(
        &fs::read_to_string(layout.config()).expect("project config should be readable"),
    )
    .expect("project config should parse");
    config.catalog.url = Some(url.to_owned());
    let mut rendered = toml::to_string_pretty(&config).expect("project config should serialize");
    if !rendered.ends_with('\n') {
        rendered.push('\n');
    }
    fs::write(layout.config(), rendered).expect("catalog URL should be configured");
}

fn package_digest(manifest: &str, rules: &str, checks: &str) -> String {
    let mut input = Vec::new();
    for (name, source) in [
        ("manifest.toml", manifest),
        ("rules.toml", rules),
        ("checks.toml", checks),
    ] {
        let normalized_source = source.replace("\r\n", "\n").replace('\r', "\n");
        input.extend_from_slice(name.as_bytes());
        input.push(0);
        input.extend_from_slice(normalized_source.as_bytes());
        input.push(0);
    }
    sha256_digest(&input)
}

fn handle_connection(
    mut stream: TcpStream,
    routes: &BTreeMap<String, Vec<u8>>,
    requests: &Mutex<Vec<String>>,
) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
    let mut buffer = [0_u8; 1024];
    loop {
        let mut bytes = Vec::new();
        while bytes.len() < 16 * 1024 {
            match stream.read(&mut buffer) {
                Ok(0) | Err(_) => return,
                Ok(count) => {
                    bytes.extend_from_slice(&buffer[..count]);
                    if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
            }
        }
        let request = String::from_utf8_lossy(&bytes);
        let path = request
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .unwrap_or("/")
            .to_owned();
        requests
            .lock()
            .expect("request log should be available")
            .push(path.clone());
        let (status, reason, body) = routes
            .get(&path)
            .map_or((404, "Not Found", b"not found".as_slice()), |body| {
                (200, "OK", body.as_slice())
            });
        let header = format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n",
            body.len()
        );
        if stream.write_all(header.as_bytes()).is_err()
            || stream.write_all(body).is_err()
            || stream.flush().is_err()
        {
            return;
        }
    }
}

fn run_json(binary: &Path, root: &Path, command: &[String]) -> Output {
    run_with_global_flags(binary, root, &["--format", "json", "--no-input"], command)
}

fn run_with_global_flags(binary: &Path, root: &Path, flags: &[&str], command: &[String]) -> Output {
    let mut arguments = vec!["--root".to_owned(), root.to_string_lossy().into_owned()];
    arguments.extend(flags.iter().map(|value| (*value).to_owned()));
    arguments.extend(command.iter().cloned());
    run_binary(binary, &arguments)
}

fn run_binary(binary: &Path, arguments: &[String]) -> Output {
    Command::new(binary)
        .args(arguments)
        .stdin(Stdio::null())
        .output()
        .expect("Mino binary should run")
}

fn assert_success(output: &Output) -> Value {
    assert_successful_process(output, "Mino JSON command");
    assert!(output.stderr.is_empty());
    let value: Value = serde_json::from_slice(&output.stdout).expect("stdout should be JSON");
    assert_eq!(value["kind"], "mino.result/v1");
    assert_eq!(value["ok"], true);
    value
}

fn assert_agent_success(output: &Output) -> Value {
    assert_successful_process(output, "Mino Agent command");
    assert!(output.stderr.is_empty());
    serde_json::from_slice(&output.stdout).expect("Agent stdout should be JSON")
}

fn assert_failure(output: &Output, exit_code: i32, code: &str) -> Value {
    assert_eq!(
        output.status.code(),
        Some(exit_code),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let value: Value = serde_json::from_slice(&output.stdout).expect("failure should be JSON");
    assert_eq!(value["kind"], "mino.result/v1");
    assert_eq!(value["ok"], false);
    assert_eq!(value["error"]["code"], code);
    assert_eq!(value["error"]["exit_code"], exit_code);
    value
}

fn assert_successful_process(output: &Output, label: &str) {
    assert!(
        output.status.success(),
        "{label} failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn mutation_arguments(
    command: &[&str],
    plan_id: &str,
    revision: u64,
    request_number: u64,
) -> Vec<String> {
    let mut arguments = command
        .iter()
        .map(|value| (*value).to_owned())
        .collect::<Vec<_>>();
    arguments.extend([
        "--plan".to_owned(),
        plan_id.to_owned(),
        "--expect-revision".to_owned(),
        revision.to_string(),
        "--request-id".to_owned(),
        request_id(request_number),
        "--actor".to_owned(),
        "codex".to_owned(),
    ]);
    arguments
}

fn exec_arguments(
    command: &[&str],
    plan_id: &str,
    revision: u64,
    request_number: u64,
    extra: &[&str],
) -> Vec<String> {
    let mut path = vec!["exec"];
    path.extend(command.iter().copied());
    let mut arguments = mutation_arguments(&path, plan_id, revision, request_number);
    arguments.extend(extra.iter().map(|value| (*value).to_owned()));
    arguments
}

fn protocol_migration_arguments(
    plan_id: &str,
    revision: u64,
    request_number: u64,
    target: &str,
) -> Vec<String> {
    vec![
        "protocol".to_owned(),
        "migrate".to_owned(),
        "--plan".to_owned(),
        plan_id.to_owned(),
        "--expect-revision".to_owned(),
        revision.to_string(),
        "--request-id".to_owned(),
        request_id(request_number),
        "--to".to_owned(),
        target.to_owned(),
    ]
}

fn read_arguments(command: &str, plan_id: &str) -> Vec<String> {
    strings(&["plan", command, "--plan", plan_id])
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

fn request_id(number: u64) -> String {
    format!("90000000-0000-0000-0000-{number:012}")
}

fn fixture_path(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(relative)
}

fn projection_path(project: &Path, plan_id: &str) -> PathBuf {
    project
        .join("docs")
        .join("plan")
        .join(format!("{plan_id}.md"))
}

fn plan_bytes(project: &Path, plan_id: &str) -> PlanBytes {
    let typed_plan = PlanId::parse(plan_id).expect("plan ID should parse");
    let store = PlanStore::new(project);
    PlanBytes {
        state: fs::read(store.paths().current_plan(&typed_plan))
            .expect("plan state should be readable"),
        events: fs::read(store.paths().event_log(&typed_plan))
            .expect("plan events should be readable"),
        projection: fs::read(projection_path(project, plan_id))
            .expect("plan projection should be readable"),
    }
}

fn git_status(root: &Path) -> Vec<u8> {
    let output = Command::new("git")
        .args([
            "-C",
            &root.to_string_lossy(),
            "status",
            "--short",
            "--untracked-files=all",
        ])
        .output()
        .expect("git status should run");
    assert_successful_process(&output, "git status");
    output.stdout
}
