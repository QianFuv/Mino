//! Bounded validation and canonicalization for team-catalog source trees.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use semver::Version;

use crate::standards::SourcePolicy;
use crate::store::sha256_digest;
use crate::{ErrorCategory, MinoError};

use super::source::{
    PreparedCatalog, SOURCE_FILE_NAME, STATIC_CATALOG_VERSION, StaticCatalogDocument,
    StaticCatalogPackage, TEAM_CATALOG_SOURCE_VERSION, TeamCatalogFileReport,
    TeamCatalogPackageReport, TeamCatalogSourceDocument,
};
use super::{
    StandardsPackage, canonical_package_documents, parse_package_documents, validate_package_set,
};

const MAX_CATALOG_PACKAGES: usize = 128;
const MAX_DOCUMENT_BYTES: u64 = 1024 * 1024;
const MAX_TOTAL_BYTES: u64 = 16 * 1024 * 1024;
const MAX_RELATIVE_PATH_BYTES: usize = 512;
const MAX_NAMESPACE_BYTES: usize = 96;
const MAX_PACKAGE_ID_BYTES: usize = 128;
const MAX_LOCAL_ID_BYTES: usize = 31;

struct PreparedPackage {
    package: StandardsPackage,
    documents: [(String, Vec<u8>); 3],
}

pub(super) fn prepare_team_catalog(
    source: &Path,
    source_policy: SourcePolicy,
) -> Result<PreparedCatalog, MinoError> {
    let source = canonical_source_root(source)?;
    require_exact_entries(&source, &[SOURCE_FILE_NAME, "packages"])?;
    let mut source_bytes = 0_u64;
    let source_document =
        read_document(&source, &source.join(SOURCE_FILE_NAME), &mut source_bytes)?;
    let source_config: TeamCatalogSourceDocument =
        toml::from_str(&source_document).map_err(|error| {
            validation_error(format!(
                "Failed to parse {}: {error}",
                source.join(SOURCE_FILE_NAME).display()
            ))
        })?;
    if source_config.source_version != TEAM_CATALOG_SOURCE_VERSION {
        return Err(validation_error(format!(
            "Team catalog source version {} is unsupported",
            source_config.source_version
        )));
    }
    validate_namespace(&source_config.namespace)?;
    let base_url = validate_base_url(&source_config.base_url, source_policy)?;
    let package_directories = package_directories(&source.join("packages"))?;
    let mut raw_packages = Vec::with_capacity(package_directories.len());
    for (local_id, directory) in package_directories {
        raw_packages.push(read_package(
            &source,
            &source_config.namespace,
            &local_id,
            &directory,
            &mut source_bytes,
        )?);
    }
    let raw_package_values = raw_packages
        .iter()
        .map(|prepared| prepared.package.clone())
        .collect::<Vec<_>>();
    validate_package_set(&raw_package_values)
        .map_err(|error| validation_error(format!("Team catalog identifiers conflict: {error}")))?;
    raw_packages.sort_by(|left, right| left.package.package_id().cmp(right.package.package_id()));
    assemble_catalog(source, source_config.namespace, base_url, raw_packages)
}

pub(super) fn validate_namespace(namespace: &str) -> Result<(), MinoError> {
    if namespace.is_empty()
        || namespace.len() > MAX_NAMESPACE_BYTES
        || !namespace.contains('.')
        || namespace.split('.').any(|label| !is_dns_label(label))
    {
        return Err(validation_error(format!(
            "Team catalog namespace {namespace:?} must be a lowercase DNS-like name with at least two labels"
        )));
    }
    Ok(())
}

