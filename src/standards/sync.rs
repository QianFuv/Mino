//! Explicit bounded catalog synchronization with digest verification and atomic activation.

use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use fs4::{FileExt, TryLockError};
use serde::{Deserialize, Serialize};

use crate::managed_fs::{
    ManagedEntryKind, ManagedFsError, ManagedFsErrorKind, ManagedPath, ProjectFs,
};
use crate::project::{LockedStandard, ProjectConfig, ProjectLayout, StandardsLock};
use crate::store::sha256_digest;
use crate::{ErrorCategory, MinoError, NextAction};

use super::catalog::{StandardsPackage, parse_package_documents, validate_package_set};

const CATALOG_VERSION: u32 = 1;
const MAX_CATALOG_PACKAGES: usize = 128;
const SYNC_LOCK_TIMEOUT: Duration = Duration::from_secs(2);
const SYNC_LOCK_RETRY_INTERVAL: Duration = Duration::from_millis(10);
static NEXT_STAGING_DIRECTORY: AtomicU64 = AtomicU64::new(1);

/// URL policy used by an explicit standards synchronization request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourcePolicy {
    /// Permit only HTTPS catalog and package document URLs.
    HttpsOnly,
    /// Permit HTTPS and loopback-only HTTP URLs for deterministic local tests.
    HttpsOrLoopbackHttp,
}

/// Resource and elapsed-time limits for one complete catalog synchronization.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SyncLimits {
    timeout: Duration,
    max_catalog_bytes: usize,
    max_document_bytes: usize,
    max_total_bytes: usize,
}

impl SyncLimits {
    /// Creates a positive, internally consistent synchronization limit set.
    ///
    /// # Errors
    ///
    /// Returns a validation error when a timeout or byte limit is zero, or
    /// when either per-resource limit exceeds the complete-request limit.
    pub fn new(
        timeout: Duration,
        max_catalog_bytes: usize,
        max_document_bytes: usize,
        max_total_bytes: usize,
    ) -> Result<Self, MinoError> {
        if timeout.is_zero()
            || max_catalog_bytes == 0
            || max_document_bytes == 0
            || max_total_bytes == 0
            || max_catalog_bytes > max_total_bytes
            || max_document_bytes > max_total_bytes
        {
            return Err(MinoError::new(
                ErrorCategory::IncompleteOrValidation,
                "Synchronization limits must be positive and fit within the total byte limit",
            ));
        }
        Ok(Self {
            timeout,
            max_catalog_bytes,
            max_document_bytes,
            max_total_bytes,
        })
    }

    /// Returns the end-to-end synchronization timeout.
    #[must_use]
    pub const fn timeout(self) -> Duration {
        self.timeout
    }

    /// Returns the maximum catalog document size.
    #[must_use]
    pub const fn max_catalog_bytes(self) -> usize {
        self.max_catalog_bytes
    }

    /// Returns the maximum size of one package document.
    #[must_use]
    pub const fn max_document_bytes(self) -> usize {
        self.max_document_bytes
    }

    /// Returns the maximum total number of downloaded bytes.
    #[must_use]
    pub const fn max_total_bytes(self) -> usize {
        self.max_total_bytes
    }
}

impl Default for SyncLimits {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            max_catalog_bytes: 1024 * 1024,
            max_document_bytes: 1024 * 1024,
            max_total_bytes: 16 * 1024 * 1024,
        }
    }
}

/// Complete policy for one explicit synchronization request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SyncOptions {
    limits: SyncLimits,
    source_policy: SourcePolicy,
}

impl SyncOptions {
    /// Creates synchronization options from resource limits and a URL policy.
    #[must_use]
    pub const fn new(limits: SyncLimits, source_policy: SourcePolicy) -> Self {
        Self {
            limits,
            source_policy,
        }
    }

    /// Returns the configured resource limits.
    #[must_use]
    pub const fn limits(self) -> SyncLimits {
        self.limits
    }

    /// Returns the configured source policy.
    #[must_use]
    pub const fn source_policy(self) -> SourcePolicy {
        self.source_policy
    }
}

