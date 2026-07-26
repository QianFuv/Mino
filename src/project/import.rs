//! Conservative parsing of legacy managed Markdown plans into authored Draft input.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path};

use serde::Serialize;

use crate::domain::{
    CheckId, CriterionId, DraftCommitGateInput, DraftContextInput, DraftCriterionInput,
    DraftDecisionInput, DraftEdgeCaseInput, DraftFileInput, DraftMetadataInput, DraftPlanInput,
    DraftScopeInput, DraftTaskInput, DraftVerificationInput, FileChange, Plan, PlanId, TaskId,
    Timestamp,
};
use crate::store::sha256_digest;
use crate::{ErrorCategory, MinoError};

const MAX_LEGACY_PLAN_BYTES: usize = 1024 * 1024;
const MAX_REPORT_ENTRIES: usize = 4096;
const MAX_IMPORTED_TASKS: usize = 256;

/// How one legacy source fragment was handled during conservative import.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LegacyPlanMappingDisposition {
    /// The fragment was converted into a current authored Draft field.
    MappedAuthored,
    /// The fragment described historical execution and was deliberately ignored.
    IgnoredHistorical,
    /// The fragment has no safe, supported current Draft mapping.
    Unsupported,
}

/// One exact source-to-Draft mapping decision.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct LegacyPlanMapping {
    /// One-based source line.
    pub source_line: usize,
    /// Exact trimmed source fragment used for the decision.
    pub source_fragment: String,
    /// Current Draft field or review target.
    pub target: String,
    /// Mapping disposition.
    pub disposition: LegacyPlanMappingDisposition,
}

/// One stable warning produced while parsing a legacy plan.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct LegacyPlanWarning {
    /// Stable machine-readable warning code.
    pub code: String,
    /// One-based source line when the warning is line-specific.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_line: Option<usize>,
    /// Human-readable explanation of the conservative decision.
    pub message: String,
}

/// Immutable identity of the legacy plan source.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct LegacyPlanSource {
    /// Exact source path supplied by the caller.
    pub path: String,
    /// Exact source byte count.
    pub bytes: usize,
    /// SHA-256 digest of the exact source bytes.
    pub digest: String,
}

/// Parsed authored Draft data plus a complete conservative mapping report.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct LegacyPlanParseReport {
    /// Immutable source identity.
    pub source: LegacyPlanSource,
    /// Legacy title suitable for a caller-provided stable import name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggested_name: Option<String>,
    /// Original request used when creating the new Draft.
    pub original_request: String,
    /// Strict authored fields accepted by the normal Draft API.
    pub draft: DraftPlanInput,
    /// Exact source-to-target decisions in deterministic source order.
    pub mappings: Vec<LegacyPlanMapping>,
    /// Conservative omissions and review requirements.
    pub warnings: Vec<LegacyPlanWarning>,
    /// Always false because imported historical assertions are never trusted.
    pub historical_execution_trusted: bool,
}

/// Parses one bounded UTF-8 legacy Markdown plan without modifying any file.
///
/// # Errors
///
/// Returns a validation or environment error for a missing, empty, oversized,
/// NUL-containing, or non-UTF-8 source, or when mapped authored fields violate
/// the current Draft domain invariants.
pub fn parse_legacy_plan(path: &Path) -> Result<LegacyPlanParseReport, MinoError> {
    let bytes = read_source(path)?;
    let text = std::str::from_utf8(&bytes).map_err(|error| {
        MinoError::new(
            ErrorCategory::IncompleteOrValidation,
            format!("Legacy plan {} is not UTF-8: {error}", path.display()),
        )
    })?;
    if text.trim().is_empty() || text.contains('\0') {
        return Err(validation_error(format!(
            "Legacy plan {} must contain non-NUL Markdown content",
            path.display()
        )));
    }
    let source = LegacyPlanSource {
        path: path.to_string_lossy().into_owned(),
        bytes: bytes.len(),
        digest: sha256_digest(&bytes),
    };
    Parser::new(text, source).parse()
}

/// Verifies that a previously parsed legacy source still has identical bytes.
///
/// # Errors
///
/// Returns a drift error when the path can no longer be read or its byte count
/// or digest differs from the parse report.
pub fn verify_legacy_plan_source(source: &LegacyPlanSource) -> Result<(), MinoError> {
    let path = Path::new(&source.path);
    let bytes = fs::read(path).map_err(|error| {
        MinoError::new(
            ErrorCategory::DriftDetected,
            format!(
                "Legacy plan source {} could not be re-read: {error}",
                path.display()
            ),
        )
    })?;
    let digest = sha256_digest(&bytes);
    if bytes.len() != source.bytes || digest != source.digest {
        return Err(MinoError::new(
            ErrorCategory::DriftDetected,
            format!(
                "Legacy plan source {} changed during import",
                path.display()
            ),
        ));
    }
    Ok(())
}

fn read_source(path: &Path) -> Result<Vec<u8>, MinoError> {
    let metadata = fs::metadata(path).map_err(|error| {
        MinoError::new(
            ErrorCategory::EnvironmentUnavailable,
            format!("Failed to inspect legacy plan {}: {error}", path.display()),
        )
    })?;
    if !metadata.is_file() {
        return Err(validation_error(format!(
            "Legacy plan {} must be a regular file",
            path.display()
        )));
    }
    if metadata.len() == 0 || metadata.len() > MAX_LEGACY_PLAN_BYTES as u64 {
        return Err(validation_error(format!(
            "Legacy plan {} must be non-empty and no larger than {MAX_LEGACY_PLAN_BYTES} bytes",
            path.display()
        )));
    }
    let bytes = fs::read(path).map_err(|error| {
        MinoError::new(
            ErrorCategory::EnvironmentUnavailable,
            format!("Failed to read legacy plan {}: {error}", path.display()),
        )
    })?;
    if bytes.is_empty() || bytes.len() > MAX_LEGACY_PLAN_BYTES {
        return Err(validation_error(format!(
            "Legacy plan {} must be non-empty and no larger than {MAX_LEGACY_PLAN_BYTES} bytes",
            path.display()
        )));
    }
    Ok(bytes)
}

#[derive(Clone, Debug)]
struct Heading {
    line: usize,
    level: usize,
    title: String,
    key: String,
    end_line: usize,
}

#[derive(Clone, Debug)]
struct Table {
    headers: Vec<String>,
    rows: Vec<TableRow>,
}

#[derive(Clone, Debug)]
struct TableRow {
    line: usize,
    cells: Vec<String>,
}

struct Document<'a> {
    lines: Vec<&'a str>,
    visible: Vec<bool>,
    headings: Vec<Heading>,
}

impl<'a> Document<'a> {
    fn new(text: &'a str) -> Self {
        let lines = text.lines().collect::<Vec<_>>();
        let visible = visible_lines(&lines);
        let mut headings = lines
            .iter()
            .enumerate()
            .filter(|(index, _)| visible[*index])
            .filter_map(|(index, line)| {
                parse_heading(line).map(|(level, title)| (index, level, title))
            })
            .map(|(index, level, title)| Heading {
                line: index + 1,
                level,
                key: heading_key(&title),
                title,
                end_line: lines.len() + 1,
            })
            .collect::<Vec<_>>();
        for index in 0..headings.len() {
            let level = headings[index].level;
            if let Some(next) = headings[index + 1..]
                .iter()
                .find(|heading| heading.level <= level)
            {
                headings[index].end_line = next.line;
            }
        }
        Self {
            lines,
            visible,
            headings,
        }
    }

    fn line(&self, number: usize) -> &str {
        self.lines[number - 1]
    }

    fn section_lines(&self, heading: &Heading) -> Vec<(usize, &str)> {
        ((heading.line + 1)..heading.end_line)
            .filter(|line| self.visible[*line - 1])
            .map(|line| (line, self.line(line)))
            .collect()
    }

