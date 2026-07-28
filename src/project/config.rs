//! Project configuration, protocol locks, standards locks, and layout paths.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::domain::{CURRENT_PROTOCOL_REVISION, CURRENT_PROTOCOL_VERSION, CURRENT_SCHEMA_VERSION};
use crate::managed_fs::{ManagedFsError, ManagedFsErrorKind, ManagedPath, ProjectFs};
use crate::render::RENDERER_VERSION;
use crate::{ErrorCategory, MinoError};

static NEXT_INITIALIZATION_FILE: AtomicU64 = AtomicU64::new(1);
const MAX_CONFIG_OR_LOCK_BYTES: u64 = 1_024 * 1_024;

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

    /// Returns the project-level selected plan and alternatives path.
    #[must_use]
    pub fn plan_selection(&self) -> PathBuf {
        self.mino_directory().join("plan-selection.json")
    }

    /// Returns the planning-authority decision path.
    #[must_use]
    pub fn authority(&self) -> PathBuf {
        self.mino_directory().join("authority.json")
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

    /// Returns the recoverable repository-integration transaction directory.
    #[must_use]
    pub fn integration_transactions(&self) -> PathBuf {
        self.mino_directory().join("integration-transactions")
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

    pub(crate) fn mino_managed() -> ManagedPath {
        managed_path(".mino")
    }

    pub(crate) fn config_managed() -> ManagedPath {
        managed_path(".mino/config.toml")
    }

    pub(crate) fn protocol_lock_managed() -> ManagedPath {
        managed_path(".mino/protocol.lock")
    }

    pub(crate) fn standards_lock_managed() -> ManagedPath {
        managed_path(".mino/standards.lock")
    }

    pub(crate) fn active_bindings_managed() -> ManagedPath {
        managed_path(".mino/active.json")
    }

    pub(crate) fn plan_selection_managed() -> ManagedPath {
        managed_path(".mino/plan-selection.json")
    }

    pub(crate) fn authority_managed() -> ManagedPath {
        managed_path(".mino/authority.json")
    }

    pub(crate) fn plans_directory_managed() -> ManagedPath {
        managed_path(".mino/plans")
    }

    pub(crate) fn standards_cache_managed() -> ManagedPath {
        managed_path(".mino/cache/standards")
    }

    pub(crate) fn projection_directory_managed() -> ManagedPath {
        managed_path("docs/plan")
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

pub(crate) fn parse_managed_toml<T>(
    filesystem: &ProjectFs,
    path: &ManagedPath,
) -> Result<T, MinoError>
where
    T: for<'de> Deserialize<'de>,
{
    let bytes = filesystem
        .read_bounded(path, MAX_CONFIG_OR_LOCK_BYTES)
        .map_err(map_managed_error)?;
    let contents = std::str::from_utf8(&bytes).map_err(|error| {
        MinoError::new(
            ErrorCategory::IncompleteOrValidation,
            format!(
                "Failed to decode {} as UTF-8: {error}",
                filesystem.display_path(path).display()
            ),
        )
    })?;
    toml::from_str(contents).map_err(|error| {
        MinoError::new(
            ErrorCategory::IncompleteOrValidation,
            format!(
                "Failed to parse {}: {error}",
                filesystem.display_path(path).display()
            ),
        )
    })
}

pub(crate) fn create_file(
    filesystem: &ProjectFs,
    path: &ManagedPath,
    bytes: &[u8],
) -> Result<(), MinoError> {
    let parent = path.parent();
    let file_name = path
        .as_path()
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            MinoError::new(
                ErrorCategory::EnvironmentUnavailable,
                format!(
                    "Mino-owned path {} has no UTF-8 file name",
                    filesystem.display_path(path).display()
                ),
            )
        })?;
    if let Some(parent) = &parent {
        filesystem
            .ensure_directory(parent)
            .map_err(map_managed_error)?;
    }
    let sequence = NEXT_INITIALIZATION_FILE.fetch_add(1, Ordering::Relaxed);
    let temporary_name = format!(
        ".{file_name}.mino-init-{}-{sequence}.tmp",
        std::process::id()
    );
    let temporary = match &parent {
        Some(parent) => parent.join(temporary_name).map_err(map_managed_error)?,
        None => ManagedPath::new(temporary_name).map_err(map_managed_error)?,
    };
    let mut file = filesystem
        .create_new_file(&temporary)
        .map_err(map_managed_error)?;
    if let Err(error) = file.write_all(bytes).and_then(|()| file.sync_all()) {
        drop(file);
        let _ = filesystem.remove_file_if_exists(&temporary);
        return Err(io_error(
            "write",
            &filesystem.display_path(&temporary),
            &error,
        ));
    }
    drop(file);
    if filesystem.exists(path).map_err(map_managed_error)? {
        let _ = filesystem.remove_file_if_exists(&temporary);
        return Err(MinoError::new(
            ErrorCategory::EnvironmentUnavailable,
            format!(
                "Mino-owned file {} appeared during initialization",
                filesystem.display_path(path).display()
            ),
        ));
    }
    if let Err(error) = filesystem.rename(&temporary, path) {
        let _ = filesystem.remove_file_if_exists(&temporary);
        return Err(map_managed_error(error));
    }
    filesystem.sync_parent(path).map_err(map_managed_error)
}

pub(crate) fn map_managed_error(error: ManagedFsError) -> MinoError {
    let category = match error.kind() {
        ManagedFsErrorKind::InvalidPath | ManagedFsErrorKind::UnsafeComponent => {
            ErrorCategory::DriftDetected
        }
        ManagedFsErrorKind::Io => ErrorCategory::EnvironmentUnavailable,
    };
    MinoError::new(category, error.into_message())
}

fn managed_path(path: &str) -> ManagedPath {
    ManagedPath::new(path).expect("static project layout path should be valid")
}

pub(crate) fn io_error(action: &str, path: &Path, error: &std::io::Error) -> MinoError {
    MinoError::new(
        ErrorCategory::EnvironmentUnavailable,
        format!("Failed to {action} {}: {error}", path.display()),
    )
}
