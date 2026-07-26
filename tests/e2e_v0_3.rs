//! Packaged-entry proof for the complete local Mino v0.3 design milestone.

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use mino::distribution::{
    PluginPackageRequest, host_target, package_plugin, validate_mino_plugin_source,
    validate_plugin_artifact_directory,
};
use mino::domain::PlanId;
use mino::evidence::EvidenceStore;
use mino::project::{ProjectConfig, ProjectLayout};
use mino::standards::{
    SourcePolicy, SyncLimits, SyncOptions, build_team_catalog_with_policy,
    synchronize_all_with_options,
};
use mino::store::PlanStore;
use serde_json::Value;
use zip::ZipArchive;

static NEXT_TEMP_DIRECTORY: AtomicU64 = AtomicU64::new(1);

struct TestWorkspace {
    root: PathBuf,
    project: PathBuf,
}

impl TestWorkspace {
    fn new() -> Self {
        let sequence = NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("mino-e2e-v0-3-{}-{sequence}", std::process::id()));
        let project = root.join("packaged project");
        fs::create_dir_all(project.join("src")).expect("E2E source directory should be created");
        fs::write(
            project.join("Cargo.toml"),
            "[package]\nname = \"mino-v0-3-proof\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
        )
        .expect("E2E Cargo manifest should be written");
        fs::write(
            project.join("src/lib.rs"),
            "//! Mino v0.3 E2E fixture.\n\n/// Returns the fixture identity.\npub const fn identity() -> u8 { 3 }\n",
        )
        .expect("E2E Rust source should be written");
        fs::write(project.join(".gitignore"), "/target/\n")
            .expect("E2E ignore rules should be written");
        let root = root.canonicalize().expect("E2E workspace should resolve");
        Self {
            project: root.join("packaged project"),
            root,
        }
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }
}

