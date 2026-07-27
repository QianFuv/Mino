//! Deterministic standards precedence detection and source-bound assessment.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path};

use serde::{Deserialize, Serialize};

use crate::domain::{
    Plan, StandardConflict, StandardConflictCandidate, StandardConflictDecision,
    StandardSourceKind, StandardsConflictState,
};
use crate::managed_fs::{ManagedFsError, ManagedFsErrorKind, ManagedPath, ProjectFs};
use crate::store::sha256_digest;
use crate::{ErrorCategory, MinoError};

use super::EmbeddedCatalog;

const MAX_STANDARDS_SOURCE_BYTES: usize = 1024 * 1024;
const LOCAL_STANDARDS_PATH: &str = ".mino/standards.local.toml";

/// Identity of the optional local conflict-declaration document.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct LocalStandardsSource {
    /// Project-relative local declaration path.
    pub path: String,
    /// Exact declaration byte count.
    pub bytes: usize,
    /// SHA-256 digest of the declaration bytes.
    pub digest: String,
}

/// Current deterministic conflict set detected from selected packages and local sources.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DetectedStandardsConflicts {
    /// Digest of the complete current conflict set.
    pub source_digest: String,
    /// Optional local declaration identity.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub local_source: Option<LocalStandardsSource>,
    /// Conflicts in stable conflict-ID order.
    pub conflicts: Vec<StandardConflict>,
}

/// Relationship between one live conflict and persisted plan state.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StandardConflictStatus {
    /// The live conflict has not yet been snapshotted into the plan.
    Untracked,
    /// The exact live conflict is tracked but has no decision.
    Unresolved,
    /// The exact live conflict has an explicit current decision.
    Resolved,
    /// Persisted candidates or their source digests differ from the live conflict.
    Stale,
}

/// One live conflict together with its persisted decision status.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct AssessedStandardConflict {
    /// Current live conflict and candidates.
    pub conflict: StandardConflict,
    /// Previously persisted decision when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decision: Option<StandardConflictDecision>,
    /// Relationship between live and persisted state.
    pub status: StandardConflictStatus,
}

/// Complete source-bound comparison used by validation and Agent guidance.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct StandardsConflictAssessment {
    /// Digest of the current live conflict set.
    pub source_digest: String,
    /// Current conflicts with persisted-state dispositions.
    pub conflicts: Vec<AssessedStandardConflict>,
    /// Persisted conflict IDs that no longer exist in the live source set.
    pub stale_conflict_ids: Vec<String>,
}