    fn find_first(&self, keys: &[&str]) -> Option<&Heading> {
        self.headings
            .iter()
            .find(|heading| keys.contains(&heading.key.as_str()))
    }

    fn tables(&self, heading: &Heading) -> Vec<Table> {
        parse_tables(&self.section_lines(heading))
    }
}

struct Parser<'a> {
    document: Document<'a>,
    source: LegacyPlanSource,
    draft: DraftPlanInput,
    suggested_name: Option<String>,
    original_request: Option<String>,
    mappings: Vec<LegacyPlanMapping>,
    mapping_keys: BTreeSet<(usize, String, LegacyPlanMappingDisposition)>,
    warnings: Vec<LegacyPlanWarning>,
    warning_keys: BTreeSet<(String, Option<usize>, String)>,
}

impl<'a> Parser<'a> {
    fn new(text: &'a str, source: LegacyPlanSource) -> Self {
        Self {
            document: Document::new(text),
            source,
            draft: DraftPlanInput::default(),
            suggested_name: None,
            original_request: None,
            mappings: Vec::new(),
            mapping_keys: BTreeSet::new(),
            warnings: Vec::new(),
            warning_keys: BTreeSet::new(),
        }
    }

    fn parse(mut self) -> Result<LegacyPlanParseReport, MinoError> {
        self.parse_front_matter();
        self.parse_title();
        self.parse_metadata();
        self.parse_single_prose(&["summary"], "summary", |draft, value| {
            draft.summary = Some(value);
        });
        self.parse_original_request();
        self.parse_context();
        self.parse_scope();
        self.parse_decisions();
        self.parse_single_prose(&["approach"], "approach", |draft, value| {
            draft.approach = Some(value);
        });
        self.parse_single_prose(
            &["interfaces data flow", "interfaces and data flow"],
            "interfaces",
            |draft, value| draft.interfaces = Some(value),
        );
        self.parse_edge_cases();
        let global_files = self.parse_global_file_map();
        let commit_gates = self.parse_git_flow();
        self.parse_tasks(global_files, commit_gates);
        self.parse_global_verification();
        self.classify_historical_fields();
        self.classify_remaining_sections();
        self.add_provenance();
        let original_request = self.original_request.clone().unwrap_or_else(|| {
            format!(
                "Review and re-author the legacy plan preserved at {} with digest {}.",
                self.source.path, self.source.digest
            )
        });
        if self.original_request.is_none() {
            self.warn(
                "legacy_plan_missing_original_request",
                None,
                "No supported original-request field was found; a provenance-only request was generated for the new Draft",
            );
        }
        preview_draft(&self.draft, &original_request)?;
        self.mappings.sort_by(|left, right| {
            (left.source_line, &left.target, &left.source_fragment).cmp(&(
                right.source_line,
                &right.target,
                &right.source_fragment,
            ))
        });
        self.warnings.sort_by(|left, right| {
            (left.source_line, &left.code, &left.message).cmp(&(
                right.source_line,
                &right.code,
                &right.message,
            ))
        });
        Ok(LegacyPlanParseReport {
            source: self.source,
            suggested_name: self.suggested_name,
            original_request,
            draft: self.draft,
            mappings: self.mappings,
            warnings: self.warnings,
            historical_execution_trusted: false,
        })
    }

    fn parse_front_matter(&mut self) {
        let Some(first) = self.document.lines.first() else {
            return;
        };
        if first.trim_start_matches('\u{feff}').trim() != "---" {
            return;
        }
        let Some(end) = self.document.lines[1..]
            .iter()
            .position(|line| line.trim() == "---")
            .map(|index| index + 2)
        else {
            self.warn(
                "legacy_plan_unclosed_front_matter",
                Some(1),
                "Opening front matter delimiter has no closing delimiter and was ignored",
            );
            return;
        };
        for line_number in 2..end {
            let line = self.document.line(line_number).trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once(':') else {
                self.warn(
                    "legacy_plan_front_matter_syntax",
                    Some(line_number),
                    "Front matter entry is not a simple key-value pair and was ignored",
                );
                continue;
            };
            let key = heading_key(key);
            let value = clean_value(value);
            if !is_authored_value(&value) {
                continue;
            }
            match key.as_str() {
                "title" | "name" => self.set_name(value, line_number),
                "summary" => self.set_summary(value, line_number),
                "priority" => self.set_metadata_field("priority", value, line_number),
                "type" | "plan type" => {
                    self.set_metadata_field("plan_type", value, line_number);
                }
                "area" => self.set_metadata_field("area", value, line_number),
                "owner" => self.set_metadata_field("owner", value, line_number),
                "original request" | "request" => {
                    self.set_original_request(value, line_number);
                }
                "goal" => self.set_scope_field("goal", vec![value], line_number),
                key if is_historical_key(key) => self.ignore_historical(line_number, key),
                _ => {
                    self.map(
                        line_number,
                        "manual_review.front_matter",
                        LegacyPlanMappingDisposition::Unsupported,
                    );
                    self.warn(
                        "legacy_plan_unknown_front_matter",
                        Some(line_number),
                        format!("Front matter key {key} has no supported Draft mapping"),
                    );
                }
            }
        }
    }

    fn parse_title(&mut self) {
        let Some(heading) = self
            .document
            .headings
            .iter()
            .find(|heading| heading.level == 1)
        else {
            self.warn(
                "legacy_plan_missing_title",
                None,
                "No level-one Markdown title was found",
            );
            return;
        };
        let title = clean_value(&heading.title);
        if is_authored_value(&title) {
            self.set_name(title, heading.line);
        }
    }

    fn parse_metadata(&mut self) {
        let Some(heading) = self.document.find_first(&["metadata"]).cloned() else {
            return;
        };
        let rows = key_value_rows(&self.document.tables(&heading));
        for row in rows {
            let key = heading_key(&row.cells[0]);
            let value = clean_value(&row.cells[1]);
            if !is_authored_value(&value) {
                continue;
            }
            match key.as_str() {
                "name" => self.set_name(value, row.line),
                "priority" => self.set_metadata_field("priority", value, row.line),
                "type" | "plan type" => {
                    self.set_metadata_field("plan_type", value, row.line);
                }
                "area" => self.set_metadata_field("area", value, row.line),
                "owner" => self.set_metadata_field("owner", value, row.line),
                key if is_historical_key(key) => self.ignore_historical(row.line, key),
                _ => {
                    self.map(
                        row.line,
                        "manual_review.metadata",
                        LegacyPlanMappingDisposition::Unsupported,
                    );
                    self.warn(
                        "legacy_plan_unknown_metadata",
                        Some(row.line),
                        format!("Metadata field {key} has no supported Draft mapping"),
                    );
                }
            }
        }
    }

    fn parse_single_prose<F>(&mut self, keys: &[&str], target: &str, apply: F)
    where
        F: FnOnce(&mut DraftPlanInput, String),
    {
        let Some(heading) = self.document.find_first(keys).cloned() else {
            return;
        };
        let Some((line, value)) = section_prose(&self.document, &heading) else {
            return;
        };
        if target == "summary" && self.draft.summary.is_some() {
            self.warn_duplicate(line, target);
            return;
        }
        apply(&mut self.draft, value);
        self.map(line, target, LegacyPlanMappingDisposition::MappedAuthored);
    }

    fn parse_original_request(&mut self) {
        let Some(heading) = self.document.find_first(&["original request"]).cloned() else {
            return;
        };
        if let Some((line, value)) = section_prose(&self.document, &heading) {
            self.set_original_request(value, line);
        }
    }