impl Default for SyncOptions {
    fn default() -> Self {
        Self {
            limits: SyncLimits::default(),
            source_policy: SourcePolicy::HttpsOnly,
        }
    }
}

/// One exact remote package installed into an immutable cache generation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SyncedPackage {
    /// Stable package identifier.
    pub package_id: String,
    /// Exact package version.
    pub version: String,
    /// Verified aggregate SHA-256 digest.
    pub digest: String,
}

/// Successful catalog synchronization result.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct StandardsSyncReport {
    /// Discovered project root.
    pub root: PathBuf,
    /// Explicit catalog URL read from project configuration.
    pub catalog_url: String,
    /// SHA-256 digest of the exact catalog document bytes.
    pub catalog_digest: String,
    /// Immutable cache generation activated by the standards lock.
    pub generation: PathBuf,
    /// Every catalog package in stable package-ID order.
    pub packages: Vec<SyncedPackage>,
    /// Whether an identical verified cache generation already existed.
    pub reused_generation: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogDocument {
    catalog_version: u32,
    packages: Vec<CatalogPackage>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogPackage {
    package_id: String,
    version: String,
    digest: String,
    manifest_url: String,
    rules_url: String,
    checks_url: String,
}

struct DownloadedPackage {
    package: StandardsPackage,
    manifest: Vec<u8>,
    rules: Vec<u8>,
    checks: Vec<u8>,
}

struct FetchSession {
    started_at: Instant,
    options: SyncOptions,
    downloaded_bytes: usize,
    agent: ureq::Agent,
}

impl FetchSession {
    fn new(options: SyncOptions) -> Self {
        let agent = ureq::Agent::config_builder()
            .https_only(options.source_policy == SourcePolicy::HttpsOnly)
            .max_redirects(0)
            .build()
            .into();
        Self {
            started_at: Instant::now(),
            options,
            downloaded_bytes: 0,
            agent,
        }
    }

    fn fetch(&mut self, url: &str, max_bytes: usize) -> Result<Vec<u8>, MinoError> {
        validate_url(url, self.options.source_policy)?;
        let remaining = self
            .options
            .limits
            .timeout
            .checked_sub(self.started_at.elapsed())
            .filter(|remaining| !remaining.is_zero())
            .ok_or_else(|| sync_error("Catalog synchronization timed out"))?;
        let mut response = self
            .agent
            .get(url)
            .config()
            .timeout_global(Some(remaining))
            .build()
            .call()
            .map_err(|error| sync_error(format!("Failed to fetch {url}: {error}")))?;
        let read_limit = u64::try_from(max_bytes)
            .map_err(|_| sync_error("Response byte limit does not fit into u64"))?;
        let bytes = response
            .body_mut()
            .with_config()
            .limit(read_limit)
            .read_to_vec()
            .map_err(|error| {
                sync_error(format!(
                    "Failed to read {url} within the {max_bytes}-byte limit: {error}"
                ))
            })?;
        let next_total = self
            .downloaded_bytes
            .checked_add(bytes.len())
            .ok_or_else(|| sync_error("Downloaded byte count overflowed"))?;
        if next_total > self.options.limits.max_total_bytes {
            return Err(sync_error(format!(
                "Catalog synchronization exceeded the {}-byte total limit",
                self.options.limits.max_total_bytes
            )));
        }
        self.downloaded_bytes = next_total;
        Ok(bytes)
    }
}

struct SyncLock {
    file: std::fs::File,
}

impl SyncLock {
    fn acquire(filesystem: &ProjectFs, path: &ManagedPath) -> Result<Self, MinoError> {
        let file = filesystem
            .open_lock_file(path)
            .map_err(managed_sync_error)?;
        let started_at = Instant::now();
        loop {
            match FileExt::try_lock(&file) {
                Ok(()) => return Ok(Self { file }),
                Err(TryLockError::WouldBlock) if started_at.elapsed() < SYNC_LOCK_TIMEOUT => {
                    let remaining = SYNC_LOCK_TIMEOUT.saturating_sub(started_at.elapsed());
                    thread::sleep(SYNC_LOCK_RETRY_INTERVAL.min(remaining));
                }
                Err(TryLockError::WouldBlock) => {
                    return Err(sync_error(format!(
                        "Timed out acquiring synchronization lock {}",
                        filesystem.display_path(path).display()
                    )));
                }
                Err(TryLockError::Error(error)) => {
                    return Err(path_error(
                        "acquire synchronization lock",
                        &filesystem.display_path(path),
                        &error,
                    ));
                }
            }
        }
    }
}

impl Drop for SyncLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

struct StagingDirectory {
    filesystem: ProjectFs,
    path: ManagedPath,
    should_remove: bool,
}

impl StagingDirectory {
    fn create(filesystem: &ProjectFs, parent: &ManagedPath) -> Result<Self, MinoError> {
        for _ in 0..100 {
            let sequence = NEXT_STAGING_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let path = parent
                .join(format!(".staging-{}-{sequence}", std::process::id()))
                .map_err(managed_sync_error)?;
            match filesystem.create_directory(&path) {
                Ok(()) => {
                    return Ok(Self {
                        filesystem: filesystem.clone(),
                        path,
                        should_remove: true,
                    });
                }
                Err(error)
                    if error.kind() == ManagedFsErrorKind::Io
                        && error.to_string().contains("already exists") => {}
                Err(error) => return Err(managed_sync_error(error)),
            }
        }
        Err(sync_error(
            "Could not allocate a unique standards staging directory",
        ))
    }

    fn mark_published(&mut self) {
        self.should_remove = false;
    }
}

impl Drop for StagingDirectory {
    fn drop(&mut self) {
        if self.should_remove
            && self
                .path
                .as_path()
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with(".staging-"))
        {
            let _ = self.filesystem.remove_directory_all(&self.path);
        }
    }
}

