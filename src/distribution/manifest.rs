//! Stable schemas for reproducible native plugin artifacts.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Stable schema identifier for an artifact manifest.
pub const MINO_PLUGIN_ARTIFACT_KIND: &str = "mino.plugin-artifact-manifest/v1";
/// Stable archive format identifier for stored deterministic ZIP bytes.
pub const MINO_PLUGIN_ARCHIVE_KIND: &str = "mino.plugin-archive/zip-stored-v1";

/// One exact regular file stored in a native plugin archive.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactFile {
    /// Forward-slash path below the archive root.
    pub path: String,
    /// SHA-256 digest of exact file bytes.
    pub digest: String,
    /// Exact uncompressed byte length.
    pub bytes: u64,
    /// Normalized Unix permission bits represented in every ZIP entry.
    pub mode: u32,
}

/// Exact identity of the ZIP file described by an artifact manifest.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactArchive {
    /// Stable archive format identifier.
    pub kind: String,
    /// Archive file name within the target output directory.
    pub file: String,
    /// SHA-256 digest of exact archive bytes.
    pub digest: String,
    /// Exact archive byte length.
    pub bytes: u64,
}

/// Deterministic proof declarations for the isolated native smoke.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactSmokeProof {
    /// Exact bounded probes that passed through the archived binary.
    pub probes: Vec<String>,
    /// Whether HOME and USERPROFILE were redirected below the smoke directory.
    pub isolated_home: bool,
    /// Whether the smoke relied on mutating the host PATH.
    pub path_mutated: bool,
    /// Whether artifact assembly or smoke performed network access.
    pub network_access: bool,
}

/// Canonical manifest for one target-specific native plugin archive.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginArtifactManifest {
    /// Stable artifact-manifest schema identifier.
    pub kind: String,
    /// Exact plugin name.
    pub plugin_name: String,
    /// Exact Cargo/plugin semantic version.
    pub plugin_version: String,
    /// Native Rust target triple.
    pub target: String,
    /// Exact protocol version and revision identity.
    pub protocol: String,
    /// Exact Agent capabilities schema identifier.
    pub capabilities_kind: String,
    /// Digest of canonical Agent capabilities JSON.
    pub capabilities_digest: String,
    /// Exact embedded standards pins.
    pub standards: Vec<String>,
    /// Digest of canonical plugin Skill assets.
    pub skill_digest: String,
    /// Digest of the canonical binary-free plugin source.
    pub source_digest: String,
    /// Exact archive identity.
    pub archive: ArtifactArchive,
    /// Sorted regular-file archive inventory.
    pub files: Vec<ArtifactFile>,
    /// Bounded isolated smoke declarations.
    pub smoke: ArtifactSmokeProof,
}

/// Runtime report from packaging or reusing one verified native artifact directory.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PluginArtifactReport {
    /// Stable artifact-manifest schema identifier.
    pub kind: String,
    /// Native Rust target triple.
    pub target: String,
    /// Canonical target output directory.
    pub output_directory: PathBuf,
    /// Canonical archive path.
    pub archive_path: PathBuf,
    /// Canonical artifact-manifest path.
    pub manifest_path: PathBuf,
    /// Canonical checksum-list path.
    pub checksums_path: PathBuf,
    /// SHA-256 digest of exact archive bytes.
    pub archive_digest: String,
    /// SHA-256 digest of exact manifest bytes.
    pub manifest_digest: String,
    /// Digest over the canonical binary-free plugin source.
    pub source_digest: String,
    /// Whether an identical complete target directory already existed.
    pub reused: bool,
    /// Number of regular files inside the archive.
    pub file_count: u64,
}