    fn parse_context(&mut self) {
        let Some(heading) = self
            .document
            .find_first(&["current state and references", "context"])
            .cloned()
        else {
            return;
        };
        for table in self.document.tables(&heading) {
            let Some(indices) = table_indices(&table, &["reference", "fact", "implication"]) else {
                continue;
            };
            for row in &table.rows {
                let reference = clean_value(&row.cells[indices[0]]);
                let fact = clean_value(&row.cells[indices[1]]);
                let implication = clean_value(&row.cells[indices[2]]);
                if [reference.as_str(), fact.as_str(), implication.as_str()]
                    .iter()
                    .all(|value| is_authored_value(value))
                {
                    self.draft.context.push(DraftContextInput {
                        reference,
                        fact,
                        implication,
                    });
                    self.map(
                        row.line,
                        format!("context[{}]", self.draft.context.len() - 1),
                        LegacyPlanMappingDisposition::MappedAuthored,
                    );
                }
            }
        }
    }

    fn parse_scope(&mut self) {
        for (keys, field) in [
            (&["goal"][..], "goal"),
            (&["deliverables"][..], "deliverables"),
            (&["in scope"][..], "in_scope"),
            (
                &["out of scope must not change", "out of scope"][..],
                "out_of_scope",
            ),
        ] {
            let Some(heading) = self.document.find_first(keys).cloned() else {
                continue;
            };
            if field == "goal" {
                if let Some((line, value)) = section_prose(&self.document, &heading) {
                    self.set_scope_field(field, vec![value], line);
                }
            } else {
                let values = section_list(&self.document, &heading);
                if !values.is_empty() {
                    let line = values[0].0;
                    self.set_scope_field(
                        field,
                        values.into_iter().map(|(_, value)| value).collect(),
                        line,
                    );
                }
            }
        }
    }

    fn parse_decisions(&mut self) {
        let Some(heading) = self
            .document
            .find_first(&[
                "decisions assumptions and open questions",
                "decisions assumptions and questions",
            ])
            .cloned()
        else {
            return;
        };
        for table in self.document.tables(&heading) {
            let Some(indices) =
                table_indices(&table, &["item", "type", "decision", "reason", "status"]).or_else(
                    || {
                        table_indices(
                            &table,
                            &["item", "type", "default decision", "reason", "status"],
                        )
                    },
                )
            else {
                continue;
            };
            for row in &table.rows {
                let values = indices
                    .iter()
                    .map(|index| clean_value(&row.cells[*index]))
                    .collect::<Vec<_>>();
                if values.iter().all(|value| is_authored_value(value)) {
                    self.draft.decisions.push(DraftDecisionInput {
                        item: values[0].clone(),
                        kind: values[1].clone(),
                        value: values[2].clone(),
                        reason: values[3].clone(),
                        status: values[4].clone(),
                    });
                    self.map(
                        row.line,
                        format!("decisions[{}]", self.draft.decisions.len() - 1),
                        LegacyPlanMappingDisposition::MappedAuthored,
                    );
                }
            }
        }
    }

    fn parse_edge_cases(&mut self) {
        let Some(heading) = self.document.find_first(&["edge cases"]).cloned() else {
            return;
        };
        for table in self.document.tables(&heading) {
            let Some(indices) = table_indices(&table, &["case", "expected behavior", "covered by"])
            else {
                continue;
            };
            for row in &table.rows {
                let case_ = clean_value(&row.cells[indices[0]]);
                let expected_behavior = clean_value(&row.cells[indices[1]]);
                if !is_authored_value(&case_) || !is_authored_value(&expected_behavior) {
                    continue;
                }
                self.draft.edge_cases.push(DraftEdgeCaseInput {
                    case_,
                    expected_behavior,
                    covered_by: split_cell_list(&row.cells[indices[2]]),
                });
                self.map(
                    row.line,
                    format!("edge_cases[{}]", self.draft.edge_cases.len() - 1),
                    LegacyPlanMappingDisposition::MappedAuthored,
                );
            }
        }
    }

    fn parse_global_file_map(&mut self) -> BTreeMap<String, Vec<DraftFileInput>> {
        let mut files = BTreeMap::<String, Vec<DraftFileInput>>::new();
        let headings = self
            .document
            .headings
            .iter()
            .filter(|heading| heading.key == "file map" && !self.is_inside_task(heading.line))
            .cloned()
            .collect::<Vec<_>>();
        for heading in headings {
            for table in self.document.tables(&heading) {
                let Some(indices) = table_indices(&table, &["path", "change", "reason", "task"])
                else {
                    continue;
                };
                for row in &table.rows {
                    let task = clean_value(&row.cells[indices[3]]);
                    let Some(file) = self.parse_file_row(
                        row.line,
                        &row.cells[indices[0]],
                        &row.cells[indices[1]],
                        &row.cells[indices[2]],
                    ) else {
                        continue;
                    };
                    if !is_original_task_id(&task) {
                        self.warn(
                            "legacy_plan_invalid_file_task",
                            Some(row.line),
                            format!("File Map task {task} is not an original T<n> task"),
                        );
                        continue;
                    }
                    let task_files = files.entry(task.clone()).or_default();
                    if let Some(existing) = task_files
                        .iter()
                        .find(|existing| existing.path == file.path)
                    {
                        if existing != &file {
                            self.warn(
                                "legacy_plan_conflicting_file_map",
                                Some(row.line),
                                format!(
                                    "Conflicting duplicate File Map path {} was omitted",
                                    file.path
                                ),
                            );
                        }
                        continue;
                    }
                    task_files.push(file);
                    self.map(
                        row.line,
                        format!("tasks.{task}.files"),
                        LegacyPlanMappingDisposition::MappedAuthored,
                    );
                }
            }
        }
        files
    }

    fn parse_git_flow(&mut self) -> BTreeMap<String, DraftCommitGateInput> {
        let mut gates = BTreeMap::new();
        let Some(heading) = self.document.find_first(&["git flow"]).cloned() else {
            return gates;
        };
        for table in self.document.tables(&heading) {
            let Some(task_index) = header_index(&table, &["task"]) else {
                continue;
            };
            let Some(required_index) = header_index(&table, &["commit required", "required"])
            else {
                continue;
            };
            let Some(message_index) =
                header_index(&table, &["planned commit message", "planned message"])
            else {
                continue;
            };
            let Some(scope_index) = header_index(&table, &["commit scope", "scope"]) else {
                continue;
            };
            for row in &table.rows {
                let task = clean_value(&row.cells[task_index]);
                if !is_original_task_id(&task) {
                    continue;
                }
                let Some(required) = parse_bool(&row.cells[required_index]) else {
                    self.warn(
                        "legacy_plan_invalid_commit_required",
                        Some(row.line),
                        format!("Task {task} has an unsupported commit-required value"),
                    );
                    continue;
                };
                let message = clean_value(&row.cells[message_index]);
                let scope = self.safe_scope(&row.cells[scope_index], row.line);
                if required && (!is_authored_value(&message) || scope.is_empty()) {
                    self.warn(
                        "legacy_plan_incomplete_commit_gate",
                        Some(row.line),
                        format!(
                            "Task {task} required commit declaration is incomplete and was omitted"
                        ),
                    );
                    continue;
                }
                if gates.contains_key(&task) {
                    self.warn(
                        "legacy_plan_duplicate_commit_gate",
                        Some(row.line),
                        format!("A duplicate Git Flow declaration for {task} was omitted"),
                    );
                    continue;
                }
                gates.insert(
                    task.clone(),
                    DraftCommitGateInput {
                        required,
                        planned_message: if is_authored_value(&message) {
                            message
                        } else {
                            String::new()
                        },
                        scope,
                    },
                );
                self.map(
                    row.line,
                    format!("tasks.{task}.commit_gate"),
                    LegacyPlanMappingDisposition::MappedAuthored,
                );
                for historical in [
                    "commit status",
                    "actual commit hash",
                    "actual commit",
                    "committed files",
                    "git evidence",
                    "evidence",
                ] {
                    if header_index(&table, &[historical]).is_some() {
                        self.ignore_historical(row.line, historical);
                    }
                }
            }
        }
        gates
    }