/// Downloads, validates, caches, and activates every configured catalog package.
///
/// Network access occurs only when this function is called. The default policy
/// requires HTTPS and never follows redirects or executes downloaded content.
///
/// # Errors
///
/// Returns an environment-unavailable error for missing configuration, network
/// failures, limit violations, malformed data, digest mismatch, or publication
/// failures. The previously active cache and lock remain unchanged on failure.
pub fn synchronize_all(start: &Path) -> Result<StandardsSyncReport, MinoError> {
    synchronize_all_with_options(start, SyncOptions::default())
}

/// Synchronizes every catalog package under an explicit source and limit policy.
///
/// `HttpsOrLoopbackHttp` exists for deterministic local-server verification and
/// accepts HTTP only when the authority is exactly localhost or a loopback IP.
///
/// # Errors
///
/// Returns an environment-unavailable error under the same conditions as
/// [`synchronize_all`].
pub fn synchronize_all_with_options(
    start: &Path,
    options: SyncOptions,
) -> Result<StandardsSyncReport, MinoError> {
    let project_root = crate::project::discover(start)?;
    let layout = ProjectLayout::new(project_root.path());
    let filesystem = ProjectFs::open(layout.root()).map_err(managed_sync_error)?;
    let catalog_url = load_catalog_url(&layout, &filesystem)?;
    validate_url(&catalog_url, options.source_policy)?;
    let mut fetch_session = FetchSession::new(options);
    let catalog_bytes = fetch_session.fetch(&catalog_url, options.limits.max_catalog_bytes)?;
    let mut catalog = parse_catalog(&catalog_bytes)?;
    validate_catalog(&mut catalog, options.source_policy)?;
    let mut downloaded = Vec::with_capacity(catalog.packages.len());
    for entry in &catalog.packages {
        downloaded.push(download_package(entry, &mut fetch_session)?);
    }
    let packages = downloaded
        .iter()
        .map(|download| download.package.clone())
        .collect::<Vec<_>>();
    validate_package_set(&packages).map_err(|error| {
        sync_error(format!(
            "Downloaded catalog has conflicting identifiers: {error}"
        ))
    })?;
    let catalog_digest = sha256_digest(&catalog_bytes);
    let sync_lock_path = managed_path(".mino/standards-sync.lock");
    let sync_lock = SyncLock::acquire(&filesystem, &sync_lock_path)?;
    let current_url = load_catalog_url(&layout, &filesystem)?;
    if current_url != catalog_url {
        return Err(sync_error(
            "Catalog configuration changed while synchronization was in progress",
        ));
    }
    let (generation, reused_generation) =
        publish_generation(&filesystem, &catalog_digest, &catalog_bytes, &downloaded)?;
    let lock = standards_lock(&catalog_digest, &downloaded);
    replace_lock(&filesystem, &ProjectLayout::standards_lock_managed(), &lock)?;
    drop(sync_lock);
    let packages = downloaded
        .into_iter()
        .map(|download| SyncedPackage {
            package_id: download.package.package_id().to_owned(),
            version: download.package.version().to_owned(),
            digest: download.package.digest().to_owned(),
        })
        .collect();
    Ok(StandardsSyncReport {
        root: layout.root().to_path_buf(),
        catalog_url,
        catalog_digest,
        generation,
        packages,
        reused_generation,
    })
}

