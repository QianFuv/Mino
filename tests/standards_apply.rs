//! Contract tests for embedded standards recommendations and check resolution.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use mino::application::agent::AgentService;
use mino::application::amendment::AmendmentService;
use mino::application::plan::{CreatePlanRequest, DraftMutation, PlanMutationRequest, PlanService};
use mino::application::standards::StandardsPlanService;
use mino::domain::{
    AcceptanceCriterion, AmendmentClassification, AmendmentPatch, Approval, CheckId, CheckStatus,
    CommitGate, CriterionId, DomainError, DraftCriterionInput, DraftFileInput, DraftTaskInput,
    DraftVerificationInput, FileChange, FileMapEntry, GitFlowConsent, GitReadiness, Plan,
    PlanDraftSeed, PlanId, PlanStatus, RequestId, StandardSelection, Task, TaskId, Timestamp,
    VerificationCheck,
};
use mino::project::{Language, initialize, scan_root};
use mino::render::{render_plan, write_projection};
use mino::standards::{
    CommandSource, EmbeddedCatalog, ResolvedCheckStatus, ToolProbe, ToolProbeOutcome,
    apply_recommendation, recommend_for_paths, recommend_initial,
};
use mino::store::{MutationRequest, PlanStore};
use serde_json::{Value, json};

static NEXT_TEMP_DIRECTORY: AtomicU64 = AtomicU64::new(1);

struct TestProject {
    path: PathBuf,
}

