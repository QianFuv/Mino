//! Contract tests for the canonical Mino Codex plugin source and compatibility metadata.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use mino::ErrorCategory;
use mino::distribution::{
    MINO_PLUGIN_CONTRACT_KIND, PluginPackageRequest, host_target, package_plugin,
    validate_mino_plugin_source, validate_plugin_artifact_directory, validate_plugin_source,
};
use serde_json::Value;

static NEXT_TEMP_DIRECTORY: AtomicU64 = AtomicU64::new(1);

struct TestDirectory {
    path: PathBuf,
}

impl TestDirectory {
    fn new(label: &str) -> Self {
        let sequence = NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "mino-plugin-contract-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("test directory should be created");
        Self { path }
    }

    fn plugin_copy(&self, label: &str) -> PathBuf {
        let parent = self.path.join(label);
        fs::create_dir(&parent).expect("plugin copy parent should be created");
        let destination = parent.join("mino");
        copy_tree(&canonical_plugin(), &destination);
        destination
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let temporary_root = std::env::temp_dir();
        if self.path.starts_with(&temporary_root)
            && self
                .path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("mino-plugin-contract-"))
        {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

fn canonical_plugin() -> PathBuf {
    repository_root().join("plugins/mino")
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
            fs::copy(&source_path, &destination_path).expect("plugin file should copy");
        }
    }
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).expect("JSON file should be readable"))
        .expect("JSON file should parse")
}

fn write_json(path: &Path, value: &Value) {
    let mut bytes = serde_json::to_vec_pretty(value).expect("JSON should serialize");
    bytes.push(b'\n');
    fs::write(path, bytes).expect("JSON file should update");
}

fn assert_launcher_drift(label: &str, mutate: impl FnOnce(&mut Value)) {
    let temporary = TestDirectory::new(label);
    let plugin = temporary.plugin_copy("source");
    let launcher_path = plugin.join("launcher.json");
    let mut launcher = read_json(&launcher_path);
    mutate(&mut launcher);
    write_json(&launcher_path, &launcher);
    let error = validate_plugin_source(&repository_root(), &plugin)
        .expect_err("launcher drift should fail validation");
    assert_eq!(error.category(), ErrorCategory::DriftDetected);
}

fn native_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_mino"))
}

fn binary_name(target: &str) -> &'static str {
    if target == "x86_64-pc-windows-msvc" {
        "mino.exe"
    } else {
        "mino"
    }
}

fn snapshot_files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fs::read_dir(root)
        .expect("artifact directory should be readable")
        .map(|entry| {
            let entry = entry.expect("artifact entry should be readable");
            let path = entry.path();
            assert!(
                entry
                    .file_type()
                    .expect("artifact entry should inspect")
                    .is_file()
            );
            (
                PathBuf::from(entry.file_name()),
                fs::read(path).expect("artifact file should be readable"),
            )
        })
        .collect()
}

#[test]
fn canonical_source_matches_cli_protocol_capabilities_standards_and_skill() {
    let report = validate_mino_plugin_source(&repository_root())
        .expect("canonical plugin source should validate");
    assert_eq!(report.kind, MINO_PLUGIN_CONTRACT_KIND);
    assert_eq!(report.name, "mino");
    assert_eq!(report.version, env!("CARGO_PKG_VERSION"));
    assert_eq!(report.protocol, "2026-05-11.review-rework-git-flow-v1");
    assert_eq!(report.capabilities_kind, "mino.agent-capabilities/v1");
    assert_eq!(
        report.capabilities_digest,
        "sha256:976f9e64c47d63419e9cc97543d0646783307b055084008e51f15b7cc76c33f1"
    );
    assert_eq!(
        report.standards,
        [
            "common@1.0.0",
            "python@1.0.0",
            "rust@1.0.0",
            "typescript-javascript@1.0.0",
        ]
    );
    assert_eq!(report.file_count, 7);
    assert_eq!(
        report.targets,
        [
            "aarch64-apple-darwin",
            "aarch64-unknown-linux-gnu",
            "x86_64-apple-darwin",
            "x86_64-pc-windows-msvc",
            "x86_64-unknown-linux-gnu",
        ]
    );
    assert_eq!(
        report.skill_digest,
        "sha256:64a438f64361b04a3688b0cc06fcc00516bfc43c6d4a8f469a408bf777a65587"
    );
    assert_eq!(
        report.source_digest,
        "sha256:b05c249ac4469094c7a915fecfd0f5a4e98ceb56b436211192b1e3f4fe73b1d7"
    );
}