impl Drop for TestWorkspace {
    fn drop(&mut self) {
        let temporary_root = std::env::temp_dir();
        if self.root.starts_with(&temporary_root)
            && self
                .root
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("mino-e2e-v0-3-"))
        {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

struct StaticServer {
    address: SocketAddr,
    stop: Arc<AtomicBool>,
    requests: Arc<AtomicU64>,
    thread: Option<JoinHandle<()>>,
}

impl StaticServer {
    fn new(root: PathBuf) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("test server should bind");
        listener
            .set_nonblocking(true)
            .expect("test listener should be nonblocking");
        let address = listener
            .local_addr()
            .expect("test server address should resolve");
        let stop = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(AtomicU64::new(0));
        let server_stop = Arc::clone(&stop);
        let server_requests = Arc::clone(&requests);
        let server_root = Arc::new(root);
        let thread = thread::spawn(move || {
            while !server_stop.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let root = Arc::clone(&server_root);
                        let requests = Arc::clone(&server_requests);
                        thread::spawn(move || serve_files(stream, &root, &requests));
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

    fn base_url(&self) -> String {
        format!("http://{}", self.address)
    }

    fn request_count(&self) -> u64 {
        self.requests.load(Ordering::Relaxed)
    }
}

impl Drop for StaticServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = TcpStream::connect(self.address);
        if let Some(thread) = self.thread.take() {
            thread.join().expect("test server should stop");
        }
    }
}

#[test]
#[allow(clippy::too_many_lines)]
fn packaged_entry_completes_catalog_observation_plan_and_variant_flow() {
    let workspace = TestWorkspace::new();
    let binary = package_and_extract(&workspace);
    assert_packaged_identity(&binary);
    initialize_project(&binary, &workspace.project);
    prove_catalog_build_sync_and_separate_apply(&workspace, &binary);

    let mut request_number = 1;
    let plan_id = create_and_approve_plan(&workspace, &binary, &mut request_number);
    let mut revision = 4;
    let started = parse_success(&run_mino(
        &binary,
        &workspace.project,
        &mutation_arguments(
            &["exec", "start"],
            &plan_id,
            revision,
            next_request(&mut request_number),
            arguments(&["--task", "T1"]),
        ),
    ));
    revision = result_revision(&started);
    assert_eq!(started["status"], "In Progress");

    let schedule = parse_success(&run_mino(
        &binary,
        &workspace.project,
        &schedule_arguments(&plan_id, revision),
    ));
    assert_eq!(schedule["spec_kind"], "mino.scheduled-task-spec/v1");
    assert_eq!(schedule["project"]["plan_revision"], revision);
    assert_eq!(
        schedule["authorization"]["external_creation_required"],
        true
    );
    assert_eq!(schedule["authorization"]["authorization_granted"], false);
    assert_eq!(
        schedule["emission_side_effects"]["scheduler_mutated"],
        false
    );
    assert_eq!(schedule["emission_side_effects"]["network_accessed"], false);
    assert_eq!(
        schedule["emission_side_effects"]["mino_state_mutated"],
        false
    );
    assert!(
        !workspace
            .project
            .join("scheduled-results/result.json")
            .exists()
    );
    assert_eq!(
        parse_success(&run_mino(
            &binary,
            &workspace.project,
            &arguments(&["plan", "show", "--plan", &plan_id]),
        ))["revision"],
        revision
    );

    let monitor_request = next_request(&mut request_number);
    let monitored = parse_success(&run_mino(
        &binary,
        &workspace.project,
        &mutation_arguments(
            &["exec", "check", "monitor"],
            &plan_id,
            revision,
            monitor_request,
            arguments(&[
                "--check",
                "V3-TASK",
                "--max-attempts",
                "1",
                "--interval-milliseconds",
                "1",
                "--deadline-milliseconds",
                "10000",
            ]),
        ),
    ));
    assert_eq!(monitored["monitor_kind"], "mino.monitor/v1");
    assert_eq!(monitored["terminal_reason"], "passed");
    assert_eq!(monitored["attempts"].as_array().map(Vec::len), Some(1));
    let evidence_id = monitored["attempts"][0]["evidence_id"]
        .as_str()
        .expect("monitor evidence ID should be text")
        .to_owned();
    revision = result_revision(&monitored);
    let summary = workspace
        .project
        .join(".mino/plans")
        .join(&plan_id)
        .join("monitors")
        .join(request_id(monitor_request))
        .join("summary.json");
    assert!(summary.is_file());

    let criterion = parse_success(&run_mino(
        &binary,
        &workspace.project,
        &mutation_arguments(
            &["exec", "criterion", "pass"],
            &plan_id,
            revision,
            next_request(&mut request_number),
            vec![
                "--criterion".to_owned(),
                "T1-A1".to_owned(),
                "--evidence".to_owned(),
                evidence_id,
            ],
        ),
    ));
    revision = result_revision(&criterion);
    let completed = parse_success(&run_mino(
        &binary,
        &workspace.project,
        &mutation_arguments(
            &["exec", "complete"],
            &plan_id,
            revision,
            next_request(&mut request_number),
            arguments(&["--task", "T1"]),
        ),
    ));
    revision = result_revision(&completed);
    let completed_plan = parse_success(&run_mino(
        &binary,
        &workspace.project,
        &arguments(&["plan", "show", "--plan", &plan_id]),
    ));
    assert_eq!(completed_plan["tasks"][0]["status"], "Done");

    let global = parse_success(&run_mino(
        &binary,
        &workspace.project,
        &mutation_arguments(
            &["exec", "check", "run"],
            &plan_id,
            revision,
            next_request(&mut request_number),
            arguments(&["--check", "V3-GLOBAL"]),
        ),
    ));
    assert_eq!(global["run"]["outcome"], "passed");
    revision = result_revision(&global);
    let review = parse_success(&run_mino(
        &binary,
        &workspace.project,
        &mutation_arguments(
            &["exec", "finish"],
            &plan_id,
            revision,
            next_request(&mut request_number),
            Vec::new(),
        ),
    ));
    revision = result_revision(&review);
    assert_eq!(review["status"], "Review");
    let accepted = parse_success(&run_mino(
        &binary,
        &workspace.project,
        &mutation_arguments(
            &["review", "accept"],
            &plan_id,
            revision,
            next_request(&mut request_number),
            arguments(&["--approval-ref", "chat:v0-3-final-acceptance"]),
        ),
    ));
    revision = result_revision(&accepted);
    assert_eq!(accepted["status"], "Done");
    verify_audited_done_state(&workspace.project, &plan_id, revision);

    let forked = parse_success(&run_mino(
        &binary,
        &workspace.project,
        &[
            arguments(&[
                "plan",
                "fork",
                "--plan",
                &plan_id,
                "--from-revision",
                &revision.to_string(),
                "--name",
                "Alternative v0.3 proof",
                "--reason",
                "Compare retained complete history",
            ]),
            vec![
                "--request-id".to_owned(),
                request_id(next_request(&mut request_number)),
                "--actor".to_owned(),
                "codex".to_owned(),
            ],
        ]
        .concat(),
    ));
    assert_eq!(forked["status"], "Draft");
    assert_eq!(forked["revision"], 1);
    assert_eq!(forked["lineage"]["parent_plan_id"], plan_id);
    let fork_id = forked["plan_id"]
        .as_str()
        .expect("fork plan ID should be text");
    let fork_plan = parse_success(&run_mino(
        &binary,
        &workspace.project,
        &arguments(&["plan", "show", "--plan", fork_id]),
    ));
    assert_eq!(fork_plan["approvals"], Value::Array(Vec::new()));
    let diff = parse_success(&run_mino(
        &binary,
        &workspace.project,
        &arguments(&["plan", "diff", "--left", &plan_id, "--right", fork_id]),
    ));
    assert_eq!(diff["diff_kind"], "mino.plan-diff/v1");
    assert!(diff["changes"].as_array().is_some_and(|changes| {
        changes
            .iter()
            .any(|change| change["category"] == "changed" && change["path"] == "metadata.name")
    }));
    let source = parse_success(&run_mino(
        &binary,
        &workspace.project,
        &arguments(&["plan", "show", "--plan", &plan_id]),
    ));
    assert_eq!(source["status"], "Done");
    assert_eq!(source["revision"], revision);
    assert!(!workspace.project.join(".git").exists());
}

#[test]
#[allow(clippy::too_many_lines)]
fn release_surface_excludes_unsupported_runtime_and_git_operations() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"));
    let binary = Path::new(env!("CARGO_BIN_EXE_mino"));
    let capabilities = parse_direct_json(&run_direct(
        binary,
        &["agent", "capabilities", "--format", "json", "--no-input"],
    ));
    let actions = capabilities["actions"]
        .as_array()
        .expect("capability actions should be an array")
        .iter()
        .map(|action| {
            action["id"]
                .as_str()
                .expect("action ID should be text")
                .to_owned()
        })
        .collect::<BTreeSet<_>>();
    for forbidden in [
        "cloud.sync",
        "daemon.start",
        "git.branch.delete",
        "git.force-push",
        "git.merge",
        "git.push",
        "git.rebase",
        "git.reset",
        "git.tag",
        "git.worktree.create",
        "git.worktree.delete",
        "llm.run",
        "plan.merge",
        "plugin.execute",
        "plugin.update",
        "scheduler.create",
        "web.serve",
    ] {
        assert!(
            !actions.contains(forbidden),
            "unsupported action {forbidden} exists"
        );
    }

    let top_level = run_direct(binary, &["--help"]);
    assert_successful(&top_level, "top-level help");
    let top_level = String::from_utf8(top_level.stdout).expect("help should be UTF-8");
    for forbidden in [
        "llm",
        "daemon",
        "cloud",
        "web",
        "scheduler",
        "update",
        "plugin",
    ] {
        assert!(
            !top_level.lines().any(|line| {
                line.strip_prefix("  ")
                    .and_then(|value| value.split_whitespace().next())
                    == Some(forbidden)
            }),
            "unsupported top-level command {forbidden} exists"
        );
    }
    let git_help = run_direct(binary, &["git", "--help"]);
    assert_successful(&git_help, "Git help");
    let git_help = String::from_utf8(git_help.stdout).expect("Git help should be UTF-8");
    for forbidden in [
        "push",
        "merge",
        "rebase",
        "reset",
        "amend",
        "force-push",
        "tag",
        "delete",
        "worktree",
    ] {
        assert!(
            !git_help.lines().any(|line| {
                line.strip_prefix("  ")
                    .and_then(|value| value.split_whitespace().next())
                    == Some(forbidden)
            }),
            "unsupported Git command {forbidden} exists"
        );
    }

    let plugin = validate_mino_plugin_source(repository)
        .expect("canonical binary-free plugin source should validate");
    assert_eq!(plugin.name, "mino");
    assert!(!plugin.plugin_root.join("bin").exists());
    assert!(!plugin.plugin_root.join(".mcp.json").exists());
    assert!(!plugin.plugin_root.join(".app.json").exists());

    let security = fs::read_to_string(repository.join("docs/security.md"))
        .expect("security guide should be readable");
    for boundary in [
        "doc-contract: no-llm-execution",
        "doc-contract: no-daemon",
        "doc-contract: no-cloud-control-plane",
        "doc-contract: no-built-in-scheduler",
        "doc-contract: no-auto-update",
        "doc-contract: no-arbitrary-plugin-runtime",
        "doc-contract: no-git-remote-or-destructive",
    ] {
        assert!(
            security.contains(boundary),
            "security boundary is missing {boundary}"
        );
    }
    let workflow = fs::read_to_string(repository.join(".github/workflows/release-artifacts.yml"))
        .expect("artifact workflow should be readable");
    for forbidden in [
        "actions/upload-artifact",
        "cargo publish",
        "gh release",
        "secrets.",
    ] {
        assert!(!workflow.contains(forbidden));
    }
}

fn package_and_extract(workspace: &TestWorkspace) -> PathBuf {
    let target = host_target().expect("E2E host should be a declared plugin target");
    let report = package_plugin(&PluginPackageRequest::new(
        env!("CARGO_MANIFEST_DIR"),
        env!("CARGO_BIN_EXE_mino"),
        target,
        workspace.path("artifacts"),
    ))
    .expect("native plugin artifact should package and smoke");
    let manifest = validate_plugin_artifact_directory(&report.output_directory)
        .expect("native plugin artifact should validate");
    assert_eq!(manifest.target, target);
    assert_eq!(manifest.files.len(), 10);
    let install = workspace.path("plugin install");
    let archive_file = File::open(&report.archive_path).expect("plugin ZIP should open");
    let mut archive = ZipArchive::new(archive_file).expect("plugin ZIP should parse");
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .expect("plugin ZIP entry should read");
        let relative = entry
            .enclosed_name()
            .expect("plugin ZIP path should be enclosed");
        let destination = install.join(relative);
        let parent = destination
            .parent()
            .expect("plugin ZIP destination should have a parent");
        fs::create_dir_all(parent).expect("plugin ZIP parent should be created");
        let mut output = File::create(&destination).expect("plugin ZIP file should be created");
        std::io::copy(&mut entry, &mut output).expect("plugin ZIP file should extract");
        output
            .sync_all()
            .expect("plugin ZIP file should synchronize");
        #[cfg(unix)]
        {
            let mode = if destination.ends_with("mino/bin/mino") {
                0o755
            } else {
                0o644
            };
            fs::set_permissions(&destination, fs::Permissions::from_mode(mode))
                .expect("plugin ZIP permissions should apply");
        }
    }
    install
        .join("mino/bin")
        .join(if cfg!(windows) { "mino.exe" } else { "mino" })
}

