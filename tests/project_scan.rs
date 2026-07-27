//! Contract tests for ignore-aware workspace discovery and language scoring.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use mino::ErrorCategory;
use mino::project::{Language, ProjectScan, ScanLimits, scan_root, scan_root_with_limits};
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

fn run_mino_with_isolated_git_home(arguments: &[&str], home: &Path, config_home: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mino"))
        .args(arguments)
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("XDG_CONFIG_HOME", config_home)
        .env_remove("GIT_CONFIG_GLOBAL")
        .env(
            "GIT_CONFIG_SYSTEM",
            config_home.join("missing-system-config"),
        )
        .output()
        .expect("Mino binary should run with an isolated Git home")
}

fn write_repeated(path: &Path, byte: u8, byte_count: u64) {
    let mut file = File::create(path).expect("bounded test file should be created");
    let chunk = vec![byte; 64 * 1024].into_boxed_slice();
    let mut remaining = byte_count;
    while remaining != 0 {
        let count = usize::try_from(remaining.min(chunk.len() as u64)).expect("chunk should fit");
        file.write_all(&chunk[..count])
            .expect("bounded test bytes should be written");
        remaining = remaining.saturating_sub(count as u64);
    }
}

fn limits(
    max_depth: usize,
    max_files: u64,
    max_total_bytes: u64,
    max_file_bytes: u64,
) -> ScanLimits {
    ScanLimits {
        max_depth,
        max_files,
        max_total_bytes,
        max_file_bytes,
    }
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
    assert_eq!(
        first.digest().expect("scan digest should be computed"),
        second.digest().expect("repeated digest should be computed")
    );
    assert!(
        first
            .digest()
            .expect("scan digest should be computed")
            .starts_with("sha256:")
    );
    assert_eq!(first.files_scanned, 13);
    assert_eq!(first.directories_excluded, 2);
    assert_eq!(first.symlinks_skipped, 0);
    assert!(first.bytes_read > 0);
    assert!(!first.truncated);
    assert!(first.truncation_reasons.is_empty());
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
    assert_eq!(value["truncated"], false);
    assert!(value["bytes_read"].as_u64().is_some_and(|bytes| bytes > 0));
    assert_eq!(value["truncation_reasons"], serde_json::json!([]));
}

#[test]
fn root_nested_and_repository_excludes_remove_language_evidence() {
    let project = TestProject::new("ignore-rules");
    fs::write(project.path().join(".gitignore"), "custom-output/\n")
        .expect("root ignore should be written");
    fs::create_dir(project.path().join("custom-output"))
        .expect("root ignored directory should be created");
    fs::write(
        project.path().join("custom-output/ignored.py"),
        "print('ignored')\n",
    )
    .expect("root ignored source should be written");
    fs::create_dir(project.path().join("nested")).expect("nested directory should be created");
    fs::write(project.path().join("nested/.gitignore"), "ignored.ts\n")
        .expect("nested ignore should be written");
    fs::write(
        project.path().join("nested/ignored.ts"),
        "export const ignored = true;\n",
    )
    .expect("nested ignored source should be written");
    fs::write(
        project.path().join("nested/included.js"),
        "export const included = true;\n",
    )
    .expect("included source should be written");
    fs::create_dir_all(project.path().join(".git/info"))
        .expect("Git exclude directory should be created");
    fs::write(
        project.path().join(".git/info/exclude"),
        "repository-only.py\n",
    )
    .expect("repository exclude should be written");
    fs::write(
        project.path().join("repository-only.py"),
        "print('repository ignored')\n",
    )
    .expect("repository ignored source should be written");

    let scan = scan_root(project.path()).expect("ignore-aware project should scan");
    assert_eq!(
        language_score(&scan, Language::TypeScriptJavaScript).source_files,
        1
    );
    assert!(
        scan.languages
            .iter()
            .all(|score| score.language != Language::Python)
    );
}

#[test]
fn global_git_exclude_is_applied_by_the_cli_scan() {
    let project = TestProject::new("global-ignore");
    fs::write(
        project.path().join("Cargo.toml"),
        "[package]\nname='global-ignore'\nversion='0.1.0'\n",
    )
    .expect("manifest should be written");
    fs::write(project.path().join("visible.rs"), "pub fn visible() {}\n")
        .expect("visible source should be written");
    fs::write(
        project.path().join("global-only.py"),
        "print('globally ignored')\n",
    )
    .expect("globally ignored source should be written");
    let home = project.path().join("isolated-home");
    fs::create_dir(&home).expect("isolated home should be created");
    let config_home = project.path().join("isolated-config");
    fs::create_dir_all(config_home.join("git"))
        .expect("global Git ignore directory should be created");
    fs::write(config_home.join("git/ignore"), "global-only.py\n")
        .expect("global Git ignore should be written");
    let root = project.path().to_str().expect("test path should be UTF-8");

    let output = run_mino_with_isolated_git_home(
        &[
            "project",
            "scan",
            "--root",
            root,
            "--format",
            "json",
            "--no-input",
        ],
        &home,
        &config_home,
    );
    assert!(output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).expect("scan should return JSON");
    assert!(
        value["languages"]
            .as_array()
            .is_some_and(|languages| languages.iter().all(|item| item["language"] != "python"))
    );
}