    fn parse_tasks(
        &mut self,
        mut global_files: BTreeMap<String, Vec<DraftFileInput>>,
        mut commit_gates: BTreeMap<String, DraftCommitGateInput>,
    ) {
        let headings = self
            .document
            .headings
            .iter()
            .filter_map(|heading| {
                parse_task_heading(heading).map(|(id, title)| (heading.clone(), id, title))
            })
            .collect::<Vec<_>>();
        let mut expected_number = 1usize;
        let mut seen = BTreeSet::new();
        for (heading, id, title) in headings {
            if self.draft.tasks.len() >= MAX_IMPORTED_TASKS {
                self.warn(
                    "legacy_plan_task_limit",
                    Some(heading.line),
                    format!("Only the first {MAX_IMPORTED_TASKS} supported tasks can be imported"),
                );
                break;
            }
            if !seen.insert(id.clone()) {
                self.warn(
                    "legacy_plan_duplicate_task_id",
                    Some(heading.line),
                    format!("Duplicate task {id} was omitted"),
                );
                continue;
            }
            let expected = format!("T{expected_number}");
            if id != expected {
                self.warn(
                    "legacy_plan_noncontiguous_task_id",
                    Some(heading.line),
                    format!("Expected {expected} but found {id}; the task was omitted"),
                );
                continue;
            }
            if !is_authored_value(&title) {
                self.warn(
                    "legacy_plan_placeholder_task",
                    Some(heading.line),
                    format!("Task {id} has no authored title and was omitted"),
                );
                continue;
            }
            let task_id = TaskId::parse(id.clone()).expect("validated original task ID");
            let mut task = DraftTaskInput {
                id: Some(task_id.clone()),
                title,
                depends_on: self.parse_dependencies(&heading, &task_id),
                steps: self.parse_task_steps(&heading),
                files: global_files.remove(&id).unwrap_or_default(),
                acceptance_criteria: self.parse_task_criteria(&heading, &task_id),
                verification: self.parse_task_verification(&heading, &task_id),
                commit_gate: commit_gates.remove(&id),
            };
            self.merge_task_files(&heading, &id, &mut task.files);
            if task.commit_gate.is_none() {
                task.commit_gate = self.parse_task_commit_gate(&heading, &id);
            }
            self.map(
                heading.line,
                format!("tasks.{id}"),
                LegacyPlanMappingDisposition::MappedAuthored,
            );
            self.draft.tasks.push(task);
            expected_number += 1;
        }
        for task in global_files.keys().chain(commit_gates.keys()) {
            self.warn(
                "legacy_plan_orphan_task_declaration",
                None,
                format!("Authored declaration for omitted or missing task {task} was not imported"),
            );
        }
    }

    fn parse_global_verification(&mut self) {
        let Some(heading) = self.document.find_first(&["verification plan"]).cloned() else {
            return;
        };
        let mut number = 1usize;
        for table in self.document.tables(&heading) {
            let Some(command_index) = header_index(&table, &["command", "command steps"]) else {
                continue;
            };
            let cwd_index = header_index(&table, &["cwd"]);
            let exit_index = header_index(&table, &["expected exit"]);
            let required_index = header_index(&table, &["required"]);
            for row in &table.rows {
                let Some(command) = self.safe_command(&row.cells[command_index], row.line) else {
                    continue;
                };
                let cwd = cwd_index
                    .map(|index| clean_value(&row.cells[index]))
                    .filter(|value| is_safe_cwd(value))
                    .unwrap_or_else(|| ".".to_owned());
                let expected_exit_code = exit_index
                    .and_then(|index| clean_value(&row.cells[index]).parse::<i32>().ok())
                    .unwrap_or_default();
                let required = required_index
                    .and_then(|index| parse_bool(&row.cells[index]))
                    .unwrap_or(true);
                let id = CheckId::parse(format!("LEGACY-GLOBAL-V{number}"))
                    .expect("generated global check ID");
                self.draft.verification_plan.push(DraftVerificationInput {
                    id,
                    command,
                    cwd,
                    expected_exit_code,
                    required,
                });
                self.map(
                    row.line,
                    format!("verification_plan[{}]", number - 1),
                    LegacyPlanMappingDisposition::MappedAuthored,
                );
                self.ignore_table_history(&table, row);
                number += 1;
            }
        }
    }

    fn classify_remaining_sections(&mut self) {
        let headings = self.document.headings.clone();
        for heading in headings {
            if parse_task_heading(&heading).is_some() || is_known_authored_heading(&heading.key) {
                continue;
            }
            if is_historical_heading(&heading.key) {
                self.ignore_historical(heading.line, &heading.key);
            } else if !is_structural_heading(&heading.key) {
                self.map(
                    heading.line,
                    "manual_review.unsupported_section",
                    LegacyPlanMappingDisposition::Unsupported,
                );
                self.warn(
                    "legacy_plan_unsupported_section",
                    Some(heading.line),
                    format!("Section {} has no supported Draft mapping", heading.title),
                );
            }
        }
    }

    fn classify_historical_fields(&mut self) {
        let source_lines = self
            .document
            .lines
            .iter()
            .enumerate()
            .filter(|(index, _)| self.document.visible[*index])
            .map(|(index, line)| (index + 1, (*line).to_owned()))
            .collect::<Vec<_>>();
        let table_lines = source_lines
            .iter()
            .map(|(line, source)| (*line, source.as_str()))
            .collect::<Vec<_>>();
        for table in parse_tables(&table_lines) {
            for row in key_value_rows(&[table]) {
                let key = heading_key(&row.cells[0]);
                if is_historical_key(&key) && is_authored_value(&clean_value(&row.cells[1])) {
                    self.ignore_historical(row.line, &key);
                }
            }
        }
        for (line, source) in source_lines {
            let trimmed = source.trim();
            if trimmed.starts_with('|') {
                continue;
            }
            let Some((field, value)) = trimmed.trim_matches('*').split_once(':') else {
                continue;
            };
            let key = heading_key(field);
            if is_historical_key(&key) && is_authored_value(&clean_value(value)) {
                self.ignore_historical(line, &key);
            }
        }
    }

    fn add_provenance(&mut self) {
        self.draft.context.insert(
            0,
            DraftContextInput {
                reference: self.source.path.clone(),
                fact: format!(
                    "Legacy Markdown source preserved as {} bytes with digest {}.",
                    self.source.bytes, self.source.digest
                ),
                implication: "Imported authored fields require explicit review; historical statuses, approvals, checks, commits, and evidence remain unverified."
                    .to_owned(),
            },
        );
    }

    fn set_name(&mut self, value: String, line: usize) {
        if self.suggested_name.is_some() {
            self.warn_duplicate(line, "metadata.name");
            return;
        }
        self.suggested_name = Some(value.clone());
        self.metadata().name = Some(value);
        self.map(
            line,
            "metadata.name",
            LegacyPlanMappingDisposition::MappedAuthored,
        );
    }

    fn set_summary(&mut self, value: String, line: usize) {
        if self.draft.summary.is_some() {
            self.warn_duplicate(line, "summary");
            return;
        }
        self.draft.summary = Some(value);
        self.map(
            line,
            "summary",
            LegacyPlanMappingDisposition::MappedAuthored,
        );
    }

    fn set_original_request(&mut self, value: String, line: usize) {
        if self.original_request.is_some() {
            self.warn_duplicate(line, "original_request");
            return;
        }
        self.original_request = Some(value);
        self.map(
            line,
            "original_request",
            LegacyPlanMappingDisposition::MappedAuthored,
        );
    }

    fn set_metadata_field(&mut self, field: &str, value: String, line: usize) {
        let metadata = self.metadata();
        let destination = match field {
            "priority" => &mut metadata.priority,
            "plan_type" => &mut metadata.plan_type,
            "area" => &mut metadata.area,
            "owner" => &mut metadata.owner,
            _ => unreachable!("known metadata field"),
        };
        if destination.is_some() {
            self.warn_duplicate(line, &format!("metadata.{field}"));
            return;
        }
        *destination = Some(value);
        self.map(
            line,
            format!("metadata.{field}"),
            LegacyPlanMappingDisposition::MappedAuthored,
        );
    }

