//! Contract tests for bundled protocol compatibility and legacy workflow analysis.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use mino::ErrorCategory;
use mino::domain::{CURRENT_PROTOCOL_REVISION, CURRENT_PROTOCOL_VERSION, Plan, PlanId};
use mino::project::{LegacyDocumentKind, LegacyInput, analyze_legacy, initialize};
use mino::protocol::ProtocolRegistry;
use mino::render::render_plan;
use mino::store::{PlanStore, sha256_digest};
use serde_json::Value;

static NEXT_TEMP_DIRECTORY: AtomicU64 = AtomicU64::new(1);

struct TestProject {
    path: PathBuf,
}

impl TestProject {
    fn new(label: &str) -> Self {
        let sequence = NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "mino-protocol-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("temporary project should be created");
        fs::write(
            path.join("Cargo.toml"),
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .expect("fixture manifest should be written");
        fs::create_dir(path.join("src")).expect("fixture source directory should be created");
        fs::write(path.join("src/lib.rs"), "pub fn fixture() -> u8 { 1 }\n")
            .expect("fixture source should be written");
        initialize(&path).expect("temporary project should initialize");
        Self {
            path: path.canonicalize().expect("project root should resolve"),
        }
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
                .is_some_and(|name| name.to_string_lossy().starts_with("mino-protocol-"))
        {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

fn fixture_path(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(relative)
}

fn protocol_asset(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("assets/protocol/2026-05-11")
        .join(name)
}

fn run_mino(arguments: &[String]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mino"))
        .args(arguments)
        .stdin(Stdio::null())
        .output()
        .expect("Mino binary should run")
}

fn base_arguments(project: &TestProject) -> Vec<String> {
    vec![
        "--root".to_owned(),
        project.path().to_string_lossy().into_owned(),
        "--format".to_owned(),
        "json".to_owned(),
        "--no-input".to_owned(),
    ]
}

fn request_id(number: u64) -> String {
    format!("50000000-0000-0000-0000-{number:012}")
}

fn parse_json(output: &Output) -> Value {
    assert!(
        output.stderr.is_empty(),
        "stderr must be empty in JSON mode"
    );
    serde_json::from_slice(&output.stdout).expect("stdout should be JSON")
}

fn parse_success(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    parse_json(output)
}

fn create_plan(project: &TestProject, request_number: u64) -> String {
    let request_file = project.path().join("request.md");
    fs::write(&request_file, "Verify protocol upgrade compatibility.\n")
        .expect("request fixture should be written");
    let mut arguments = base_arguments(project);
    arguments.extend([
        "plan".to_owned(),
        "create".to_owned(),
        "--name".to_owned(),
        "Protocol compatibility".to_owned(),
        "--trigger".to_owned(),
        "durable".to_owned(),
        "--request-file".to_owned(),
        request_file.to_string_lossy().into_owned(),
        "--request-id".to_owned(),
        request_id(request_number),
        "--actor".to_owned(),
        "codex".to_owned(),
    ]);
    let value = parse_success(&run_mino(&arguments));
    value["plan_id"]
        .as_str()
        .expect("create should return a plan ID")
        .to_owned()
}

fn migrate_arguments(
    project: &TestProject,
    plan_id: &str,
    expected_revision: u64,
    request_number: u64,
    target_version: &str,
) -> Vec<String> {
    let mut arguments = base_arguments(project);
    arguments.extend([
        "protocol".to_owned(),
        "migrate".to_owned(),
        "--plan".to_owned(),
        plan_id.to_owned(),
        "--expect-revision".to_owned(),
        expected_revision.to_string(),
        "--request-id".to_owned(),
        request_id(request_number),
        "--to".to_owned(),
        target_version.to_owned(),
    ]);
    arguments
}

fn plan_files(project: &TestProject, plan_id: &str) -> BTreeMap<PathBuf, Vec<u8>> {
    let typed_id = PlanId::parse(plan_id).expect("plan ID should parse");
    let store = PlanStore::new(project.path());
    let mut files = snapshot_files(&store.paths().plan_directory(&typed_id));
    let projection = project
        .path()
        .join("docs")
        .join("plan")
        .join(format!("{plan_id}.md"));
    files.insert(
        PathBuf::from("projection.md"),
        fs::read(projection).expect("projection should be readable"),
    );
    files
}

fn snapshot_files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    collect_files(root, root, &mut files);
    files
}

fn collect_files(root: &Path, current: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
    let mut entries = fs::read_dir(current)
        .expect("snapshot directory should be readable")
        .collect::<Result<Vec<_>, _>>()
        .expect("snapshot entries should be readable");
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            collect_files(root, &path, files);
        } else {
            files.insert(
                path.strip_prefix(root)
                    .expect("snapshot path should be under root")
                    .to_path_buf(),
                fs::read(path).expect("snapshot file should be readable"),
            );
        }
    }
}

#[test]
fn bundle_matches_pinned_bytes_and_preserves_the_existing_render_contract() {
    let bundle = ProtocolRegistry::current().expect("embedded bundle should verify");
    let manifest = bundle.manifest();
    assert_eq!(manifest.bundle_version(), 1);
    assert_eq!(manifest.protocol_version(), CURRENT_PROTOCOL_VERSION);
    assert_eq!(manifest.protocol_revision(), CURRENT_PROTOCOL_REVISION);
    assert_eq!(manifest.schema_version(), 1);
    assert_eq!(manifest.renderer_version(), 2);
    assert_eq!(
        manifest
            .resources()
            .iter()
            .map(mino::protocol::ProtocolResource::name)
            .collect::<Vec<_>>(),
        ["PLAN_EXECUTION.md", "PLAN_TEMPLATE.md"]
    );
    for resource in manifest.resources() {
        let asset =
            fs::read(protocol_asset(resource.name())).expect("protocol asset should be readable");
        assert_eq!(sha256_digest(&asset), resource.sha256());
        assert_eq!(
            bundle
                .resource(resource.name())
                .expect("manifest resource should be embedded")
                .as_bytes(),
            asset
        );
    }

    let plan: Plan = serde_json::from_slice(include_bytes!("fixtures/render/full_plan.json"))
        .expect("pre-bundle plan fixture should deserialize");
    let rendered = render_plan(&plan).expect("pre-bundle plan should still render");
    assert_eq!(
        rendered.as_bytes(),
        include_bytes!("fixtures/render/full_plan.md")
    );
}

#[test]
fn current_protocol_status_plan_validation_and_noop_migration_are_stable() {
    let project = TestProject::new("current");
    let mut status_arguments = base_arguments(&project);
    status_arguments.extend(["protocol".to_owned(), "status".to_owned()]);
    let status = parse_success(&run_mino(&status_arguments));
    assert_eq!(status["compatible"], true);
    assert_eq!(status["complete"], true);
    assert_eq!(
        status["manifest"]["protocol_version"],
        CURRENT_PROTOCOL_VERSION
    );

    let plan_id = create_plan(&project, 1);
    let mut apply_arguments = base_arguments(&project);
    apply_arguments.extend([
        "plan".to_owned(),
        "apply".to_owned(),
        "--plan".to_owned(),
        plan_id.clone(),
        "--file".to_owned(),
        fixture_path("drafts/complete.yaml")
            .to_string_lossy()
            .into_owned(),
        "--expect-revision".to_owned(),
        "1".to_owned(),
        "--request-id".to_owned(),
        request_id(2),
        "--actor".to_owned(),
        "codex".to_owned(),
    ]);
    parse_success(&run_mino(&apply_arguments));
    let mut validate_arguments = base_arguments(&project);
    validate_arguments.extend([
        "plan".to_owned(),
        "validate".to_owned(),
        "--plan".to_owned(),
        plan_id.clone(),
    ]);
    assert_eq!(parse_success(&run_mino(&validate_arguments))["valid"], true);

    let before = plan_files(&project, &plan_id);
    let migrate = migrate_arguments(&project, &plan_id, 2, 3, CURRENT_PROTOCOL_VERSION);
    let first = parse_success(&run_mino(&migrate));
    let second = parse_success(&run_mino(&migrate));
    assert_eq!(first, second);
    assert_eq!(first["disposition"], "already_current");
    assert_eq!(first["revision"], 2);
    assert_eq!(plan_files(&project, &plan_id), before);
}

#[test]
fn incompatible_status_and_failed_migrations_preserve_all_plan_bytes() {
    let project = TestProject::new("failure");
    let plan_id = create_plan(&project, 10);
    let before = plan_files(&project, &plan_id);
    let unsupported = run_mino(&migrate_arguments(&project, &plan_id, 1, 11, "2099-01-01"));
    assert_eq!(unsupported.status.code(), Some(5));
    assert_eq!(
        parse_json(&unsupported)["error"]["code"],
        "policy_violation"
    );
    assert_eq!(plan_files(&project, &plan_id), before);

    let stale = run_mino(&migrate_arguments(
        &project,
        &plan_id,
        99,
        12,
        CURRENT_PROTOCOL_VERSION,
    ));
    assert_eq!(stale.status.code(), Some(3));
    assert_eq!(parse_json(&stale)["error"]["code"], "revision_conflict");
    assert_eq!(plan_files(&project, &plan_id), before);

    let lock_path = project.path().join(".mino/protocol.lock");
    let mut lock = fs::read_to_string(&lock_path).expect("protocol lock should be readable");
    lock = lock.replace(CURRENT_PROTOCOL_VERSION, "2025-01-01");
    fs::write(&lock_path, lock).expect("protocol lock fixture should be changed");
    let changed_lock = fs::read(&lock_path).expect("changed lock should be readable");
    let mut status_arguments = base_arguments(&project);
    status_arguments.extend(["protocol".to_owned(), "status".to_owned()]);
    let status = parse_success(&run_mino(&status_arguments));
    assert_eq!(status["compatible"], false);
    assert_eq!(status["complete"], false);
    assert_eq!(
        status["missing"],
        serde_json::json!(["protocol_version_mismatch"])
    );
    assert_eq!(
        fs::read(lock_path).expect("protocol lock should remain readable"),
        changed_lock
    );
}

#[test]
fn legacy_analysis_maps_every_heading_and_preserves_exact_sources() {
    let inputs = [
        LegacyInput {
            kind: LegacyDocumentKind::Agents,
            path: fixture_path("legacy/AGENTS.md"),
        },
        LegacyInput {
            kind: LegacyDocumentKind::PlanTemplate,
            path: fixture_path("legacy/PLAN_TEMPLATE.md"),
        },
        LegacyInput {
            kind: LegacyDocumentKind::PlanExecution,
            path: fixture_path("legacy/PLAN_EXECUTION.md"),
        },
    ];
    let source_bytes = inputs
        .iter()
        .map(|input| fs::read(&input.path).expect("legacy fixture should be readable"))
        .collect::<Vec<_>>();
    let first = analyze_legacy(&inputs).expect("legacy documents should be analyzed");
    let second = analyze_legacy(&inputs).expect("repeated analysis should succeed");
    assert_eq!(first, second);
    assert_eq!(first.sources.len(), 3);
    assert_eq!(first.mappings.len(), 15);
    assert!(!first.applied);
    assert!(first.deleted_sources.is_empty());
    assert_eq!(first.proposed_changes.len(), 3);
    assert!(
        first.proposed_changes[0]
            .proposal
            .contains("<!-- BEGIN MINO MANAGED -->")
    );
    let codes = first
        .findings
        .iter()
        .map(|finding| finding.code.as_str())
        .collect::<Vec<_>>();
    assert!(codes.contains(&"legacy_ambiguous_section"));
    assert!(codes.contains(&"legacy_duplicate_heading"));
    assert!(codes.contains(&"legacy_unsupported_section"));
    for (input, before) in inputs.iter().zip(source_bytes) {
        assert_eq!(
            fs::read(&input.path).expect("legacy fixture should remain readable"),
            before
        );
    }
}

#[test]
fn legacy_cli_and_invalid_inputs_never_modify_or_delete_sources() {
    let project = TestProject::new("legacy-cli");
    let agents = project.path().join("legacy AGENTS.md");
    let partial = project.path().join("partial plan.txt");
    fs::copy(fixture_path("legacy/AGENTS.md"), &agents)
        .expect("legacy AGENTS fixture should be copied");
    fs::copy(fixture_path("legacy/partial.txt"), &partial)
        .expect("partial fixture should be copied");
    let before = snapshot_files(project.path());
    let mut arguments = base_arguments(&project);
    arguments.extend([
        "project".to_owned(),
        "migrate".to_owned(),
        "legacy".to_owned(),
        "--agents".to_owned(),
        agents.to_string_lossy().into_owned(),
        "--template".to_owned(),
        partial.to_string_lossy().into_owned(),
    ]);
    let value = parse_success(&run_mino(&arguments));
    assert_eq!(value["applied"], false);
    assert_eq!(value["deleted_sources"], Value::Array(Vec::new()));
    assert!(
        value["missing"]
            .as_array()
            .expect("missing should be an array")
            .contains(&Value::from("legacy_no_headings"))
    );
    assert_eq!(snapshot_files(project.path()), before);

    let oversized = project.path().join("oversized.md");
    fs::write(&oversized, vec![b'x'; 1024 * 1024 + 1])
        .expect("oversized fixture should be written");
    let oversized_before = fs::read(&oversized).expect("oversized fixture should be readable");
    let error = analyze_legacy(&[LegacyInput {
        kind: LegacyDocumentKind::PlanTemplate,
        path: oversized.clone(),
    }])
    .expect_err("oversized input should be rejected");
    assert_eq!(error.category(), ErrorCategory::IncompleteOrValidation);
    assert_eq!(
        fs::read(&oversized).expect("oversized fixture should remain"),
        oversized_before
    );

    let invalid_utf8 = project.path().join("invalid.md");
    fs::write(&invalid_utf8, [0xff, 0xfe]).expect("invalid UTF-8 fixture should be written");
    let invalid_before = fs::read(&invalid_utf8).expect("invalid fixture should be readable");
    let error = analyze_legacy(&[LegacyInput {
        kind: LegacyDocumentKind::PlanExecution,
        path: invalid_utf8.clone(),
    }])
    .expect_err("non-UTF-8 input should be rejected");
    assert_eq!(error.category(), ErrorCategory::IncompleteOrValidation);
    assert_eq!(
        fs::read(invalid_utf8).expect("invalid fixture should remain"),
        invalid_before
    );
}
