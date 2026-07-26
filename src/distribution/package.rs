//! Native plugin assembly, artifact verification, and isolated smoke execution.

use std::collections::{BTreeMap, BTreeSet};
#[cfg(unix)]
use std::fs::File;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde_json::Value;

use crate::distribution::validate_mino_plugin_source;
use crate::domain::{
    CheckId, CheckRunContext, CheckRunLease, CheckRunLimits, CheckRunOutcome, PlanId, RequestId,
    Timestamp, VerificationCheck,
};
use crate::project;
use crate::runner::{ProcessRunner, Redactor, RunEnvironment};
use crate::store::sha256_digest;
use crate::{ErrorCategory, MinoError};

use super::archive::{
    ArchiveInput, build_archive, extract_archive, inventory, safe_archive_path, verify_archive,
};
use super::manifest::{
    ArtifactArchive, ArtifactFile, ArtifactSmokeProof, MINO_PLUGIN_ARCHIVE_KIND,
    MINO_PLUGIN_ARTIFACT_KIND, PluginArtifactManifest, PluginArtifactReport,
};

const ARTIFACT_MANIFEST_FILE: &str = "artifact-manifest.json";
const CHECKSUMS_FILE: &str = "SHA256SUMS";
const MAX_BINARY_BYTES: u64 = 128 * 1024 * 1024;
const MAX_PROBE_OUTPUT_BYTES: u64 = 1024 * 1024;
const STAGING_PREFIX: &str = ".mino-plugin-staging-";
const SMOKE_PREFIX: &str = "mino-plugin-smoke-";
static NEXT_TEMPORARY_DIRECTORY: AtomicU64 = AtomicU64::new(1);

/// Inputs required to package one native Mino plugin target.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PluginPackageRequest {
    repository_root: PathBuf,
    binary_path: PathBuf,
    target: String,
    output_directory: PathBuf,
}

impl PluginPackageRequest {
    /// Creates a native packaging request.
    #[must_use]
    pub fn new(
        repository_root: impl Into<PathBuf>,
        binary_path: impl Into<PathBuf>,
        target: impl Into<String>,
        output_directory: impl Into<PathBuf>,
    ) -> Self {
        Self {
            repository_root: repository_root.into(),
            binary_path: binary_path.into(),
            target: target.into(),
            output_directory: output_directory.into(),
        }
    }

    /// Returns the repository root containing canonical plugin sources.
    #[must_use]
    pub fn repository_root(&self) -> &Path {
        &self.repository_root
    }

    /// Returns the exact native binary path.
    #[must_use]
    pub fn binary_path(&self) -> &Path {
        &self.binary_path
    }

    /// Returns the requested native Rust target triple.
    #[must_use]
    pub fn target(&self) -> &str {
        &self.target
    }

    /// Returns the parent output directory for target artifacts.
    #[must_use]
    pub fn output_directory(&self) -> &Path {
        &self.output_directory
    }
}

struct TemporaryDirectory {
    parent: PathBuf,
    path: PathBuf,
    prefix: &'static str,
    should_remove: bool,
}