fn load_catalog_url(layout: &ProjectLayout, filesystem: &ProjectFs) -> Result<String, MinoError> {
    let path = layout.config();
    let bytes = filesystem
        .read(&ProjectLayout::config_managed())
        .map_err(managed_sync_error)?;
    let contents = std::str::from_utf8(&bytes).map_err(|error| {
        sync_error(format!(
            "Project configuration {} is not UTF-8: {error}",
            path.display()
        ))
    })?;
    let config: ProjectConfig = toml::from_str(contents).map_err(|error| {
        sync_error(format!(
            "Failed to parse project configuration {}: {error}",
            path.display()
        ))
    })?;
    if !config.is_supported() {
        return Err(sync_error(format!(
            "Project configuration {} is unsupported",
            path.display()
        )));
    }
    config
        .catalog
        .url
        .filter(|url| !url.trim().is_empty())
        .ok_or_else(missing_catalog_url_error)
}

fn missing_catalog_url_error() -> MinoError {
    sync_error("Catalog synchronization requires catalog.url in .mino/config.toml")
        .with_remediation(
            vec!["catalog.url".to_owned()],
            vec![NextAction {
                id: "project.show".to_owned(),
                argv: vec![
                    "mino".to_owned(),
                    "project".to_owned(),
                    "show".to_owned(),
                    "--format".to_owned(),
                    "json".to_owned(),
                    "--no-input".to_owned(),
                ],
            }],
        )
}

fn parse_catalog(bytes: &[u8]) -> Result<CatalogDocument, MinoError> {
    let source = std::str::from_utf8(bytes)
        .map_err(|error| sync_error(format!("Catalog is not UTF-8: {error}")))?;
    toml::from_str(source)
        .map_err(|error| sync_error(format!("Failed to parse catalog TOML: {error}")))
}

fn validate_catalog(
    catalog: &mut CatalogDocument,
    source_policy: SourcePolicy,
) -> Result<(), MinoError> {
    if catalog.catalog_version != CATALOG_VERSION {
        return Err(sync_error(format!(
            "Catalog version {} is unsupported",
            catalog.catalog_version
        )));
    }
    if catalog.packages.is_empty() || catalog.packages.len() > MAX_CATALOG_PACKAGES {
        return Err(sync_error(format!(
            "Catalog must contain between 1 and {MAX_CATALOG_PACKAGES} packages"
        )));
    }
    catalog
        .packages
        .sort_by(|left, right| left.package_id.cmp(&right.package_id));
    let mut package_ids = BTreeSet::new();
    for package in &catalog.packages {
        if !is_safe_package_id(&package.package_id)
            || !is_safe_version(&package.version)
            || !is_sha256_digest(&package.digest)
            || !package_ids.insert(package.package_id.as_str())
        {
            return Err(sync_error(format!(
                "Catalog package {} has an invalid or duplicate identity, version, or digest",
                package.package_id
            )));
        }
        for url in [
            &package.manifest_url,
            &package.rules_url,
            &package.checks_url,
        ] {
            validate_url(url, source_policy)?;
        }
    }
    Ok(())
}