fn assert_packaged_identity(binary: &Path) {
    let version = run_direct(binary, &["--version"]);
    assert_successful(&version, "packaged Mino version");
    assert_eq!(
        version.stdout,
        format!("mino {}\n", env!("CARGO_PKG_VERSION")).as_bytes()
    );
    assert!(version.stderr.is_empty());
    let capabilities = parse_direct_json(&run_direct(
        binary,
        &["agent", "capabilities", "--format", "json", "--no-input"],
    ));
    assert_eq!(capabilities["kind"], "mino.agent-capabilities/v1");
}

fn initialize_project(binary: &Path, project: &Path) {
    let initialized = parse_success(&run_mino(
        binary,
        project,
        &arguments(&[
            "project",
            "init",
            "--apply-agents-block",
            "--apply-gitignore-block",
        ]),
    ));
    assert_eq!(initialized["complete"], true);
    assert!(project.join(".agents/skills/mino/SKILL.md").is_file());
    assert!(project.join(".mino/config.toml").is_file());
}

fn prove_catalog_build_sync_and_separate_apply(workspace: &TestWorkspace, binary: &Path) {
    let cli_source = workspace.path("catalog cli source");
    copy_tree(&catalog_fixture(), &cli_source);
    let validated = parse_success(&run_mino(
        binary,
        &workspace.project,
        &path_arguments(
            &["standards", "catalog", "validate"],
            "--source",
            &cli_source,
        ),
    ));
    assert_eq!(validated["kind"], "mino.team-catalog-validation/v1");
    let cli_output = workspace.path("catalog cli output");
    let built = parse_success(&run_mino(
        binary,
        &workspace.project,
        &two_path_arguments(
            &["standards", "catalog", "build"],
            ("--source", &cli_source),
            ("--output", &cli_output),
        ),
    ));
    assert_eq!(built["kind"], "mino.team-catalog-build/v1");
    assert!(cli_output.join("catalog-manifest.json").is_file());

    let sync_source = workspace.path("catalog sync source");
    let sync_output = workspace.path("catalog sync output");
    copy_tree(&catalog_fixture(), &sync_source);
    let server = StaticServer::new(sync_output.clone());
    fs::write(
        sync_source.join("catalog-source.toml"),
        format!(
            "source_version = 1\nnamespace = \"example.com\"\nbase_url = \"{}\"\n",
            server.base_url()
        ),
    )
    .expect("loopback catalog URL should be configured");
    let built = build_team_catalog_with_policy(
        &sync_source,
        &sync_output,
        SourcePolicy::HttpsOrLoopbackHttp,
    )
    .expect("loopback catalog should build");
    configure_project_catalog(
        &workspace.project,
        &format!("{}/catalog.toml", server.base_url()),
    );
    let synchronized = synchronize_all_with_options(
        &workspace.project,
        SyncOptions::new(SyncLimits::default(), SourcePolicy::HttpsOrLoopbackHttp),
    )
    .expect("generated catalog should synchronize");
    assert_eq!(synchronized.catalog_digest, built.catalog_digest);
    assert_eq!(
        synchronized
            .packages
            .iter()
            .map(|package| package.package_id.as_str())
            .collect::<Vec<_>>(),
        ["example.com.common", "example.com.rust"]
    );
    assert!(server.request_count() >= 7);
    let shown = parse_success(&run_mino(
        binary,
        &workspace.project,
        &arguments(&["project", "show"]),
    ));
    assert_eq!(
        shown["standards_lock"]["catalog_digest"],
        built.catalog_digest
    );
    assert_eq!(
        shown["standards_lock"]["packages"][0]["package_id"],
        "example.com.common"
    );
    let applied = parse_success(&run_mino(
        binary,
        &workspace.project,
        &arguments(&["standards", "apply", "--recommended", "--seed-verification"]),
    ));
    let applied_ids = applied["standards"]
        .as_array()
        .expect("applied standards should be an array")
        .iter()
        .map(|standard| {
            standard["package_id"]
                .as_str()
                .expect("applied package ID should be text")
        })
        .collect::<Vec<_>>();
    assert_eq!(applied_ids, ["common", "rust"]);
    assert!(
        !applied_ids
            .iter()
            .any(|package| package.starts_with("example.com."))
    );
}