    fn set_scope_field(&mut self, field: &str, values: Vec<String>, line: usize) {
        let is_duplicate = self.draft.scope.as_ref().is_some_and(|scope| match field {
            "goal" => scope.goal.is_some(),
            "deliverables" => scope.deliverables.is_some(),
            "in_scope" => scope.in_scope.is_some(),
            "out_of_scope" => scope.out_of_scope.is_some(),
            _ => unreachable!("known scope field"),
        });
        if is_duplicate {
            self.warn_duplicate(line, &format!("scope.{field}"));
            return;
        }
        let scope = self.scope();
        match field {
            "goal" => scope.goal = values.into_iter().next(),
            "deliverables" => scope.deliverables = Some(values),
            "in_scope" => scope.in_scope = Some(values),
            "out_of_scope" => scope.out_of_scope = Some(values),
            _ => unreachable!("known scope field"),
        }
        self.map(
            line,
            format!("scope.{field}"),
            LegacyPlanMappingDisposition::MappedAuthored,
        );
    }

    fn metadata(&mut self) -> &mut DraftMetadataInput {
        self.draft
            .metadata
            .get_or_insert_with(DraftMetadataInput::default)
    }

    fn scope(&mut self) -> &mut DraftScopeInput {
        self.draft
            .scope
            .get_or_insert_with(DraftScopeInput::default)
    }

    fn parse_file_row(
        &mut self,
        line: usize,
        path: &str,
        change: &str,
        reason: &str,
    ) -> Option<DraftFileInput> {
        let path = clean_value(path);
        let reason = clean_value(reason);
        if !is_safe_import_path(&path) {
            self.warn(
                "legacy_plan_unsafe_path",
                Some(line),
                format!("Unsafe or Mino-owned path {path} was omitted"),
            );
            return None;
        }
        let Some(change) = parse_file_change(change) else {
            self.warn(
                "legacy_plan_unknown_file_change",
                Some(line),
                format!(
                    "Unsupported file change {} was omitted",
                    clean_value(change)
                ),
            );
            return None;
        };
        if !is_authored_value(&reason) {
            return None;
        }
        Some(DraftFileInput {
            path,
            change,
            reason,
        })
    }

    fn safe_scope(&mut self, value: &str, line: usize) -> Vec<String> {
        split_cell_list(value)
            .into_iter()
            .filter(|path| {
                if is_safe_import_path(path) {
                    true
                } else {
                    self.warn(
                        "legacy_plan_unsafe_commit_scope",
                        Some(line),
                        format!("Unsafe or Mino-owned commit scope {path} was omitted"),
                    );
                    false
                }
            })
            .collect()
    }

    fn safe_command(&mut self, value: &str, line: usize) -> Option<Vec<String>> {
        match parse_command(value) {
            Ok(command) => Some(command),
            Err(message) => {
                self.warn("legacy_plan_unsafe_command", Some(line), message);
                None
            }
        }
    }

    fn parse_dependencies(&mut self, heading: &Heading, task_id: &TaskId) -> Vec<TaskId> {
        let lines = self.document.section_lines(heading);
        let mut source = None;
        for (line, value) in &lines {
            if let Some((field, value)) = value.split_once(':')
                && heading_key(field.trim_matches('*')) == "depends on"
            {
                source = Some((*line, clean_value(value)));
                break;
            }
        }
        if source.is_none() {
            for table in parse_tables(&lines) {
                for row in key_value_rows(&[table]) {
                    if heading_key(&row.cells[0]) == "depends on" {
                        source = Some((row.line, clean_value(&row.cells[1])));
                        break;
                    }
                }
            }
        }
        let Some((line, value)) = source else {
            return Vec::new();
        };
        if matches!(heading_key(&value).as_str(), "none" | "n a") {
            return Vec::new();
        }
        let current_number = task_number(task_id.as_str()).unwrap_or_default();
        let mut dependencies = Vec::new();
        for dependency in split_cell_list(&value) {
            let Ok(parsed) = TaskId::parse(dependency.clone()) else {
                self.warn(
                    "legacy_plan_invalid_dependency",
                    Some(line),
                    format!("Task {task_id} dependency {dependency} was omitted"),
                );
                continue;
            };
            if !is_original_task_id(parsed.as_str())
                || task_number(parsed.as_str()).is_none_or(|number| number >= current_number)
                || dependencies.contains(&parsed)
            {
                self.warn(
                    "legacy_plan_invalid_dependency",
                    Some(line),
                    format!("Task {task_id} dependency {dependency} was omitted"),
                );
                continue;
            }
            dependencies.push(parsed);
        }
        dependencies
    }

    fn parse_task_steps(&mut self, heading: &Heading) -> Vec<String> {
        if let Some(section) = self.child_heading(heading, &["steps"]) {
            return section_list(&self.document, &section)
                .into_iter()
                .map(|(_, value)| value)
                .collect();
        }
        bold_block_list(&self.document, heading, "do")
    }

    fn parse_task_criteria(
        &mut self,
        heading: &Heading,
        task_id: &TaskId,
    ) -> Vec<DraftCriterionInput> {
        if let Some(section) = self.child_heading(heading, &["acceptance criteria"]) {
            let mut criteria = Vec::new();
            for table in self.document.tables(&section) {
                let Some(description_index) = header_index(&table, &["description"]) else {
                    continue;
                };
                let id_index = header_index(&table, &["id"]);
                for row in &table.rows {
                    let description = clean_value(&row.cells[description_index]);
                    if !is_authored_value(&description) {
                        continue;
                    }
                    let expected = format!("{task_id}-A{}", criteria.len() + 1);
                    let id = id_index
                        .map(|index| clean_value(&row.cells[index]))
                        .filter(|value| value == &expected)
                        .and_then(|value| CriterionId::parse(value).ok());
                    criteria.push(DraftCriterionInput { id, description });
                    self.map(
                        row.line,
                        format!(
                            "tasks.{task_id}.acceptance_criteria[{}]",
                            criteria.len() - 1
                        ),
                        LegacyPlanMappingDisposition::MappedAuthored,
                    );
                    self.ignore_table_history(&table, row);
                }
            }
            if !criteria.is_empty() {
                return criteria;
            }
        }
        let block = bold_block_lines(&self.document, heading, "acceptance criteria");
        let mut criteria = Vec::new();
        for (line, source) in block {
            let Some((checked, description)) = checkbox_item(&source) else {
                continue;
            };
            if !is_authored_value(&description) {
                continue;
            }
            if checked {
                self.warn(
                    "legacy_plan_historical_assertion_unverified",
                    Some(line),
                    format!("Checked criterion for task {task_id} was reset to Pending"),
                );
            }
            criteria.push(DraftCriterionInput {
                id: None,
                description,
            });
            self.map(
                line,
                format!(
                    "tasks.{task_id}.acceptance_criteria[{}]",
                    criteria.len() - 1
                ),
                LegacyPlanMappingDisposition::MappedAuthored,
            );
        }
        criteria
    }

    fn parse_task_verification(
        &mut self,
        heading: &Heading,
        task_id: &TaskId,
    ) -> Vec<DraftVerificationInput> {
        let mut checks = Vec::new();
        if let Some(section) = self.child_heading(heading, &["verification checks"]) {
            for table in self.document.tables(&section) {
                let Some(command_index) = header_index(&table, &["command", "command steps"])
                else {
                    continue;
                };
                for row in &table.rows {
                    if let Some(check) =
                        self.verification_row(&table, row, command_index, task_id, checks.len() + 1)
                    {
                        checks.push(check);
                    }
                }
            }
            if !checks.is_empty() {
                return checks;
            }
        }
        let block = bold_block_lines(&self.document, heading, "verification");
        for (line, source) in block {
            let Some((field, value)) = source
                .trim()
                .trim_start_matches(['-', '*'])
                .trim()
                .split_once(':')
            else {
                continue;
            };
            if heading_key(field) != "command steps" {
                continue;
            }
            let Some(command) = self.safe_command(value, line) else {
                continue;
            };
            let id = CheckId::parse(format!("{task_id}-V{}", checks.len() + 1))
                .expect("generated task check ID");
            checks.push(DraftVerificationInput {
                id,
                command,
                cwd: ".".to_owned(),
                expected_exit_code: 0,
                required: true,
            });
            self.map(
                line,
                format!("tasks.{task_id}.verification[{}]", checks.len() - 1),
                LegacyPlanMappingDisposition::MappedAuthored,
            );
        }
        checks
    }