fn download_package(
    entry: &CatalogPackage,
    fetch_session: &mut FetchSession,
) -> Result<DownloadedPackage, MinoError> {
    let max_document_bytes = fetch_session.options.limits.max_document_bytes;
    let manifest = fetch_session.fetch(&entry.manifest_url, max_document_bytes)?;
    let rules = fetch_session.fetch(&entry.rules_url, max_document_bytes)?;
    let checks = fetch_session.fetch(&entry.checks_url, max_document_bytes)?;
    let manifest_source = normalize_document(&entry.package_id, "manifest", &manifest)?;
    let rules_source = normalize_document(&entry.package_id, "rules", &rules)?;
    let checks_source = normalize_document(&entry.package_id, "checks", &checks)?;
    let package = parse_package_documents(
        &entry.package_id,
        &manifest_source,
        &rules_source,
        &checks_source,
    )
    .map_err(|error| {
        sync_error(format!(
            "Downloaded package {} is invalid: {error}",
            entry.package_id
        ))
    })?;
    if package.version() != entry.version || package.digest() != entry.digest {
        return Err(sync_error(format!(
            "Digest or version mismatch for package {}: expected {} {}, got {} {}",
            entry.package_id,
            entry.version,
            entry.digest,
            package.version(),
            package.digest()
        )));
    }
    Ok(DownloadedPackage {
        package,
        manifest: manifest_source.into_bytes(),
        rules: rules_source.into_bytes(),
        checks: checks_source.into_bytes(),
    })
}

fn normalize_document(package_id: &str, document: &str, bytes: &[u8]) -> Result<String, MinoError> {
    let source = std::str::from_utf8(bytes).map_err(|error| {
        sync_error(format!(
            "Downloaded {package_id}/{document}.toml is not UTF-8: {error}"
        ))
    })?;
    Ok(source.replace("\r\n", "\n").replace('\r', "\n"))
}

fn publish_generation(
    filesystem: &ProjectFs,
    catalog_digest: &str,
    catalog_bytes: &[u8],
    packages: &[DownloadedPackage],
) -> Result<(PathBuf, bool), MinoError> {
    let cache_root = ProjectLayout::standards_cache_managed();
    let generations = cache_root
        .join("generations")
        .expect("static generations directory should form a managed path");
    filesystem
        .ensure_directory(&generations)
        .map_err(managed_sync_error)?;
    let mut staging = StagingDirectory::create(filesystem, &generations)?;
    write_generation(filesystem, &staging.path, catalog_bytes, packages)?;
    verify_generation(filesystem, &staging.path, catalog_bytes, packages)?;
    filesystem
        .sync_directory(Some(&staging.path))
        .map_err(managed_sync_error)?;
    let generation = generations
        .join(digest_segment(catalog_digest)?)
        .map_err(managed_sync_error)?;
    let reused_generation = if filesystem.exists(&generation).map_err(managed_sync_error)? {
        verify_generation(filesystem, &generation, catalog_bytes, packages)?;
        true
    } else {
        filesystem
            .rename(&staging.path, &generation)
            .map_err(managed_sync_error)?;
        staging.mark_published();
        filesystem
            .sync_directory(Some(&generations))
            .map_err(managed_sync_error)?;
        false
    };
    Ok((filesystem.display_path(&generation), reused_generation))
}

fn write_generation(
    filesystem: &ProjectFs,
    root: &ManagedPath,
    catalog_bytes: &[u8],
    packages: &[DownloadedPackage],
) -> Result<(), MinoError> {
    write_new_file(
        filesystem,
        &root
            .join("catalog.toml")
            .expect("static catalog file name should form a managed path"),
        catalog_bytes,
    )?;
    for download in packages {
        let directory = root
            .join("packages")
            .and_then(|path| path.join(download.package.package_id()))
            .and_then(|path| path.join(download.package.version()))
            .map_err(managed_sync_error)?;
        filesystem
            .ensure_directory(&directory)
            .map_err(managed_sync_error)?;
        for (name, bytes) in [
            ("manifest.toml", download.manifest.as_slice()),
            ("rules.toml", download.rules.as_slice()),
            ("checks.toml", download.checks.as_slice()),
        ] {
            let path = directory.join(name).map_err(managed_sync_error)?;
            write_new_file(filesystem, &path, bytes)?;
        }
        filesystem
            .sync_directory(Some(&directory))
            .map_err(managed_sync_error)?;
    }
    Ok(())
}

