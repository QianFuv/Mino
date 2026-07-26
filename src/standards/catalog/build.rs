//! Atomic initialization, validation, and publication for team catalogs.

use std::collections::{BTreeMap, BTreeSet};
#[cfg(unix)]
use std::fs::File;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use semver::Version;

use crate::standards::SourcePolicy;
use crate::store::sha256_digest;
use crate::{ErrorCategory, MinoError};

use super::source::{
    ARTIFACT_MANIFEST_FILE_NAME, STATIC_CATALOG_VERSION, StaticCatalogDocument,
    TEAM_CATALOG_BUILD_KIND, TEAM_CATALOG_INIT_KIND, TEAM_CATALOG_MANIFEST_KIND,
    TeamCatalogArtifactManifest, TeamCatalogBuildReport, TeamCatalogInitReport,
    TeamCatalogValidationReport,
};
use super::validate::{
    prepare_team_catalog, relative_path_string, source_document_bytes, tree_digest,
    validate_base_url, validate_local_id, validate_member_ids, validate_namespace,
};
use super::{canonical_package_documents, parse_package_documents};

const STAGING_PREFIX: &str = ".mino-catalog-staging-";
const BACKUP_PREFIX: &str = ".mino-catalog-backup-";
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(1);

struct TemporaryDirectory {
    parent: PathBuf,
    path: PathBuf,
    should_remove: bool,
}

