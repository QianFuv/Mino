//! Deterministic compatibility and asset-drift checks for the Mino plugin source.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use semver::Version;
use serde::{Deserialize, Serialize};

use crate::application::agent::AgentService;
use crate::protocol::ProtocolRegistry;
use crate::standards::EmbeddedCatalog;
use crate::store::sha256_digest;
use crate::{ErrorCategory, MinoError};

/// Stable schema identifier for a successful plugin-source contract report.
pub const MINO_PLUGIN_CONTRACT_KIND: &str = "mino.plugin-contract/v1";

const PLUGIN_NAME: &str = "mino";
const PLUGIN_SOURCE_PATH: &str = "plugins/mino";
const CANONICAL_SKILL_PATH: &str = "assets/skill/mino";
const PLUGIN_MANIFEST_PATH: &str = ".codex-plugin/plugin.json";
const LAUNCHER_PATH: &str = "launcher.json";
const README_PATH: &str = "README.md";
const MAX_SOURCE_FILES: usize = 64;
const MAX_SOURCE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_RELATIVE_PATH_BYTES: usize = 512;

const EXPECTED_PLUGIN_CAPABILITIES: &[&str] =
    &["Planning", "Execution", "Evidence", "Git Flow", "Standards"];
const EXPECTED_TARGETS: &[&str] = &[
    "aarch64-apple-darwin",
    "aarch64-unknown-linux-gnu",
    "x86_64-apple-darwin",
    "x86_64-pc-windows-msvc",
    "x86_64-unknown-linux-gnu",
];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PluginManifest {
    name: String,
    version: String,
    description: String,
    author: PluginAuthor,
    license: String,
    keywords: Vec<String>,
    skills: String,
    interface: PluginInterface,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PluginAuthor {
    name: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "camelCase")]