fn create_and_approve_plan(
    workspace: &TestWorkspace,
    binary: &Path,
    request_number: &mut u64,
) -> String {
    fs::remove_file(workspace.project.join("Cargo.toml"))
        .expect("language probe manifest should be removed");
    fs::remove_dir_all(workspace.project.join("src"))
        .expect("language probe source should be removed");
    fs::create_dir(workspace.project.join("scheduled-results"))
        .expect("scheduled result parent should be created");
    let request_path = workspace.project.join("request.md");
    let plan_path = workspace.project.join("plan.yaml");
    fs::write(
        &request_path,
        "Prove the complete packaged Mino v0.3 lifecycle.\n",
    )
    .expect("plan request should be written");
    fs::write(&plan_path, plan_source(binary)).expect("plan source should be written");
    let created = parse_success(&run_mino(
        binary,
        &workspace.project,
        &[
            arguments(&[
                "plan",
                "create",
                "--name",
                "Complete packaged v0.3 lifecycle",
                "--trigger",
                "durable",
            ]),
            vec![
                "--request-file".to_owned(),
                request_path.to_string_lossy().into_owned(),
                "--request-id".to_owned(),
                request_id(next_request(request_number)),
                "--actor".to_owned(),
                "codex".to_owned(),
            ],
        ]
        .concat(),
    ));
    assert_eq!(created["revision"], 1);
    let plan_id = created["plan_id"]
        .as_str()
        .expect("created plan ID should be text")
        .to_owned();
    let applied = parse_success(&run_mino(
        binary,
        &workspace.project,
        &mutation_arguments(
            &["plan", "apply"],
            &plan_id,
            1,
            next_request(request_number),
            vec![
                "--file".to_owned(),
                plan_path.to_string_lossy().into_owned(),
            ],
        ),
    ));
    assert_eq!(applied["revision"], 2);
    let validation = parse_success(&run_mino(
        binary,
        &workspace.project,
        &arguments(&["plan", "validate", "--plan", &plan_id]),
    ));
    assert_eq!(validation["valid"], true);
    let finalized = parse_success(&run_mino(
        binary,
        &workspace.project,
        &mutation_arguments(
            &["plan", "finalize"],
            &plan_id,
            2,
            next_request(request_number),
            Vec::new(),
        ),
    ));
    assert_eq!(finalized["status"], "Ready");
    let approved = parse_success(&run_mino(
        binary,
        &workspace.project,
        &mutation_arguments(
            &["plan", "approve"],
            &plan_id,
            3,
            next_request(request_number),
            arguments(&[
                "--approval-ref",
                "chat:v0-3-plan-approval",
                "--git-flow-consent",
                "disabled",
            ]),
        ),
    ));
    assert_eq!(approved["revision"], 4);
    plan_id
}