impl TemporaryDirectory {
    fn create(parent: &Path, prefix: &str) -> Result<Self, MinoError> {
        for _ in 0..100 {
            let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let path = parent.join(format!("{prefix}{}-{sequence}", std::process::id()));
            match fs::create_dir(&path) {
                Ok(()) => {
                    return Ok(Self {
                        parent: parent.to_path_buf(),
                        path,
                        should_remove: true,
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => {
                    return Err(path_error(
                        "create catalog staging directory",
                        &path,
                        &error,
                    ));
                }
            }
        }
        Err(environment_error(
            "Could not allocate a unique catalog staging directory",
        ))
    }

    fn mark_published(&mut self) {
        self.should_remove = false;
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        if self.should_remove {
            let _ = remove_guarded_directory(&self.parent, &self.path, STAGING_PREFIX);
        }
    }
}

/// Atomically creates a valid example team-catalog source tree.
///
/// The destination must not already exist, and its parent directory must exist.
/// The generated source contains one inert Common package that can be edited or
/// extended before validation and build.
///
/// # Errors
///
/// Returns a validation, policy, or environment error for an invalid namespace
/// or URL, an unsafe destination, an existing destination, or an I/O failure.
pub fn initialize_team_catalog(
    source: &Path,
    namespace: &str,
    base_url: &str,
) -> Result<TeamCatalogInitReport, MinoError> {
    validate_namespace(namespace)?;
    let base_url = validate_base_url(base_url, SourcePolicy::HttpsOnly)?;
    let (parent, destination) = resolve_new_destination(source)?;
    if path_entry_exists(&destination)? {
        return Err(validation_error(format!(
            "Team catalog source {} already exists",
            destination.display()
        )));
    }
    let mut staging = TemporaryDirectory::create(&parent, STAGING_PREFIX)?;
    let example_package_id = format!("{namespace}.common");
    write_initialized_source(&staging.path, namespace, &base_url, &example_package_id)?;
    prepare_team_catalog(&staging.path, SourcePolicy::HttpsOnly)?;
    sync_directory_tree(&staging.path)?;
    fs::rename(&staging.path, &destination)
        .map_err(|error| path_error("publish team catalog source", &destination, &error))?;
    staging.mark_published();
    sync_directory(&parent)?;
    let source = fs::canonicalize(&destination)
        .map_err(|error| path_error("resolve initialized team catalog", &destination, &error))?;
    Ok(TeamCatalogInitReport {
        kind: TEAM_CATALOG_INIT_KIND.to_owned(),
        source,
        namespace: namespace.to_owned(),
        base_url,
        example_package_id,
    })
}

/// Validates a team-catalog source under the production HTTPS policy.
///
/// This function performs no writes and returns the exact canonical catalog,
/// package, file, and tree identities that a subsequent build will publish.
///
/// # Errors
///
/// Returns a validation, policy, drift, or environment error for malformed,
/// unsafe, oversized, changing, or unreadable source data.
pub fn validate_team_catalog(source: &Path) -> Result<TeamCatalogValidationReport, MinoError> {
    validate_team_catalog_with_policy(source, SourcePolicy::HttpsOnly)
}

/// Validates a team-catalog source under an explicit source URL policy.
///
/// The loopback policy exists only for deterministic local integration tests.
/// This function performs no writes.
///
/// # Errors
///
/// Returns the same errors as [`validate_team_catalog`].
pub fn validate_team_catalog_with_policy(
    source: &Path,
    source_policy: SourcePolicy,
) -> Result<TeamCatalogValidationReport, MinoError> {
    Ok(prepare_team_catalog(source, source_policy)?.validation_report())
}

/// Atomically builds a static team catalog under the production HTTPS policy.
///
/// An existing output is replaced only when its canonical manifest proves that
/// it is a complete, unmodified Mino team-catalog output. All source validation
/// and output staging complete before the previous output is renamed.
///
/// # Errors
///
/// Returns a validation, policy, drift, or environment error for invalid input,
/// an unsafe output, an unverified existing output, or failed publication.
pub fn build_team_catalog(
    source: &Path,
    output: &Path,
) -> Result<TeamCatalogBuildReport, MinoError> {
    build_team_catalog_with_policy(source, output, SourcePolicy::HttpsOnly)
}

/// Atomically builds a static team catalog under an explicit source URL policy.
///
/// The loopback policy exists only for deterministic local integration tests.
///
/// # Errors
///
/// Returns the same errors as [`build_team_catalog`].
pub fn build_team_catalog_with_policy(
    source: &Path,
    output: &Path,
    source_policy: SourcePolicy,
) -> Result<TeamCatalogBuildReport, MinoError> {
    let prepared = prepare_team_catalog(source, source_policy)?;
    let (parent, destination) = resolve_destination(output)?;
    reject_overlapping_paths(&prepared.source, &destination)?;
    let replaced_existing = if path_entry_exists(&destination)? {
        let metadata = fs::symlink_metadata(&destination)
            .map_err(|error| path_error("inspect catalog output", &destination, &error))?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(policy_error(format!(
                "Existing catalog output {} must be a real directory",
                destination.display()
            )));
        }
        verify_output_tree(&destination, None)?;
        true
    } else {
        false
    };
    let mut staging = TemporaryDirectory::create(&parent, STAGING_PREFIX)?;
    let manifest = prepared.artifact_manifest();
    let manifest_bytes = serialize_json(&manifest)?;
    write_output_tree(&staging.path, &prepared.files, &manifest_bytes)?;
    verify_output_tree(&staging.path, Some(&manifest))?;
    sync_directory_tree(&staging.path)?;
    promote_output(&mut staging, &parent, &destination, replaced_existing)?;
    Ok(TeamCatalogBuildReport {
        kind: TEAM_CATALOG_BUILD_KIND.to_owned(),
        source: prepared.source,
        output: destination,
        namespace: prepared.namespace,
        base_url: prepared.base_url,
        catalog_digest: prepared.catalog_digest,
        tree_digest: prepared.tree_digest,
        manifest_digest: sha256_digest(&manifest_bytes),
        packages: prepared.packages,
        replaced_existing,
    })
}

fn write_initialized_source(
    root: &Path,
    namespace: &str,
    base_url: &str,
    package_id: &str,
) -> Result<(), MinoError> {
    write_new_file(
        &root.join("catalog-source.toml"),
        &source_document_bytes(namespace, base_url)?,
    )?;
    let package_directory = root.join("packages").join("common");
    fs::create_dir_all(&package_directory).map_err(|error| {
        path_error(
            "create initialized catalog package",
            &package_directory,
            &error,
        )
    })?;
    let manifest = format!(
        "package_id = \"{package_id}\"\ndisplay_name = \"Team Common\"\nversion = \"1.0.0\"\nlanguages = []\n"
    );
    let rules = format!(
        "[[rules]]\nid = \"{package_id}.contribution\"\nlevel = \"required\"\ntext = \"Follow the team contribution standards.\"\n"
    );
    let checks = "checks = []\n";
    let package = parse_package_documents(package_id, &manifest, &rules, checks)?;
    let documents = canonical_package_documents(&package)?;
    write_new_file(
        &package_directory.join("manifest.toml"),
        &documents.manifest,
    )?;
    write_new_file(&package_directory.join("rules.toml"), &documents.rules)?;
    write_new_file(&package_directory.join("checks.toml"), &documents.checks)?;
    Ok(())
}

fn resolve_new_destination(path: &Path) -> Result<(PathBuf, PathBuf), MinoError> {
    resolve_destination(path)
}

fn resolve_destination(path: &Path) -> Result<(PathBuf, PathBuf), MinoError> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| {
                environment_error(format!("Failed to read current directory: {error}"))
            })?
            .join(path)
    };
    let file_name = absolute.file_name().ok_or_else(|| {
        validation_error(format!(
            "Catalog destination {} has no final path component",
            path.display()
        ))
    })?;
    if file_name.to_str().is_none() || matches!(file_name.to_str(), Some("." | "..")) {
        return Err(policy_error(format!(
            "Catalog destination {} has an unsafe final path component",
            path.display()
        )));
    }
    let parent = absolute.parent().ok_or_else(|| {
        validation_error(format!(
            "Catalog destination {} has no parent directory",
            path.display()
        ))
    })?;
    let metadata = fs::symlink_metadata(parent)
        .map_err(|error| path_error("inspect catalog destination parent", parent, &error))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(policy_error(format!(
            "Catalog destination parent {} must be a real directory",
            parent.display()
        )));
    }
    let parent = fs::canonicalize(parent)
        .map_err(|error| path_error("resolve catalog destination parent", parent, &error))?;
    Ok((parent.clone(), parent.join(file_name)))
}