impl TemporaryDirectory {
    fn create(parent: &Path, prefix: &'static str) -> Result<Self, MinoError> {
        for _ in 0..100 {
            let sequence = NEXT_TEMPORARY_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let path = parent.join(format!("{prefix}{}-{sequence}", std::process::id()));
            match fs::create_dir(&path) {
                Ok(()) => {
                    return Ok(Self {
                        parent: parent.to_path_buf(),
                        path,
                        prefix,
                        should_remove: true,
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => {
                    return Err(path_error(
                        "create temporary plugin directory",
                        &path,
                        &error,
                    ));
                }
            }
        }
        Err(environment_error(
            "Could not allocate a unique temporary plugin directory",
        ))
    }

    fn mark_published(&mut self) {
        self.should_remove = false;
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        if self.should_remove {
            let _ = remove_guarded_directory(&self.parent, &self.path, self.prefix);
        }
    }
}

/// Returns the supported native target triple for the current host.
///
/// # Errors
///
/// Returns environment-unavailable when the current OS/architecture pair is
/// outside the five declared native distribution targets.
pub fn host_target() -> Result<&'static str, MinoError> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Ok("x86_64-pc-windows-msvc"),
        ("linux", "x86_64") => Ok("x86_64-unknown-linux-gnu"),
        ("linux", "aarch64") => Ok("aarch64-unknown-linux-gnu"),
        ("macos", "x86_64") => Ok("x86_64-apple-darwin"),
        ("macos", "aarch64") => Ok("aarch64-apple-darwin"),
        (os, architecture) => Err(environment_error(format!(
            "No native plugin target is declared for {os}/{architecture}"
        ))),
    }
}

/// Packages, verifies, smoke-installs, and atomically publishes one native artifact.
///
/// Packaging is host-native only. It performs no network request, PATH mutation,
/// user installation, marketplace update, upload, or publish operation.
///
/// # Errors
///
/// Returns a typed error for source drift, target/host mismatch, missing or
/// incompatible binaries, archive defects, failed bounded smoke probes, or I/O.
pub fn package_plugin(request: &PluginPackageRequest) -> Result<PluginArtifactReport, MinoError> {
    let repository_root = canonical_directory(&request.repository_root, "repository root")?;
    let contract = validate_mino_plugin_source(&repository_root)?;
    let host = host_target()?;
    if request.target != host
        || !contract
            .targets
            .iter()
            .any(|target| target == &request.target)
    {
        return Err(environment_error(format!(
            "Native plugin packaging requires host target {host}; requested {}",
            request.target
        )));
    }
    let binary = read_native_binary(&request.binary_path, &request.target)?;
    let files = assemble_archive_inputs(&repository_root, &contract.plugin_root, binary, host)?;
    let file_inventory = inventory(&files)?;
    let archive_bytes = build_archive(&files)?;
    verify_archive(&archive_bytes, &file_inventory)?;
    let smoke = smoke_archive(
        &archive_bytes,
        &file_inventory,
        host,
        &contract.capabilities_digest,
        &contract.capabilities_kind,
    )?;
    let archive_name = format!("mino-plugin-{}-{}.zip", contract.version, request.target);
    let archive_digest = sha256_digest(&archive_bytes);
    let archive_size = u64::try_from(archive_bytes.len())
        .map_err(|_| environment_error("Plugin archive byte count does not fit into u64"))?;
    let manifest = PluginArtifactManifest {
        kind: MINO_PLUGIN_ARTIFACT_KIND.to_owned(),
        plugin_name: contract.name,
        plugin_version: contract.version,
        target: request.target.clone(),
        protocol: contract.protocol,
        capabilities_kind: contract.capabilities_kind,
        capabilities_digest: contract.capabilities_digest,
        standards: contract.standards,
        skill_digest: contract.skill_digest,
        source_digest: contract.source_digest.clone(),
        archive: ArtifactArchive {
            kind: MINO_PLUGIN_ARCHIVE_KIND.to_owned(),
            file: archive_name.clone(),
            digest: archive_digest.clone(),
            bytes: archive_size,
        },
        files: file_inventory,
        smoke,
    };
    let manifest_bytes = canonical_json(&manifest)?;
    let manifest_digest = sha256_digest(&manifest_bytes);
    let checksums = checksum_bytes([
        (archive_name.as_str(), archive_digest.as_str()),
        (ARTIFACT_MANIFEST_FILE, manifest_digest.as_str()),
    ])?;
    let artifacts = BTreeMap::from([
        (archive_name.clone(), archive_bytes),
        (ARTIFACT_MANIFEST_FILE.to_owned(), manifest_bytes),
        (CHECKSUMS_FILE.to_owned(), checksums),
    ]);
    let (output_directory, reused) =
        publish_target_directory(&request.output_directory, &request.target, &artifacts)?;
    validate_plugin_artifact_directory(&output_directory)?;
    let file_count = u64::try_from(manifest.files.len())
        .map_err(|_| environment_error("Archive file count does not fit into u64"))?;
    Ok(PluginArtifactReport {
        kind: MINO_PLUGIN_ARTIFACT_KIND.to_owned(),
        target: request.target.clone(),
        archive_path: output_directory.join(&archive_name),
        manifest_path: output_directory.join(ARTIFACT_MANIFEST_FILE),
        checksums_path: output_directory.join(CHECKSUMS_FILE),
        output_directory,
        archive_digest,
        manifest_digest,
        source_digest: contract.source_digest,
        reused,
        file_count,
    })
}

/// Validates one complete target artifact directory without executing its binary.
///
/// # Errors
///
/// Returns a typed error for unexpected paths, non-canonical manifest/checksum
/// bytes, unsafe archive entries, or any digest, size, mode, target, or name drift.
pub fn validate_plugin_artifact_directory(
    directory: &Path,
) -> Result<PluginArtifactManifest, MinoError> {
    let root = canonical_directory(directory, "plugin artifact directory")?;
    let manifest_path = root.join(ARTIFACT_MANIFEST_FILE);
    let manifest_bytes = read_regular_file(&manifest_path, 16 * 1024 * 1024)?;
    let manifest: PluginArtifactManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|error| drift_error(format!("Failed to parse artifact manifest: {error}")))?;
    if canonical_json(&manifest)? != manifest_bytes
        || manifest.kind != MINO_PLUGIN_ARTIFACT_KIND
        || manifest.plugin_name != "mino"
        || VersionLike::parse(&manifest.plugin_version).is_none()
        || manifest.archive.kind != MINO_PLUGIN_ARCHIVE_KIND
        || !manifest
            .archive
            .file
            .starts_with(&format!("mino-plugin-{}-", manifest.plugin_version))
        || !declared_target(&manifest.target)
        || manifest.archive.file
            != format!(
                "mino-plugin-{}-{}.zip",
                manifest.plugin_version, manifest.target
            )
        || manifest.smoke.probes
            != [
                "mino --version",
                "mino agent capabilities",
                "mino project doctor",
                "mino agent context",
            ]
        || !manifest.smoke.isolated_home
        || manifest.smoke.path_mutated
        || manifest.smoke.network_access
    {
        return Err(drift_error(
            "Artifact manifest identity, target, archive, or smoke policy is invalid",
        ));
    }
    let expected_paths = BTreeSet::from([
        manifest.archive.file.clone(),
        ARTIFACT_MANIFEST_FILE.to_owned(),
        CHECKSUMS_FILE.to_owned(),
    ]);
    if collect_direct_files(&root)? != expected_paths {
        return Err(drift_error(
            "Plugin artifact directory contains missing or unexpected paths",
        ));
    }
    let archive_path = root.join(&manifest.archive.file);
    let archive_bytes = read_regular_file(&archive_path, MAX_BINARY_BYTES * 2)?;
    if u64::try_from(archive_bytes.len()).ok() != Some(manifest.archive.bytes)
        || sha256_digest(&archive_bytes) != manifest.archive.digest
    {
        return Err(drift_error(
            "Plugin archive bytes differ from the artifact manifest",
        ));
    }
    verify_archive(&archive_bytes, &manifest.files)?;
    let manifest_digest = sha256_digest(&manifest_bytes);
    let expected_checksums = checksum_bytes([
        (
            manifest.archive.file.as_str(),
            manifest.archive.digest.as_str(),
        ),
        (ARTIFACT_MANIFEST_FILE, manifest_digest.as_str()),
    ])?;
    let checksums = read_regular_file(&root.join(CHECKSUMS_FILE), 1024 * 1024)?;
    if checksums != expected_checksums {
        return Err(drift_error(
            "Plugin checksum list differs from canonical artifact identities",
        ));
    }
    Ok(manifest)
}