impl StandardsConflictAssessment {
    /// Returns whether every live conflict is resolved and no stale record remains.
    #[must_use]
    pub fn is_resolved(&self) -> bool {
        self.stale_conflict_ids.is_empty()
            && self
                .conflicts
                .iter()
                .all(|conflict| conflict.status == StandardConflictStatus::Resolved)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LocalStandardsDocument {
    format_version: u32,
    #[serde(default)]
    rules: Vec<LocalRuleCandidate>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LocalRuleCandidate {
    rule_id: String,
    value: String,
    source_kind: StandardSourceKind,
    source: String,
}

/// Detects all conflicting values for selected standards rules.
///
/// The optional `.mino/standards.local.toml` file declares source-backed
/// user, repository, or project-configuration candidates. Selected embedded
/// package rules supply the lower-precedence language/Common candidates.
///
/// # Errors
///
/// Returns a validation or environment error for malformed declarations,
/// unsafe or unavailable sources, oversized bytes, or invalid candidates.
pub fn detect_standard_conflicts(
    root: &Path,
    plan: &Plan,
) -> Result<DetectedStandardsConflicts, MinoError> {
    let catalog = EmbeddedCatalog::load()?;
    let filesystem = ProjectFs::open(root).map_err(managed_conflict_error)?;
    let mut candidates = BTreeMap::<String, Vec<StandardConflictCandidate>>::new();
    collect_package_candidates(plan, &catalog, &mut candidates)?;
    let local_source = collect_local_candidates(&filesystem, plan, &mut candidates)?;
    let mut conflicts = Vec::new();
    for (rule_id, mut rule_candidates) in candidates {
        rule_candidates.sort_by(candidate_order);
        rule_candidates.dedup_by(|left, right| left.id() == right.id());
        let values = rule_candidates
            .iter()
            .map(StandardConflictCandidate::value)
            .collect::<BTreeSet<_>>();
        if values.len() < 2 {
            continue;
        }
        let fingerprint = digest_json(&rule_candidates)?;
        let conflict_id = stable_id("STANDARD-CONFLICT", rule_id.as_bytes());
        conflicts.push(
            StandardConflict::new(conflict_id, rule_id, fingerprint, rule_candidates)
                .map_err(|error| domain_error(&error))?,
        );
    }
    conflicts.sort_by(|left, right| left.id().cmp(right.id()));
    let source_digest = digest_json(&conflicts)?;
    Ok(DetectedStandardsConflicts {
        source_digest,
        local_source,
        conflicts,
    })
}

/// Compares live conflicts with the plan's persisted conflict snapshots.
#[must_use]
pub fn assess_standard_conflicts(
    detected: &DetectedStandardsConflicts,
    state: &StandardsConflictState,
) -> StandardsConflictAssessment {
    let mut conflicts = Vec::with_capacity(detected.conflicts.len());
    for conflict in &detected.conflicts {
        let stored = state
            .records()
            .iter()
            .find(|record| record.conflict().id() == conflict.id());
        let (decision, status) = match stored {
            None => (None, StandardConflictStatus::Untracked),
            Some(record) if record.conflict().fingerprint() != conflict.fingerprint() => {
                (record.decision().cloned(), StandardConflictStatus::Stale)
            }
            Some(record) => (
                record.decision().cloned(),
                if record.decision().is_some() {
                    StandardConflictStatus::Resolved
                } else {
                    StandardConflictStatus::Unresolved
                },
            ),
        };
        conflicts.push(AssessedStandardConflict {
            conflict: conflict.clone(),
            decision,
            status,
        });
    }
    let live_ids = detected
        .conflicts
        .iter()
        .map(StandardConflict::id)
        .collect::<BTreeSet<_>>();
    let stale_conflict_ids = state
        .records()
        .iter()
        .filter(|record| !live_ids.contains(record.conflict().id()))
        .map(|record| record.conflict().id().to_owned())
        .collect();
    StandardsConflictAssessment {
        source_digest: detected.source_digest.clone(),
        conflicts,
        stale_conflict_ids,
    }
}

fn collect_package_candidates(
    plan: &Plan,
    catalog: &EmbeddedCatalog,
    candidates: &mut BTreeMap<String, Vec<StandardConflictCandidate>>,
) -> Result<(), MinoError> {
    for selected in plan.standards() {
        let Some(package) = catalog.package(selected.package_id()) else {
            continue;
        };
        if package.version() != selected.version() || package.digest() != selected.digest() {
            continue;
        }
        let source_kind = if package.package_id() == "common" {
            StandardSourceKind::CommonDefault
        } else {
            StandardSourceKind::LanguagePackage
        };
        let source = format!(
            "embedded:{}@{}#{}",
            package.package_id(),
            package.version(),
            package.digest()
        );
        for rule in package.rules() {
            push_candidate(
                candidates,
                &rule.id,
                &rule.text,
                source_kind,
                &source,
                package.digest(),
            )?;
        }
    }
    Ok(())
}

fn collect_local_candidates(
    filesystem: &ProjectFs,
    plan: &Plan,
    candidates: &mut BTreeMap<String, Vec<StandardConflictCandidate>>,
) -> Result<Option<LocalStandardsSource>, MinoError> {
    let managed_path = ManagedPath::new(LOCAL_STANDARDS_PATH).map_err(managed_conflict_error)?;
    if !filesystem
        .exists(&managed_path)
        .map_err(managed_conflict_error)?
    {
        return Ok(None);
    }
    let path = filesystem.display_path(&managed_path);
    let bytes = filesystem
        .read_bounded(
            &managed_path,
            u64::try_from(MAX_STANDARDS_SOURCE_BYTES).unwrap_or(u64::MAX),
        )
        .map_err(managed_conflict_error)?;
    if bytes.is_empty() {
        return Err(validation_error(format!(
            "{} must be non-empty and no larger than {MAX_STANDARDS_SOURCE_BYTES} bytes",
            path.display()
        )));
    }
    let source = std::str::from_utf8(&bytes)
        .map_err(|error| validation_error(format!("{} is not UTF-8: {error}", path.display())))?;
    let document: LocalStandardsDocument = toml::from_str(source).map_err(|error| {
        validation_error(format!("Failed to parse {}: {error}", path.display()))
    })?;
    if document.format_version != 1 {
        return Err(validation_error(format!(
            "{} has unsupported format_version {}",
            path.display(),
            document.format_version
        )));
    }
    for rule in document.rules {
        if !matches!(
            rule.source_kind,
            StandardSourceKind::UserRequirement
                | StandardSourceKind::RepositoryRule
                | StandardSourceKind::ProjectConfiguration
        ) {
            return Err(validation_error(format!(
                "Local rule {} cannot claim a package-owned source kind",
                rule.rule_id
            )));
        }
        let source_digest = candidate_source_digest(filesystem.root(), plan, &rule)?;
        push_candidate(
            candidates,
            &rule.rule_id,
            &rule.value,
            rule.source_kind,
            &rule.source,
            &source_digest,
        )?;
    }
    Ok(Some(LocalStandardsSource {
        path: LOCAL_STANDARDS_PATH.to_owned(),
        bytes: bytes.len(),
        digest: sha256_digest(&bytes),
    }))
}

fn managed_conflict_error(error: ManagedFsError) -> MinoError {
    let category = match error.kind() {
        ManagedFsErrorKind::InvalidPath | ManagedFsErrorKind::UnsafeComponent => {
            ErrorCategory::DriftDetected
        }
        ManagedFsErrorKind::Io => ErrorCategory::EnvironmentUnavailable,
    };
    MinoError::new(category, error.into_message())
}

fn candidate_source_digest(
    root: &Path,
    plan: &Plan,
    candidate: &LocalRuleCandidate,
) -> Result<String, MinoError> {
    if candidate.source_kind == StandardSourceKind::UserRequirement {
        if candidate.source != "plan.original_request" {
            return Err(validation_error(format!(
                "User requirement {} must use source plan.original_request",
                candidate.rule_id
            )));
        }
        return Ok(sha256_digest(plan.original_request().as_bytes()));
    }
    let relative = Path::new(&candidate.source);
    if !is_safe_relative_path(relative, &candidate.source) {
        return Err(validation_error(format!(
            "Standards candidate source {} is not a safe project-relative path",
            candidate.source
        )));
    }
    let canonical_root = root.canonicalize().map_err(|error| {
        MinoError::new(
            ErrorCategory::EnvironmentUnavailable,
            format!("Failed to resolve project root {}: {error}", root.display()),
        )
    })?;
    let path = root.join(relative);
    let canonical = path.canonicalize().map_err(|error| {
        MinoError::new(
            ErrorCategory::EnvironmentUnavailable,
            format!(
                "Failed to resolve standards source {}: {error}",
                path.display()
            ),
        )
    })?;
    if !canonical.starts_with(&canonical_root) || !canonical.is_file() {
        return Err(validation_error(format!(
            "Standards source {} must resolve to a project file",
            candidate.source
        )));
    }
    let bytes = fs::read(&canonical).map_err(|error| {
        MinoError::new(
            ErrorCategory::EnvironmentUnavailable,
            format!(
                "Failed to read standards source {}: {error}",
                canonical.display()
            ),
        )
    })?;
    if bytes.is_empty() || bytes.len() > MAX_STANDARDS_SOURCE_BYTES {
        return Err(validation_error(format!(
            "Standards source {} must be non-empty and bounded",
            candidate.source
        )));
    }
    Ok(sha256_digest(&bytes))
}

fn push_candidate(
    candidates: &mut BTreeMap<String, Vec<StandardConflictCandidate>>,
    rule_id: &str,
    value: &str,
    source_kind: StandardSourceKind,
    source: &str,
    source_digest: &str,
) -> Result<(), MinoError> {
    let value = value.trim();
    let identity = format!("{rule_id}\0{value}\0{source_kind:?}\0{source}\0{source_digest}");
    let id = stable_id("STANDARD-CANDIDATE", identity.as_bytes());
    let candidate =
        StandardConflictCandidate::new(id, rule_id, value, source_kind, source, source_digest)
            .map_err(|error| domain_error(&error))?;
    candidates
        .entry(rule_id.to_owned())
        .or_default()
        .push(candidate);
    Ok(())
}

fn candidate_order(
    left: &StandardConflictCandidate,
    right: &StandardConflictCandidate,
) -> std::cmp::Ordering {
    right
        .precedence()
        .cmp(&left.precedence())
        .then_with(|| left.source().cmp(right.source()))
        .then_with(|| left.id().cmp(right.id()))
}

fn stable_id(prefix: &str, input: &[u8]) -> String {
    let digest = sha256_digest(input);
    format!(
        "{prefix}-{}",
        digest["sha256:".len().."sha256:".len() + 16].to_ascii_uppercase()
    )
}

fn digest_json(value: &impl Serialize) -> Result<String, MinoError> {
    let bytes = serde_json::to_vec(value).map_err(|error| {
        MinoError::new(
            ErrorCategory::EnvironmentUnavailable,
            format!("Failed to serialize standards conflict identity: {error}"),
        )
    })?;
    Ok(sha256_digest(&bytes))
}

fn is_safe_relative_path(path: &Path, value: &str) -> bool {
    !value.is_empty()
        && !value.contains('\\')
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn domain_error(error: &crate::domain::DomainError) -> MinoError {
    validation_error(error.to_string())
}

fn validation_error(message: impl Into<String>) -> MinoError {
    MinoError::new(ErrorCategory::IncompleteOrValidation, message)
}