struct PluginInterface {
    display_name: String,
    short_description: String,
    long_description: String,
    developer_name: String,
    category: String,
    capabilities: Vec<String>,
    default_prompt: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LauncherDocument {
    kind: String,
    cli_version: String,
    protocol: LauncherProtocol,
    agent: LauncherAgent,
    standards: Vec<String>,
    binary: LauncherBinary,
    commands: LauncherCommands,
    missing_or_incompatible_exit_code: u8,
    offline: bool,
    mutates_path: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LauncherProtocol {
    identity: String,
    version: String,
    revision: String,
    schema_version: u32,
    renderer_version: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LauncherAgent {
    capabilities_kind: String,
    context_kind: String,
    next_kind: String,
    capabilities_digest: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LauncherBinary {
    directory: String,
    unix_name: String,
    windows_name: String,
    targets: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LauncherCommands {
    capabilities: Vec<String>,
    doctor: Vec<String>,
    context: Vec<String>,
}

struct SourceTree {
    root: PathBuf,
    files: BTreeMap<String, Vec<u8>>,
    directories: BTreeSet<String>,
}

/// Successful proof that one plugin source matches the current Mino binary and assets.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PluginContractReport {
    /// Stable result schema identifier.
    pub kind: String,
    /// Canonical plugin source root.
    pub plugin_root: PathBuf,
    /// Exact plugin identifier.
    pub name: String,
    /// Exact Cargo and plugin semantic version.
    pub version: String,
    /// Exact protocol version and revision identity.
    pub protocol: String,
    /// Required Agent capabilities schema identifier.
    pub capabilities_kind: String,
    /// Digest of canonical machine capabilities JSON.
    pub capabilities_digest: String,
    /// Exact embedded standards package pins.
    pub standards: Vec<String>,
    /// Digest over canonical Skill paths and bytes.
    pub skill_digest: String,
    /// Digest over every canonical plugin source path and byte.
    pub source_digest: String,
    /// Number of regular files in the canonical plugin source.
    pub file_count: u64,
    /// Supported native target triples in stable order.
    pub targets: Vec<String>,
}

/// Validates the repository's canonical `plugins/mino` source tree.
///
/// # Errors
///
/// Returns a typed error for missing, unsafe, malformed, duplicated, extra,
/// version-drifted, protocol-drifted, capability-drifted, standards-drifted,
/// or byte-drifted plugin data.
pub fn validate_mino_plugin_source(
    repository_root: &Path,
) -> Result<PluginContractReport, MinoError> {
    validate_plugin_source(repository_root, &repository_root.join(PLUGIN_SOURCE_PATH))
}

/// Validates one Mino plugin source against authoritative assets in a repository.
///
/// This path-parameterized form supports isolated packaging and adversarial
/// contract tests without changing the canonical repository source.
///
/// # Errors
///
/// Returns the same typed errors as [`validate_mino_plugin_source`].
pub fn validate_plugin_source(
    repository_root: &Path,
    plugin_root: &Path,
) -> Result<PluginContractReport, MinoError> {
    let repository_root = canonical_directory(repository_root, "repository root")?;
    let plugin_tree = collect_source_tree(plugin_root, "plugin source")?;
    let skill_tree = collect_source_tree(
        &repository_root.join(CANONICAL_SKILL_PATH),
        "canonical Skill source",
    )?;
    validate_source_layout(&plugin_tree, &skill_tree)?;
    let manifest: PluginManifest = parse_json(
        plugin_tree
            .files
            .get(PLUGIN_MANIFEST_PATH)
            .ok_or_else(|| contract_error("Plugin manifest is missing"))?,
        "plugin manifest",
    )?;
    validate_manifest(&manifest, &plugin_tree.root)?;
    let launcher: LauncherDocument = parse_json(
        plugin_tree
            .files
            .get(LAUNCHER_PATH)
            .ok_or_else(|| contract_error("Plugin launcher metadata is missing"))?,
        "plugin launcher metadata",
    )?;
    let compatibility = validate_launcher(&launcher)?;
    validate_readme(
        plugin_tree
            .files
            .get(README_PATH)
            .ok_or_else(|| contract_error("Plugin README is missing"))?,
    )?;
    let skill_digest = source_digest(&skill_tree.files);
    let source_digest = source_digest(&plugin_tree.files);
    let file_count = u64::try_from(plugin_tree.files.len())
        .map_err(|_| contract_error("Plugin source file count does not fit into u64"))?;
    Ok(PluginContractReport {
        kind: MINO_PLUGIN_CONTRACT_KIND.to_owned(),
        plugin_root: plugin_tree.root,
        name: manifest.name,
        version: manifest.version,
        protocol: compatibility.protocol,
        capabilities_kind: compatibility.capabilities_kind,
        capabilities_digest: compatibility.capabilities_digest,
        standards: compatibility.standards,
        skill_digest,
        source_digest,
        file_count,
        targets: launcher.binary.targets,
    })
}

struct CompatibilityReport {
    protocol: String,
    capabilities_kind: String,
    capabilities_digest: String,
    standards: Vec<String>,
}

fn validate_manifest(manifest: &PluginManifest, plugin_root: &Path) -> Result<(), MinoError> {
    let directory_name = plugin_root
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| contract_error("Plugin source root has no UTF-8 directory name"))?;
    let version = Version::parse(&manifest.version)
        .map_err(|error| contract_error(format!("Plugin version is not SemVer: {error}")))?;
    let expected_keywords = [
        "coding-agent",
        "evidence",
        "git-flow",
        "planning",
        "standards",
    ];
    if directory_name != PLUGIN_NAME
        || manifest.name != PLUGIN_NAME
        || version.to_string() != manifest.version
        || manifest.version != env!("CARGO_PKG_VERSION")
        || manifest.description.trim().is_empty()
        || manifest.author.name != "Mino maintainers"
        || manifest.license != env!("CARGO_PKG_LICENSE")
        || manifest
            .keywords
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            != expected_keywords
        || manifest.skills != "./skills/"
        || manifest.interface.display_name != "Mino"
        || manifest.interface.short_description.trim().is_empty()
        || manifest.interface.long_description.trim().is_empty()
        || manifest.interface.developer_name != "Mino maintainers"
        || manifest.interface.category != "Developer Tools"
        || manifest
            .interface
            .capabilities
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            != EXPECTED_PLUGIN_CAPABILITIES
        || manifest.interface.default_prompt.is_empty()
        || manifest.interface.default_prompt.len() > 3
        || manifest
            .interface
            .default_prompt
            .iter()
            .any(|prompt| prompt.trim().is_empty() || prompt.chars().count() > 128)
    {
        return Err(contract_error(
            "Plugin manifest identity, version, metadata, capabilities, or prompts drifted",
        ));
    }
    Ok(())
}

fn validate_launcher(launcher: &LauncherDocument) -> Result<CompatibilityReport, MinoError> {
    let protocol_bundle = ProtocolRegistry::current().map_err(|error| {
        MinoError::new(
            ErrorCategory::EnvironmentUnavailable,
            format!("Failed to load embedded protocol for plugin validation: {error}"),
        )
    })?;
    let protocol = protocol_bundle.manifest();
    let capabilities = AgentService::capabilities();
    let capabilities_value = serde_json::to_value(&capabilities).map_err(|error| {
        environment_error(format!("Failed to normalize Agent capabilities: {error}"))
    })?;
    let capabilities_bytes = serde_json::to_vec(&capabilities_value).map_err(|error| {
        environment_error(format!("Failed to serialize Agent capabilities: {error}"))
    })?;
    let capabilities_digest = sha256_digest(&capabilities_bytes);
    let standards = EmbeddedCatalog::load()?
        .packages()
        .iter()
        .map(|package| format!("{}@{}", package.package_id(), package.version()))
        .collect::<Vec<_>>();
    let expected_protocol = format!(
        "{}.{}",
        protocol.protocol_version(),
        protocol.protocol_revision()
    );
    if launcher.kind != "mino.plugin-launcher/v1"
        || launcher.cli_version != env!("CARGO_PKG_VERSION")
        || launcher.protocol.identity != expected_protocol
        || launcher.protocol.version != protocol.protocol_version()
        || launcher.protocol.revision != protocol.protocol_revision()
        || launcher.protocol.schema_version != protocol.schema_version()
        || launcher.protocol.renderer_version != protocol.renderer_version()
        || launcher.agent.capabilities_kind != capabilities.kind
        || launcher.agent.context_kind != capabilities.context_kind
        || launcher.agent.next_kind != capabilities.next_kind
        || launcher.agent.capabilities_digest != capabilities_digest
        || launcher.standards != standards
        || launcher.binary.directory != "./bin"
        || launcher.binary.unix_name != "mino"
        || launcher.binary.windows_name != "mino.exe"
        || launcher
            .binary
            .targets
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            != EXPECTED_TARGETS
        || launcher.commands.capabilities
            != command(&["agent", "capabilities", "--format", "json", "--no-input"])
        || launcher.commands.doctor
            != command(&["project", "doctor", "--format", "json", "--no-input"])
        || launcher.commands.context
            != command(&["agent", "context", "--format", "json", "--no-input"])
        || launcher.missing_or_incompatible_exit_code
            != ErrorCategory::EnvironmentUnavailable.exit_code_value()
        || !launcher.offline
        || launcher.mutates_path
    {
        return Err(contract_error(
            "Plugin launcher version, protocol, Agent, standards, target, command, or safety contract drifted",
        ));
    }
    Ok(CompatibilityReport {
        protocol: expected_protocol,
        capabilities_kind: capabilities.kind.to_owned(),
        capabilities_digest,
        standards,
    })
}

fn command(arguments: &[&str]) -> Vec<String> {
    arguments
        .iter()
        .map(|argument| (*argument).to_owned())
        .collect()
}

fn validate_source_layout(
    plugin: &SourceTree,
    canonical_skill: &SourceTree,
) -> Result<(), MinoError> {
    let mut expected_files = BTreeSet::from([
        PLUGIN_MANIFEST_PATH.to_owned(),
        LAUNCHER_PATH.to_owned(),
        README_PATH.to_owned(),
    ]);
    let mut expected_directories = BTreeSet::from([
        ".codex-plugin".to_owned(),
        "skills".to_owned(),
        "skills/mino".to_owned(),
    ]);
    for path in canonical_skill.files.keys() {
        expected_files.insert(format!("skills/mino/{path}"));
    }
    for path in &canonical_skill.directories {
        expected_directories.insert(format!("skills/mino/{path}"));
    }
    if plugin.files.keys().cloned().collect::<BTreeSet<_>>() != expected_files
        || plugin.directories != expected_directories
    {
        return Err(contract_error(
            "Plugin source contains missing, duplicated, or unexpected files or directories",
        ));
    }
    for (path, expected) in &canonical_skill.files {
        let plugin_path = format!("skills/mino/{path}");
        if plugin.files.get(&plugin_path) != Some(expected) {
            return Err(contract_error(format!(
                "Plugin Skill asset {plugin_path} drifted from assets/skill/mino"
            )));
        }
    }
    Ok(())
}

fn validate_readme(bytes: &[u8]) -> Result<(), MinoError> {
    let readme = std::str::from_utf8(bytes)
        .map_err(|error| contract_error(format!("Plugin README is not UTF-8: {error}")))?;
    for required in [
        "exactly one `bin/mino` or `bin/mino.exe` binary",
        "must not mutate `PATH`",
        "does not publish, install, update, or",
    ] {
        if !readme.contains(required) {
            return Err(contract_error(format!(
                "Plugin README is missing required boundary text: {required}"
            )));
        }
    }
    Ok(())
}

fn collect_source_tree(path: &Path, label: &str) -> Result<SourceTree, MinoError> {
    let root = canonical_directory(path, label)?;
    let mut files = BTreeMap::new();
    let mut directories = BTreeSet::new();
    let mut total_bytes = 0_u64;
    collect_entries(&root, &root, &mut files, &mut directories, &mut total_bytes)?;
    if files.is_empty() || files.len() > MAX_SOURCE_FILES || total_bytes > MAX_SOURCE_BYTES {
        return Err(contract_error(format!(
            "{label} must contain between 1 and {MAX_SOURCE_FILES} files and no more than {MAX_SOURCE_BYTES} bytes"
        )));
    }
    Ok(SourceTree {
        root,
        files,
        directories,
    })
}

fn collect_entries(
    root: &Path,
    directory: &Path,
    files: &mut BTreeMap<String, Vec<u8>>,
    directories: &mut BTreeSet<String>,
    total_bytes: &mut u64,
) -> Result<(), MinoError> {
    for entry in fs::read_dir(directory)
        .map_err(|error| path_error("enumerate plugin contract path", directory, &error))?
    {
        let entry =
            entry.map_err(|error| path_error("read plugin contract entry", directory, &error))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| path_error("inspect plugin contract entry", &path, &error))?;
        if file_type.is_symlink() {
            return Err(policy_error(format!(
                "Plugin contract path {} must not be a symbolic link",
                path.display()
            )));
        }
        let relative = relative_path(root, &path)?;
        if file_type.is_dir() {
            directories.insert(relative);
            collect_entries(root, &path, files, directories, total_bytes)?;
        } else if file_type.is_file() {
            let metadata = entry
                .metadata()
                .map_err(|error| path_error("inspect plugin contract file", &path, &error))?;
            #[cfg(unix)]
            if metadata.permissions().mode() & 0o111 != 0 {
                return Err(policy_error(format!(
                    "Plugin source data file {} must not be executable",
                    path.display()
                )));
            }
            *total_bytes = total_bytes
                .checked_add(metadata.len())
                .ok_or_else(|| contract_error("Plugin source byte count overflowed"))?;
            if *total_bytes > MAX_SOURCE_BYTES {
                return Err(contract_error(format!(
                    "Plugin source exceeds {MAX_SOURCE_BYTES} bytes"
                )));
            }
            let bytes = fs::read(&path)
                .map_err(|error| path_error("read plugin contract file", &path, &error))?;
            if u64::try_from(bytes.len()).ok() != Some(metadata.len()) {
                return Err(MinoError::new(
                    ErrorCategory::DriftDetected,
                    format!(
                        "Plugin contract file {} changed while reading",
                        path.display()
                    ),
                ));
            }
            files.insert(relative, bytes);
        } else {
            return Err(policy_error(format!(
                "Plugin contract path {} has an unsupported type",
                path.display()
            )));
        }
    }
    Ok(())
}

fn canonical_directory(path: &Path, label: &str) -> Result<PathBuf, MinoError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| path_error(&format!("inspect {label}"), path, &error))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(policy_error(format!(
            "{label} {} must be a real directory",
            path.display()
        )));
    }
    fs::canonicalize(path).map_err(|error| path_error(&format!("resolve {label}"), path, &error))
}