fn reject_overlapping_paths(source: &Path, output: &Path) -> Result<(), MinoError> {
    let output_identity = if path_entry_exists(output)? {
        fs::canonicalize(output)
            .map_err(|error| path_error("resolve existing catalog output", output, &error))?
    } else {
        output.to_path_buf()
    };
    if output_identity.starts_with(source) || source.starts_with(&output_identity) {
        return Err(policy_error(format!(
            "Catalog source {} and output {} must not overlap",
            source.display(),
            output.display()
        )));
    }
    Ok(())
}

fn write_output_tree(
    root: &Path,
    files: &BTreeMap<PathBuf, Vec<u8>>,
    manifest_bytes: &[u8],
) -> Result<(), MinoError> {
    for (relative, bytes) in files {
        let path = root.join(relative);
        let parent = path.parent().ok_or_else(|| {
            environment_error(format!(
                "Catalog output path {} has no parent",
                path.display()
            ))
        })?;
        fs::create_dir_all(parent)
            .map_err(|error| path_error("create catalog output directory", parent, &error))?;
        write_new_file(&path, bytes)?;
    }
    write_new_file(&root.join(ARTIFACT_MANIFEST_FILE_NAME), manifest_bytes)
}

fn verify_output_tree(
    root: &Path,
    expected: Option<&TeamCatalogArtifactManifest>,
) -> Result<TeamCatalogArtifactManifest, MinoError> {
    let manifest_path = root.join(ARTIFACT_MANIFEST_FILE_NAME);
    let manifest_metadata = fs::symlink_metadata(&manifest_path)
        .map_err(|error| path_error("inspect catalog artifact manifest", &manifest_path, &error))?;
    if manifest_metadata.file_type().is_symlink()
        || !manifest_metadata.is_file()
        || manifest_metadata.len() > MAX_MANIFEST_BYTES
    {
        return Err(policy_error(format!(
            "Catalog artifact manifest {} must be a bounded regular file",
            manifest_path.display()
        )));
    }
    let manifest_bytes = fs::read(&manifest_path)
        .map_err(|error| path_error("read catalog artifact manifest", &manifest_path, &error))?;
    let manifest: TeamCatalogArtifactManifest =
        serde_json::from_slice(&manifest_bytes).map_err(|error| {
            policy_error(format!(
                "Catalog artifact manifest {} is invalid: {error}",
                manifest_path.display()
            ))
        })?;
    if manifest.kind != TEAM_CATALOG_MANIFEST_KIND
        || manifest.catalog_version != STATIC_CATALOG_VERSION
        || serialize_json(&manifest)? != manifest_bytes
    {
        return Err(policy_error(format!(
            "Catalog artifact manifest {} is not canonical or supported",
            manifest_path.display()
        )));
    }
    validate_namespace(&manifest.namespace)?;
    if let Some(expected) = expected
        && &manifest != expected
    {
        return Err(MinoError::new(
            ErrorCategory::DriftDetected,
            "Staged catalog manifest differs from the prepared source",
        ));
    }
    let expected_payload_paths = expected_payload_paths(&manifest)?;
    let reported_paths = manifest
        .files
        .iter()
        .map(|file| file.path.as_str())
        .collect::<Vec<_>>();
    let reported_path_set = manifest
        .files
        .iter()
        .map(|file| file.path.clone())
        .collect::<BTreeSet<_>>();
    if !reported_paths.windows(2).all(|pair| pair[0] < pair[1])
        || reported_path_set.len() != reported_paths.len()
        || reported_path_set != expected_payload_paths
    {
        return Err(policy_error(
            "Catalog artifact manifest contains missing, duplicate, unordered, or unexpected file paths",
        ));
    }
    let actual_paths = collect_output_paths(root)?;
    let mut expected_all_paths = expected_payload_paths;
    expected_all_paths.insert(ARTIFACT_MANIFEST_FILE_NAME.to_owned());
    if actual_paths != expected_all_paths {
        return Err(policy_error(format!(
            "Catalog output {} contains missing or unexpected files",
            root.display()
        )));
    }
    let mut payload = BTreeMap::new();
    for report in &manifest.files {
        let relative = manifest_path_value(&report.path)?;
        let path = root.join(&relative);
        let bytes = fs::read(&path)
            .map_err(|error| path_error("read catalog output file", &path, &error))?;
        if u64::try_from(bytes.len()).ok() != Some(report.bytes)
            || sha256_digest(&bytes) != report.digest
        {
            return Err(MinoError::new(
                ErrorCategory::DriftDetected,
                format!(
                    "Catalog output file {} differs from its manifest",
                    path.display()
                ),
            ));
        }
        payload.insert(relative, bytes);
    }
    let catalog = payload
        .get(Path::new("catalog.toml"))
        .ok_or_else(|| policy_error("Catalog artifact manifest does not include catalog.toml"))?;
    if sha256_digest(catalog) != manifest.catalog_digest
        || tree_digest(&payload)? != manifest.tree_digest
    {
        return Err(MinoError::new(
            ErrorCategory::DriftDetected,
            "Catalog output digest identities do not match its manifest",
        ));
    }
    verify_payload_contract(&manifest, &payload)?;
    Ok(manifest)
}