pub(super) fn validate_base_url(
    base_url: &str,
    source_policy: SourcePolicy,
) -> Result<String, MinoError> {
    if base_url.trim() != base_url
        || base_url.contains(['?', '#', '\\'])
        || base_url.ends_with("/.")
        || base_url.ends_with("/..")
    {
        return Err(validation_error(
            "Team catalog base_url must be an absolute URL without whitespace, query, fragment, or dot segments",
        ));
    }
    let normalized = base_url.trim_end_matches('/');
    if normalized.is_empty() || normalized == "https:" || normalized == "http:" {
        return Err(validation_error("Team catalog base_url has no authority"));
    }
    super::super::sync::validate_url(normalized, source_policy).map_err(|error| {
        validation_error(format!("Team catalog base_url is not allowed: {error}"))
    })?;
    let remainder = normalized
        .split_once("://")
        .map(|(_, remainder)| remainder)
        .unwrap_or_default();
    let authority = remainder.split('/').next().unwrap_or_default();
    if authority.is_empty() || authority.contains('@') {
        return Err(validation_error(
            "Team catalog base_url must have an authority without user information",
        ));
    }
    if remainder
        .split('/')
        .skip(1)
        .any(|segment| segment == "." || segment == ".." || segment.is_empty())
    {
        return Err(validation_error(
            "Team catalog base_url path must contain only non-empty non-dot segments",
        ));
    }
    Ok(normalized.to_owned())
}

pub(super) fn source_document_bytes(namespace: &str, base_url: &str) -> Result<Vec<u8>, MinoError> {
    serialize_toml(&TeamCatalogSourceDocument {
        source_version: TEAM_CATALOG_SOURCE_VERSION,
        namespace: namespace.to_owned(),
        base_url: base_url.to_owned(),
    })
}

fn canonical_source_root(source: &Path) -> Result<PathBuf, MinoError> {
    let metadata = fs::symlink_metadata(source)
        .map_err(|error| path_error("inspect team catalog source", source, &error))?;
    if metadata.file_type().is_symlink() {
        return Err(policy_error(format!(
            "Team catalog source {} must not be a symbolic link",
            source.display()
        )));
    }
    if !metadata.is_dir() {
        return Err(validation_error(format!(
            "Team catalog source {} is not a directory",
            source.display()
        )));
    }
    fs::canonicalize(source)
        .map_err(|error| path_error("resolve team catalog source", source, &error))
}

fn package_directories(packages_root: &Path) -> Result<Vec<(String, PathBuf)>, MinoError> {
    let metadata = fs::symlink_metadata(packages_root)
        .map_err(|error| path_error("inspect team catalog packages", packages_root, &error))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(policy_error(format!(
            "Team catalog packages path {} must be a real directory",
            packages_root.display()
        )));
    }
    let mut directories = Vec::new();
    for entry in fs::read_dir(packages_root)
        .map_err(|error| path_error("enumerate team catalog packages", packages_root, &error))?
    {
        let entry = entry.map_err(|error| {
            path_error("read team catalog package entry", packages_root, &error)
        })?;
        let path = entry.path();
        let local_id = entry.file_name().into_string().map_err(|_| {
            policy_error(format!(
                "Team catalog package path {} is not UTF-8",
                path.display()
            ))
        })?;
        validate_local_id(&local_id)?;
        let file_type = entry
            .file_type()
            .map_err(|error| path_error("inspect team catalog package entry", &path, &error))?;
        if file_type.is_symlink() || !file_type.is_dir() {
            return Err(policy_error(format!(
                "Team catalog package entry {} must be a real directory",
                path.display()
            )));
        }
        directories.push((local_id, path));
    }
    if directories.is_empty() || directories.len() > MAX_CATALOG_PACKAGES {
        return Err(validation_error(format!(
            "Team catalog must contain between 1 and {MAX_CATALOG_PACKAGES} package directories"
        )));
    }
    directories.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(directories)
}