fn verify_generation(
    filesystem: &ProjectFs,
    root: &ManagedPath,
    catalog_bytes: &[u8],
    packages: &[DownloadedPackage],
) -> Result<(), MinoError> {
    let mut expected_paths = BTreeSet::from([PathBuf::from("catalog.toml")]);
    verify_cached_file(filesystem, root, Path::new("catalog.toml"), catalog_bytes)?;
    for download in packages {
        let base = PathBuf::from("packages")
            .join(download.package.package_id())
            .join(download.package.version());
        for (name, bytes) in [
            ("manifest.toml", download.manifest.as_slice()),
            ("rules.toml", download.rules.as_slice()),
            ("checks.toml", download.checks.as_slice()),
        ] {
            let relative = base.join(name);
            verify_cached_file(filesystem, root, &relative, bytes)?;
            expected_paths.insert(relative);
        }
    }
    let mut actual_paths = BTreeSet::new();
    collect_files(filesystem, root, root, &mut actual_paths)?;
    if actual_paths != expected_paths {
        return Err(sync_error(format!(
            "Cache generation {} contains missing or unexpected files",
            filesystem.display_path(root).display()
        )));
    }
    Ok(())
}

fn verify_cached_file(
    filesystem: &ProjectFs,
    root: &ManagedPath,
    relative: &Path,
    expected: &[u8],
) -> Result<(), MinoError> {
    let path = root.join(relative).map_err(managed_sync_error)?;
    let actual = filesystem.read(&path).map_err(managed_sync_error)?;
    if actual != expected {
        return Err(sync_error(format!(
            "Cached document {} differs from verified downloaded bytes",
            filesystem.display_path(&path).display()
        )));
    }
    Ok(())
}

fn collect_files(
    filesystem: &ProjectFs,
    root: &ManagedPath,
    directory: &ManagedPath,
    files: &mut BTreeSet<PathBuf>,
) -> Result<(), MinoError> {
    for entry in filesystem
        .read_directory(directory)
        .map_err(managed_sync_error)?
    {
        let path = directory.join(&entry.name).map_err(managed_sync_error)?;
        if entry.kind == ManagedEntryKind::Symlink {
            return Err(sync_error(format!(
                "Cache generation {} contains a symbolic link",
                filesystem.display_path(&path).display()
            )));
        }
        if entry.kind == ManagedEntryKind::Directory {
            collect_files(filesystem, root, &path, files)?;
        } else if entry.kind == ManagedEntryKind::File {
            let relative = path
                .as_path()
                .strip_prefix(root.as_path())
                .map_err(|error| {
                    sync_error(format!(
                        "Cache path {} escaped its generation: {error}",
                        filesystem.display_path(&path).display()
                    ))
                })?;
            files.insert(relative.to_path_buf());
        } else {
            return Err(sync_error(format!(
                "Cache generation {} contains an unsupported entry",
                filesystem.display_path(&path).display()
            )));
        }
    }
    Ok(())
}

fn standards_lock(catalog_digest: &str, packages: &[DownloadedPackage]) -> StandardsLock {
    StandardsLock {
        lock_version: StandardsLock::default().lock_version,
        catalog_digest: Some(catalog_digest.to_owned()),
        packages: packages
            .iter()
            .map(|download| LockedStandard {
                package_id: download.package.package_id().to_owned(),
                version: download.package.version().to_owned(),
                digest: download.package.digest().to_owned(),
            })
            .collect(),
    }
}

fn replace_lock(
    filesystem: &ProjectFs,
    path: &ManagedPath,
    lock: &StandardsLock,
) -> Result<(), MinoError> {
    let bytes = serialize_lock(lock)?;
    let parent = path.parent().ok_or_else(|| {
        sync_error(format!(
            "Standards lock path {} has no parent directory",
            filesystem.display_path(path).display()
        ))
    })?;
    let sequence = NEXT_STAGING_DIRECTORY.fetch_add(1, Ordering::Relaxed);
    let temporary = parent
        .join(format!(
            ".standards.lock.sync-{}-{sequence}.tmp",
            std::process::id()
        ))
        .map_err(managed_sync_error)?;
    write_new_file(filesystem, &temporary, &bytes)?;
    if let Err(error) = filesystem.rename(&temporary, path) {
        let _ = filesystem.remove_file_if_exists(&temporary);
        return Err(managed_sync_error(error));
    }
    filesystem.sync_parent(path).map_err(managed_sync_error)
}

