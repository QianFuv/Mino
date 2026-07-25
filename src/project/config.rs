//! Project configuration, protocol locks, standards locks, and layout paths.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::domain::{CURRENT_PROTOCOL_REVISION, CURRENT_PROTOCOL_VERSION, CURRENT_SCHEMA_VERSION};
use crate::render::RENDERER_VERSION;
use crate::{ErrorCategory, MinoError};

static NEXT_INITIALIZATION_FILE: AtomicU64 = AtomicU64::new(1);

/// Current project configuration format version.
pub const PROJECT_CONFIG_VERSION: u32 = 1;
/// Current protocol-lock format version.
pub const PROTOCOL_LOCK_VERSION: u32 = 1;
/// Current standards-lock format version.
pub const STANDARDS_LOCK_VERSION: u32 = 1;

/// Deterministic paths owned by project initialization.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectLayout {
    root: PathBuf,
}

impl ProjectLayout {
    /// Creates layout paths rooted at a normalized project directory.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Returns the project root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Returns the Mino state directory.
    #[must_use]
    pub fn mino_directory(&self) -> PathBuf {
        self.root.join(".mino")
    }

    /// Returns the project configuration path.
    #[must_use]
    pub fn config(&self) -> PathBuf {
        self.mino_directory().join("config.toml")
    }

    /// Returns the pinned protocol lock path.
    #[must_use]
    pub fn protocol_lock(&self) -> PathBuf {
        self.mino_directory().join("protocol.lock")
    }

    /// Returns the selected standards lock path.
    #[must_use]
    pub fn standards_lock(&self) -> PathBuf {
        self.mino_directory().join("standards.lock")
    }

    /// Returns the worktree-aware active-plan binding path.
    #[must_use]
    pub fn active_bindings(&self) -> PathBuf {
        self.mino_directory().join("active.json")
    }

    /// Returns the plan-state directory.
    #[must_use]
    pub fn plans_directory(&self) -> PathBuf {
        self.mino_directory().join("plans")
    }

    /// Returns the standards cache directory.
    #[must_use]
    pub fn standards_cache(&self) -> PathBuf {
        self.mino_directory().join("cache").join("standards")
    }

    /// Returns the repository-level Mino Skill entry point.
    #[must_use]
    pub fn skill_file(&self) -> PathBuf {
        self.root.join(".agents/skills/mino/SKILL.md")
    }

    /// Returns the repository instruction file.
    #[must_use]
    pub fn agents_file(&self) -> PathBuf {
        self.root.join("AGENTS.md")
    }
}

/// Catalog synchronization configuration.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogConfig {
    /// Explicit catalog URL used only by `standards sync`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// Project-local Mino configuration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectConfig {
    /// Configuration format version.
    pub schema_version: u32,
    /// Root location relative to the configuration file.
    pub project_root: String,
    /// Explicit catalog synchronization settings.
    #[serde(default)]
    pub catalog: CatalogConfig,
}

impl Default for ProjectConfig {
    fn default() -> Self {
        Self {
            schema_version: PROJECT_CONFIG_VERSION,
            project_root: "..".to_owned(),
            catalog: CatalogConfig::default(),
        }
    }
}

impl ProjectConfig {
    /// Returns whether the configuration format and root semantics are supported.
    #[must_use]
    pub fn is_supported(&self) -> bool {
        self.schema_version == PROJECT_CONFIG_VERSION && self.project_root == ".."
    }
}

/// Pinned plan protocol and renderer compatibility values.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProtocolLock {
    /// Lock-file format version.
    pub lock_version: u32,
    /// Serialized plan schema version.
    pub schema_version: u32,
    /// Calendar protocol version.
    pub protocol_version: String,
    /// Named protocol revision.
    pub protocol_revision: String,
    /// Deterministic Markdown renderer version.
    pub renderer_version: u32,
}

impl Default for ProtocolLock {
    fn default() -> Self {
        Self {
            lock_version: PROTOCOL_LOCK_VERSION,
            schema_version: CURRENT_SCHEMA_VERSION,
            protocol_version: CURRENT_PROTOCOL_VERSION.to_owned(),
            protocol_revision: CURRENT_PROTOCOL_REVISION.to_owned(),
            renderer_version: RENDERER_VERSION,
        }
    }
}