    fn verification_row(
        &mut self,
        table: &Table,
        row: &TableRow,
        command_index: usize,
        task_id: &TaskId,
        number: usize,
    ) -> Option<DraftVerificationInput> {
        let command = self.safe_command(&row.cells[command_index], row.line)?;
        let cwd = header_index(table, &["cwd"])
            .map(|index| clean_value(&row.cells[index]))
            .filter(|value| is_safe_cwd(value))
            .unwrap_or_else(|| ".".to_owned());
        let expected_exit_code = header_index(table, &["expected exit"])
            .and_then(|index| clean_value(&row.cells[index]).parse::<i32>().ok())
            .unwrap_or_default();
        let required = header_index(table, &["required"])
            .and_then(|index| parse_bool(&row.cells[index]))
            .unwrap_or(true);
        let id = CheckId::parse(format!("{task_id}-V{number}")).expect("generated task check ID");
        self.map(
            row.line,
            format!("tasks.{task_id}.verification[{}]", number - 1),
            LegacyPlanMappingDisposition::MappedAuthored,
        );
        self.ignore_table_history(table, row);
        Some(DraftVerificationInput {
            id,
            command,
            cwd,
            expected_exit_code,
            required,
        })
    }

    fn merge_task_files(
        &mut self,
        heading: &Heading,
        task_id: &str,
        files: &mut Vec<DraftFileInput>,
    ) {
        let Some(section) = self.child_heading(heading, &["file map"]) else {
            return;
        };
        for table in self.document.tables(&section) {
            let Some(path_index) = header_index(&table, &["path"]) else {
                continue;
            };
            let Some(change_index) = header_index(&table, &["change"]) else {
                continue;
            };
            let Some(reason_index) = header_index(&table, &["reason"]) else {
                continue;
            };
            for row in table.rows {
                let Some(file) = self.parse_file_row(
                    row.line,
                    &row.cells[path_index],
                    &row.cells[change_index],
                    &row.cells[reason_index],
                ) else {
                    continue;
                };
                if let Some(existing) = files.iter().find(|existing| existing.path == file.path) {
                    if existing != &file {
                        self.warn(
                            "legacy_plan_conflicting_file_map",
                            Some(row.line),
                            format!(
                                "Conflicting duplicate File Map path {} was omitted",
                                file.path
                            ),
                        );
                    }
                    continue;
                }
                files.push(file);
                self.map(
                    row.line,
                    format!("tasks.{task_id}.files"),
                    LegacyPlanMappingDisposition::MappedAuthored,
                );
            }
        }
    }

    fn parse_task_commit_gate(
        &mut self,
        heading: &Heading,
        task_id: &str,
    ) -> Option<DraftCommitGateInput> {
        let section = self.child_heading(heading, &["commit gate"])?;
        let rows = key_value_rows(&self.document.tables(&section));
        let mut values = BTreeMap::new();
        for row in &rows {
            values.insert(
                heading_key(&row.cells[0]),
                (row.line, clean_value(&row.cells[1])),
            );
        }
        let (line, required_value) = values.get("required")?;
        let required = parse_bool(required_value)?;
        let message = values
            .get("planned message")
            .map(|(_, value)| value.clone())
            .unwrap_or_default();
        let scope = values
            .get("scope")
            .map(|(line, value)| self.safe_scope(value, *line))
            .unwrap_or_default();
        for historical in ["status", "actual commit", "committed files", "evidence"] {
            if let Some((line, _)) = values.get(historical) {
                self.ignore_historical(*line, historical);
            }
        }
        if required && (!is_authored_value(&message) || scope.is_empty()) {
            self.warn(
                "legacy_plan_incomplete_commit_gate",
                Some(*line),
                format!("Task {task_id} required commit declaration is incomplete and was omitted"),
            );
            return None;
        }
        self.map(
            *line,
            format!("tasks.{task_id}.commit_gate"),
            LegacyPlanMappingDisposition::MappedAuthored,
        );
        Some(DraftCommitGateInput {
            required,
            planned_message: message,
            scope,
        })
    }

    fn child_heading(&self, parent: &Heading, keys: &[&str]) -> Option<Heading> {
        self.document
            .headings
            .iter()
            .find(|heading| {
                heading.line > parent.line
                    && heading.line < parent.end_line
                    && heading.level > parent.level
                    && keys.contains(&heading.key.as_str())
            })
            .cloned()
    }

    fn is_inside_task(&self, line: usize) -> bool {
        self.document.headings.iter().any(|heading| {
            parse_task_heading(heading).is_some() && line > heading.line && line < heading.end_line
        })
    }

    fn ignore_table_history(&mut self, table: &Table, row: &TableRow) {
        for historical in ["status", "evidence", "actual commit", "committed files"] {
            if header_index(table, &[historical]).is_some() {
                self.ignore_historical(row.line, historical);
            }
        }
    }

    fn ignore_historical(&mut self, line: usize, field: &str) {
        self.map(
            line,
            format!("unverified_history.{field}"),
            LegacyPlanMappingDisposition::IgnoredHistorical,
        );
        self.warn(
            "legacy_plan_historical_state_unverified",
            Some(line),
            format!("Historical {field} was ignored and cannot affect the imported Draft"),
        );
    }

    fn warn_duplicate(&mut self, line: usize, target: &str) {
        self.warn(
            "legacy_plan_duplicate_mapping",
            Some(line),
            format!("A later value for {target} was ignored"),
        );
    }

    fn map(
        &mut self,
        source_line: usize,
        target: impl Into<String>,
        disposition: LegacyPlanMappingDisposition,
    ) {
        if self.mappings.len() >= MAX_REPORT_ENTRIES {
            self.warn(
                "legacy_plan_mapping_limit",
                None,
                format!("Mapping report was capped at {MAX_REPORT_ENTRIES} entries"),
            );
            return;
        }
        let target = target.into();
        if !self
            .mapping_keys
            .insert((source_line, target.clone(), disposition))
        {
            return;
        }
        let source_fragment = self.document.line(source_line).trim().to_owned();
        self.mappings.push(LegacyPlanMapping {
            source_line,
            source_fragment,
            target,
            disposition,
        });
    }

    fn warn(
        &mut self,
        code: impl Into<String>,
        source_line: Option<usize>,
        message: impl Into<String>,
    ) {
        let code = code.into();
        let message = message.into();
        let key = (code.clone(), source_line, message.clone());
        if self.warnings.len() < MAX_REPORT_ENTRIES && self.warning_keys.insert(key) {
            self.warnings.push(LegacyPlanWarning {
                code,
                source_line,
                message,
            });
        }
    }
}

fn preview_draft(draft: &DraftPlanInput, original_request: &str) -> Result<(), MinoError> {
    let timestamp =
        Timestamp::parse("2000-01-01T00:00:00Z").expect("static preview timestamp must be valid");
    let plan_id = PlanId::parse("2000-01-01-legacy-import-preview")
        .expect("static preview plan ID must be valid");
    let mut plan = Plan::new(plan_id, original_request, timestamp.clone());
    plan.apply_draft_input(draft.clone(), timestamp)
        .map_err(|error| validation_error(format!("Legacy authored fields are invalid: {error}")))
}