impl TestProject {
    fn new(label: &str) -> Self {
        let sequence = NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "mino-standards-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("temporary standards project should be created");
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
                .is_some_and(|name| name.to_string_lossy().starts_with("mino-standards-"))
        {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

struct FixedProbe {
    available: BTreeSet<String>,
}

impl FixedProbe {
    fn new(tools: &[&str]) -> Self {
        Self {
            available: tools.iter().map(|tool| (*tool).to_owned()).collect(),
        }
    }
}

impl ToolProbe for FixedProbe {
    fn probe(&self, tool: &str, _working_directory: &Path) -> ToolProbeOutcome {
        if self.available.contains(tool) {
            ToolProbeOutcome::Available
        } else {
            ToolProbeOutcome::Unavailable
        }
    }
}

struct OutcomeProbe(ToolProbeOutcome);

impl ToolProbe for OutcomeProbe {
    fn probe(&self, _tool: &str, _working_directory: &Path) -> ToolProbeOutcome {
        self.0
    }
}

struct PanicProbe;

impl ToolProbe for PanicProbe {
    fn probe(&self, _tool: &str, _working_directory: &Path) -> ToolProbeOutcome {
        panic!("exact replay must not probe the host environment")
    }
}

fn request_id(number: u64) -> RequestId {
    RequestId::parse(format!("00000000-0000-0000-0000-{number:012}"))
        .expect("request ID should parse")
}

fn timestamp(minute: u8) -> Timestamp {
    Timestamp::parse(format!("2026-07-27T10:{minute:02}:00Z")).expect("timestamp should parse")
}

fn plan_request(
    plan_id: &PlanId,
    expected_revision: u64,
    request_number: u64,
    minute: u8,
    command: &[&str],
) -> PlanMutationRequest {
    PlanMutationRequest {
        plan_id: plan_id.clone(),
        expected_revision,
        request_id: request_id(request_number),
        actor: "codex".to_owned(),
        command: command.iter().map(|part| (*part).to_owned()).collect(),
        updated_at: timestamp(minute),
    }
}

fn commit_plan<F>(
    store: &PlanStore,
    plan_id: &PlanId,
    expected_revision: u64,
    request_number: u64,
    changed_fields: &[&str],
    mutation: F,
) where
    F: FnOnce(&mut Plan) -> Result<(), DomainError>,
{
    let request = MutationRequest::new(
        expected_revision,
        request_id(request_number),
        "codex",
        vec!["test".to_owned(), "prepare-language-amendment".to_owned()],
        changed_fields
            .iter()
            .map(|field| (*field).to_owned())
            .collect(),
    )
    .expect("mutation request should be valid");
    store
        .commit(plan_id, request, mutation)
        .expect("plan mutation should persist");
}

fn embedded_standard(package_id: &str) -> StandardSelection {
    StandardSelection::new(
        package_id,
        "1.0.0",
        format!("sha256:{}", "1".repeat(64)),
        "embedded",
    )
}

fn language_plan_seed(plan_id: PlanId, projection_path: String) -> Plan {
    Plan::from_draft_seed(
        PlanDraftSeed {
            id: plan_id,
            name: "Reconcile an amended language".to_owned(),
            trigger: "durable".to_owned(),
            original_request: "Add a new-language fixture during execution.".to_owned(),
            branch: None,
            markdown_path: projection_path,
            git_readiness: GitReadiness::detected(
                "Missing",
                "Not Applicable",
                None,
                None,
                "Not Applicable",
                false,
            ),
            standards: vec![embedded_standard("common"), embedded_standard("rust")],
            verification_plan: vec![VerificationCheck::new(
                CheckId::parse("CUSTOM-SMOKE").expect("check ID should parse"),
                vec!["cargo".to_owned(), "test".to_owned()],
                ".",
                0,
                false,
            )],
        },
        timestamp(0),
    )
}

fn configured_rust_task() -> Task {
    let task_id = TaskId::parse("T1").expect("task ID should parse");
    let mut task = Task::new(task_id.clone(), "Implement the Rust behavior", Vec::new());
    task.add_step("Implement the approved behavior")
        .expect("step should be added");
    task.add_file_map_entry(FileMapEntry::new(
        "src/lib.rs",
        FileChange::Modify,
        "Own the Rust implementation",
        task_id,
    ))
    .expect("file map should be added");
    task.add_acceptance_criterion(AcceptanceCriterion::new(
        CriterionId::parse("T1-A1").expect("criterion ID should parse"),
        "The Rust behavior remains observable",
    ))
    .expect("criterion should be added");
    task.add_verification_check(VerificationCheck::new(
        CheckId::parse("TASK-CUSTOM").expect("check ID should parse"),
        vec!["cargo".to_owned(), "test".to_owned()],
        ".",
        0,
        true,
    ))
    .expect("verification should be added");
    task.set_commit_gate(CommitGate::new(
        true,
        "test(fixture): verify language reconciliation",
        vec!["src/lib.rs".to_owned()],
    ))
    .expect("commit gate should be added");
    task
}

fn persist_started_rust_plan(project: &TestProject) -> (PlanId, u64) {
    let plan_id =
        PlanId::parse("2026-07-27-language-amendment").expect("plan identifier should be valid");
    let projection_path = format!("docs/plan/{plan_id}.md");
    let plan = language_plan_seed(plan_id.clone(), projection_path.clone());
    let store = PlanStore::new(project.path());
    store
        .create_plan(
            &plan,
            request_id(100),
            "codex",
            vec!["test".to_owned(), "create-language-plan".to_owned()],
        )
        .expect("plan should persist");
    let task = configured_rust_task();
    commit_plan(&store, &plan_id, 1, 101, &["tasks"], move |plan| {
        plan.add_task(task, timestamp(1))
    });
    let task_id = TaskId::parse("T1").expect("task ID should parse");
    commit_plan(
        &store,
        &plan_id,
        2,
        102,
        &["tasks.T1.status"],
        move |plan| plan.mark_task_ready(&task_id, timestamp(2)),
    );
    commit_plan(&store, &plan_id, 3, 103, &["status"], |plan| {
        plan.finalize(timestamp(3))
    });
    commit_plan(&store, &plan_id, 4, 104, &["approvals"], |plan| {
        plan.record_approval(Approval::plan(
            "user",
            "chat:language-plan-approved",
            timestamp(4),
            GitFlowConsent::Disabled,
        ))
    });
    let task_id = TaskId::parse("T1").expect("task ID should parse");
    commit_plan(
        &store,
        &plan_id,
        5,
        105,
        &["status", "tasks.T1.status"],
        move |plan| plan.start_task(&task_id, timestamp(5)),
    );
    let current = store.load_plan(&plan_id).expect("started plan should load");
    write_projection(
        &project.path().join(projection_path),
        &render_plan(&current).expect("plan should render"),
        None,
    )
    .expect("projection should be written");
    (plan_id, current.revision())
}

fn apply_language_amendment(project: &TestProject, plan_id: &PlanId, started_revision: u64) -> u64 {
    let patch: AmendmentPatch = serde_json::from_value(json!({
        "operations": [{
            "operation": "add-task-file",
            "kind": "Test Fixture",
            "task_id": "T1",
            "path": "tests/fixtures/**/*.py",
            "change": "Test",
            "reason": "Exercise the approved behavior in Python"
        }]
    }))
    .expect("amendment patch should parse");
    let amendments =
        AmendmentService::discover(project.path()).expect("amendment service should discover");
    let proposed = amendments
        .propose(
            plan_request(
                plan_id,
                started_revision,
                106,
                6,
                &["mino", "plan", "amend", "propose"],
            ),
            "Add the Python fixture".to_owned(),
            patch,
            None,
        )
        .expect("new language should produce a Material proposal");
    assert_eq!(proposed.assigned_id.as_deref(), Some("C1"));
    let plans = PlanService::discover(project.path()).expect("plan service should discover");
    let blocked = plans
        .load_verified(plan_id)
        .expect("blocked plan should load");
    assert_eq!(blocked.status(), PlanStatus::Blocked);
    assert_eq!(
        blocked
            .amendment("C1")
            .expect("amendment should exist")
            .minimum_classification(),
        AmendmentClassification::Material
    );
    let approved = amendments
        .approve(
            PlanMutationRequest {
                actor: "user".to_owned(),
                ..plan_request(
                    plan_id,
                    proposed.revision,
                    107,
                    7,
                    &["mino", "plan", "amend", "approve"],
                )
            },
            "C1".to_owned(),
            "chat:approve-language-amendment".to_owned(),
        )
        .expect("Material amendment should be approved");
    amendments
        .apply(
            plan_request(
                plan_id,
                approved.revision,
                108,
                8,
                &["mino", "plan", "amend", "apply"],
            ),
            "C1".to_owned(),
        )
        .expect("Material amendment should apply")
        .revision
}

fn assert_standards_reconciliation_action(project: &TestProject, plan_id: &PlanId) {
    let context = AgentService::discover(project.path())
        .expect("Agent service should discover")
        .context()
        .expect("Agent context should derive");
    let reconcile = context
        .next_actions
        .iter()
        .find(|action| action.id == "standards.apply")
        .expect("Agent should route the missing package to standards apply");
    assert!(
        reconcile
            .argv
            .windows(2)
            .any(|pair| pair == ["--plan", plan_id.as_str()])
    );
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/projects")
        .join(name)
}

fn run_mino(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mino"))
        .args(arguments)
        .output()
        .expect("Mino binary should run")
}

#[test]
fn embedded_catalog_is_complete_versioned_inert_and_deterministic() {
    let first = EmbeddedCatalog::load().expect("embedded catalog should load");
    let second = EmbeddedCatalog::load().expect("embedded catalog should load repeatedly");
    assert_eq!(first, second);
    assert_eq!(
        first
            .packages()
            .iter()
            .map(mino::standards::StandardsPackage::package_id)
            .collect::<Vec<_>>(),
        vec!["common", "python", "rust", "typescript-javascript"]
    );
    for package in first.packages() {
        assert_eq!(package.version(), "1.0.0");
        assert!(package.digest().starts_with("sha256:"));
        assert_eq!(package.digest().len(), 71);
        assert!(
            package
                .rules()
                .iter()
                .all(|rule| !rule.text.trim().is_empty())
        );
        assert!(
            package
                .checks()
                .iter()
                .all(|check| !check.argv.is_empty() && !check.tool.is_empty())
        );
    }
    let check_ids = first
        .packages()
        .iter()
        .flat_map(|package| package.checks().iter().map(|check| check.id.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(
        check_ids,
        vec![
            "PY-MYPY",
            "PY-RUFF-CHECK",
            "PY-RUFF-FORMAT",
            "RUST-CARGO-SORT",
            "RUST-CLIPPY",
            "RUST-FMT",
            "RUST-MIRI",
            "RUST-TEST",
            "TS-ESLINT",
            "TS-PRETTIER",
            "TS-TSC"
        ]
    );
}

#[test]
fn recommendations_limit_initial_languages_but_cover_every_file_map_language() {
    let catalog = EmbeddedCatalog::load().expect("catalog should load");
    let scan = scan_root(&fixture("monorepo")).expect("monorepo should scan");
    let initial = recommend_initial(&catalog, &scan).expect("initial recommendation should work");
    assert_eq!(
        initial
            .packages
            .iter()
            .map(|package| package.package_id.as_str())
            .collect::<Vec<_>>(),
        vec!["common", "rust", "typescript-javascript"]
    );
    assert_eq!(
        initial
            .packages
            .iter()
            .filter(|package| package.package_id != "common")
            .count(),
        2
    );
    let file_map = vec![
        PathBuf::from("src/lib.rs"),
        PathBuf::from("web/app.tsx"),
        PathBuf::from("tools/task.py"),
    ];
    let complete = recommend_for_paths(&catalog, &scan, &file_map)
        .expect("file-map recommendation should work");
    assert_eq!(
        complete
            .packages
            .iter()
            .map(|package| package.package_id.as_str())
            .collect::<Vec<_>>(),
        vec!["common", "python", "rust", "typescript-javascript"]
    );
    assert_eq!(
        catalog
            .package_for_language(Language::Python)
            .expect("Python package should exist")
            .digest(),
        complete.packages[1].digest
    );
}

#[test]
fn application_is_idempotent_prefers_project_scripts_and_marks_missing_tools() {
    let project = TestProject::new("typescript");
    fs::write(
        project.path().join("package.json"),
        r#"{
  "scripts": {
    "lint": "eslint src",
    "typecheck": "tsc --noEmit"
  }
}"#,
    )
    .expect("package manifest should be written");
    fs::write(
        project.path().join("pnpm-lock.yaml"),
        "lockfileVersion: '9.0'\n",
    )
    .expect("lockfile should be written");
    fs::create_dir(project.path().join("src")).expect("source directory should be created");
    fs::write(
        project.path().join("src/app.tsx"),
        "export const app = 1;\n",
    )
    .expect("source should be written");
    let catalog = EmbeddedCatalog::load().expect("catalog should load");
    let scan = scan_root(project.path()).expect("project should scan");
    let recommendation = recommend_for_paths(&catalog, &scan, &[PathBuf::from("src/app.tsx")])
        .expect("recommendation should work");
    let probe = FixedProbe::new(&["pnpm"]);
    let first = apply_recommendation(project.path(), &catalog, &recommendation, &probe)
        .expect("standards should apply");
    let second = apply_recommendation(project.path(), &catalog, &recommendation, &probe)
        .expect("repeated application should work");
    assert_eq!(first, second);
    assert_eq!(
        first
            .standards
            .iter()
            .map(|standard| standard.package_id.as_str())
            .collect::<Vec<_>>(),
        vec!["common", "typescript-javascript"]
    );
    let eslint = first
        .checks
        .iter()
        .find(|check| check.id == "TS-ESLINT")
        .expect("ESLint check should exist");
    assert_eq!(eslint.argv, vec!["pnpm", "run", "lint"]);
    assert_eq!(eslint.source, CommandSource::ProjectScript);
    assert_eq!(eslint.status, ResolvedCheckStatus::Runnable);
    let prettier = first
        .checks
        .iter()
        .find(|check| check.id == "TS-PRETTIER")
        .expect("Prettier check should exist");
    assert_eq!(prettier.source, CommandSource::EmbeddedTemplate);
    assert_eq!(
        prettier.argv,
        vec!["pnpm", "exec", "prettier", "--check", "."]
    );

    let python_recommendation =
        recommend_for_paths(&catalog, &scan, &[PathBuf::from("tools/task.py")])
            .expect("Python recommendation should work");
    let python = apply_recommendation(
        project.path(),
        &catalog,
        &python_recommendation,
        &FixedProbe::new(&["ruff"]),
    )
    .expect("Python standards should resolve");
    assert_eq!(
        python
            .checks
            .iter()
            .find(|check| check.id == "PY-RUFF-CHECK")
            .expect("Ruff check should exist")
            .status,
        ResolvedCheckStatus::Runnable
    );
    let mypy = python
        .checks
        .iter()
        .find(|check| check.id == "PY-MYPY")
        .expect("Mypy check should exist");
    assert_eq!(mypy.status, ResolvedCheckStatus::Unresolved);
    assert!(
        mypy.unresolved_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("mypy"))
    );
}

#[test]
fn typed_probe_failures_produce_stable_unresolved_reasons() {
    let project = TestProject::new("probe-outcomes");
    fs::write(
        project.path().join("Cargo.toml"),
        "[package]\nname='probe-outcomes'\nversion='0.1.0'\n",
    )
    .expect("manifest should be written");
    fs::create_dir(project.path().join("src")).expect("source directory should be created");
    fs::write(project.path().join("src/lib.rs"), "pub fn value() {}\n")
        .expect("source should be written");
    let catalog = EmbeddedCatalog::load().expect("catalog should load");
    let scan = scan_root(project.path()).expect("project should scan");
    let recommendation = recommend_initial(&catalog, &scan).expect("recommendation should work");

    for (outcome, expected_reason) in [
        (ToolProbeOutcome::TimedOut, "Tool probe timed out"),
        (
            ToolProbeOutcome::OutputLimitExceeded,
            "Tool probe output exceeded 65536 bytes",
        ),
        (ToolProbeOutcome::Failed, "Tool probe failed"),
    ] {
        let application = apply_recommendation(
            project.path(),
            &catalog,
            &recommendation,
            &OutcomeProbe(outcome),
        )
        .expect("typed probe failure should not fail standards application");
        assert!(!application.checks.is_empty());
        assert!(application.checks.iter().all(|check| {
            check.status == ResolvedCheckStatus::Unresolved
                && check.unresolved_reason.as_deref() == Some(expected_reason)
        }));
    }
}

#[test]
fn standards_cli_detects_recommends_and_requires_explicit_apply_flags() {
    let project = TestProject::new("cli");
    fs::write(
        project.path().join("Cargo.toml"),
        "[package]\nname='standards-cli'\nversion='0.1.0'\n",
    )
    .expect("manifest should be written");
    fs::create_dir(project.path().join("src")).expect("source directory should be created");
    fs::write(
        project.path().join("src/lib.rs"),
        "pub fn value() -> u8 { 1 }\n",
    )
    .expect("source should be written");
    let root = project.path().to_str().expect("test path should be UTF-8");
    let recommendation = run_mino(&[
        "standards",
        "recommend",
        "--root",
        root,
        "--format",
        "json",
        "--no-input",
    ]);
    assert!(recommendation.status.success());
    let value: Value =
        serde_json::from_slice(&recommendation.stdout).expect("recommendation should be JSON");
    assert_eq!(value["packages"][0]["package_id"], "common");
    assert_eq!(value["packages"][1]["package_id"], "rust");

    let invalid = run_mino(&["standards", "apply", "--root", root, "--format", "json"]);
    assert_eq!(invalid.status.code(), Some(2));
    assert!(invalid.stderr.is_empty());
    let invalid_value: Value =
        serde_json::from_slice(&invalid.stdout).expect("failure should be JSON");
    assert_eq!(invalid_value["error"]["code"], "incomplete_or_validation");

    let applied = run_mino(&[
        "standards",
        "apply",
        "--recommended",
        "--seed-verification",
        "--root",
        root,
        "--format",
        "json",
        "--no-input",
    ]);
    assert!(applied.status.success());
    let applied_value: Value =
        serde_json::from_slice(&applied.stdout).expect("application should be JSON");
    assert_eq!(applied_value["standards"][0]["package_id"], "common");
    assert!(
        applied_value["checks"]
            .as_array()
            .is_some_and(|checks| !checks.is_empty())
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn plan_scoped_apply_reconciles_file_map_languages_and_replays_without_probing() {
    let project = TestProject::new("plan-reconcile");
    fs::write(
        project.path().join("Cargo.toml"),
        "[package]\nname='plan-reconcile'\nversion='0.1.0'\n",
    )
    .expect("manifest should be written");
    fs::create_dir(project.path().join("src")).expect("source directory should be created");
    fs::write(project.path().join("src/lib.rs"), "pub fn value() {}\n")
        .expect("Rust source should be written");
    initialize(project.path()).expect("project should initialize");
    let plans = PlanService::discover(project.path()).expect("plan service should discover");
    let created = plans
        .create(CreatePlanRequest {
            name: "Reconcile standards".to_owned(),
            trigger: "durable".to_owned(),
            original_request: "Add a Python tool without losing custom checks.".to_owned(),
            request_id: request_id(1),
            actor: "codex".to_owned(),
            command: vec!["mino".to_owned(), "plan".to_owned(), "create".to_owned()],
            created_at: timestamp(0),
        })
        .expect("plan should be created");
    plans
        .mutate(
            PlanMutationRequest {
                plan_id: created.plan_id.clone(),
                expected_revision: 1,
                request_id: request_id(2),
                actor: "codex".to_owned(),
                command: vec!["mino".to_owned(), "plan".to_owned(), "task".to_owned()],
                updated_at: timestamp(1),
            },
            &DraftMutation::Task(DraftTaskInput {
                id: Some(TaskId::parse("T1").expect("task ID should parse")),
                title: "Add the Python tool".to_owned(),
                depends_on: Vec::new(),
                steps: vec!["Implement the tool".to_owned()],
                files: vec![DraftFileInput {
                    path: "tools/task.py".to_owned(),
                    change: FileChange::Create,
                    reason: "Own the new language boundary".to_owned(),
                }],
                acceptance_criteria: vec![DraftCriterionInput {
                    id: None,
                    description: "The tool runs deterministically".to_owned(),
                }],
                verification: vec![DraftVerificationInput {
                    id: CheckId::parse("TASK-CUSTOM").expect("check ID should parse"),
                    command: vec!["python".to_owned(), "-m".to_owned(), "pytest".to_owned()],
                    cwd: ".".to_owned(),
                    expected_exit_code: 0,
                    required: true,
                }],
                commit_gate: None,
            }),
        )
        .expect("task should be authored");
    plans
        .mutate(
            PlanMutationRequest {
                plan_id: created.plan_id.clone(),
                expected_revision: 2,
                request_id: request_id(3),
                actor: "codex".to_owned(),
                command: vec![
                    "mino".to_owned(),
                    "plan".to_owned(),
                    "verification".to_owned(),
                ],
                updated_at: timestamp(2),
            },
            &DraftMutation::GlobalVerification(DraftVerificationInput {
                id: CheckId::parse("CUSTOM-SMOKE").expect("check ID should parse"),
                command: vec!["custom-smoke".to_owned()],
                cwd: ".".to_owned(),
                expected_exit_code: 0,
                required: false,
            }),
        )
        .expect("custom global check should be authored");

    let request = PlanMutationRequest {
        plan_id: created.plan_id.clone(),
        expected_revision: 3,
        request_id: request_id(4),
        actor: "codex".to_owned(),
        command: vec![
            "mino".to_owned(),
            "standards".to_owned(),
            "apply".to_owned(),
            "--recommended".to_owned(),
            "--seed-verification".to_owned(),
        ],
        updated_at: timestamp(3),
    };
    let standards =
        StandardsPlanService::discover(project.path()).expect("standards service should discover");
    let applied = standards
        .reconcile_with_probe(request.clone(), &FixedProbe::new(&["uv"]))
        .expect("plan standards should reconcile");
    assert_eq!(applied.operation.revision, 4);
    assert!(!applied.operation.replayed);
    let plan = plans
        .load_verified(&created.plan_id)
        .expect("reconciled plan should load");
    assert_eq!(
        plan.standards()
            .iter()
            .map(mino::domain::StandardSelection::package_id)
            .collect::<Vec<_>>(),
        vec!["common", "python"]
    );
    let check_ids = plan
        .global_verification()
        .iter()
        .map(|check| check.id().as_str())
        .collect::<BTreeSet<_>>();
    assert!(check_ids.contains("CUSTOM-SMOKE"));
    assert!(check_ids.contains("PY-MYPY"));
    assert!(!check_ids.contains("RUST-TEST"));

    let replayed = standards
        .reconcile_with_probe(request, &PanicProbe)
        .expect("exact request should replay without a scan probe");
    assert!(replayed.operation.replayed);
    assert_eq!(replayed.operation.revision, 4);
}

#[test]
fn material_file_language_amendment_reconciles_before_reapproval() {
    let project = TestProject::new("amended-language-reconcile");
    fs::write(
        project.path().join("Cargo.toml"),
        "[package]\nname='amended-language-reconcile'\nversion='0.1.0'\n",
    )
    .expect("manifest should be written");
    fs::create_dir(project.path().join("src")).expect("source directory should be created");
    fs::write(project.path().join("src/lib.rs"), "pub fn value() {}\n")
        .expect("Rust source should be written");
    initialize(project.path()).expect("project should initialize");
    let (plan_id, started_revision) = persist_started_rust_plan(&project);
    let applied_revision = apply_language_amendment(&project, &plan_id, started_revision);
    let plans = PlanService::discover(project.path()).expect("plan service should discover");
    let ready = plans
        .load_verified(&plan_id)
        .expect("amended plan should load");
    assert_eq!(ready.status(), PlanStatus::Ready);
    assert!(!ready.has_plan_approval());
    assert!(
        ready
            .standards()
            .iter()
            .all(|standard| standard.package_id() != "python")
    );
    assert_standards_reconciliation_action(&project, &plan_id);

    let standards =
        StandardsPlanService::discover(project.path()).expect("standards service should discover");
    standards
        .reconcile_with_probe(
            plan_request(
                &plan_id,
                applied_revision,
                109,
                9,
                &["mino", "standards", "apply"],
            ),
            &FixedProbe::new(&["cargo", "cargo-sort", "cargo-miri", "uv"]),
        )
        .expect("amended File Map languages should reconcile");
    let reconciled = plans
        .load_verified(&plan_id)
        .expect("reconciled plan should load");
    assert_eq!(reconciled.status(), PlanStatus::Ready);
    assert!(!reconciled.has_plan_approval());
    assert_eq!(
        reconciled
            .standards()
            .iter()
            .map(StandardSelection::package_id)
            .collect::<Vec<_>>(),
        vec!["common", "python", "rust"]
    );
    assert!(
        reconciled
            .global_verification()
            .iter()
            .any(|check| check.id().as_str() == "PY-MYPY")
    );
}

#[test]
fn changed_catalog_check_definitions_discard_old_passing_evidence() {
    let project = TestProject::new("check-reset");
    fs::write(
        project.path().join("Cargo.toml"),
        "[package]\nname='check-reset'\nversion='0.1.0'\n",
    )
    .expect("manifest should be written");
    fs::create_dir(project.path().join("src")).expect("source directory should be created");
    fs::write(project.path().join("src/lib.rs"), "pub fn value() {}\n")
        .expect("Rust source should be written");
    initialize(project.path()).expect("project should initialize");
    let plans = PlanService::discover(project.path()).expect("plan service should discover");
    let created = plans
        .create(CreatePlanRequest {
            name: "Reset changed checks".to_owned(),
            trigger: "durable".to_owned(),
            original_request: "Reset catalog evidence after definition drift.".to_owned(),
            request_id: request_id(10),
            actor: "codex".to_owned(),
            command: vec!["mino".to_owned(), "plan".to_owned(), "create".to_owned()],
            created_at: timestamp(10),
        })
        .expect("plan should be created");
    let original = plans
        .load_verified(&created.plan_id)
        .expect("initial plan should load");
    let desired_checks = original.global_verification().to_vec();
    assert!(desired_checks.len() >= 2);
    let changed_id = desired_checks[0].id().clone();
    let retained_id = desired_checks[1].id().clone();
    let mut value = serde_json::to_value(&original).expect("plan should serialize");
    let checks = value["verification_plan"]
        .as_array_mut()
        .expect("verification plan should be an array");
    checks[0]["status"] = Value::from("Passed");
    checks[0]["evidence_refs"] = serde_json::json!(["E0001"]);
    checks[0]["command"] = serde_json::json!(["changed-command"]);
    checks[1]["status"] = Value::from("Passed");
    checks[1]["evidence_refs"] = serde_json::json!(["E0002"]);
    let mut drifted: Plan =
        serde_json::from_value(value).expect("drifted plan should remain valid");
    let catalog = EmbeddedCatalog::load().expect("catalog should load");
    let catalog_check_ids = catalog
        .packages()
        .iter()
        .flat_map(mino::standards::StandardsPackage::checks)
        .map(|check| CheckId::parse(check.id.clone()).expect("catalog check ID should parse"))
        .collect::<BTreeSet<_>>();
    drifted
        .reconcile_standards(
            drifted.standards().to_vec(),
            &catalog_check_ids,
            desired_checks,
            drifted
                .project_scan_summary()
                .expect("scan summary should decode")
                .expect("scan summary should exist"),
            &mino::domain::StandardsConflictState::default(),
            timestamp(11),
        )
        .expect("changed catalog definition should reconcile");
    let changed = drifted
        .global_verification()
        .iter()
        .find(|check| check.id() == &changed_id)
        .expect("changed check should remain selected");
    assert_eq!(changed.status(), CheckStatus::Pending);
    assert!(changed.evidence_refs().is_empty());
    let retained = drifted
        .global_verification()
        .iter()
        .find(|check| check.id() == &retained_id)
        .expect("unchanged check should remain selected");
    assert_eq!(retained.status(), CheckStatus::Passed);
    assert_eq!(retained.evidence_refs()[0].as_str(), "E0002");
}