fn relative_path(root: &Path, path: &Path) -> Result<String, MinoError> {
    let relative = path.strip_prefix(root).map_err(|_| {
        policy_error(format!(
            "Plugin contract path {} escaped {}",
            path.display(),
            root.display()
        ))
    })?;
    let mut segments = Vec::new();
    for component in relative.components() {
        let Component::Normal(segment) = component else {
            return Err(policy_error(format!(
                "Plugin contract path {} is not normal",
                path.display()
            )));
        };
        segments.push(segment.to_str().ok_or_else(|| {
            policy_error(format!(
                "Plugin contract path {} is not UTF-8",
                path.display()
            ))
        })?);
    }
    let rendered = segments.join("/");
    if rendered.is_empty() || rendered.len() > MAX_RELATIVE_PATH_BYTES {
        return Err(policy_error(format!(
            "Plugin contract relative path {rendered:?} is empty or exceeds {MAX_RELATIVE_PATH_BYTES} bytes"
        )));
    }
    Ok(rendered)
}

fn source_digest(files: &BTreeMap<String, Vec<u8>>) -> String {
    let mut digest_input = Vec::new();
    for (path, bytes) in files {
        digest_input.extend_from_slice(path.as_bytes());
        digest_input.push(0);
        digest_input.extend_from_slice(bytes);
        digest_input.push(0);
    }
    sha256_digest(&digest_input)
}

fn parse_json<T: for<'de> Deserialize<'de>>(bytes: &[u8], label: &str) -> Result<T, MinoError> {
    serde_json::from_slice(bytes)
        .map_err(|error| contract_error(format!("Failed to parse {label}: {error}")))
}

fn path_error(action: &str, path: &Path, error: &std::io::Error) -> MinoError {
    environment_error(format!("Failed to {action} {}: {error}", path.display()))
}

fn contract_error(message: impl Into<String>) -> MinoError {
    MinoError::new(ErrorCategory::DriftDetected, message)
}

fn policy_error(message: impl Into<String>) -> MinoError {
    MinoError::new(ErrorCategory::PolicyViolation, message)
}

fn environment_error(message: impl Into<String>) -> MinoError {
    MinoError::new(ErrorCategory::EnvironmentUnavailable, message)
}