fn verify_payload_contract(
    manifest: &TeamCatalogArtifactManifest,
    payload: &BTreeMap<PathBuf, Vec<u8>>,
) -> Result<(), MinoError> {
    let source_policy = if manifest.base_url.starts_with("https://") {
        SourcePolicy::HttpsOnly
    } else {
        SourcePolicy::HttpsOrLoopbackHttp
    };
    let base_url = validate_base_url(&manifest.base_url, source_policy)?;
    if base_url != manifest.base_url {
        return Err(policy_error("Catalog artifact base URL is not canonical"));
    }
    let catalog_bytes = payload
        .get(Path::new("catalog.toml"))
        .ok_or_else(|| policy_error("Catalog artifact has no catalog.toml payload"))?;
    let catalog_source = std::str::from_utf8(catalog_bytes)
        .map_err(|error| policy_error(format!("Catalog artifact TOML is not UTF-8: {error}")))?;
    let catalog: StaticCatalogDocument = toml::from_str(catalog_source)
        .map_err(|error| policy_error(format!("Catalog artifact TOML is invalid: {error}")))?;
    if catalog.catalog_version != STATIC_CATALOG_VERSION
        || catalog.packages.len() != manifest.packages.len()
        || !catalog
            .packages
            .windows(2)
            .all(|pair| pair[0].package_id < pair[1].package_id)
    {
        return Err(policy_error(
            "Catalog artifact TOML has an invalid version, package count, or ordering",
        ));
    }
    for (catalog_package, package_report) in catalog.packages.iter().zip(&manifest.packages) {
        let package_root = PathBuf::from("packages")
            .join(&package_report.package_id)
            .join(&package_report.version);
        let manifest_bytes = payload
            .get(&package_root.join("manifest.toml"))
            .ok_or_else(|| policy_error("Catalog artifact package manifest is missing"))?;
        let rules_bytes = payload
            .get(&package_root.join("rules.toml"))
            .ok_or_else(|| policy_error("Catalog artifact package rules are missing"))?;
        let checks_bytes = payload
            .get(&package_root.join("checks.toml"))
            .ok_or_else(|| policy_error("Catalog artifact package checks are missing"))?;
        let package = parse_package_documents(
            &package_report.package_id,
            artifact_text(manifest_bytes, &package_report.package_id, "manifest")?,
            artifact_text(rules_bytes, &package_report.package_id, "rules")?,
            artifact_text(checks_bytes, &package_report.package_id, "checks")?,
        )
        .map_err(|error| {
            policy_error(format!(
                "Catalog artifact package {} is invalid: {error}",
                package_report.package_id
            ))
        })?;
        validate_member_ids(&package)?;
        let package_bytes = [manifest_bytes, rules_bytes, checks_bytes]
            .into_iter()
            .try_fold(0_u64, |total, bytes| {
                let bytes = u64::try_from(bytes.len())
                    .map_err(|_| policy_error("Catalog artifact package is too large"))?;
                total
                    .checked_add(bytes)
                    .ok_or_else(|| policy_error("Catalog artifact package size overflowed"))
            })?;
        let url_root = format!(
            "{base_url}/packages/{}/{}",
            package_report.package_id, package_report.version
        );
        if package.version() != package_report.version
            || package.digest() != package_report.digest
            || package_bytes != package_report.bytes
            || catalog_package.package_id != package_report.package_id
            || catalog_package.version != package_report.version
            || catalog_package.digest != package_report.digest
            || catalog_package.manifest_url != format!("{url_root}/manifest.toml")
            || catalog_package.rules_url != format!("{url_root}/rules.toml")
            || catalog_package.checks_url != format!("{url_root}/checks.toml")
        {
            return Err(MinoError::new(
                ErrorCategory::DriftDetected,
                format!(
                    "Catalog artifact package {} identities or URLs do not match",
                    package_report.package_id
                ),
            ));
        }
    }
    Ok(())
}