struct VersionLike;

impl VersionLike {
    fn parse(value: &str) -> Option<()> {
        semver::Version::parse(value)
            .ok()
            .filter(|version| version.to_string() == value)
            .map(|_| ())
    }
}

fn read_native_binary(path: &Path, target: &str) -> Result<Vec<u8>, MinoError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| path_error("inspect native Mino binary", path, &error))?;
    let expected_name = binary_name(target);
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > MAX_BINARY_BYTES
        || path.file_name().and_then(|name| name.to_str()) != Some(expected_name)
    {
        return Err(environment_error(format!(
            "Native Mino binary {} must be a regular {expected_name} file between 1 and {MAX_BINARY_BYTES} bytes",
            path.display()
        )));
    }
    let bytes =
        fs::read(path).map_err(|error| path_error("read native Mino binary", path, &error))?;
    if u64::try_from(bytes.len()).ok() != Some(metadata.len()) {
        return Err(MinoError::new(
            ErrorCategory::DriftDetected,
            format!(
                "Native Mino binary {} changed while reading",
                path.display()
            ),
        ));
    }
    Ok(bytes)
}

fn assemble_archive_inputs(
    repository_root: &Path,
    plugin_root: &Path,
    binary: Vec<u8>,
    target: &str,
) -> Result<Vec<ArchiveInput>, MinoError> {
    let mut files = Vec::new();
    collect_plugin_files(plugin_root, plugin_root, &mut files)?;
    for license in ["LICENSE-APACHE", "LICENSE-MIT"] {
        let bytes = read_regular_file(&repository_root.join(license), 64 * 1024)?;
        files.push(ArchiveInput {
            path: format!("mino/{license}"),
            bytes,
            mode: 0o644,
        });
    }
    files.push(ArchiveInput {
        path: format!("mino/bin/{}", binary_name(target)),
        bytes: binary,
        mode: 0o755,
    });
    files.sort_by(|left, right| left.path.cmp(&right.path));
    if files.windows(2).any(|pair| pair[0].path == pair[1].path) {
        return Err(drift_error("Plugin archive inputs contain duplicate paths"));
    }
    Ok(files)
}

