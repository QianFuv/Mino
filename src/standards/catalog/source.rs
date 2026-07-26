//! Stable source and artifact schemas for data-only team standards catalogs.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Current team-catalog source schema version.
pub const TEAM_CATALOG_SOURCE_VERSION: u32 = 1;
/// Stable schema identifier for generated team-catalog artifact manifests.
pub const TEAM_CATALOG_MANIFEST_KIND: &str = "mino.team-catalog-manifest/v1";
pub(super) const TEAM_CATALOG_VALIDATION_KIND: &str = "mino.team-catalog-validation/v1";
pub(super) const TEAM_CATALOG_BUILD_KIND: &str = "mino.team-catalog-build/v1";
pub(super) const TEAM_CATALOG_INIT_KIND: &str = "mino.team-catalog-init/v1";
pub(super) const STATIC_CATALOG_VERSION: u32 = 1;
pub(super) const SOURCE_FILE_NAME: &str = "catalog-source.toml";
pub(super) const ARTIFACT_MANIFEST_FILE_NAME: &str = "catalog-manifest.json";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TeamCatalogSourceDocument {
    pub(super) source_version: u32,
    pub(super) namespace: String,
    pub(super) base_url: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StaticCatalogDocument {
    pub(super) catalog_version: u32,
    pub(super) packages: Vec<StaticCatalogPackage>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StaticCatalogPackage {
    pub(super) package_id: String,
    pub(super) version: String,
    pub(super) digest: String,
    pub(super) manifest_url: String,
    pub(super) rules_url: String,
    pub(super) checks_url: String,
}

/// One canonical file recorded in a generated catalog manifest.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TeamCatalogFileReport {
    /// Forward-slash relative path below the generated catalog root.
    pub path: String,
    /// Exact file length in bytes.
    pub bytes: u64,
    /// SHA-256 digest of the exact file bytes.
    pub digest: String,
}

/// One versioned team package recorded in catalog reports and manifests.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TeamCatalogPackageReport {
    /// DNS-namespaced package identifier.
    pub package_id: String,
    /// Exact semantic version.
    pub version: String,
    /// Aggregate digest over the three canonical package documents.
    pub digest: String,
    /// Total canonical package-document bytes.
    pub bytes: u64,
}

/// Read-only result of validating and normalizing a team-catalog source tree.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct TeamCatalogValidationReport {
    /// Stable result schema identifier.
    pub kind: String,
    /// Canonical source-tree path.
    pub source: PathBuf,
    /// Validated DNS-like namespace.
    pub namespace: String,
    /// Canonical HTTPS or explicitly test-authorized loopback base URL.
    pub base_url: String,
    /// Digest of the exact generated `catalog.toml` bytes.
    pub catalog_digest: String,
    /// Digest over every generated sync payload path and its exact bytes.
    pub tree_digest: String,
    /// Packages in stable package-ID order.
    pub packages: Vec<TeamCatalogPackageReport>,
    /// Generated sync payload files in stable path order.
    pub files: Vec<TeamCatalogFileReport>,
}

/// Result of atomically building a static team-catalog output tree.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct TeamCatalogBuildReport {
    /// Stable result schema identifier.
    pub kind: String,
    /// Canonical source-tree path.
    pub source: PathBuf,
    /// Canonical generated output path.
    pub output: PathBuf,
    /// Validated DNS-like namespace.
    pub namespace: String,
    /// Canonical configured base URL embedded in `catalog.toml`.
    pub base_url: String,
    /// Digest of the exact generated `catalog.toml` bytes.
    pub catalog_digest: String,
    /// Digest over every generated sync payload path and its exact bytes.
    pub tree_digest: String,
    /// Digest of the supplemental `catalog-manifest.json` bytes.
    pub manifest_digest: String,
    /// Packages in stable package-ID order.
    pub packages: Vec<TeamCatalogPackageReport>,
    /// Whether a previously verified Mino catalog output was replaced.
    pub replaced_existing: bool,
}

/// Result of atomically initializing a valid example team-catalog source tree.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct TeamCatalogInitReport {
    /// Stable result schema identifier.
    pub kind: String,
    /// Canonical initialized source-tree path.
    pub source: PathBuf,
    /// Validated DNS-like namespace.
    pub namespace: String,
    /// Canonical configured base URL.
    pub base_url: String,
    /// Fully namespaced example package identifier.
    pub example_package_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TeamCatalogArtifactManifest {
    pub(super) kind: String,
    pub(super) catalog_version: u32,
    pub(super) namespace: String,
    pub(super) base_url: String,
    pub(super) catalog_digest: String,
    pub(super) tree_digest: String,
    pub(super) packages: Vec<TeamCatalogPackageReport>,
    pub(super) files: Vec<TeamCatalogFileReport>,
}

pub(super) struct PreparedCatalog {
    pub(super) source: PathBuf,
    pub(super) namespace: String,
    pub(super) base_url: String,
    pub(super) catalog_digest: String,
    pub(super) tree_digest: String,
    pub(super) packages: Vec<TeamCatalogPackageReport>,
    pub(super) files: BTreeMap<PathBuf, Vec<u8>>,
    pub(super) file_reports: Vec<TeamCatalogFileReport>,
}

impl PreparedCatalog {
    pub(super) fn validation_report(&self) -> TeamCatalogValidationReport {
        TeamCatalogValidationReport {
            kind: TEAM_CATALOG_VALIDATION_KIND.to_owned(),
            source: self.source.clone(),
            namespace: self.namespace.clone(),
            base_url: self.base_url.clone(),
            catalog_digest: self.catalog_digest.clone(),
            tree_digest: self.tree_digest.clone(),
            packages: self.packages.clone(),
            files: self.file_reports.clone(),
        }
    }

    pub(super) fn artifact_manifest(&self) -> TeamCatalogArtifactManifest {
        TeamCatalogArtifactManifest {
            kind: TEAM_CATALOG_MANIFEST_KIND.to_owned(),
            catalog_version: STATIC_CATALOG_VERSION,
            namespace: self.namespace.clone(),
            base_url: self.base_url.clone(),
            catalog_digest: self.catalog_digest.clone(),
            tree_digest: self.tree_digest.clone(),
            packages: self.packages.clone(),
            files: self.file_reports.clone(),
        }
    }
}
