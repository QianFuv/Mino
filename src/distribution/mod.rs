//! Canonical plugin source and native distribution contracts.

mod archive;
mod contract;
mod manifest;
mod package;

pub use contract::{
    MINO_PLUGIN_CONTRACT_KIND, PluginContractReport, validate_mino_plugin_source,
    validate_plugin_source,
};
pub use manifest::{
    ArtifactArchive, ArtifactFile, ArtifactSmokeProof, MINO_PLUGIN_ARCHIVE_KIND,
    MINO_PLUGIN_ARTIFACT_KIND, PluginArtifactManifest, PluginArtifactReport,
};
pub use package::{
    PluginPackageRequest, host_target, package_plugin, validate_plugin_artifact_directory,
};
