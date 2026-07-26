//! Contract tests for embedded standards recommendations and check resolution.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use mino::project::{Language, scan_root};
use mino::standards::{
    CommandSource, EmbeddedCatalog, ResolvedCheckStatus, ToolProbe, ToolProbeOutcome,
    apply_recommendation, recommend_for_paths, recommend_initial,
};
use serde_json::Value;

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