#[test]
fn each_scan_budget_reports_deterministic_truncation() {
    let depth_project = TestProject::new("depth-budget");
    fs::write(depth_project.path().join("visible.rs"), "fn visible() {}\n")
        .expect("visible source should be written");
    fs::create_dir_all(depth_project.path().join("one/two"))
        .expect("deep directory should be created");
    fs::write(
        depth_project.path().join("one/two/deep.rs"),
        "fn deep() {}\n",
    )
    .expect("deep source should be written");
    let depth = scan_root_with_limits(depth_project.path(), limits(2, 100, 1_000, 1_000))
        .expect("depth-bounded scan should succeed");
    assert_eq!(depth.truncation_reasons, vec!["depth_limit"]);
    assert_eq!(language_score(&depth, Language::Rust).source_files, 1);

    let file_project = TestProject::new("file-budget");
    fs::write(file_project.path().join("a.rs"), "fn a() {}\n")
        .expect("first source should be written");
    fs::write(file_project.path().join("b.py"), "print('b')\n")
        .expect("second source should be written");
    fs::write(file_project.path().join("c.ts"), "export const c = 1;\n")
        .expect("third source should be written");
    let file_limits = limits(64, 2, 1_000, 1_000);
    let first = scan_root_with_limits(file_project.path(), file_limits)
        .expect("file-bounded scan should succeed");
    let second = scan_root_with_limits(file_project.path(), file_limits)
        .expect("repeated file-bounded scan should succeed");
    assert_eq!(first, second);
    assert_eq!(first.files_scanned, 2);
    assert_eq!(first.truncation_reasons, vec!["file_limit"]);

    let byte_project = TestProject::new("byte-budgets");
    fs::write(byte_project.path().join("large.js"), "abcdefghij")
        .expect("bounded source should be written");
    let total = scan_root_with_limits(byte_project.path(), limits(64, 100, 4, 100))
        .expect("total-byte-bounded scan should succeed");
    assert_eq!(total.bytes_read, 4);
    assert_eq!(total.truncation_reasons, vec!["total_byte_limit"]);
    let per_file = scan_root_with_limits(byte_project.path(), limits(64, 100, 100, 4))
        .expect("per-file-bounded scan should succeed");
    assert_eq!(per_file.bytes_read, 4);
    assert_eq!(per_file.truncation_reasons, vec!["per_file_byte_limit"]);
    let combined = scan_root_with_limits(byte_project.path(), limits(64, 100, 3, 4))
        .expect("combined-byte-bounded scan should succeed");
    assert_eq!(combined.bytes_read, 3);
    assert_eq!(
        combined.truncation_reasons,
        vec!["per_file_byte_limit", "total_byte_limit"]
    );
}

#[test]
fn zero_scan_limits_are_rejected() {
    let project = TestProject::new("invalid-budget");
    let error = scan_root_with_limits(project.path(), limits(0, 0, 0, 0))
        .expect_err("zero limits should be rejected");
    assert_eq!(error.category(), ErrorCategory::IncompleteOrValidation);
    assert!(error.message().contains("max_depth"));
    assert!(error.message().contains("max_file_bytes"));
}

#[test]
fn default_per_file_budget_bounds_a_huge_single_line() {
    const DEFAULT_MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;

    let project = TestProject::new("single-line");
    write_repeated(
        &project.path().join("huge.js"),
        b'x',
        DEFAULT_MAX_FILE_BYTES + 1,
    );
    let scan = scan_root(project.path()).expect("huge single-line source should scan");
    assert_eq!(scan.bytes_read, DEFAULT_MAX_FILE_BYTES);
    assert_eq!(scan.truncation_reasons, vec!["per_file_byte_limit"]);
    assert_eq!(
        language_score(&scan, Language::TypeScriptJavaScript).source_lines,
        1
    );
}

#[cfg(unix)]
#[test]
fn ignored_unreadable_directory_is_not_opened() {
    use std::os::unix::fs::PermissionsExt;

    let project = TestProject::new("ignored-unreadable");
    fs::write(project.path().join(".gitignore"), "private/\n")
        .expect("ignore rule should be written");
    fs::create_dir(project.path().join("private")).expect("ignored directory should be created");
    fs::write(
        project.path().join("private/ignored.py"),
        "print('private')\n",
    )
    .expect("ignored source should be written");
    fs::set_permissions(
        project.path().join("private"),
        fs::Permissions::from_mode(0o000),
    )
    .expect("ignored directory should become unreadable");
    let result = scan_root(project.path());
    fs::set_permissions(
        project.path().join("private"),
        fs::Permissions::from_mode(0o700),
    )
    .expect("ignored directory permissions should be restored");
    let scan = result.expect("ignored unreadable directory should not be opened");
    assert!(scan.languages.is_empty());
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
    if let Err(error) = fs::write(root.join(name), b"ignored\n") {
        #[cfg(target_os = "macos")]
        assert_eq!(
            error.raw_os_error(),
            Some(92),
            "macOS should reject an invalid byte sequence with EILSEQ"
        );
        #[cfg(not(target_os = "macos"))]
        panic!("non-UTF8 file should be created: {error}");
    }
}

#[cfg(not(unix))]
fn create_non_utf8_file(_root: &Path) {}