fn collect_plugin_files(
    root: &Path,
    directory: &Path,
    files: &mut Vec<ArchiveInput>,
) -> Result<(), MinoError> {
    for entry in fs::read_dir(directory)
        .map_err(|error| path_error("enumerate plugin source", directory, &error))?
    {
        let entry =
            entry.map_err(|error| path_error("read plugin source entry", directory, &error))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| path_error("inspect plugin source entry", &path, &error))?;
        if file_type.is_symlink() {
            return Err(drift_error(format!(
                "Plugin source {} contains a symbolic link",
                path.display()
            )));
        }
        if file_type.is_dir() {
            collect_plugin_files(root, &path, files)?;
        } else if file_type.is_file() {
            let relative = relative_path(root, &path)?;
            let bytes = read_regular_file(&path, 16 * 1024 * 1024)?;
            files.push(ArchiveInput {
                path: format!("mino/{relative}"),
                bytes,
                mode: 0o644,
            });
        } else {
            return Err(drift_error(format!(
                "Plugin source {} has an unsupported entry",
                path.display()
            )));
        }
    }
    Ok(())
}

fn smoke_archive(
    archive_bytes: &[u8],
    files: &[ArtifactFile],
    target: &str,
    expected_capabilities_digest: &str,
    expected_capabilities_kind: &str,
) -> Result<ArtifactSmokeProof, MinoError> {
    let temporary_root = canonical_or_create_directory(&std::env::temp_dir())?;
    let smoke = TemporaryDirectory::create(&temporary_root, SMOKE_PREFIX)?;
    let install = smoke.path.join("install");
    extract_archive(archive_bytes, files, &install)?;
    let binary = install.join("mino/bin").join(binary_name(target));
    let home = smoke.path.join("home");
    let project_root = smoke.path.join("project");
    fs::create_dir(&home).map_err(|error| path_error("create isolated home", &home, &error))?;
    fs::create_dir(&project_root)
        .map_err(|error| path_error("create isolated smoke project", &project_root, &error))?;
    write_new_file(
        &project_root.join("Cargo.toml"),
        b"[package]\nname = \"mino-plugin-smoke\"\nversion = \"0.0.0\"\n",
    )?;
    project::initialize(&project_root)?;
    let environment = isolated_environment(&home, &smoke.path)?;
    let version = run_probe(
        &install,
        &binary,
        &["--version"],
        "PLUGIN-VERSION",
        "00000000-0000-0000-0000-000000000001",
        &environment,
    )?;
    if version.trim() != format!("mino {}", env!("CARGO_PKG_VERSION")) {
        return Err(environment_error(format!(
            "Archived Mino version probe returned incompatible output: {version:?}"
        )));
    }
    let capabilities = run_json_probe(
        &install,
        &binary,
        &["agent", "capabilities", "--format", "json", "--no-input"],
        "PLUGIN-CAPABILITIES",
        "00000000-0000-0000-0000-000000000002",
        &environment,
    )?;
    let capabilities_bytes = serde_json::to_vec(&capabilities).map_err(|error| {
        environment_error(format!(
            "Failed to canonicalize smoke capabilities: {error}"
        ))
    })?;
    if capabilities["kind"] != expected_capabilities_kind
        || sha256_digest(&capabilities_bytes) != expected_capabilities_digest
    {
        return Err(environment_error(
            "Archived Mino capabilities are incompatible with launcher metadata",
        ));
    }
    let doctor = run_json_probe(
        &project_root,
        &binary,
        &["project", "doctor", "--format", "json", "--no-input"],
        "PLUGIN-DOCTOR",
        "00000000-0000-0000-0000-000000000003",
        &environment,
    )?;
    if doctor["ok"] != true {
        return Err(environment_error(
            "Archived Mino doctor probe did not return an OK result",
        ));
    }
    let context = run_json_probe(
        &project_root,
        &binary,
        &["agent", "context", "--format", "json", "--no-input"],
        "PLUGIN-CONTEXT",
        "00000000-0000-0000-0000-000000000004",
        &environment,
    )?;
    if context["kind"] != "mino.agent-context/v1" {
        return Err(environment_error(
            "Archived Mino context probe returned an incompatible schema",
        ));
    }
    Ok(ArtifactSmokeProof {
        probes: vec![
            "mino --version".to_owned(),
            "mino agent capabilities".to_owned(),
            "mino project doctor".to_owned(),
            "mino agent context".to_owned(),
        ],
        isolated_home: true,
        path_mutated: false,
        network_access: false,
    })
}

