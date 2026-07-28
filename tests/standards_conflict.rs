//! Contract tests for explicit source-bound standards conflict decisions.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use mino::domain::{PlanId, PlanStatus, StandardConflictCandidate};
use mino::project::initialize;
use mino::render::render_plan;
use mino::standards::detect_standard_conflicts;
use mino::store::PlanStore;
use serde_json::Value;

static NEXT_TEMP_DIRECTORY: AtomicU64 = AtomicU64::new(1);

struct TestProject {
    path: PathBuf,
}

impl TestProject {
    fn new(label: &str) -> Self {
        let sequence = NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "mino-standards-conflict-{label}-{}-{sequence}",
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
        fs::copy(fixture("AGENTS.md"), path.join("AGENTS.md"))
            .expect("repository policy fixture should be copied");
        fs::copy(fixture("rustdoc.toml"), path.join("rustdoc.toml"))
            .expect("project configuration fixture should be copied");
        initialize(&path).expect("temporary project should initialize");
        fs::copy(
            fixture("conflict.toml"),
            path.join(".mino/standards.local.toml"),
        )
        .expect("local standards fixture should be copied");
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
            && self.path.file_name().is_some_and(|name| {
                name.to_string_lossy()
                    .starts_with("mino-standards-conflict-")
            })
        {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/standards")
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

fn run_mino(arguments: &[String]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mino"))
        .args(arguments)
        .stdin(Stdio::null())
        .output()
        .expect("Mino binary should run")
}

fn initialize_git_baseline(root: &Path) {
    let initialized = Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(root)
        .output()
        .expect("Git should initialize");
    assert!(initialized.status.success());
    fs::write(
        root.join(".git/info/exclude"),
        ".agents/\n.mino/\ndocs/plan/\n",
    )
    .expect("managed paths should be excluded");
    for arguments in [
        vec!["config", "user.name", "Mino Standards Test"],
        vec!["config", "user.email", "mino-standards@example.invalid"],
        vec!["add", "--all"],
        vec!["commit", "--quiet", "-m", "test: create standards fixture"],
    ] {
        let output = Command::new("git")
            .args(arguments)
            .current_dir(root)
            .output()
            .expect("Git should run");
        assert!(
            output.status.success(),
            "git stdout: {}\ngit stderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
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

fn request_id(number: u64) -> String {
    format!("80000000-0000-0000-0000-{number:012}")
}

fn create_complete_plan(project: &TestProject) -> String {
    let request = project.path().join("request.md");
    fs::write(
        &request,
        "The current task explicitly requires documentation on every function.\n",
    )
    .expect("request should be written");
    initialize_git_baseline(project.path());
    let mut create = base_arguments(project);
    create.extend([
        "plan".to_owned(),
        "create".to_owned(),
        "--name".to_owned(),
        "Standards conflict fixture".to_owned(),
        "--trigger".to_owned(),
        "durable".to_owned(),
        "--request-file".to_owned(),
        request.to_string_lossy().into_owned(),
        "--request-id".to_owned(),
        request_id(1),
        "--actor".to_owned(),
        "codex".to_owned(),
    ]);
    let created = parse_success(&run_mino(&create));
    let plan_id = created["plan_id"]
        .as_str()
        .expect("create should return a plan ID")
        .to_owned();
    let mut apply = base_arguments(project);
    apply.extend([
        "plan".to_owned(),
        "apply".to_owned(),
        "--plan".to_owned(),
        plan_id.clone(),
        "--file".to_owned(),
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/drafts/complete.yaml")
            .to_string_lossy()
            .into_owned(),
        "--expect-revision".to_owned(),
        "1".to_owned(),
        "--request-id".to_owned(),
        request_id(2),
        "--actor".to_owned(),
        "codex".to_owned(),
    ]);
    let applied = parse_success(&run_mino(&apply));
    assert_eq!(applied["revision"], 2);
    plan_id
}

fn conflict_arguments(project: &TestProject, action: &str, plan_id: &str) -> Vec<String> {
    let mut arguments = base_arguments(project);
    arguments.extend([
        "standards".to_owned(),
        "conflict".to_owned(),
        action.to_owned(),
        "--plan".to_owned(),
        plan_id.to_owned(),
    ]);
    arguments
}

fn mutate_conflict_arguments(
    project: &TestProject,
    action: &str,
    plan_id: &str,
    revision: u64,
    request_number: u64,
) -> Vec<String> {
    mutation_arguments(
        project,
        &["standards", "conflict", action],
        plan_id,
        revision,
        request_number,
    )
}

fn mutation_arguments(
    project: &TestProject,
    command: &[&str],
    plan_id: &str,
    revision: u64,
    request_number: u64,
) -> Vec<String> {
    let mut arguments = base_arguments(project);
    arguments.extend(command.iter().map(|part| (*part).to_owned()));
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

fn validate(project: &TestProject, plan_id: &str) -> Output {
    let mut arguments = base_arguments(project);
    arguments.extend([
        "plan".to_owned(),
        "validate".to_owned(),
        "--plan".to_owned(),
        plan_id.to_owned(),
    ]);
    run_mino(&arguments)
}

fn load_plan(project: &TestProject, plan_id: &str) -> mino::domain::Plan {
    PlanStore::new(project.path())
        .load_plan(&PlanId::parse(plan_id).expect("plan ID should parse"))
        .expect("plan should load")
}

#[test]
fn detection_displays_every_source_in_strict_precedence_order_without_merging() {
    let project = TestProject::new("detect");
    let plan_id = create_complete_plan(&project);
    let plan = load_plan(&project, &plan_id);
    let first = detect_standard_conflicts(project.path(), &plan)
        .expect("standards conflicts should be detected");
    let second = detect_standard_conflicts(project.path(), &plan)
        .expect("repeated detection should be deterministic");
    assert_eq!(first, second);
    assert_eq!(first.conflicts.len(), 1);
    let conflict = &first.conflicts[0];
    assert_eq!(conflict.rule_id(), "rust.docs");
    assert_eq!(
        conflict
            .candidates()
            .iter()
            .map(StandardConflictCandidate::precedence)
            .collect::<Vec<_>>(),
        [5, 4, 3, 2]
    );
    assert_eq!(
        conflict.default_candidate_id(),
        Some(conflict.candidates()[0].id())
    );
    assert_eq!(
        conflict
            .candidates()
            .iter()
            .map(StandardConflictCandidate::value)
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        4
    );
}

#[test]
fn cli_requires_refresh_then_explicit_decision_and_replays_exactly() {
    let project = TestProject::new("resolve");
    let plan_id = create_complete_plan(&project);
    let initial = validate(&project, &plan_id);
    assert_eq!(initial.status.code(), Some(2));
    let initial = parse_json(&initial);
    assert_eq!(
        initial["findings"][0]["id"],
        "POLICY-STANDARD-CONFLICT-UNTRACKED"
    );
    assert_eq!(
        initial["next_actions"][0]["id"],
        "standards.conflict.refresh"
    );

    let listed = parse_success(&run_mino(&conflict_arguments(&project, "list", &plan_id)));
    assert_eq!(listed["conflicts"][0]["status"], "untracked");
    assert_eq!(
        listed["conflicts"][0]["conflict"]["candidates"][0]["precedence"],
        5
    );

    let refresh = mutate_conflict_arguments(&project, "refresh", &plan_id, 2, 3);
    let refreshed = parse_success(&run_mino(&refresh));
    assert_eq!(refreshed["revision"], 3);
    assert_eq!(
        refreshed["standards_conflicts"]["conflicts"][0]["status"],
        "unresolved"
    );
    let mut forged =
        serde_json::to_value(load_plan(&project, &plan_id)).expect("tracked plan should serialize");
    let lower_candidate = forged["extensions"]["standards_conflicts"]["records"][0]["conflict"]
        ["candidates"][1]["id"]
        .clone();
    forged["extensions"]["standards_conflicts"]["records"][0]["conflict"]["default_candidate_id"] =
        lower_candidate;
    assert!(
        serde_json::from_value::<mino::domain::Plan>(forged).is_err(),
        "stored default candidate must be derived from strict precedence"
    );
    let unresolved = validate(&project, &plan_id);
    assert_eq!(unresolved.status.code(), Some(2));
    let unresolved = parse_json(&unresolved);
    assert_eq!(
        unresolved["findings"][0]["id"],
        "POLICY-STANDARD-CONFLICT-UNRESOLVED"
    );
    assert_eq!(
        unresolved["next_actions"][0]["id"],
        "standards.conflict.list"
    );

    let current = parse_success(&run_mino(&conflict_arguments(&project, "list", &plan_id)));
    let conflict_id = current["conflicts"][0]["conflict"]["id"]
        .as_str()
        .expect("conflict ID should be text");
    let language_candidate = current["conflicts"][0]["conflict"]["candidates"]
        .as_array()
        .expect("candidates should be an array")
        .iter()
        .find(|candidate| candidate["source_kind"] == "language_package")
        .and_then(|candidate| candidate["id"].as_str())
        .expect("language package candidate should exist");
    let mut resolve = mutate_conflict_arguments(&project, "resolve", &plan_id, 3, 4);
    resolve.extend([
        "--conflict".to_owned(),
        conflict_id.to_owned(),
        "--candidate".to_owned(),
        language_candidate.to_owned(),
        "--rationale".to_owned(),
        "Use the language package for this plan after reviewing repository policy.".to_owned(),
        "--decision-ref".to_owned(),
        "chat:standards-choice".to_owned(),
    ]);
    let resolved = parse_success(&run_mino(&resolve));
    assert_eq!(resolved["revision"], 4);
    assert_eq!(resolved["complete"], true);
    assert_eq!(
        resolved["standards_conflicts"]["conflicts"][0]["decision"]["reference"],
        "chat:standards-choice"
    );
    let replay = parse_success(&run_mino(&resolve));
    assert_eq!(replay["revision"], 4);
    assert_eq!(replay["replayed"], true);
    assert_eq!(parse_success(&validate(&project, &plan_id))["valid"], true);
    let rendered = render_plan(&load_plan(&project, &plan_id))
        .expect("resolved conflict should render as first-class Markdown");
    assert!(rendered.markdown().contains("## Standards Conflicts"));
    assert!(rendered.markdown().contains("#### Candidates"));
    assert!(rendered.markdown().contains("chat:standards-choice"));
    assert!(!rendered.markdown().contains("\"standards_conflicts\""));
}

#[test]
fn changed_sources_invalidate_decisions_and_refresh_clears_them() {
    let project = TestProject::new("stale");
    let plan_id = create_complete_plan(&project);
    parse_success(&run_mino(&mutate_conflict_arguments(
        &project, "refresh", &plan_id, 2, 10,
    )));
    let current = parse_success(&run_mino(&conflict_arguments(&project, "list", &plan_id)));
    let conflict_id = current["conflicts"][0]["conflict"]["id"]
        .as_str()
        .expect("conflict ID should be text");
    let candidate_id = current["conflicts"][0]["conflict"]["candidates"][0]["id"]
        .as_str()
        .expect("candidate ID should be text");
    let mut resolve = mutate_conflict_arguments(&project, "resolve", &plan_id, 3, 11);
    resolve.extend([
        "--conflict".to_owned(),
        conflict_id.to_owned(),
        "--candidate".to_owned(),
        candidate_id.to_owned(),
        "--rationale".to_owned(),
        "The explicit task requirement has the highest declared precedence.".to_owned(),
        "--decision-ref".to_owned(),
        "chat:user-requirement".to_owned(),
    ]);
    parse_success(&run_mino(&resolve));
    let before = load_plan(&project, &plan_id);
    assert_eq!(before.status(), PlanStatus::Draft);
    assert!(
        before
            .standards_conflict_state()
            .expect("state should decode")
            .records()[0]
            .decision()
            .is_some()
    );

    fs::write(
        project.path().join("AGENTS.md"),
        "# Changed policy\nRepository source bytes changed.\n",
    )
    .expect("source drift should be injected");
    let stale = validate(&project, &plan_id);
    assert_eq!(stale.status.code(), Some(2));
    assert!(
        parse_json(&stale)["findings"]
            .as_array()
            .expect("findings should be an array")
            .iter()
            .any(|finding| finding["id"] == "POLICY-STANDARD-CONFLICT-STALE")
    );
    let refreshed = parse_success(&run_mino(&mutate_conflict_arguments(
        &project, "refresh", &plan_id, 4, 12,
    )));
    assert_eq!(refreshed["revision"], 5);
    assert_eq!(
        refreshed["standards_conflicts"]["conflicts"][0]["status"],
        "unresolved"
    );
    assert!(
        load_plan(&project, &plan_id)
            .standards_conflict_state()
            .expect("state should decode")
            .records()[0]
            .decision()
            .is_none()
    );
}

#[test]
fn approved_execution_revalidates_conflict_sources_before_starting() {
    let project = TestProject::new("approved-drift");
    let plan_id = create_complete_plan(&project);
    parse_success(&run_mino(&mutate_conflict_arguments(
        &project, "refresh", &plan_id, 2, 20,
    )));
    let current = parse_success(&run_mino(&conflict_arguments(&project, "list", &plan_id)));
    let conflict_id = current["conflicts"][0]["conflict"]["id"]
        .as_str()
        .expect("conflict ID should be text");
    let candidate_id = current["conflicts"][0]["conflict"]["candidates"][0]["id"]
        .as_str()
        .expect("candidate ID should be text");
    let mut resolve = mutate_conflict_arguments(&project, "resolve", &plan_id, 3, 21);
    resolve.extend([
        "--conflict".to_owned(),
        conflict_id.to_owned(),
        "--candidate".to_owned(),
        candidate_id.to_owned(),
        "--rationale".to_owned(),
        "Approve this exact source set before execution.".to_owned(),
        "--decision-ref".to_owned(),
        "chat:approved-source-set".to_owned(),
    ]);
    parse_success(&run_mino(&resolve));
    let finalized = parse_success(&run_mino(&mutation_arguments(
        &project,
        &["plan", "finalize"],
        &plan_id,
        4,
        22,
    )));
    assert_eq!(finalized["status"], "Ready");
    let mut approve = mutation_arguments(&project, &["plan", "approve"], &plan_id, 5, 23);
    approve.extend([
        "--approval-ref".to_owned(),
        "chat:approved-plan".to_owned(),
        "--git-flow-consent".to_owned(),
        "disabled".to_owned(),
    ]);
    parse_success(&run_mino(&approve));

    fs::write(
        project.path().join("AGENTS.md"),
        "Repository documentation rules changed after approval.\n",
    )
    .expect("repository source should change");
    let mut start = mutation_arguments(&project, &["exec", "start"], &plan_id, 6, 24);
    start.extend(["--task".to_owned(), "T1".to_owned()]);
    let refused = run_mino(&start);
    assert_eq!(refused.status.code(), Some(2));
    let refused = parse_json(&refused);
    assert!(refused["findings"].as_array().is_some_and(|findings| {
        findings
            .iter()
            .any(|finding| finding["id"] == "POLICY-STANDARD-CONFLICT-STALE")
    }));
    let stored = load_plan(&project, &plan_id);
    assert_eq!(stored.revision(), 6);
    assert_eq!(stored.status(), PlanStatus::Ready);
}

#[test]
fn unsafe_or_malformed_local_sources_fail_without_plan_mutation() {
    let project = TestProject::new("unsafe");
    let plan_id = create_complete_plan(&project);
    let before = load_plan(&project, &plan_id);
    fs::write(
        project.path().join(".mino/standards.local.toml"),
        "format_version = 1\n[[rules]]\nrule_id = \"rust.docs\"\nvalue = \"unsafe\"\nsource_kind = \"repository_rule\"\nsource = \"../outside\"\n",
    )
    .expect("unsafe declaration should be written");
    let output = run_mino(&conflict_arguments(&project, "list", &plan_id));
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(load_plan(&project, &plan_id).revision(), before.revision());
}
