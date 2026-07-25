//! Non-destructive analysis of legacy planning workflow documents.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::store::sha256_digest;
use crate::{ErrorCategory, MinoError};

const MAX_LEGACY_DOCUMENT_BYTES: usize = 1024 * 1024;

/// Supported legacy planning document roles.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum LegacyDocumentKind {
    /// Repository-level Agent instructions.
    Agents,
    /// A historical plan template.
    PlanTemplate,
    /// A historical execution guide.
    PlanExecution,
}

/// One explicitly supplied legacy source document.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LegacyInput {
    /// Semantic document role selected by the caller.
    pub kind: LegacyDocumentKind,
    /// Exact source path read without modification.
    pub path: PathBuf,
}

/// How one legacy heading maps into Mino ownership.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LegacyMappingDisposition {
    /// The section has one deterministic Mino destination.
    Mapped,
    /// The section overlaps more than one Mino responsibility.
    Ambiguous,
    /// No v0.1 destination is defined.
    Unsupported,
}

/// Mapping of one source Markdown heading into a Mino-owned concern.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct LegacyMapping {
    /// Source document role.
    pub document: LegacyDocumentKind,
    /// Source path as supplied by the caller.
    pub source: String,
    /// One-based source line containing the heading.
    pub line: usize,
    /// Heading text without Markdown markers.
    pub heading: String,
    /// Deterministic mapping classification.
    pub disposition: LegacyMappingDisposition,
    /// Proposed Mino destination or manual review target.
    pub target: String,
}

/// One deterministic ambiguity or unsupported-content finding.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct LegacyFinding {
    /// Stable machine-readable finding code.
    pub code: String,
    /// Explanatory source-specific message.
    pub message: String,
    /// Source path associated with the finding.
    pub source: String,
    /// Optional one-based source line.
    pub line: Option<usize>,
}

/// Proposed destination or unified diff that is never applied by analysis.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct LegacyProposedChange {
    /// Human-readable target path or ownership surface.
    pub target: String,
    /// Proposed unified diff or inert migration action summary.
    pub proposal: String,
}

/// Immutable source identity recorded in one migration report.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct LegacySourceSummary {
    /// Semantic document role.
    pub kind: LegacyDocumentKind,
    /// Source path as supplied by the caller.
    pub path: String,
    /// Exact byte length observed.
    pub bytes: usize,
    /// SHA-256 digest of the unchanged source bytes.
    pub digest: String,
}

/// Complete read-only legacy workflow analysis.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct LegacyMigrationReport {
    /// Immutable identities for every supplied source.
    pub sources: Vec<LegacySourceSummary>,
    /// Heading mappings in document and line order.
    pub mappings: Vec<LegacyMapping>,
    /// Ambiguity and unsupported findings in deterministic order.
    pub findings: Vec<LegacyFinding>,
    /// Proposed changes that were not applied.
    pub proposed_changes: Vec<LegacyProposedChange>,
    /// Always false in v0.1 because analysis never mutates sources.
    pub applied: bool,
    /// Always empty because legacy cleanup is never automatic.
    pub deleted_sources: Vec<String>,
}

/// Analyzes explicitly supplied legacy planning documents without writing them.
///
/// # Errors
///
/// Returns a validation or environment error for empty input, unreadable files,
/// oversized bytes, or non-UTF-8 content.
pub fn analyze_legacy(inputs: &[LegacyInput]) -> Result<LegacyMigrationReport, MinoError> {
    if inputs.is_empty() {
        return Err(MinoError::new(
            ErrorCategory::IncompleteOrValidation,
            "Legacy migration analysis requires at least one source document",
        ));
    }
    let mut sources = Vec::with_capacity(inputs.len());
    let mut mappings = Vec::new();
    let mut findings = Vec::new();
    let mut proposed_changes = Vec::new();
    for input in inputs {
        analyze_document(
            input,
            &mut sources,
            &mut mappings,
            &mut findings,
            &mut proposed_changes,
        )?;
    }
    findings.sort_by(|left, right| {
        (&left.source, left.line, &left.code).cmp(&(&right.source, right.line, &right.code))
    });
    Ok(LegacyMigrationReport {
        sources,
        mappings,
        findings,
        proposed_changes,
        applied: false,
        deleted_sources: Vec::new(),
    })
}