fn run_json_probe(
    root: &Path,
    binary: &Path,
    arguments: &[&str],
    check_id: &str,
    request_id: &str,
    environment: &RunEnvironment,
) -> Result<Value, MinoError> {
    let output = run_probe(root, binary, arguments, check_id, request_id, environment)?;
    serde_json::from_str(&output).map_err(|error| {
        environment_error(format!(
            "Smoke probe {check_id} returned invalid JSON: {error}"
        ))
    })
}

fn run_probe(
    root: &Path,
    binary: &Path,
    arguments: &[&str],
    check_id: &str,
    request_id: &str,
    environment: &RunEnvironment,
) -> Result<String, MinoError> {
    let redactor = Redactor::new(Vec::new()).map_err(|error| runner_error(&error))?;
    let check = VerificationCheck::new(
        CheckId::parse(check_id).map_err(|error| domain_error(&error))?,
        std::iter::once(binary.to_string_lossy().into_owned())
            .chain(arguments.iter().map(|argument| (*argument).to_owned()))
            .collect(),
        ".",
        0,
        true,
    );
    let context = CheckRunContext::new(
        PlanId::parse("2026-07-26-plugin-smoke").map_err(|error| domain_error(&error))?,
        1,
        None,
        RequestId::parse(request_id).map_err(|error| domain_error(&error))?,
        "xtask",
        Timestamp::now_utc(),
    )
    .map_err(|error| domain_error(&error))?;
    let limits = CheckRunLimits::new(Duration::from_secs(30), MAX_PROBE_OUTPUT_BYTES)
        .map_err(|error| domain_error(&error))?;
    let lease = CheckRunLease::new(
        context,
        &check,
        limits,
        environment.variable_names(),
        environment.digest(),
        redactor.policy_digest(),
    )
    .map_err(|error| domain_error(&error))?;
    let result = ProcessRunner::default()
        .run(root, lease, environment, &redactor)
        .map_err(|error| runner_error(&error))?;
    if result.outcome() != CheckRunOutcome::Passed || result.exit_code() != Some(0) {
        return Err(environment_error(format!(
            "Smoke probe {check_id} failed with {:?}/{}: stdout={} stderr={}",
            result.outcome(),
            result
                .exit_code()
                .map_or_else(|| "none".to_owned(), |code| code.to_string()),
            result.stdout_summary(),
            result.stderr_summary()
        )));
    }
    Ok(result.stdout_summary().to_owned())
}

fn isolated_environment(home: &Path, temporary: &Path) -> Result<RunEnvironment, MinoError> {
    let mut environment = RunEnvironment::empty();
    for (name, value) in [
        ("HOME", home.to_string_lossy().into_owned()),
        ("USERPROFILE", home.to_string_lossy().into_owned()),
        ("TEMP", temporary.to_string_lossy().into_owned()),
        ("TMP", temporary.to_string_lossy().into_owned()),
        ("TMPDIR", temporary.to_string_lossy().into_owned()),
    ] {
        environment = environment
            .with_variable(name, value)
            .map_err(|error| runner_error(&error))?;
    }
    for name in ["PATH", "PATHEXT", "SYSTEMROOT", "WINDIR"] {
        if let Ok(value) = std::env::var(name) {
            environment = environment
                .with_variable(name, value)
                .map_err(|error| runner_error(&error))?;
        }
    }
    Ok(environment)
}

