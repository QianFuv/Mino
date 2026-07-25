//! Contract tests for ignore-aware workspace discovery and language scoring.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use mino::project::{Language, ProjectScan, scan_root};
use serde_json::Value;

static NEXT_TEMP_DIRECTORY: AtomicU64 = AtomicU64::new(1);

struct TestProject {
    path: PathBuf,
}

impl TestProject {
    fn new(label: &str) -> Self {
        let sequence = NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "mino-scan-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("temporary scan project should be created");
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
                .is_some_and(|name| name.to_string_lossy().starts_with("mino-scan-"))
        {
            let _ = fs::remove_dir_all(&self.path);
        }
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

fn language_score(scan: &ProjectScan, language: Language) -> &mino::project::LanguageScore {
    scan.languages
        .iter()
        .find(|score| score.language == language)
        .expect("language should be ranked")
}

#[test]
fn monorepo_rankings_match_documented_weights_and_are_stable() {
    let first = scan_root(&fixture("monorepo")).expect("monorepo should scan");
    let second = scan_root(&fixture("monorepo")).expect("repeated monorepo should scan");
    assert_eq!(first, second);
    assert_eq!(first.files_scanned, 13);
    assert_eq!(first.directories_excluded, 2);
    assert_eq!(first.symlinks_skipped, 0);
    assert_eq!(
        first
            .workspaces
            .iter()
            .map(|workspace| workspace.root.clone())
            .collect::<Vec<_>>(),
        vec![
            PathBuf::from("."),
            PathBuf::from("packages/web"),
            PathBuf::from("tools/python")
        ]
    );
    assert_eq!(first.languages[0].language, Language::Rust);
    assert_eq!(first.languages[0].score_basis_points, 8_863);
    assert_eq!(first.languages[1].language, Language::TypeScriptJavaScript);
    assert_eq!(first.languages[1].score_basis_points, 7_681);
    assert_eq!(first.languages[2].language, Language::Python);
    assert_eq!(first.languages[2].score_basis_points, 6_754);
    assert_eq!(language_score(&first, Language::Rust).source_files, 2);
    assert_eq!(language_score(&first, Language::Rust).source_lines, 6);
    assert_eq!(
        language_score(&first, Language::TypeScriptJavaScript).source_lines,
        3
    );
    assert_eq!(language_score(&first, Language::Python).source_lines, 2);

    let workspace_scores = first
        .workspaces
        .iter()
        .map(|workspace| workspace.languages[0].score_basis_points)
        .collect::<Vec<_>>();
    assert_eq!(workspace_scores, vec![10_000, 9_500, 8_800]);
    for score in &first.languages {
        assert_eq!(
            score.score_basis_points,
            score
                .evidence
                .iter()
                .map(|evidence| evidence.weight_basis_points)
                .sum::<u16>()
        );
        assert!(
            (score.confidence() - f64::from(score.score_basis_points) / 10_000.0).abs()
                < f64::EPSILON
        );
    }
}

#[test]
fn nested_workspaces_do_not_double_count_sources_or_excluded_content() {
    let scan = scan_root(&fixture("monorepo")).expect("monorepo should scan");
    for language in [
        Language::Rust,
        Language::TypeScriptJavaScript,
        Language::Python,
    ] {
        let project_lines = language_score(&scan, language).source_lines;
        let workspace_lines = scan
            .workspaces
            .iter()
            .flat_map(|workspace| &workspace.languages)
            .filter(|score| score.language == language)
            .map(|score| score.source_lines)
            .sum::<u64>();
        assert_eq!(project_lines, workspace_lines);
    }
    let serialized = serde_json::to_string(&scan).expect("scan should serialize");
    assert!(!serialized.contains("node_modules"));
    assert!(!serialized.contains("bundle.min.js"));
    assert!(!serialized.contains("dist/ignored.js"));
}

#[test]
fn empty_isolated_symlink_and_non_utf8_cases_terminate_safely() {
    let empty = scan_root(&fixture("empty")).expect("empty fixture should scan");
    assert!(empty.languages.is_empty());
    assert_eq!(empty.workspaces.len(), 1);
    let isolated = scan_root(&fixture("isolated")).expect("isolated fixture should scan");
    assert_eq!(isolated.languages.len(), 1);
    assert_eq!(isolated.languages[0].language, Language::Rust);
    assert_eq!(isolated.languages[0].score_basis_points, 2_500);

    let project = TestProject::new("edges");
    fs::write(
        project.path().join("Cargo.toml"),
        "[package]\nname='edge'\nversion='0.1.0'\n",
    )
    .expect("manifest should be written");
    fs::create_dir(project.path().join("src")).expect("source directory should be created");
    fs::write(project.path().join("src/lib.rs"), "pub fn edge() {}\n")
        .expect("source should be written");
    fs::create_dir(project.path().join("target")).expect("excluded directory should be created");
    fs::write(project.path().join("target/ignored.rs"), "ignored\n")
        .expect("excluded source should be written");
    create_loop_symlink(project.path());
    create_non_utf8_file(project.path());
    let scan = scan_root(project.path()).expect("edge fixture should terminate");
    assert_eq!(scan.directories_excluded, 1);
    assert!(scan.symlinks_skipped <= 1);
    assert_eq!(language_score(&scan, Language::Rust).source_files, 1);
}

#[test]
fn project_scan_cli_returns_ordered_evidence_without_external_access() {
    let project = TestProject::new("cli");
    fs::write(
        project.path().join("Cargo.toml"),
        "[package]\nname='cli-scan'\nversion='0.1.0'\n",
    )
    .expect("manifest should be written");
    fs::create_dir(project.path().join("src")).expect("source directory should be created");
    fs::write(project.path().join("src/main.rs"), "fn main() {}\n")
        .expect("source should be written");
    let root = project.path().to_str().expect("test path should be UTF-8");
    let output = run_mino(&[
        "project",
        "scan",
        "--root",
        root,
        "--format",
        "json",
        "--no-input",
    ]);
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let value: Value = serde_json::from_slice(&output.stdout).expect("scan should return JSON");
    assert_eq!(value["kind"], "mino.result/v1");
    assert_eq!(value["complete"], true);
    assert_eq!(value["languages"][0]["language"], "rust");
    assert_eq!(value["languages"][0]["score_basis_points"], 7_500);
    assert_eq!(value["languages"][0]["evidence"][0]["code"], "manifest");
}

#[cfg(unix)]
fn create_loop_symlink(root: &Path) {
    std::os::unix::fs::symlink(root, root.join("loop")).expect("loop symlink should be created");
}

#[cfg(windows)]
fn create_loop_symlink(root: &Path) {
    let _ = std::os::windows::fs::symlink_dir(root, root.join("loop"));
}

#[cfg(not(any(unix, windows)))]
fn create_loop_symlink(_root: &Path) {}

#[cfg(unix)]
fn create_non_utf8_file(root: &Path) {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let name = OsString::from_vec(vec![b'n', b'o', b'n', 0xff]);
    fs::write(root.join(name), b"ignored\n").expect("non-UTF8 file should be created");
}

#[cfg(not(unix))]
fn create_non_utf8_file(_root: &Path) {}