fn analyze_document(
    input: &LegacyInput,
    sources: &mut Vec<LegacySourceSummary>,
    mappings: &mut Vec<LegacyMapping>,
    findings: &mut Vec<LegacyFinding>,
    proposed_changes: &mut Vec<LegacyProposedChange>,
) -> Result<(), MinoError> {
    let bytes = fs::read(&input.path).map_err(|error| {
        MinoError::new(
            ErrorCategory::EnvironmentUnavailable,
            format!(
                "Failed to read legacy document {}: {error}",
                input.path.display()
            ),
        )
    })?;
    if bytes.is_empty() || bytes.len() > MAX_LEGACY_DOCUMENT_BYTES {
        return Err(MinoError::new(
            ErrorCategory::IncompleteOrValidation,
            format!(
                "Legacy document {} must be non-empty and no larger than {MAX_LEGACY_DOCUMENT_BYTES} bytes",
                input.path.display()
            ),
        ));
    }
    let text = std::str::from_utf8(&bytes).map_err(|error| {
        MinoError::new(
            ErrorCategory::IncompleteOrValidation,
            format!(
                "Legacy document {} is not UTF-8: {error}",
                input.path.display()
            ),
        )
    })?;
    let source = input.path.to_string_lossy().into_owned();
    sources.push(LegacySourceSummary {
        kind: input.kind,
        path: source.clone(),
        bytes: bytes.len(),
        digest: sha256_digest(&bytes),
    });
    let headings = markdown_headings(text);
    if headings.is_empty() {
        findings.push(LegacyFinding {
            code: "legacy_no_headings".to_owned(),
            message: "Document has no Markdown headings to map".to_owned(),
            source: source.clone(),
            line: None,
        });
    }
    let mut seen = BTreeSet::new();
    for (line, heading) in headings {
        let normalized = heading.to_ascii_lowercase();
        let (disposition, target) = classify_heading(input.kind, &normalized);
        if !seen.insert(normalized) {
            findings.push(LegacyFinding {
                code: "legacy_duplicate_heading".to_owned(),
                message: format!("Heading {heading} appears more than once"),
                source: source.clone(),
                line: Some(line),
            });
        }
        if disposition != LegacyMappingDisposition::Mapped {
            findings.push(LegacyFinding {
                code: match disposition {
                    LegacyMappingDisposition::Ambiguous => "legacy_ambiguous_section",
                    LegacyMappingDisposition::Unsupported => "legacy_unsupported_section",
                    LegacyMappingDisposition::Mapped => {
                        unreachable!("mapped sections are not findings")
                    }
                }
                .to_owned(),
                message: format!("Section {heading} requires manual migration review"),
                source: source.clone(),
                line: Some(line),
            });
        }
        mappings.push(LegacyMapping {
            document: input.kind,
            source: source.clone(),
            line,
            heading,
            disposition,
            target: target.to_owned(),
        });
    }
    proposed_changes.push(proposal(input.kind, &input.path, text));
    Ok(())
}

fn markdown_headings(text: &str) -> Vec<(usize, String)> {
    text.lines()
        .enumerate()
        .filter_map(|(index, line)| {
            let trimmed = line.trim_start();
            let marker_count = trimmed.bytes().take_while(|byte| *byte == b'#').count();
            (marker_count > 0
                && marker_count <= 6
                && trimmed.as_bytes().get(marker_count) == Some(&b' '))
            .then(|| (index + 1, trimmed[marker_count + 1..].trim().to_owned()))
        })
        .collect()
}

fn classify_heading(
    kind: LegacyDocumentKind,
    heading: &str,
) -> (LegacyMappingDisposition, &'static str) {
    match kind {
        LegacyDocumentKind::Agents if contains_any(heading, &["plan", "git", "commit"]) => (
            LegacyMappingDisposition::Mapped,
            "Mino CLI policy and repository AGENTS managed block",
        ),
        LegacyDocumentKind::Agents
            if contains_any(
                heading,
                &["coding", "rust", "python", "typescript", "common"],
            ) =>
        {
            (
                LegacyMappingDisposition::Mapped,
                "Mino standards packages and repository hard rules",
            )
        }
        LegacyDocumentKind::Agents if heading.contains("scope") => (
            LegacyMappingDisposition::Ambiguous,
            "manual split between repository policy and plan scope",
        ),
        LegacyDocumentKind::PlanTemplate
            if contains_any(
                heading,
                &[
                    "metadata",
                    "summary",
                    "context",
                    "scope",
                    "decision",
                    "approach",
                    "interface",
                    "edge",
                    "task",
                    "verification",
                    "git flow",
                    "outcome",
                    "review",
                ],
            ) =>
        {
            (
                LegacyMappingDisposition::Mapped,
                "versioned Plan schema and managed Markdown projection",
            )
        }
        LegacyDocumentKind::PlanExecution
            if contains_any(
                heading,
                &[
                    "phase",
                    "rule",
                    "checkpoint",
                    "verification",
                    "commit",
                    "review",
                    "resume",
                    "recovery",
                ],
            ) =>
        {
            (
                LegacyMappingDisposition::Mapped,
                "Mino execution state machine and policy services",
            )
        }
        _ => (
            LegacyMappingDisposition::Unsupported,
            "manual review outside the v0.1 migration map",
        ),
    }
}

fn contains_any(value: &str, fragments: &[&str]) -> bool {
    fragments.iter().any(|fragment| value.contains(fragment))
}

fn proposal(kind: LegacyDocumentKind, path: &Path, text: &str) -> LegacyProposedChange {
    match kind {
        LegacyDocumentKind::Agents => LegacyProposedChange {
            target: path.to_string_lossy().into_owned(),
            proposal: agents_diff(path, text),
        },
        LegacyDocumentKind::PlanTemplate => LegacyProposedChange {
            target: "bundled protocol PLAN_TEMPLATE.md".to_owned(),
            proposal: "Use the digest-verified embedded template; preserve this source as legacy reference."
                .to_owned(),
        },
        LegacyDocumentKind::PlanExecution => LegacyProposedChange {
            target: "bundled protocol PLAN_EXECUTION.md".to_owned(),
            proposal: "Use the digest-verified embedded execution guide; preserve this source as legacy reference."
                .to_owned(),
        },
    }
}

fn agents_diff(path: &Path, text: &str) -> String {
    let line_count = text.lines().count();
    format!(
        "--- {}\n+++ {} (proposed)\n@@ -{line_count},0 +{},5 @@\n+<!-- BEGIN MINO MANAGED -->\n+Use `mino agent context --format json --no-input` for protocol state.\n+Follow only returned canonical `next_actions` and stop when approval is required.\n+Do not edit `.mino/` state or managed plan Markdown directly.\n+<!-- END MINO MANAGED -->\n",
        path.display(),
        path.display(),
        line_count.saturating_add(1)
    )
}