fn artifact_text<'a>(
    bytes: &'a [u8],
    package_id: &str,
    document: &str,
) -> Result<&'a str, MinoError> {
    std::str::from_utf8(bytes).map_err(|error| {
        policy_error(format!(
            "Catalog artifact {package_id}/{document}.toml is not UTF-8: {error}"
        ))
    })
}

fn expected_payload_paths(
    manifest: &TeamCatalogArtifactManifest,
) -> Result<BTreeSet<String>, MinoError> {
    if manifest.packages.is_empty()
        || !manifest
            .packages
            .windows(2)
            .all(|pair| pair[0].package_id < pair[1].package_id)
    {
        return Err(policy_error(
            "Catalog artifact packages must be non-empty and ordered by unique package ID",
        ));
    }
    let namespace_prefix = format!("{}.", manifest.namespace);
    let mut expected = BTreeSet::from(["catalog.toml".to_owned()]);
    for package in &manifest.packages {
        let local_id = package
            .package_id
            .strip_prefix(&namespace_prefix)
            .ok_or_else(|| {
                policy_error(format!(
                    "Catalog artifact package {} is outside namespace {}",
                    package.package_id, manifest.namespace
                ))
            })?;
        validate_local_id(local_id)?;
        let version = Version::parse(&package.version).map_err(|error| {
            policy_error(format!(
                "Catalog artifact package {} has invalid Semantic Versioning text: {error}",
                package.package_id
            ))
        })?;
        if version.to_string() != package.version
            || !is_sha256_digest(&package.digest)
            || package.bytes == 0
        {
            return Err(policy_error(format!(
                "Catalog artifact package {} has an invalid identity",
                package.package_id
            )));
        }
        for file_name in ["checks.toml", "manifest.toml", "rules.toml"] {
            let path = format!(
                "packages/{}/{}/{}",
                package.package_id, package.version, file_name
            );
            expected.insert(path);
        }
    }
    Ok(expected)
}