fn read_package(
    source_root: &Path,
    namespace: &str,
    local_id: &str,
    directory: &Path,
    source_bytes: &mut u64,
) -> Result<PreparedPackage, MinoError> {
    require_exact_entries(directory, &["checks.toml", "manifest.toml", "rules.toml"])?;
    let package_id = format!("{namespace}.{local_id}");
    if package_id.len() > MAX_PACKAGE_ID_BYTES {
        return Err(validation_error(format!(
            "Team catalog package ID {package_id} exceeds {MAX_PACKAGE_ID_BYTES} bytes"
        )));
    }
    let manifest = read_document(source_root, &directory.join("manifest.toml"), source_bytes)?;
    let rules = read_document(source_root, &directory.join("rules.toml"), source_bytes)?;
    let checks = read_document(source_root, &directory.join("checks.toml"), source_bytes)?;
    let package =
        parse_package_documents(&package_id, &manifest, &rules, &checks).map_err(|error| {
            validation_error(format!("Team package {package_id} is invalid: {error}"))
        })?;
    let version = Version::parse(package.version()).map_err(|error| {
        validation_error(format!(
            "Team package {package_id} version {} is not semantic: {error}",
            package.version()
        ))
    })?;
    if version.to_string() != package.version() {
        return Err(validation_error(format!(
            "Team package {package_id} version {} is not canonical Semantic Versioning text",
            package.version()
        )));
    }
    validate_member_ids(&package)?;
    let canonical = canonical_package_documents(&package)?;
    let canonical_manifest = bytes_as_str(&canonical.manifest, &package_id, "manifest")?;
    let canonical_rules = bytes_as_str(&canonical.rules, &package_id, "rules")?;
    let canonical_checks = bytes_as_str(&canonical.checks, &package_id, "checks")?;
    let package = parse_package_documents(
        &package_id,
        canonical_manifest,
        canonical_rules,
        canonical_checks,
    )?;
    Ok(PreparedPackage {
        package,
        documents: [
            ("manifest.toml".to_owned(), canonical.manifest),
            ("rules.toml".to_owned(), canonical.rules),
            ("checks.toml".to_owned(), canonical.checks),
        ],
    })
}

fn assemble_catalog(
    source: PathBuf,
    namespace: String,
    base_url: String,
    packages: Vec<PreparedPackage>,
) -> Result<PreparedCatalog, MinoError> {
    let mut files = BTreeMap::new();
    let mut catalog_packages = Vec::with_capacity(packages.len());
    let mut package_reports = Vec::with_capacity(packages.len());
    for prepared in packages {
        let package_id = prepared.package.package_id().to_owned();
        let version = prepared.package.version().to_owned();
        let package_root = PathBuf::from("packages").join(&package_id).join(&version);
        let mut package_bytes = 0_u64;
        for (file_name, bytes) in prepared.documents {
            package_bytes = checked_total(package_bytes, bytes.len(), "package output")?;
            files.insert(package_root.join(file_name), bytes);
        }
        let url_root = format!("{base_url}/packages/{package_id}/{version}");
        catalog_packages.push(StaticCatalogPackage {
            package_id: package_id.clone(),
            version: version.clone(),
            digest: prepared.package.digest().to_owned(),
            manifest_url: format!("{url_root}/manifest.toml"),
            rules_url: format!("{url_root}/rules.toml"),
            checks_url: format!("{url_root}/checks.toml"),
        });
        package_reports.push(TeamCatalogPackageReport {
            package_id,
            version,
            digest: prepared.package.digest().to_owned(),
            bytes: package_bytes,
        });
    }
    let catalog_bytes = serialize_toml(&StaticCatalogDocument {
        catalog_version: STATIC_CATALOG_VERSION,
        packages: catalog_packages,
    })?;
    if catalog_bytes.len() as u64 > MAX_DOCUMENT_BYTES {
        return Err(validation_error(format!(
            "Generated catalog.toml exceeds the {MAX_DOCUMENT_BYTES}-byte limit"
        )));
    }
    files.insert(PathBuf::from("catalog.toml"), catalog_bytes.clone());
    let total_bytes = files.values().try_fold(0_u64, |total, bytes| {
        checked_total(total, bytes.len(), "catalog output")
    })?;
    if total_bytes > MAX_TOTAL_BYTES {
        return Err(validation_error(format!(
            "Generated catalog exceeds the {MAX_TOTAL_BYTES}-byte total limit"
        )));
    }
    let file_reports = file_reports(&files)?;
    let tree_digest = tree_digest(&files)?;
    Ok(PreparedCatalog {
        source,
        namespace,
        base_url,
        catalog_digest: sha256_digest(&catalog_bytes),
        tree_digest,
        packages: package_reports,
        files,
        file_reports,
    })
}