fn serialize_lock(lock: &StandardsLock) -> Result<Vec<u8>, MinoError> {
    let mut rendered = toml::to_string_pretty(lock)
        .map_err(|error| sync_error(format!("Failed to serialize standards lock: {error}")))?;
    rendered = rendered.replace("\r\n", "\n").replace('\r', "\n");
    if !rendered.ends_with('\n') {
        rendered.push('\n');
    }
    Ok(rendered.into_bytes())
}

fn write_new_file(
    filesystem: &ProjectFs,
    path: &ManagedPath,
    bytes: &[u8],
) -> Result<(), MinoError> {
    let mut file = filesystem
        .create_new_file(path)
        .map_err(managed_sync_error)?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| path_error("write cache file", &filesystem.display_path(path), &error))
}

fn digest_segment(digest: &str) -> Result<&str, MinoError> {
    if is_sha256_digest(digest) {
        Ok(&digest["sha256:".len()..])
    } else {
        Err(sync_error(
            "Catalog digest is not a canonical SHA-256 value",
        ))
    }
}

pub(super) fn validate_url(url: &str, source_policy: SourcePolicy) -> Result<(), MinoError> {
    if let Some(remainder) = url.strip_prefix("https://")
        && !remainder.is_empty()
        && !url.chars().any(char::is_whitespace)
    {
        return Ok(());
    }
    if source_policy == SourcePolicy::HttpsOrLoopbackHttp && is_loopback_http(url) {
        return Ok(());
    }
    Err(sync_error(format!(
        "Catalog sources require HTTPS; only loopback HTTP is allowed by the local-test policy: {url}"
    )))
}

fn is_loopback_http(url: &str) -> bool {
    let Some(remainder) = url.strip_prefix("http://") else {
        return false;
    };
    if remainder.is_empty() || url.chars().any(char::is_whitespace) {
        return false;
    }
    let authority = remainder.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.is_empty() || authority.contains('@') {
        return false;
    }
    if let Some(bracketed) = authority.strip_prefix('[') {
        let Some((host, port)) = bracketed.split_once(']') else {
            return false;
        };
        return host == "::1" && valid_optional_port(port);
    }
    let (host, port) = authority
        .split_once(':')
        .map_or((authority, ""), |(host, port)| (host, port));
    matches!(
        host.to_ascii_lowercase().as_str(),
        "localhost" | "127.0.0.1"
    ) && (port.is_empty() || port.chars().all(|character| character.is_ascii_digit()))
}

fn valid_optional_port(port: &str) -> bool {
    port.is_empty()
        || port
            .strip_prefix(':')
            .is_some_and(|value| !value.is_empty() && value.chars().all(|c| c.is_ascii_digit()))
}

fn is_safe_package_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        && value
            .bytes()
            .next_back()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        && !value.contains("..")
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_' | b'.')
        })
}

fn is_safe_version(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value != "."
        && value != ".."
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && value
            .bytes()
            .next_back()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'+' | b'_'))
}

fn is_sha256_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    })
}

fn path_error(action: &str, path: &Path, error: &std::io::Error) -> MinoError {
    sync_error(format!("Failed to {action} {}: {error}", path.display()))
}

fn sync_error(message: impl Into<String>) -> MinoError {
    MinoError::new(ErrorCategory::EnvironmentUnavailable, message)
}

fn managed_path(path: &str) -> ManagedPath {
    ManagedPath::new(path).expect("static standards path should be valid")
}

fn managed_sync_error(error: ManagedFsError) -> MinoError {
    let category = match error.kind() {
        ManagedFsErrorKind::InvalidPath | ManagedFsErrorKind::UnsafeComponent => {
            ErrorCategory::DriftDetected
        }
        ManagedFsErrorKind::Io => ErrorCategory::EnvironmentUnavailable,
    };
    MinoError::new(category, error.into_message())
}