fn is_sha256_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    })
}

fn collect_output_paths(root: &Path) -> Result<BTreeSet<String>, MinoError> {
    fn visit(root: &Path, directory: &Path, paths: &mut Vec<String>) -> Result<(), MinoError> {
        for entry in fs::read_dir(directory)
            .map_err(|error| path_error("enumerate catalog output", directory, &error))?
        {
            let entry = entry
                .map_err(|error| path_error("read catalog output entry", directory, &error))?;
            let path = entry.path();
            let file_type = entry
                .file_type()
                .map_err(|error| path_error("inspect catalog output entry", &path, &error))?;
            if file_type.is_symlink() {
                return Err(policy_error(format!(
                    "Catalog output {} contains a symbolic link",
                    path.display()
                )));
            }
            if file_type.is_dir() {
                visit(root, &path, paths)?;
            } else if file_type.is_file() {
                let relative = path.strip_prefix(root).map_err(|_| {
                    policy_error(format!(
                        "Catalog output path {} escaped its root",
                        path.display()
                    ))
                })?;
                paths.push(relative_path_string(relative)?);
            } else {
                return Err(policy_error(format!(
                    "Catalog output {} contains an unsupported entry",
                    path.display()
                )));
            }
        }
        Ok(())
    }

    let mut owned = Vec::new();
    visit(root, root, &mut owned)?;
    owned.sort();
    Ok(owned.into_iter().collect())
}

fn manifest_path_value(value: &str) -> Result<PathBuf, MinoError> {
    if value.is_empty() || value.contains('\\') || value.len() > 512 {
        return Err(policy_error(format!(
            "Catalog manifest path {value:?} is unsafe"
        )));
    }
    let mut path = PathBuf::new();
    for segment in value.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return Err(policy_error(format!(
                "Catalog manifest path {value:?} is unsafe"
            )));
        }
        path.push(segment);
    }
    Ok(path)
}