/// One exact standards package pinned by a project.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockedStandard {
    /// Stable package identifier.
    pub package_id: String,
    /// Exact package version.
    pub version: String,
    /// SHA-256 digest of inert package bytes.
    pub digest: String,
}

/// Selected and synchronized standards package versions.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StandardsLock {
    /// Lock-file format version.
    pub lock_version: u32,
    /// Optional digest of the synchronized catalog manifest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_digest: Option<String>,
    /// Exact selected or cached packages in stable package-ID order.
    #[serde(default)]
    pub packages: Vec<LockedStandard>,
}

impl Default for StandardsLock {
    fn default() -> Self {
        Self {
            lock_version: STANDARDS_LOCK_VERSION,
            catalog_digest: None,
            packages: Vec::new(),
        }
    }
}

impl StandardsLock {
    /// Returns whether the lock format and package ordering are supported.
    #[must_use]
    pub fn is_supported(&self) -> bool {
        self.lock_version == STANDARDS_LOCK_VERSION
            && self
                .packages
                .windows(2)
                .all(|pair| pair[0].package_id < pair[1].package_id)
    }
}

pub(crate) fn serialize_toml<T: Serialize>(value: &T) -> Result<Vec<u8>, MinoError> {
    let mut rendered = toml::to_string_pretty(value).map_err(|error| {
        MinoError::new(
            ErrorCategory::EnvironmentUnavailable,
            format!("Failed to serialize Mino TOML: {error}"),
        )
    })?;
    rendered = rendered.replace("\r\n", "\n").replace('\r', "\n");
    if !rendered.ends_with('\n') {
        rendered.push('\n');
    }
    Ok(rendered.into_bytes())
}

pub(crate) fn parse_toml<T>(path: &Path) -> Result<T, MinoError>
where
    T: for<'de> Deserialize<'de>,
{
    let contents = fs::read_to_string(path).map_err(|error| io_error("read", path, &error))?;
    toml::from_str(&contents).map_err(|error| {
        MinoError::new(
            ErrorCategory::IncompleteOrValidation,
            format!("Failed to parse {}: {error}", path.display()),
        )
    })
}

pub(crate) fn create_file(path: &Path, bytes: &[u8]) -> Result<(), MinoError> {
    let parent = path.parent().ok_or_else(|| {
        MinoError::new(
            ErrorCategory::EnvironmentUnavailable,
            format!("Mino-owned path {} has no parent directory", path.display()),
        )
    })?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            MinoError::new(
                ErrorCategory::EnvironmentUnavailable,
                format!("Mino-owned path {} has no UTF-8 file name", path.display()),
            )
        })?;
    let sequence = NEXT_INITIALIZATION_FILE.fetch_add(1, Ordering::Relaxed);
    let temporary = parent.join(format!(
        ".{file_name}.mino-init-{}-{sequence}.tmp",
        std::process::id()
    ));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .map_err(|error| io_error("create", &temporary, &error))?;
    if let Err(error) = file.write_all(bytes).and_then(|()| file.sync_all()) {
        drop(file);
        let _ = fs::remove_file(&temporary);
        return Err(io_error("write", &temporary, &error));
    }
    drop(file);
    if path.exists() {
        let _ = fs::remove_file(&temporary);
        return Err(MinoError::new(
            ErrorCategory::EnvironmentUnavailable,
            format!(
                "Mino-owned file {} appeared during initialization",
                path.display()
            ),
        ));
    }
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(io_error("publish", path, &error));
    }
    sync_directory(parent)?;
    Ok(())
}

pub(crate) fn io_error(action: &str, path: &Path, error: &std::io::Error) -> MinoError {
    MinoError::new(
        ErrorCategory::EnvironmentUnavailable,
        format!("Failed to {action} {}: {error}", path.display()),
    )
}

#[cfg(unix)]
fn sync_directory(directory: &Path) -> Result<(), MinoError> {
    use std::fs::File;

    File::open(directory)
        .and_then(|file| file.sync_all())
        .map_err(|error| io_error("synchronize", directory, &error))
}

#[cfg(not(unix))]
fn sync_directory(directory: &Path) -> Result<(), MinoError> {
    fs::metadata(directory)
        .map(|_| ())
        .map_err(|error| io_error("inspect", directory, &error))
}