pub(super) fn validate_member_ids(package: &StandardsPackage) -> Result<(), MinoError> {
    let prefix = format!("{}.", package.package_id());
    for identifier in package
        .rules()
        .iter()
        .map(|rule| rule.id.as_str())
        .chain(package.checks().iter().map(|check| check.id.as_str()))
    {
        let Some(suffix) = identifier.strip_prefix(&prefix) else {
            return Err(validation_error(format!(
                "Team package {} member ID {identifier} is outside its package namespace",
                package.package_id()
            )));
        };
        if suffix.split('.').any(|segment| !is_local_label(segment)) {
            return Err(validation_error(format!(
                "Team package {} member ID {identifier} contains an invalid segment",
                package.package_id()
            )));
        }
    }
    Ok(())
}

pub(super) fn validate_local_id(local_id: &str) -> Result<(), MinoError> {
    if local_id.len() > MAX_LOCAL_ID_BYTES || !is_local_label(local_id) {
        return Err(validation_error(format!(
            "Team catalog local package ID {local_id:?} must be a lowercase ASCII slug no longer than {MAX_LOCAL_ID_BYTES} bytes"
        )));
    }
    Ok(())
}

fn is_dns_label(label: &str) -> bool {
    label.len() <= 63 && is_local_label(label)
}

fn is_local_label(label: &str) -> bool {
    !label.is_empty()
        && label
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        && label
            .bytes()
            .next_back()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        && label
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn require_exact_entries(directory: &Path, expected: &[&str]) -> Result<(), MinoError> {
    let expected = expected
        .iter()
        .map(|entry| (*entry).to_owned())
        .collect::<BTreeSet<_>>();
    let mut actual = BTreeSet::new();
    for entry in fs::read_dir(directory)
        .map_err(|error| path_error("enumerate team catalog directory", directory, &error))?
    {
        let entry = entry
            .map_err(|error| path_error("read team catalog directory entry", directory, &error))?;
        let name = entry.file_name().into_string().map_err(|_| {
            policy_error(format!(
                "Team catalog entry {} is not UTF-8",
                entry.path().display()
            ))
        })?;
        actual.insert(name);
    }
    if actual != expected {
        return Err(policy_error(format!(
            "Team catalog directory {} contains missing or unexpected entries: expected {expected:?}, found {actual:?}",
            directory.display()
        )));
    }
    Ok(())
}

fn read_document(
    source_root: &Path,
    path: &Path,
    total_bytes: &mut u64,
) -> Result<String, MinoError> {
    validate_relative_path(source_root, path)?;
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| path_error("inspect team catalog document", path, &error))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(policy_error(format!(
            "Team catalog document {} must be a regular file",
            path.display()
        )));
    }
    #[cfg(unix)]
    require_non_executable(path, &metadata)?;
    if metadata.len() > MAX_DOCUMENT_BYTES {
        return Err(validation_error(format!(
            "Team catalog document {} exceeds the {MAX_DOCUMENT_BYTES}-byte limit",
            path.display()
        )));
    }
    *total_bytes = total_bytes
        .checked_add(metadata.len())
        .ok_or_else(|| validation_error("catalog source byte count overflowed"))?;
    if *total_bytes > MAX_TOTAL_BYTES {
        return Err(validation_error(format!(
            "Team catalog source exceeds the {MAX_TOTAL_BYTES}-byte total limit"
        )));
    }
    let bytes =
        fs::read(path).map_err(|error| path_error("read team catalog document", path, &error))?;
    if bytes.len() as u64 != metadata.len() || bytes.len() as u64 > MAX_DOCUMENT_BYTES {
        return Err(MinoError::new(
            ErrorCategory::DriftDetected,
            format!(
                "Team catalog document {} changed while reading",
                path.display()
            ),
        ));
    }
    let source = String::from_utf8(bytes).map_err(|error| {
        validation_error(format!(
            "Team catalog document {} is not UTF-8: {error}",
            path.display()
        ))
    })?;
    Ok(source.replace("\r\n", "\n").replace('\r', "\n"))
}