#[test]
fn manifest_or_cli_version_drift_fails_before_packaging() {
    let temporary = TestDirectory::new("manifest-version");
    let plugin = temporary.plugin_copy("source");
    let manifest_path = plugin.join(".codex-plugin/plugin.json");
    let mut manifest = read_json(&manifest_path);
    manifest["version"] = Value::String("0.1.1".to_owned());
    write_json(&manifest_path, &manifest);
    let error = validate_plugin_source(&repository_root(), &plugin)
        .expect_err("plugin version drift should fail validation");
    assert_eq!(error.category(), ErrorCategory::DriftDetected);

    assert_launcher_drift("launcher-version", |launcher| {
        launcher["cli_version"] = Value::String("0.1.1".to_owned());
    });
}

#[test]
fn protocol_capability_and_standards_drift_fail_before_packaging() {
    assert_launcher_drift("protocol", |launcher| {
        launcher["protocol"]["revision"] = Value::String("stale-revision".to_owned());
    });
    assert_launcher_drift("capabilities", |launcher| {
        launcher["agent"]["capabilities_digest"] =
            Value::String(format!("sha256:{}", "0".repeat(64)));
    });
    assert_launcher_drift("standards", |launcher| {
        launcher["standards"]
            .as_array_mut()
            .expect("standards should be an array")
            .push(Value::String("unexpected@1.0.0".to_owned()));
    });
}

#[test]
fn changed_missing_or_duplicated_skill_assets_fail_before_packaging() {
    let temporary = TestDirectory::new("skill-drift");

    let changed = temporary.plugin_copy("changed");
    let changed_skill = changed.join("skills/mino/SKILL.md");
    let mut bytes = fs::read(&changed_skill).expect("Skill should be readable");
    bytes.extend_from_slice(b"\nDrift.\n");
    fs::write(&changed_skill, bytes).expect("Skill drift should be injected");
    let changed_error = validate_plugin_source(&repository_root(), &changed)
        .expect_err("changed Skill should fail validation");
    assert_eq!(changed_error.category(), ErrorCategory::DriftDetected);

    let missing = temporary.plugin_copy("missing");
    fs::remove_file(missing.join("skills/mino/references/approval-boundaries.md"))
        .expect("Skill reference should be removed");
    let missing_error = validate_plugin_source(&repository_root(), &missing)
        .expect_err("missing Skill asset should fail validation");
    assert_eq!(missing_error.category(), ErrorCategory::DriftDetected);

    let duplicated = temporary.plugin_copy("duplicated");
    fs::copy(
        duplicated.join("skills/mino/references/command-contract.md"),
        duplicated.join("skills/mino/references/command-contract-copy.md"),
    )
    .expect("duplicate Skill asset should be added");
    let duplicate_error = validate_plugin_source(&repository_root(), &duplicated)
        .expect_err("duplicated Skill asset should fail validation");
    assert_eq!(duplicate_error.category(), ErrorCategory::DriftDetected);
}

#[test]
fn canonical_source_is_offline_binary_free_and_documents_exact_resolution() {
    assert!(!canonical_plugin().join("bin").exists());
    assert!(!canonical_plugin().join(".mcp.json").exists());
    assert!(!canonical_plugin().join(".app.json").exists());
    let skill = fs::read_to_string(canonical_plugin().join("skills/mino/SKILL.md"))
        .expect("plugin Skill should be readable");
    assert!(skill.contains("`launcher.json` exists two\nparent directories above"));
    assert!(skill.contains("Never modify `PATH`, download a binary"));
    let readme = fs::read_to_string(canonical_plugin().join("README.md"))
        .expect("plugin README should be readable");
    assert!(readme.contains("does not publish, install, update, or"));
}

#[test]
fn native_artifacts_are_byte_reproducible_reusable_and_strictly_verified() {
    let temporary = TestDirectory::new("native-artifact");
    let target = host_target().expect("current host should be supported");
    let first_output = temporary.path.join("first");
    let second_output = temporary.path.join("second");
    let first = package_plugin(&PluginPackageRequest::new(
        repository_root(),
        native_binary(),
        target,
        &first_output,
    ))
    .expect("first native artifact should package");
    let second = package_plugin(&PluginPackageRequest::new(
        repository_root(),
        native_binary(),
        target,
        &second_output,
    ))
    .expect("second native artifact should package");
    assert!(!first.reused);
    assert!(!second.reused);
    assert_eq!(first.archive_digest, second.archive_digest);
    assert_eq!(first.manifest_digest, second.manifest_digest);
    assert_eq!(
        snapshot_files(&first.output_directory),
        snapshot_files(&second.output_directory)
    );
    let manifest = validate_plugin_artifact_directory(&first.output_directory)
        .expect("complete native artifact should validate");
    assert_eq!(manifest.target, target);
    assert_eq!(manifest.files.len(), 10);
    assert_eq!(
        manifest
            .files
            .iter()
            .filter(|file| file.mode == 0o755)
            .map(|file| file.path.clone())
            .collect::<Vec<_>>(),
        vec![format!("mino/bin/{}", binary_name(target))]
    );
    let reused = package_plugin(&PluginPackageRequest::new(
        repository_root(),
        native_binary(),
        target,
        &first_output,
    ))
    .expect("identical artifact should be reused");
    assert!(reused.reused);
    assert_eq!(reused.archive_digest, first.archive_digest);

    let mut archive = OpenOptions::new()
        .append(true)
        .open(&second.archive_path)
        .expect("archive should open for corruption");
    archive
        .write_all(b"corrupt")
        .expect("archive corruption should be injected");
    let error = validate_plugin_artifact_directory(&second.output_directory)
        .expect_err("corrupt archive should fail validation");
    assert_eq!(error.category(), ErrorCategory::DriftDetected);
}

