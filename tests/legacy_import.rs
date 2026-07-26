//! Contract tests for conservative legacy Markdown plan import.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use mino::domain::{CheckStatus, CriterionStatus, PlanId, PlanStatus, TaskStatus};
use mino::project::{ProjectLayout, initialize, parse_legacy_plan};
use mino::store::{PlanStore, sha256_digest};
use mino::{ErrorCategory, MinoError};
use serde_json::Value;

static NEXT_TEMP_DIRECTORY: AtomicU64 = AtomicU64::new(1);

struct TestProject {
    path: PathBuf,
}

impl TestProject {
    fn new(label: &str) -> Self {
        let sequence = NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "mino-legacy-import-{label}-{}-{sequence}",
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

    fn copy_fixture(&self, name: &str) -> PathBuf {
        let destination = self.path.join(name);
        fs::copy(fixture_path(name), &destination).expect("legacy fixture should be copied");
        destination
    }
}

impl Drop for TestProject {
    fn drop(&mut self) {
        let temporary_root = std::env::temp_dir();
        if self.path.starts_with(&temporary_root)
            && self
                .path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("mino-legacy-import-"))
        {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/legacy_plans")
        .join(name)
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

fn import_arguments(
    project: &TestProject,
    source: &Path,
    name: &str,
    request_number: u64,
) -> Vec<String> {
    let mut arguments = base_arguments(project);
    arguments.extend([
        "project".to_owned(),
        "import".to_owned(),
        "legacy".to_owned(),
        "--source".to_owned(),
        source.to_string_lossy().into_owned(),
        "--name".to_owned(),
        name.to_owned(),
        "--request-id".to_owned(),
        request_id(request_number),
        "--actor".to_owned(),
        "codex".to_owned(),
    ]);
    arguments
}

fn request_id(number: u64) -> String {
    format!("70000000-0000-0000-0000-{number:012}")
}

fn run_mino(arguments: &[String]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mino"))
        .args(arguments)
        .stdin(Stdio::null())
        .output()
        .expect("Mino binary should run")
}

fn parse_json(output: &Output) -> Value {
    assert!(
        output.stderr.is_empty(),
        "stderr should be empty: {}",
        String::from_utf8_lossy(&output.stderr)
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

fn load_plan(project: &TestProject, plan_id: &str) -> mino::domain::Plan {
    PlanStore::new(project.path())
        .load_plan(&PlanId::parse(plan_id).expect("plan ID should parse"))
        .expect("imported plan should load")
}

fn warning_codes(value: &Value) -> Vec<&str> {
    value["warnings"]
        .as_array()
        .expect("warnings should be an array")
        .iter()
        .map(|warning| {
            warning["code"]
                .as_str()
                .expect("warning code should be text")
        })
        .collect()
}

fn assert_input_error(error: &MinoError) {
    assert_eq!(error.category(), ErrorCategory::IncompleteOrValidation);
}

#[test]
fn complete_supported_plan_maps_authored_fields_and_resets_history() {
    let source = fixture_path("complete.md");
    let before = fs::read(&source).expect("fixture should be readable");
    let parsed = parse_legacy_plan(&source).expect("complete legacy plan should parse");
    assert_eq!(
        fs::read(&source).expect("fixture should remain readable"),
        before
    );
    assert_eq!(parsed.source.bytes, before.len());
    assert_eq!(parsed.source.digest, sha256_digest(&before));
    assert_eq!(
        parsed.suggested_name.as_deref(),
        Some("Legacy Import Contract")
    );
    assert!(!parsed.historical_execution_trusted);
    assert_eq!(parsed.draft.tasks.len(), 1);
    let task = &parsed.draft.tasks[0];
    assert_eq!(
        task.id.as_ref().map(ToString::to_string).as_deref(),
        Some("T1")
    );
    assert_eq!(task.files.len(), 1);
    assert_eq!(task.acceptance_criteria.len(), 2);
    assert_eq!(task.verification.len(), 1);
    assert_eq!(task.verification[0].command, ["cargo", "test", "--lib"]);
    assert!(task.commit_gate.as_ref().is_some_and(|gate| gate.required));
    assert_eq!(parsed.draft.verification_plan.len(), 1);
    assert_eq!(
        parsed.draft.verification_plan[0].command,
        ["cargo", "fmt", "--all", "--", "--check"]
    );
    let codes = parsed
        .warnings
        .iter()
        .map(|warning| warning.code.as_str())
        .collect::<Vec<_>>();
    assert!(codes.contains(&"legacy_plan_historical_state_unverified"));
    assert!(codes.contains(&"legacy_plan_historical_assertion_unverified"));
    assert!(parsed.mappings.iter().any(|mapping| {
        mapping.target == "tasks.T1.commit_gate" && mapping.source_fragment.contains("feat(import)")
    }));
}

#[test]
fn current_managed_projection_shape_remains_parseable_as_unverified_authored_input() {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/render/full_plan.md");
    let parsed = parse_legacy_plan(&source).expect("current managed projection should parse");
    assert_eq!(parsed.draft.tasks.len(), 1);
    assert_eq!(
        parsed.draft.tasks[0]
            .id
            .as_ref()
            .map(ToString::to_string)
            .as_deref(),
        Some("T1")
    );
    assert!(
        parsed.draft.tasks[0]
            .files
            .iter()
            .any(|file| file.path == "src/render/**")
    );
    assert_eq!(
        parsed.draft.tasks[0].verification[0].command,
        ["cargo", "fmt", "--all", "--", "--check"]
    );
    assert!(
        parsed
            .warnings
            .iter()
            .any(|warning| warning.code == "legacy_plan_historical_state_unverified")
    );
}

#[test]
fn cli_import_is_source_preserving_valid_draft_and_retry_safe() {
    let project = TestProject::new("complete");
    let source = project.copy_fixture("complete.md");
    let before = fs::read(&source).expect("copied source should be readable");
    let arguments = import_arguments(&project, &source, "Imported legacy contract", 1);
    let first = parse_success(&run_mino(&arguments));
    assert_eq!(first["complete"], false);
    assert_eq!(first["status"], "Draft");
    assert_eq!(first["revision"], 2);
    assert_eq!(first["source_preserved"], true);
    assert_eq!(first["historical_execution_trusted"], false);
    assert_eq!(first["draft_review_required"], true);
    let plan_id = first["plan_id"]
        .as_str()
        .expect("import should return a plan ID");
    let plan = load_plan(&project, plan_id);
    assert_eq!(plan.status(), PlanStatus::Draft);
    assert!(plan.approvals().is_empty());
    assert_eq!(plan.tasks().len(), 1);
    assert_eq!(plan.tasks()[0].status(), TaskStatus::Draft);
    assert!(
        plan.tasks()[0]
            .acceptance_criteria()
            .iter()
            .all(|criterion| criterion.status() == CriterionStatus::Pending)
    );
    assert!(
        plan.tasks()[0]
            .verification_checks()
            .iter()
            .all(|check| check.status() == CheckStatus::Pending)
    );
    assert_eq!(
        fs::read(&source).expect("source should remain readable"),
        before
    );

    let second = parse_success(&run_mino(&arguments));
    assert_eq!(second["plan_id"], plan_id);
    assert_eq!(second["revision"], 2);
    assert_eq!(second["replayed"], true);
    assert_eq!(
        fs::read(&source).expect("source should remain readable"),
        before
    );

    let mut validate = base_arguments(&project);
    validate.extend([
        "plan".to_owned(),
        "validate".to_owned(),
        "--plan".to_owned(),
        plan_id.to_owned(),
    ]);
    assert_eq!(parse_success(&run_mino(&validate))["valid"], true);
}

#[test]
fn partial_and_malicious_sources_remain_incomplete_inert_drafts() {
    let partial = parse_legacy_plan(&fixture_path("partial.md"))
        .expect("partial legacy plan should produce a reviewable report");
    assert!(partial.draft.tasks.is_empty());
    assert!(
        partial
            .warnings
            .iter()
            .any(|warning| warning.code == "legacy_plan_missing_original_request")
    );
    assert!(
        partial
            .warnings
            .iter()
            .any(|warning| warning.code == "legacy_plan_unsupported_section")
    );

    let project = TestProject::new("malicious");
    let source = project.copy_fixture("malicious.md");
    let before = fs::read(&source).expect("malicious source should be readable");
    let value = parse_success(&run_mino(&import_arguments(
        &project,
        &source,
        "Adversarial import",
        2,
    )));
    assert_eq!(value["status"], "Draft");
    assert_eq!(value["complete"], false);
    let codes = warning_codes(&value);
    for expected in [
        "legacy_plan_unsafe_path",
        "legacy_plan_unsafe_command",
        "legacy_plan_duplicate_task_id",
        "legacy_plan_noncontiguous_task_id",
        "legacy_plan_invalid_dependency",
        "legacy_plan_unsupported_section",
    ] {
        assert!(
            codes.contains(&expected),
            "missing warning {expected}: {codes:?}"
        );
    }
    let plan_id = value["plan_id"].as_str().expect("plan ID should be text");
    let plan = load_plan(&project, plan_id);
    assert_eq!(plan.status(), PlanStatus::Draft);
    assert!(plan.approvals().is_empty());
    assert_eq!(plan.tasks().len(), 1);
    assert!(plan.tasks()[0].file_map().is_empty());
    assert!(plan.tasks()[0].verification_checks().is_empty());
    assert_eq!(
        fs::read(source).expect("source should remain readable"),
        before
    );
}

#[test]
fn invalid_bytes_and_digest_fail_before_any_plan_write() {
    for (label, bytes) in [
        ("empty", Vec::new()),
        ("non-utf8", vec![0xff, 0xfe]),
        ("nul", b"# Plan\n\0payload\n".to_vec()),
        ("oversized", vec![b'x'; 1024 * 1024 + 1]),
    ] {
        let project = TestProject::new(label);
        let source = project.path().join(format!("{label}.md"));
        fs::write(&source, &bytes).expect("invalid source should be written");
        let error = parse_legacy_plan(&source).expect_err("invalid source should fail parsing");
        assert_input_error(&error);
        let output = run_mino(&import_arguments(&project, &source, label, 10));
        assert_eq!(output.status.code(), Some(2));
        assert_eq!(
            parse_json(&output)["error"]["code"],
            "incomplete_or_validation"
        );
        assert!(
            fs::read_dir(ProjectLayout::new(project.path()).plans_directory())
                .expect("plans directory should be readable")
                .next()
                .is_none()
        );
        assert_eq!(
            fs::read(source).expect("invalid source should remain"),
            bytes
        );
    }

    let project = TestProject::new("digest");
    let source = project.copy_fixture("partial.md");
    let mut arguments = import_arguments(&project, &source, "Digest mismatch", 11);
    arguments.extend([
        "--source-digest".to_owned(),
        "sha256:0000000000000000000000000000000000000000000000000000000000000000".to_owned(),
    ]);
    let output = run_mino(&arguments);
    assert_eq!(output.status.code(), Some(2));
    assert!(
        fs::read_dir(ProjectLayout::new(project.path()).plans_directory())
            .expect("plans directory should be readable")
            .next()
            .is_none()
    );
}

#[test]
fn active_plan_and_name_collision_cannot_create_an_import_side_plan() {
    let project = TestProject::new("active");
    let source = project.copy_fixture("partial.md");
    let first = parse_success(&run_mino(&import_arguments(
        &project,
        &source,
        "First legacy draft",
        20,
    )));
    let first_plan_id = first["plan_id"]
        .as_str()
        .expect("first plan ID should be text");
    let second = run_mino(&import_arguments(
        &project,
        &source,
        "Second legacy draft",
        21,
    ));
    assert_eq!(second.status.code(), Some(5));
    assert_eq!(parse_json(&second)["error"]["code"], "policy_violation");
    let layout = ProjectLayout::new(project.path());
    let plan_directories = fs::read_dir(layout.plans_directory())
        .expect("plans directory should be readable")
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .count();
    assert_eq!(plan_directories, 1);
    assert_eq!(load_plan(&project, first_plan_id).revision(), 2);
}