fn verify_audited_done_state(project: &Path, plan_id: &str, revision: u64) {
    let typed_plan = PlanId::parse(plan_id).expect("plan ID should parse");
    let audit = PlanStore::new(project)
        .audit(&typed_plan)
        .expect("plan store should audit");
    assert_eq!(audit.revision(), revision);
    assert_eq!(
        audit.event_count(),
        usize::try_from(revision).expect("revision should fit usize")
    );
    assert_eq!(audit.event_count(), audit.snapshot_count());
    let evidence = EvidenceStore::new(project);
    assert!(
        evidence
            .audit(&typed_plan)
            .expect("evidence should audit")
            .is_healthy()
    );
    assert!(
        evidence
            .list(&typed_plan)
            .expect("evidence should list")
            .len()
            >= 2
    );
}

fn plan_source(binary: &Path) -> String {
    let binary = serde_json::to_string(&binary.to_string_lossy())
        .expect("packaged binary path should encode as YAML");
    format!(
        "metadata:\n  priority: P1\n  plan_type: integration\n  area: release\n  owner: codex\nsummary: Prove the packaged Mino v0.3 entry through bounded, audited execution.\ncontext:\n  - reference: Native plugin artifact\n    fact: The exact archived binary passed its compatibility probes.\n    implication: All lifecycle commands must use that same binary.\nscope:\n  goal: Complete one evidence-backed packaged-entry lifecycle.\n  deliverables:\n    - An audited Done plan with monitor and schedule-spec evidence\n  in_scope:\n    - Local plan state and inert observation contracts\n  out_of_scope:\n    - Git mutation, external scheduling, and publication\ndecisions:\n  - item: Observation\n    type: Decision\n    decision: Use one bounded foreground monitor and one inert schedule handoff\n    reason: Prove both observation paths without a daemon or scheduler\n    status: Accepted\napproach: Execute the exact packaged binary against a disposable initialized project.\ninterfaces: JSON results carry every revision, evidence identity, and next boundary.\nedge_cases:\n  - case: Scheduled work is requested without external authorization\n    expected_behavior: Emit inert data with authorization_granted false\n    covered_by:\n      - T1-A1\ntasks:\n  - id: T1\n    title: Verify the packaged entry\n    depends_on: []\n    steps:\n      - Emit a scheduler-neutral bounded handoff\n      - Monitor the planned packaged-binary check\n      - Complete the task from immutable evidence\n    files:\n      - path: .gitignore\n        change: N/A\n        reason: The runtime proof intentionally makes no source change\n    acceptance_criteria:\n      - id: T1-A1\n        description: The packaged binary passes a bounded planned check\n    verification:\n      - id: V3-TASK\n        command:\n          - {binary}\n          - --version\n        cwd: .\n        expected_exit_code: 0\n        required: true\nverification_plan:\n  - id: V3-GLOBAL\n    command:\n      - {binary}\n      - agent\n      - capabilities\n      - --format\n      - json\n      - --no-input\n    cwd: .\n    expected_exit_code: 0\n    required: true\n"
    )
}