#[test]
fn missing_wrong_platform_or_incompatible_binaries_emit_no_artifact() {
    let temporary = TestDirectory::new("binary-failures");
    let target = host_target().expect("current host should be supported");
    let wrong_target = [
        "aarch64-apple-darwin",
        "aarch64-unknown-linux-gnu",
        "x86_64-apple-darwin",
        "x86_64-pc-windows-msvc",
        "x86_64-unknown-linux-gnu",
    ]
    .into_iter()
    .find(|candidate| *candidate != target)
    .expect("another target should exist");
    let wrong_target_output = temporary.path.join("wrong-target-output");
    let wrong_target_error = package_plugin(&PluginPackageRequest::new(
        repository_root(),
        native_binary(),
        wrong_target,
        &wrong_target_output,
    ))
    .expect_err("wrong-platform packaging should fail");
    assert_eq!(
        wrong_target_error.category(),
        ErrorCategory::EnvironmentUnavailable
    );
    assert_eq!(
        wrong_target_error.exit_code(),
        std::process::ExitCode::from(7)
    );
    assert!(!wrong_target_output.exists());

    let missing_output = temporary.path.join("missing-output");
    let missing_binary = temporary.path.join(binary_name(target));
    let missing_error = package_plugin(&PluginPackageRequest::new(
        repository_root(),
        &missing_binary,
        target,
        &missing_output,
    ))
    .expect_err("missing binary should fail");
    assert_eq!(
        missing_error.category(),
        ErrorCategory::EnvironmentUnavailable
    );
    assert!(!missing_output.exists());

    let incompatible_directory = temporary.path.join("incompatible");
    fs::create_dir(&incompatible_directory).expect("incompatible directory should be created");
    let incompatible_binary = incompatible_directory.join(binary_name(target));
    fs::write(&incompatible_binary, b"not a native executable")
        .expect("incompatible binary should be written");
    let incompatible_output = temporary.path.join("incompatible-output");
    let incompatible_error = package_plugin(&PluginPackageRequest::new(
        repository_root(),
        &incompatible_binary,
        target,
        &incompatible_output,
    ))
    .expect_err("incompatible binary should fail smoke");
    assert_eq!(
        incompatible_error.category(),
        ErrorCategory::EnvironmentUnavailable
    );
    assert!(!incompatible_output.exists());
}

#[test]
fn xtask_packages_the_current_host_without_installing_or_publishing() {
    let temporary = TestDirectory::new("xtask");
    let target = host_target().expect("current host should be supported");
    let output = temporary.path.join("dist");
    let result = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("package-plugin")
        .arg("--repository")
        .arg(repository_root())
        .arg("--binary")
        .arg(native_binary())
        .arg("--target")
        .arg(target)
        .arg("--output")
        .arg(&output)
        .stdin(Stdio::null())
        .output()
        .expect("xtask should run");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(result.stderr.is_empty());
    let report: Value =
        serde_json::from_slice(&result.stdout).expect("xtask report should be JSON");
    assert_eq!(report["kind"], "mino.plugin-artifact-manifest/v1");
    assert_eq!(report["target"], target);
    assert_eq!(report["reused"], false);
    validate_plugin_artifact_directory(&output.join(target))
        .expect("xtask artifact should validate");
}

#[test]
fn native_workflow_covers_every_target_and_has_no_upload_or_publish_step() {
    let workflow =
        fs::read_to_string(repository_root().join(".github/workflows/release-artifacts.yml"))
            .expect("native artifact workflow should be readable");
    for required in [
        "windows-latest",
        "ubuntu-24.04",
        "ubuntu-24.04-arm",
        "macos-15-intel",
        "macos-15",
        "x86_64-pc-windows-msvc",
        "x86_64-unknown-linux-gnu",
        "aarch64-unknown-linux-gnu",
        "x86_64-apple-darwin",
        "aarch64-apple-darwin",
        "cargo run --release --locked --bin xtask -- package-plugin",
        "permissions:\n  contents: read",
    ] {
        assert!(
            workflow.contains(required),
            "workflow is missing {required}"
        );
    }
    for forbidden in [
        "actions/upload-artifact",
        "cargo publish",
        "gh release",
        "secrets.",
        "plugin marketplace",
    ] {
        assert!(
            !workflow.contains(forbidden),
            "workflow contains forbidden text {forbidden}"
        );
    }
}