fn publish_target_directory(
    output_root: &Path,
    target: &str,
    artifacts: &BTreeMap<String, Vec<u8>>,
) -> Result<(PathBuf, bool), MinoError> {
    let output_root = canonical_or_create_directory(output_root)?;
    let destination = output_root.join(target);
    if path_exists(&destination)? {
        verify_artifact_bytes(&destination, artifacts)?;
        return Ok((destination, true));
    }
    let mut staging = TemporaryDirectory::create(&output_root, STAGING_PREFIX)?;
    for (name, bytes) in artifacts {
        if name.contains(['/', '\\']) || name.is_empty() {
            return Err(drift_error(format!("Unsafe artifact file name {name:?}")));
        }
        write_new_file(&staging.path.join(name), bytes)?;
    }
    verify_artifact_bytes(&staging.path, artifacts)?;
    sync_directory(&staging.path)?;
    fs::rename(&staging.path, &destination)
        .map_err(|error| path_error("publish plugin artifact directory", &destination, &error))?;
    staging.mark_published();
    sync_directory(&output_root)?;
    Ok((destination, false))
}

fn verify_artifact_bytes(
    directory: &Path,
    expected: &BTreeMap<String, Vec<u8>>,
) -> Result<(), MinoError> {
    let actual_paths = collect_direct_files(directory)?;
    if actual_paths != expected.keys().cloned().collect() {
        return Err(drift_error(format!(
            "Artifact directory {} contains missing or unexpected files",
            directory.display()
        )));
    }
    for (name, bytes) in expected {
        let actual = fs::read(directory.join(name))
            .map_err(|error| path_error("read plugin artifact", &directory.join(name), &error))?;
        if &actual != bytes {
            return Err(drift_error(format!(
                "Artifact file {} differs from prepared bytes",
                directory.join(name).display()
            )));
        }
    }
    Ok(())
}

fn collect_direct_files(directory: &Path) -> Result<BTreeSet<String>, MinoError> {
    let mut files = BTreeSet::new();
    for entry in fs::read_dir(directory)
        .map_err(|error| path_error("enumerate artifact directory", directory, &error))?
    {
        let entry = entry
            .map_err(|error| path_error("read artifact directory entry", directory, &error))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| path_error("inspect artifact directory entry", &path, &error))?;
        if file_type.is_symlink() || !file_type.is_file() {
            return Err(drift_error(format!(
                "Artifact directory entry {} must be a regular file",
                path.display()
            )));
        }
        let name = entry.file_name().into_string().map_err(|_| {
            drift_error(format!(
                "Artifact file {} has a non-UTF-8 name",
                path.display()
            ))
        })?;
        files.insert(name);
    }
    Ok(files)
}

fn checksum_bytes<const N: usize>(entries: [(&str, &str); N]) -> Result<Vec<u8>, MinoError> {
    let mut entries = entries
        .into_iter()
        .map(|(name, digest)| {
            let digest = digest.strip_prefix("sha256:").ok_or_else(|| {
                drift_error(format!("Checksum for {name} is not canonical SHA-256"))
            })?;
            Ok((name, digest))
        })
        .collect::<Result<Vec<_>, MinoError>>()?;
    entries.sort_by(|left, right| left.0.cmp(right.0));
    let mut output = String::new();
    for (name, digest) in entries {
        output.push_str(digest);
        output.push_str("  ");
        output.push_str(name);
        output.push('\n');
    }
    Ok(output.into_bytes())
}

fn canonical_json<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, MinoError> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|error| {
        environment_error(format!("Failed to serialize plugin artifact JSON: {error}"))
    })?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn declared_target(target: &str) -> bool {
    matches!(
        target,
        "x86_64-pc-windows-msvc"
            | "x86_64-unknown-linux-gnu"
            | "aarch64-unknown-linux-gnu"
            | "x86_64-apple-darwin"
            | "aarch64-apple-darwin"
    )
}