fn schedule_arguments(plan_id: &str, revision: u64) -> Vec<String> {
    vec![
        "exec".to_owned(),
        "schedule".to_owned(),
        "spec".to_owned(),
        "--plan".to_owned(),
        plan_id.to_owned(),
        "--expect-revision".to_owned(),
        revision.to_string(),
        "--check".to_owned(),
        "V3-TASK".to_owned(),
        "--execution-request-id".to_owned(),
        "d0000000-0000-0000-0000-000000000001".to_owned(),
        "--actor".to_owned(),
        "codex".to_owned(),
        "--execution-environment".to_owned(),
        "packaged-e2e".to_owned(),
        "--max-attempts".to_owned(),
        "1".to_owned(),
        "--interval-milliseconds".to_owned(),
        "1".to_owned(),
        "--deadline-milliseconds".to_owned(),
        "10000".to_owned(),
        "--trigger-at".to_owned(),
        "2099-01-01T00:00:00Z".to_owned(),
        "--expires-at".to_owned(),
        "2099-01-01T00:01:00Z".to_owned(),
        "--max-dispatch-attempts".to_owned(),
        "1".to_owned(),
        "--dispatch-retry-milliseconds".to_owned(),
        "1".to_owned(),
        "--success-condition".to_owned(),
        "The packaged planned check reports passed".to_owned(),
        "--stop-condition".to_owned(),
        "Stop after the bounded terminal monitor report".to_owned(),
        "--failure-handling".to_owned(),
        "Preserve the report and notify the plan owner".to_owned(),
        "--result-destination".to_owned(),
        "scheduled-results/result.json".to_owned(),
    ]
}