fn visible_lines(lines: &[&str]) -> Vec<bool> {
    let mut visible = Vec::with_capacity(lines.len());
    let mut fence: Option<(char, usize)> = None;
    for line in lines {
        let trimmed = line.trim_start();
        let marker = fence_marker(trimmed);
        if let Some((fence_character, fence_length)) = fence {
            visible.push(false);
            if marker.is_some_and(|(character, length)| {
                character == fence_character && length >= fence_length
            }) {
                fence = None;
            }
        } else if let Some(marker) = marker {
            visible.push(false);
            fence = Some(marker);
        } else {
            visible.push(true);
        }
    }
    visible
}

fn fence_marker(value: &str) -> Option<(char, usize)> {
    let character = value.chars().next()?;
    if !matches!(character, '`' | '~') {
        return None;
    }
    let length = value
        .chars()
        .take_while(|current| *current == character)
        .count();
    (length >= 3).then_some((character, length))
}

fn parse_heading(line: &str) -> Option<(usize, String)> {
    let trimmed = line.trim_start().trim_start_matches('\u{feff}');
    let level = trimmed.bytes().take_while(|byte| *byte == b'#').count();
    if !(1..=6).contains(&level) || trimmed.as_bytes().get(level) != Some(&b' ') {
        return None;
    }
    let title = trimmed[level + 1..]
        .trim()
        .trim_end_matches('#')
        .trim()
        .to_owned();
    (!title.is_empty()).then_some((level, title))
}

fn heading_key(value: &str) -> String {
    let cleaned = clean_value(value).to_ascii_lowercase();
    let mut result = String::new();
    let mut needs_space = false;
    for character in cleaned.chars() {
        if character.is_ascii_alphanumeric() {
            if needs_space && !result.is_empty() {
                result.push(' ');
            }
            result.push(character);
            needs_space = false;
        } else if !result.is_empty() {
            needs_space = true;
        }
    }
    result
}

fn parse_task_heading(heading: &Heading) -> Option<(String, String)> {
    if heading.level != 3 {
        return None;
    }
    let cleaned = clean_value(&heading.title);
    let (raw_id, title) = cleaned.split_once(':')?;
    let raw_id = clean_value(raw_id.trim().strip_prefix("Task ").unwrap_or(raw_id.trim()));
    if !is_original_task_id(&raw_id) {
        return None;
    }
    Some((raw_id, title.trim().to_owned()))
}

fn parse_tables(lines: &[(usize, &str)]) -> Vec<Table> {
    let mut tables = Vec::new();
    let mut index = 0usize;
    while index + 1 < lines.len() {
        let Some(headers) = split_table_row(lines[index].1) else {
            index += 1;
            continue;
        };
        let Some(separator) = split_table_row(lines[index + 1].1) else {
            index += 1;
            continue;
        };
        if headers.is_empty()
            || headers.len() != separator.len()
            || !separator.iter().all(|cell| is_table_separator(cell))
        {
            index += 1;
            continue;
        }
        let normalized_headers = headers
            .iter()
            .map(|header| heading_key(header))
            .collect::<Vec<_>>();
        let mut rows = Vec::new();
        index += 2;
        while index < lines.len() {
            let Some(cells) = split_table_row(lines[index].1) else {
                break;
            };
            if cells.len() != normalized_headers.len() {
                break;
            }
            rows.push(TableRow {
                line: lines[index].0,
                cells,
            });
            index += 1;
        }
        tables.push(Table {
            headers: normalized_headers,
            rows,
        });
    }
    tables
}