fn validate_relative_path(root: &Path, path: &Path) -> Result<(), MinoError> {
    let relative = path.strip_prefix(root).map_err(|_| {
        policy_error(format!(
            "Team catalog path {} escapes source {}",
            path.display(),
            root.display()
        ))
    })?;
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(policy_error(format!(
            "Team catalog path {} is not a normal relative path",
            path.display()
        )));
    }
    let rendered = relative_path_string(relative)?;
    if rendered.len() > MAX_RELATIVE_PATH_BYTES {
        return Err(policy_error(format!(
            "Team catalog path {rendered} exceeds {MAX_RELATIVE_PATH_BYTES} bytes"
        )));
    }
    Ok(())
}

#[cfg(unix)]
fn require_non_executable(path: &Path, metadata: &fs::Metadata) -> Result<(), MinoError> {
    if metadata.permissions().mode() & 0o111 != 0 {
        Err(policy_error(format!(
            "Team catalog data file {} must not be executable",
            path.display()
        )))
    } else {
        Ok(())
    }
}

pub(super) fn file_reports(
    files: &BTreeMap<PathBuf, Vec<u8>>,
) -> Result<Vec<TeamCatalogFileReport>, MinoError> {
    files
        .iter()
        .map(|(path, bytes)| {
            Ok(TeamCatalogFileReport {
                path: relative_path_string(path)?,
                bytes: u64::try_from(bytes.len()).map_err(|_| {
                    validation_error(format!("Catalog file {} is too large", path.display()))
                })?,
                digest: sha256_digest(bytes),
            })
        })
        .collect()
}

pub(super) fn tree_digest(files: &BTreeMap<PathBuf, Vec<u8>>) -> Result<String, MinoError> {
    let mut digest_input = Vec::new();
    for (path, bytes) in files {
        digest_input.extend_from_slice(relative_path_string(path)?.as_bytes());
        digest_input.push(0);
        digest_input.extend_from_slice(bytes);
        digest_input.push(0);
    }
    Ok(sha256_digest(&digest_input))
}

pub(super) fn relative_path_string(path: &Path) -> Result<String, MinoError> {
    let mut segments = Vec::new();
    for component in path.components() {
        let Component::Normal(segment) = component else {
            return Err(policy_error(format!(
                "Catalog artifact path {} is not relative and normal",
                path.display()
            )));
        };
        segments.push(segment.to_str().ok_or_else(|| {
            policy_error(format!(
                "Catalog artifact path {} is not UTF-8",
                path.display()
            ))
        })?);
    }
    if segments.is_empty() {
        return Err(policy_error("Catalog artifact path is empty"));
    }
    Ok(segments.join("/"))
}

pub(super) fn serialize_toml<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, MinoError> {
    let mut rendered = toml::to_string_pretty(value).map_err(|error| {
        MinoError::new(
            ErrorCategory::EnvironmentUnavailable,
            format!("Failed to serialize team catalog TOML: {error}"),
        )
    })?;
    rendered = rendered.replace("\r\n", "\n").replace('\r', "\n");
    if !rendered.ends_with('\n') {
        rendered.push('\n');
    }
    Ok(rendered.into_bytes())
}

fn checked_total(total: u64, bytes: usize, label: &str) -> Result<u64, MinoError> {
    let bytes = u64::try_from(bytes)
        .map_err(|_| validation_error(format!("{label} byte count does not fit into u64")))?;
    total
        .checked_add(bytes)
        .ok_or_else(|| validation_error(format!("{label} byte count overflowed")))
}

fn bytes_as_str<'a>(
    bytes: &'a [u8],
    package_id: &str,
    document: &str,
) -> Result<&'a str, MinoError> {
    std::str::from_utf8(bytes).map_err(|error| {
        MinoError::new(
            ErrorCategory::EnvironmentUnavailable,
            format!("Canonical {package_id}/{document}.toml is not UTF-8: {error}"),
        )
    })
}

fn path_error(action: &str, path: &Path, error: &std::io::Error) -> MinoError {
    MinoError::new(
        ErrorCategory::EnvironmentUnavailable,
        format!("Failed to {action} {}: {error}", path.display()),
    )
}

fn validation_error(message: impl Into<String>) -> MinoError {
    MinoError::new(ErrorCategory::IncompleteOrValidation, message)
}

fn policy_error(message: impl Into<String>) -> MinoError {
    MinoError::new(ErrorCategory::PolicyViolation, message)
}