fn mutation_arguments(
    action: &[&str],
    plan_id: &str,
    revision: u64,
    request_number: u64,
    extra: Vec<String>,
) -> Vec<String> {
    let mut command = action.iter().map(ToString::to_string).collect::<Vec<_>>();
    command.extend([
        "--plan".to_owned(),
        plan_id.to_owned(),
        "--expect-revision".to_owned(),
        revision.to_string(),
        "--request-id".to_owned(),
        request_id(request_number),
        "--actor".to_owned(),
        "codex".to_owned(),
    ]);
    command.extend(extra);
    command
}

fn result_revision(value: &Value) -> u64 {
    value["revision"]
        .as_u64()
        .or_else(|| value["plan"]["revision"].as_u64())
        .or_else(|| value["final_revision"].as_u64())
        .expect("result should expose a revision")
}

fn request_id(number: u64) -> String {
    format!("c0000000-0000-0000-0000-{number:012}")
}

fn next_request(number: &mut u64) -> u64 {
    let current = *number;
    *number += 1;
    current
}

fn arguments(values: &[&str]) -> Vec<String> {
    values.iter().map(ToString::to_string).collect()
}

fn path_arguments(action: &[&str], name: &str, path: &Path) -> Vec<String> {
    let mut arguments = action.iter().map(ToString::to_string).collect::<Vec<_>>();
    arguments.extend([name.to_owned(), path.to_string_lossy().into_owned()]);
    arguments
}