fn split_table_row(line: &str) -> Option<Vec<String>> {
    let trimmed = line.trim();
    if !trimmed.starts_with('|') || !trimmed.ends_with('|') {
        return None;
    }
    let body = &trimmed[1..trimmed.len() - 1];
    let mut cells = Vec::new();
    let mut current = String::new();
    let mut escaped = false;
    for character in body.chars() {
        if escaped {
            current.push('\\');
            current.push(character);
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character == '|' {
            cells.push(clean_value(&current));
            current.clear();
        } else {
            current.push(character);
        }
    }
    if escaped {
        current.push('\\');
    }
    cells.push(clean_value(&current));
    Some(cells)
}

fn is_table_separator(value: &str) -> bool {
    let value = value.trim_matches(':');
    value.len() >= 3 && value.bytes().all(|byte| byte == b'-')
}

fn table_indices<const N: usize>(table: &Table, names: &[&str; N]) -> Option<[usize; N]> {
    let mut indices = [0usize; N];
    for (position, name) in names.iter().enumerate() {
        indices[position] = header_index(table, &[*name])?;
    }
    Some(indices)
}

fn header_index(table: &Table, aliases: &[&str]) -> Option<usize> {
    table
        .headers
        .iter()
        .position(|header| aliases.contains(&header.as_str()))
}

fn key_value_rows(tables: &[Table]) -> Vec<TableRow> {
    tables
        .iter()
        .filter(|table| table_indices(table, &["field", "value"]).is_some())
        .flat_map(|table| table.rows.clone())
        .collect()
}

fn section_prose(document: &Document<'_>, heading: &Heading) -> Option<(usize, String)> {
    let mut values = Vec::new();
    let mut first_line = None;
    for (line, source) in document.section_lines(heading) {
        let value = source.trim();
        if value.is_empty()
            || value.starts_with('|')
            || value.starts_with('#')
            || value.starts_with("<!--")
            || value.starts_with("**")
            || list_item(value).is_some()
        {
            continue;
        }
        let value = clean_value(value);
        if is_authored_value(&value) {
            first_line.get_or_insert(line);
            values.push(value);
        }
    }
    first_line.map(|line| (line, values.join("\n")))
}

fn section_list(document: &Document<'_>, heading: &Heading) -> Vec<(usize, String)> {
    document
        .section_lines(heading)
        .into_iter()
        .filter_map(|(line, source)| list_item(source).map(|value| (line, value)))
        .filter(|(_, value)| is_authored_value(value))
        .collect()
}

fn bold_block_lines(document: &Document<'_>, heading: &Heading, key: &str) -> Vec<(usize, String)> {
    let lines = document.section_lines(heading);
    let Some(start) = lines.iter().position(|(_, line)| {
        let trimmed = line.trim();
        trimmed.starts_with("**")
            && trimmed.ends_with("**")
            && heading_key(trimmed.trim_matches('*')) == key
    }) else {
        return Vec::new();
    };
    lines[start + 1..]
        .iter()
        .take_while(|(_, line)| {
            let trimmed = line.trim();
            !(trimmed.starts_with("**") && trimmed.ends_with("**"))
        })
        .map(|(line, source)| (*line, (*source).to_owned()))
        .collect()
}

fn bold_block_list(document: &Document<'_>, heading: &Heading, key: &str) -> Vec<String> {
    bold_block_lines(document, heading, key)
        .into_iter()
        .filter_map(|(_, source)| list_item(&source))
        .filter(|value| is_authored_value(value))
        .collect()
}

fn list_item(value: &str) -> Option<String> {
    let trimmed = value.trim();
    let value = ['-', '*', '+']
        .into_iter()
        .find_map(|marker| {
            trimmed
                .strip_prefix(marker)
                .and_then(|value| value.strip_prefix(' '))
        })
        .or_else(|| {
            let (number, value) = trimmed.split_once(". ")?;
            number
                .bytes()
                .all(|byte| byte.is_ascii_digit())
                .then_some(value)
        })?;
    let value = value
        .strip_prefix("[ ] ")
        .or_else(|| value.strip_prefix("[x] "))
        .or_else(|| value.strip_prefix("[X] "))
        .unwrap_or(value);
    Some(clean_value(value))
}

fn checkbox_item(value: &str) -> Option<(bool, String)> {
    let trimmed = value
        .trim()
        .strip_prefix("- ")
        .or_else(|| value.trim().strip_prefix("* "))?;
    if let Some(value) = trimmed.strip_prefix("[ ] ") {
        Some((false, clean_value(value)))
    } else {
        trimmed
            .strip_prefix("[x] ")
            .or_else(|| trimmed.strip_prefix("[X] "))
            .map(|value| (true, clean_value(value)))
    }
}

fn clean_value(value: &str) -> String {
    let mut value = value.trim().trim_end_matches("  ").trim().to_owned();
    let is_single_quoted_scalar =
        value.starts_with('"') && value.ends_with('"') && !value[1..value.len() - 1].contains('"');
    if value.len() >= 2
        && ((value.starts_with('`') && value.ends_with('`')) || is_single_quoted_scalar)
    {
        value = value[1..value.len() - 1].to_owned();
    }
    value = value
        .replace("<br>", "\n")
        .replace("<br/>", "\n")
        .replace("<br />", "\n");
    let mut cleaned = String::new();
    let mut escaped = false;
    for character in value.chars() {
        if escaped {
            if matches!(character, '\\' | '|' | '*' | '_' | '<' | '>' | '`') {
                cleaned.push(character);
            } else {
                cleaned.push('\\');
                cleaned.push(character);
            }
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else {
            cleaned.push(character);
        }
    }
    if escaped {
        cleaned.push('\\');
    }
    cleaned.trim().to_owned()
}

fn is_authored_value(value: &str) -> bool {
    let trimmed = value.trim();
    !(trimmed.is_empty()
        || matches!(
            heading_key(trimmed).as_str(),
            "n a" | "none" | "pending" | "unknown"
        )
        || trimmed.starts_with('<') && trimmed.ends_with('>'))
}

fn parse_file_change(value: &str) -> Option<FileChange> {
    match heading_key(value).as_str() {
        "create" => Some(FileChange::Create),
        "modify" => Some(FileChange::Modify),
        "delete" => Some(FileChange::Delete),
        "test" => Some(FileChange::Test),
        "n a" => Some(FileChange::NotApplicable),
        _ => None,
    }
}

fn parse_bool(value: &str) -> Option<bool> {
    match heading_key(value).as_str() {
        "yes" | "true" => Some(true),
        "no" | "false" => Some(false),
        _ => None,
    }
}

fn split_cell_list(value: &str) -> Vec<String> {
    let value = clean_value(value);
    let mut values = Vec::new();
    let mut current = String::new();
    let mut brace_depth = 0usize;
    for character in value.chars() {
        match character {
            '{' => {
                brace_depth += 1;
                current.push(character);
            }
            '}' => {
                brace_depth = brace_depth.saturating_sub(1);
                current.push(character);
            }
            ',' | '\n' if brace_depth == 0 => {
                let item = clean_value(&current);
                if is_authored_value(&item) {
                    values.push(item);
                }
                current.clear();
            }
            _ => current.push(character),
        }
    }
    let item = clean_value(&current);
    if is_authored_value(&item) {
        values.push(item);
    }
    values
}

fn parse_command(value: &str) -> Result<Vec<String>, String> {
    let cleaned = clean_value(value);
    if !is_authored_value(&cleaned) {
        return Err("Empty or placeholder verification command was omitted".to_owned());
    }
    let mut command = Vec::new();
    let bytes = cleaned.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if index == bytes.len() {
            break;
        }
        if bytes[index] == b'"' {
            let start = index;
            index += 1;
            let mut escaped = false;
            while index < bytes.len() {
                let byte = bytes[index];
                index += 1;
                if escaped {
                    escaped = false;
                } else if byte == b'\\' {
                    escaped = true;
                } else if byte == b'"' {
                    break;
                }
            }
            if bytes.get(index.saturating_sub(1)) != Some(&b'"') {
                return Err("Unclosed quoted verification argument was omitted".to_owned());
            }
            let token = serde_json::from_str::<String>(&cleaned[start..index])
                .map_err(|_| "Invalid quoted verification argument was omitted".to_owned())?;
            command.push(token);
        } else {
            let start = index;
            while index < bytes.len() && !bytes[index].is_ascii_whitespace() {
                index += 1;
            }
            command.push(cleaned[start..index].to_owned());
        }
    }
    if command.is_empty() || command.iter().any(|part| part.trim().is_empty()) {
        return Err("Incomplete verification command was omitted".to_owned());
    }
    if command.iter().any(|part| {
        part.contains(['\r', '\n', ';', '|', '&', '>', '<', '`'])
            || part.contains("$(")
            || part.contains("${")
    }) {
        return Err("Verification command containing shell control syntax was omitted".to_owned());
    }
    let executable = Path::new(&command[0])
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(&command[0])
        .trim_end_matches(".exe")
        .to_ascii_lowercase();
    if matches!(
        executable.as_str(),
        "sh" | "bash"
            | "zsh"
            | "cmd"
            | "powershell"
            | "pwsh"
            | "rm"
            | "rmdir"
            | "del"
            | "erase"
            | "move"
            | "mv"
    ) {
        return Err(format!(
            "Potentially mutating or shell verification executable {executable} was omitted"
        ));
    }
    Ok(command)
}

fn is_safe_cwd(value: &str) -> bool {
    value == "." || is_safe_import_path(value)
}

fn is_safe_import_path(value: &str) -> bool {
    let path = Path::new(value);
    let components = path
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_string_lossy().to_ascii_lowercase()),
            _ => None,
        })
        .collect::<Vec<_>>();
    !value.is_empty()
        && !value.contains('\\')
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
        && components
            .first()
            .is_none_or(|component| component != ".mino")
        && !matches!(components.as_slice(), [docs, plan, ..] if docs == "docs" && plan == "plan")
}

fn is_original_task_id(value: &str) -> bool {
    task_number(value).is_some()
}

fn task_number(value: &str) -> Option<usize> {
    value
        .strip_prefix('T')?
        .parse::<usize>()
        .ok()
        .filter(|number| *number > 0)
}

fn is_historical_key(key: &str) -> bool {
    matches!(
        key,
        "status"
            | "task id"
            | "plan id"
            | "revision"
            | "schema version"
            | "protocol"
            | "protocol version"
            | "protocol revision"
            | "created"
            | "updated"
            | "created at"
            | "updated at"
            | "branch"
            | "markdown path"
            | "git repository"
            | "git working tree"
            | "git base commit"
            | "git base status"
            | "git flow enabled"
            | "git flow consent"
            | "plan approved at"
            | "approved at"
            | "pre plan cleanup required"
            | "pre plan cleanup decision"
            | "resume status"
            | "blocker"
            | "evidence"
            | "actual commit"
            | "actual commit hash"
            | "committed files"
    )
}

fn is_historical_heading(key: &str) -> bool {
    matches!(
        key,
        "git readiness"
            | "git readiness decision"
            | "pre plan cleanup"
            | "progress log"
            | "implementation notes"
            | "verification results"
            | "approvals"
            | "protected amendments"
            | "review feedback"
            | "follow ups"
            | "lineage"
            | "final outcome"
            | "remaining risk"
            | "outcome follow up tasks"
            | "extensions"
    )
}

fn is_known_authored_heading(key: &str) -> bool {
    matches!(
        key,
        "metadata"
            | "summary"
            | "original request"
            | "context"
            | "current state and references"
            | "scope"
            | "goal"
            | "deliverables"
            | "in scope"
            | "out of scope"
            | "out of scope must not change"
            | "decisions assumptions and open questions"
            | "decisions assumptions and questions"
            | "approach"
            | "file map"
            | "interfaces data flow"
            | "interfaces and data flow"
            | "edge cases"
            | "git flow"
            | "verification plan"
            | "steps"
            | "acceptance criteria"
            | "verification checks"
            | "commit gate"
    )
}

fn is_structural_heading(key: &str) -> bool {
    matches!(key, "plan" | "tasks" | "implementation task order")
}

fn validation_error(message: impl Into<String>) -> MinoError {
    MinoError::new(ErrorCategory::IncompleteOrValidation, message)
}
