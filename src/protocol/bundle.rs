//! Digest-verified inert resources for the current planning protocol.

use serde::{Deserialize, Serialize};

use crate::domain::{CURRENT_PROTOCOL_REVISION, CURRENT_PROTOCOL_VERSION, CURRENT_SCHEMA_VERSION};
use crate::render::RENDERER_VERSION;
use crate::store::sha256_digest;

use super::{ProtocolError, ProtocolErrorKind};

const MANIFEST_JSON: &str = include_str!("../../assets/protocol/2026-05-11/manifest.json");
const PLAN_TEMPLATE: &str = include_str!("../../assets/protocol/2026-05-11/PLAN_TEMPLATE.md");
const PLAN_EXECUTION: &str = include_str!("../../assets/protocol/2026-05-11/PLAN_EXECUTION.md");

/// One inert resource and its exact SHA-256 digest.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProtocolResource {
    name: String,
    sha256: String,
}

impl ProtocolResource {
    /// Returns the stable bundled file name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the expected prefixed SHA-256 digest.
    #[must_use]
    pub fn sha256(&self) -> &str {
        &self.sha256
    }
}

/// Version, schema, renderer, and resource identities for one bundle.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProtocolManifest {
    bundle_version: u32,
    protocol_version: String,
    protocol_revision: String,
    schema_version: u32,
    renderer_version: u32,
    resources: Vec<ProtocolResource>,
}

impl ProtocolManifest {
    /// Returns the bundle manifest format version.
    #[must_use]
    pub const fn bundle_version(&self) -> u32 {
        self.bundle_version
    }

    /// Returns the calendar protocol version.
    #[must_use]
    pub fn protocol_version(&self) -> &str {
        &self.protocol_version
    }

    /// Returns the named protocol revision.
    #[must_use]
    pub fn protocol_revision(&self) -> &str {
        &self.protocol_revision
    }

    /// Returns the serialized plan schema version.
    #[must_use]
    pub const fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// Returns the deterministic renderer version.
    #[must_use]
    pub const fn renderer_version(&self) -> u32 {
        self.renderer_version
    }

    /// Returns resources in canonical name order.
    #[must_use]
    pub fn resources(&self) -> &[ProtocolResource] {
        &self.resources
    }
}

/// Verified current protocol manifest and inert source text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProtocolBundle {
    manifest: ProtocolManifest,
}

impl ProtocolBundle {
    /// Returns the verified manifest.
    #[must_use]
    pub const fn manifest(&self) -> &ProtocolManifest {
        &self.manifest
    }

    /// Returns one verified inert resource by stable file name.
    #[must_use]
    pub fn resource(&self, name: &str) -> Option<&'static str> {
        match name {
            "PLAN_EXECUTION.md" => Some(PLAN_EXECUTION),
            "PLAN_TEMPLATE.md" => Some(PLAN_TEMPLATE),
            _ => None,
        }
    }
}

/// Registry for exact protocol versions embedded at compile time.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ProtocolRegistry;

impl ProtocolRegistry {
    /// Loads and verifies the current embedded protocol bundle.
    ///
    /// # Errors
    ///
    /// Returns an invalid-bundle error for malformed manifest bytes, version
    /// disagreement, missing resources, ordering defects, or digest mismatch.
    pub fn current() -> Result<ProtocolBundle, ProtocolError> {
        let manifest: ProtocolManifest = serde_json::from_str(MANIFEST_JSON)
            .map_err(|error| invalid(format!("Embedded protocol manifest is invalid: {error}")))?;
        validate_manifest(&manifest)?;
        Ok(ProtocolBundle { manifest })
    }
}

fn validate_manifest(manifest: &ProtocolManifest) -> Result<(), ProtocolError> {
    if manifest.bundle_version != 1
        || manifest.protocol_version != CURRENT_PROTOCOL_VERSION
        || manifest.protocol_revision != CURRENT_PROTOCOL_REVISION
        || manifest.schema_version != CURRENT_SCHEMA_VERSION
        || manifest.renderer_version != RENDERER_VERSION
    {
        return Err(invalid(
            "Embedded protocol manifest disagrees with compiled protocol constants",
        ));
    }
    if manifest.resources.len() != 2
        || !manifest
            .resources
            .windows(2)
            .all(|pair| pair[0].name < pair[1].name)
    {
        return Err(invalid(
            "Embedded protocol resources must be complete and name-sorted",
        ));
    }
    for resource in &manifest.resources {
        let bytes = match resource.name.as_str() {
            "PLAN_EXECUTION.md" => PLAN_EXECUTION.as_bytes(),
            "PLAN_TEMPLATE.md" => PLAN_TEMPLATE.as_bytes(),
            name => {
                return Err(invalid(format!(
                    "Unknown embedded protocol resource {name}"
                )));
            }
        };
        if bytes.is_empty() || sha256_digest(bytes) != resource.sha256 {
            return Err(invalid(format!(
                "Embedded protocol resource {} failed digest validation",
                resource.name
            )));
        }
    }
    Ok(())
}

fn invalid(message: impl Into<String>) -> ProtocolError {
    ProtocolError::new(ProtocolErrorKind::InvalidBundle, message)
}