fn binary_name(target: &str) -> &'static str {
    if target == "x86_64-pc-windows-msvc" {
        "mino.exe"
    } else {
        "mino"
    }
}

fn relative_path(root: &Path, path: &Path) -> Result<String, MinoError> {
    let relative = path.strip_prefix(root).map_err(|_| {
        drift_error(format!(
            "Plugin path {} escaped {}",
            path.display(),
            root.display()
        ))
    })?;
    let mut segments = Vec::new();
    for component in relative.components() {
        let Component::Normal(segment) = component else {
            return Err(drift_error(format!(
                "Plugin path {} is not normal",
                path.display()
            )));
        };
        segments.push(
            segment.to_str().ok_or_else(|| {
                drift_error(format!("Plugin path {} is not UTF-8", path.display()))
            })?,
        );
    }
    let rendered = segments.join("/");
    safe_archive_path(&format!("mino/{rendered}"))?;
    Ok(rendered)
}

fn read_regular_file(path: &Path, maximum_bytes: u64) -> Result<Vec<u8>, MinoError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| path_error("inspect distribution file", path, &error))?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > maximum_bytes
    {
        return Err(drift_error(format!(
            "Distribution file {} must be a bounded non-empty regular file",
            path.display()
        )));
    }
    let bytes =
        fs::read(path).map_err(|error| path_error("read distribution file", path, &error))?;
    if u64::try_from(bytes.len()).ok() != Some(metadata.len()) {
        return Err(MinoError::new(
            ErrorCategory::DriftDetected,
            format!("Distribution file {} changed while reading", path.display()),
        ));
    }
    Ok(bytes)
}

fn canonical_directory(path: &Path, label: &str) -> Result<PathBuf, MinoError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| path_error(&format!("inspect {label}"), path, &error))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(drift_error(format!(
            "{label} {} must be a real directory",
            path.display()
        )));
    }
    fs::canonicalize(path).map_err(|error| path_error(&format!("resolve {label}"), path, &error))
}

fn canonical_or_create_directory(path: &Path) -> Result<PathBuf, MinoError> {
    if path_exists(path)? {
        return canonical_directory(path, "distribution directory");
    }
    fs::create_dir_all(path)
        .map_err(|error| path_error("create distribution directory", path, &error))?;
    canonical_directory(path, "distribution directory")
}

fn write_new_file(path: &Path, bytes: &[u8]) -> Result<(), MinoError> {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|error| path_error("create distribution file", path, &error))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| path_error("write distribution file", path, &error))
}

fn remove_guarded_directory(parent: &Path, path: &Path, prefix: &str) -> Result<(), MinoError> {
    if path.parent() != Some(parent)
        || !path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(prefix))
    {
        return Err(drift_error(format!(
            "Refused to remove unguarded temporary directory {}",
            path.display()
        )));
    }
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(path_error(
            "remove temporary plugin directory",
            path,
            &error,
        )),
    }
}

fn path_exists(path: &Path) -> Result<bool, MinoError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(path_error("inspect distribution path", path, &error)),
    }
}

#[cfg(unix)]
fn sync_directory(directory: &Path) -> Result<(), MinoError> {
    File::open(directory)
        .and_then(|file| file.sync_all())
        .map_err(|error| path_error("synchronize distribution directory", directory, &error))
}

#[cfg(not(unix))]
fn sync_directory(directory: &Path) -> Result<(), MinoError> {
    fs::metadata(directory)
        .map(|_| ())
        .map_err(|error| path_error("inspect distribution directory", directory, &error))
}

fn domain_error(error: &crate::domain::DomainError) -> MinoError {
    environment_error(format!("Failed to construct plugin smoke probe: {error}"))
}

fn runner_error(error: &crate::runner::RunnerError) -> MinoError {
    environment_error(format!("Plugin smoke runner failed: {error}"))
}

fn path_error(action: &str, path: &Path, error: &std::io::Error) -> MinoError {
    environment_error(format!("Failed to {action} {}: {error}", path.display()))
}

fn drift_error(message: impl Into<String>) -> MinoError {
    MinoError::new(ErrorCategory::DriftDetected, message)
}

fn environment_error(message: impl Into<String>) -> MinoError {
    MinoError::new(ErrorCategory::EnvironmentUnavailable, message)
}