fn two_path_arguments(action: &[&str], first: (&str, &Path), second: (&str, &Path)) -> Vec<String> {
    let mut arguments = action.iter().map(ToString::to_string).collect::<Vec<_>>();
    arguments.extend([
        first.0.to_owned(),
        first.1.to_string_lossy().into_owned(),
        second.0.to_owned(),
        second.1.to_string_lossy().into_owned(),
    ]);
    arguments
}

fn run_mino(binary: &Path, root: &Path, command: &[String]) -> Output {
    Command::new(binary)
        .args([
            "--root",
            root.to_string_lossy().as_ref(),
            "--format",
            "json",
            "--no-input",
        ])
        .args(command)
        .stdin(Stdio::null())
        .output()
        .expect("packaged Mino binary should run")
}

fn run_direct(binary: &Path, arguments: &[&str]) -> Output {
    Command::new(binary)
        .args(arguments)
        .stdin(Stdio::null())
        .output()
        .expect("Mino binary should run directly")
}

fn parse_success(output: &Output) -> Value {
    assert_successful(output, "Mino JSON command");
    assert!(output.stderr.is_empty());
    serde_json::from_slice(&output.stdout).expect("successful Mino output should be JSON")
}

fn parse_direct_json(output: &Output) -> Value {
    assert_successful(output, "direct Mino JSON command");
    assert!(output.stderr.is_empty());
    serde_json::from_slice(&output.stdout).expect("direct Mino output should be JSON")
}

fn assert_successful(output: &Output, label: &str) {
    assert!(
        output.status.success(),
        "{label} failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn catalog_fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/catalog/valid")
}

fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir(destination).expect("copy destination should be created");
    for entry in fs::read_dir(source).expect("copy source should be readable") {
        let entry = entry.expect("copy entry should be readable");
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        let file_type = entry.file_type().expect("copy entry should inspect");
        if file_type.is_dir() {
            copy_tree(&source_path, &destination_path);
        } else {
            fs::copy(&source_path, &destination_path).expect("fixture file should copy");
        }
    }
}

fn configure_project_catalog(project: &Path, catalog_url: &str) {
    let layout = ProjectLayout::new(project);
    let mut config: ProjectConfig = toml::from_str(
        &fs::read_to_string(layout.config()).expect("project config should be readable"),
    )
    .expect("project config should parse");
    config.catalog.url = Some(catalog_url.to_owned());
    let mut rendered = toml::to_string_pretty(&config).expect("project config should serialize");
    if !rendered.ends_with('\n') {
        rendered.push('\n');
    }
    fs::write(layout.config(), rendered).expect("catalog URL should be configured");
}

fn serve_files(mut stream: TcpStream, root: &Path, requests: &AtomicU64) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
    let mut buffer = [0_u8; 1024];
    loop {
        let mut request = Vec::new();
        while request.len() < 16 * 1024 {
            match stream.read(&mut buffer) {
                Ok(0) | Err(_) => return,
                Ok(count) => {
                    request.extend_from_slice(&buffer[..count]);
                    if request.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
            }
        }
        let request_text = String::from_utf8_lossy(&request);
        let path = request_text
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .unwrap_or("/");
        requests.fetch_add(1, Ordering::Relaxed);
        let relative = path.strip_prefix('/').unwrap_or(path);
        let is_safe = !relative.is_empty()
            && !relative.contains(['?', '#', '\\'])
            && relative
                .split('/')
                .all(|segment| !segment.is_empty() && segment != "." && segment != "..");
        let body = is_safe
            .then(|| fs::read(root.join(relative)).ok())
            .flatten();
        let (status, reason, body) = body
            .map_or((404, "Not Found", b"not found".to_vec()), |body| {
                (200, "OK", body)
            });
        let header = format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n",
            body.len()
        );
        if stream.write_all(header.as_bytes()).is_err()
            || stream.write_all(&body).is_err()
            || stream.flush().is_err()
        {
            return;
        }
    }
}
