//! Non-destructive analysis of legacy planning workflow documents.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::store::sha256_digest;
use crate::{ErrorCategory, MinoError};

const MAX_LEGACY_DOCUMENT_BYTES: usize = 1024 * 1024;

/// Legacy durable-planning clauses that conflict with Mino workflow ownership.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LegacyPlanningClauseKind {
    /// Rules that require a task-specific formal plan or template.
    FormalPlanTrigger,
    /// Rules that retrieve pinned external planning resources or Gists.
    PinnedExternalResource,
    /// Rules that require a separate plan review gate.
    PlanReviewGate,
    /// Rules that retrieve or execute a separate plan execution guide.
    PlanExecution,
}

/// One active legacy durable-planning clause outside fenced examples.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LegacyPlanningClause {
    /// Stable clause classification.
    pub kind: LegacyPlanningClauseKind,
    /// One-based source line containing the first active occurrence.
    pub line: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LegacyPlanningSection {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) start_line: usize,
    pub(crate) end_line: usize,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct LegacyPlanningDetection {
    pub(crate) clauses: Vec<LegacyPlanningClause>,
    pub(crate) sections: Vec<LegacyPlanningSection>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct MarkdownLine {
    line: usize,
    start: usize,
    text: String,
    is_fenced: bool,
    heading: Option<MarkdownHeading>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct MarkdownHeading {
    level: usize,
    title: String,
}

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
    scan_markdown_lines(text)
        .into_iter()
        .filter_map(|line| line.heading.map(|heading| (line.line, heading.title)))
        .collect()
}

pub(crate) fn detect_legacy_planning_authority(text: &str) -> LegacyPlanningDetection {
    let lines = scan_markdown_lines(text);
    let heading_indexes = lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| line.heading.as_ref().map(|heading| (index, heading)))
        .collect::<Vec<_>>();
    let mut sections = Vec::new();
    for (position, (line_index, heading)) in heading_indexes.iter().enumerate() {
        if !heading.title.eq_ignore_ascii_case("Planning Documents") {
            continue;
        }
        let end_index = heading_indexes
            .iter()
            .skip(position + 1)
            .find(|(_, candidate)| candidate.level <= heading.level)
            .map_or(lines.len(), |(index, _)| *index);
        let end = lines.get(end_index).map_or(text.len(), |line| line.start);
        let end_line = lines
            .get(end_index.saturating_sub(1))
            .map_or(lines[*line_index].line, |line| line.line);
        sections.push(LegacyPlanningSection {
            start: lines[*line_index].start,
            end,
            start_line: lines[*line_index].line,
            end_line,
        });
    }
    let mut clauses = Vec::new();
    let mut seen = BTreeSet::new();
    for line in lines.iter().filter(|line| {
        !line.is_fenced
            && sections
                .iter()
                .any(|section| line.start >= section.start && line.start < section.end)
    }) {
        let normalized = line.text.to_ascii_lowercase();
        for kind in clause_kinds(&normalized) {
            if seen.insert(kind) {
                clauses.push(LegacyPlanningClause {
                    kind,
                    line: line.line,
                });
            }
        }
    }
    if sections.is_empty() {
        for line in lines.iter().filter(|line| !line.is_fenced) {
            let Some(heading) = &line.heading else {
                continue;
            };
            for kind in clause_kinds(&heading.title.to_ascii_lowercase()) {
                if seen.insert(kind) {
                    clauses.push(LegacyPlanningClause {
                        kind,
                        line: line.line,
                    });
                }
            }
        }
    }
    LegacyPlanningDetection { clauses, sections }
}

fn scan_markdown_lines(text: &str) -> Vec<MarkdownLine> {
    let mut lines = Vec::new();
    let mut offset = 0;
    let mut active_fence: Option<(u8, usize)> = None;
    for (index, segment) in text.split_inclusive('\n').enumerate() {
        let line = segment.strip_suffix('\n').unwrap_or(segment);
        let line = line.strip_suffix('\r').unwrap_or(line);
        let trimmed = line.trim_start();
        let fence = fence_marker(trimmed);
        let was_fenced = active_fence.is_some();
        if let Some((marker, count)) = fence {
            match active_fence {
                None => active_fence = Some((marker, count)),
                Some((active_marker, active_count))
                    if marker == active_marker
                        && count >= active_count
                        && trimmed[count..].trim().is_empty() =>
                {
                    active_fence = None;
                }
                Some(_) => {}
            }
        }
        let is_fenced = was_fenced || fence.is_some();
        let heading = (!is_fenced).then(|| parse_heading(trimmed)).flatten();
        lines.push(MarkdownLine {
            line: index + 1,
            start: offset,
            text: line.to_owned(),
            is_fenced,
            heading,
        });
        offset += segment.len();
    }
    lines
}

fn fence_marker(value: &str) -> Option<(u8, usize)> {
    let marker = *value.as_bytes().first()?;
    if !matches!(marker, b'`' | b'~') {
        return None;
    }
    let count = value.bytes().take_while(|byte| *byte == marker).count();
    (count >= 3).then_some((marker, count))
}

fn parse_heading(value: &str) -> Option<MarkdownHeading> {
    let level = value.bytes().take_while(|byte| *byte == b'#').count();
    (level > 0 && level <= 6 && value.as_bytes().get(level) == Some(&b' ')).then(|| {
        MarkdownHeading {
            level,
            title: value[level + 1..].trim().to_owned(),
        }
    })
}

fn clause_kinds(value: &str) -> Vec<LegacyPlanningClauseKind> {
    let mut kinds = Vec::new();
    if value.contains("formal plan trigger") {
        kinds.push(LegacyPlanningClauseKind::FormalPlanTrigger);
    }
    if value.contains("pinned external resource") || value.contains("pinned gist") {
        kinds.push(LegacyPlanningClauseKind::PinnedExternalResource);
    }
    if value.contains("plan review gate") {
        kinds.push(LegacyPlanningClauseKind::PlanReviewGate);
    }
    if value.contains("plan execution") {
        kinds.push(LegacyPlanningClauseKind::PlanExecution);
    }
    kinds
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
    let detection = detect_legacy_planning_authority(text);
    if detection.clauses.is_empty() {
        return "No active legacy durable-planning clauses were detected; preserve the source unchanged."
            .to_owned();
    }
    if detection.sections.len() != 1 {
        return "Active legacy planning clauses do not have one deterministic Planning Documents section; manual authority resolution is required."
            .to_owned();
    }
    let section = &detection.sections[0];
    let line_ending = if text.contains("\r\n") { "\r\n" } else { "\n" };
    format!(
        "--- {}\n+++ {} (proposed)\n@@ -{},{} @@\n{}",
        path.display(),
        path.display(),
        section.start_line,
        section.end_line.saturating_sub(section.start_line) + 1,
        crate::integration::planning_supersession(line_ending)
    )
}