fn promote_output(
    staging: &mut TemporaryDirectory,
    parent: &Path,
    destination: &Path,
    replaced_existing: bool,
) -> Result<(), MinoError> {
    if !replaced_existing {
        fs::rename(&staging.path, destination)
            .map_err(|error| path_error("publish catalog output", destination, &error))?;
        staging.mark_published();
        return sync_directory(parent);
    }
    let backup = unused_sibling(parent, BACKUP_PREFIX)?;
    fs::rename(destination, &backup)
        .map_err(|error| path_error("back up catalog output", destination, &error))?;
    if let Err(error) = fs::rename(&staging.path, destination) {
        let restoration = fs::rename(&backup, destination);
        return Err(environment_error(format!(
            "Failed to publish catalog output {}: {error}; restoration result: {restoration:?}",
            destination.display()
        )));
    }
    staging.mark_published();
    sync_directory(parent)?;
    remove_guarded_directory(parent, &backup, BACKUP_PREFIX)?;
    sync_directory(parent)
}

fn unused_sibling(parent: &Path, prefix: &str) -> Result<PathBuf, MinoError> {
    for _ in 0..100 {
        let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!("{prefix}{}-{sequence}", std::process::id()));
        if !path_entry_exists(&path)? {
            return Ok(path);
        }
    }
    Err(environment_error(
        "Could not allocate a unique catalog backup path",
    ))
}

fn remove_guarded_directory(parent: &Path, path: &Path, prefix: &str) -> Result<(), MinoError> {
    if path.parent() != Some(parent)
        || !path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(prefix))
    {
        return Err(policy_error(format!(
            "Refused to remove unguarded catalog directory {}",
            path.display()
        )));
    }
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(path_error(
            "remove catalog temporary directory",
            path,
            &error,
        )),
    }
}

fn write_new_file(path: &Path, bytes: &[u8]) -> Result<(), MinoError> {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|error| path_error("create catalog file", path, &error))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| path_error("write catalog file", path, &error))
}

fn serialize_json<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, MinoError> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|error| {
        environment_error(format!("Failed to serialize catalog manifest: {error}"))
    })?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn path_entry_exists(path: &Path) -> Result<bool, MinoError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(path_error("inspect catalog path", path, &error)),
    }
}

fn sync_directory_tree(root: &Path) -> Result<(), MinoError> {
    fn visit(path: &Path, directories: &mut Vec<PathBuf>) -> Result<(), MinoError> {
        directories.push(path.to_path_buf());
        for entry in fs::read_dir(path)
            .map_err(|error| path_error("enumerate catalog directory", path, &error))?
        {
            let entry =
                entry.map_err(|error| path_error("read catalog directory entry", path, &error))?;
            if entry
                .file_type()
                .map_err(|error| {
                    path_error("inspect catalog directory entry", &entry.path(), &error)
                })?
                .is_dir()
            {
                visit(&entry.path(), directories)?;
            }
        }
        Ok(())
    }

    let mut directories = Vec::new();
    visit(root, &mut directories)?;
    directories.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    for directory in directories {
        sync_directory(&directory)?;
    }
    Ok(())
}

#[cfg(unix)]
fn sync_directory(directory: &Path) -> Result<(), MinoError> {
    File::open(directory)
        .and_then(|file| file.sync_all())
        .map_err(|error| path_error("synchronize catalog directory", directory, &error))
}

#[cfg(not(unix))]
fn sync_directory(directory: &Path) -> Result<(), MinoError> {
    fs::metadata(directory)
        .map(|_| ())
        .map_err(|error| path_error("inspect catalog directory", directory, &error))
}

fn path_error(action: &str, path: &Path, error: &std::io::Error) -> MinoError {
    environment_error(format!("Failed to {action} {}: {error}", path.display()))
}

fn validation_error(message: impl Into<String>) -> MinoError {
    MinoError::new(ErrorCategory::IncompleteOrValidation, message)
}

fn policy_error(message: impl Into<String>) -> MinoError {
    MinoError::new(ErrorCategory::PolicyViolation, message)
}

fn environment_error(message: impl Into<String>) -> MinoError {
    MinoError::new(ErrorCategory::EnvironmentUnavailable, message)
}
