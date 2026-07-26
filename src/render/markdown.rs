//! Complete deterministic Markdown projection for a plan aggregate.

use serde_json::Value;

use crate::domain::Plan;
use crate::store::{canonical_json_bytes, sha256_digest};

use super::{RenderError, RenderErrorKind};

/// Current deterministic Markdown renderer version.
pub const RENDERER_VERSION: u32 = 2;

/// Byte-stable Markdown and the digests that bind it to source state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderedPlan {
    markdown: String,
    state_hash: String,
    projection_digest: String,
}

impl RenderedPlan {
    /// Returns the UTF-8 Markdown projection.
    #[must_use]
    pub fn markdown(&self) -> &str {
        &self.markdown
    }

    /// Returns the UTF-8 projection bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        self.markdown.as_bytes()
    }

    /// Returns the digest of canonical source-state bytes.
    #[must_use]
    pub fn state_hash(&self) -> &str {
        &self.state_hash
    }

    /// Returns the digest of the rendered Markdown bytes.
    #[must_use]
    pub fn projection_digest(&self) -> &str {
        &self.projection_digest
    }
}

/// Renders every authored, execution, verification, review, and outcome field.
///
/// # Errors
///
/// Returns an error when the plan cannot be serialized to canonical JSON.
pub fn render_plan(plan: &Plan) -> Result<RenderedPlan, RenderError> {
    let state_bytes = canonical_json_bytes(plan)
        .map_err(|error| RenderError::new(RenderErrorKind::Serialization, error.to_string()))?;
    let state_hash = sha256_digest(&state_bytes);
    let value = serde_json::to_value(plan)?;
    let markdown = render_document(&value, &state_hash);
    let projection_digest = sha256_digest(markdown.as_bytes());
    Ok(RenderedPlan {
        markdown,
        state_hash,
        projection_digest,
    })
}

fn render_document(plan: &Value, state_hash: &str) -> String {
    let mut output = String::new();
    render_front_matter(&mut output, plan, state_hash);
    render_heading_and_overview(&mut output, plan);
    render_original_request(&mut output, plan);
    render_summary(&mut output, plan);
    render_context(&mut output, plan);
    render_scope(&mut output, plan);
    render_decisions(&mut output, plan);
    render_approach(&mut output, plan);
    render_interfaces(&mut output, plan);
    render_edge_cases(&mut output, plan);
    render_standards(&mut output, plan);
    render_standards_conflicts(&mut output, plan);
    render_git_readiness(&mut output, plan);
    render_task_order(&mut output, plan);
    render_tasks(&mut output, plan);
    render_global_verification(&mut output, plan);
    render_approvals(&mut output, plan);
    render_amendments(&mut output, plan);
    render_review_items(&mut output, plan);
    render_follow_ups(&mut output, plan);
    render_lineage(&mut output, plan);
    render_archive(&mut output, plan);
    render_final_outcome(&mut output, plan);
    render_extensions(&mut output, plan);
    output
}

fn render_front_matter(output: &mut String, plan: &Value, state_hash: &str) {
    output.push_str("---\n");
    output.push_str("managed_by: mino\n");
    output.push_str("plan_id: ");
    output.push_str(text(&plan["id"]));
    output.push('\n');
    output.push_str("revision: ");
    output.push_str(&scalar(&plan["revision"]));
    output.push('\n');
    output.push_str("state_hash: ");
    output.push_str(state_hash);
    output.push('\n');
    output.push_str("renderer_version: ");
    output.push_str(&RENDERER_VERSION.to_string());
    output.push('\n');
    output.push_str("manual_editing: prohibited\n---\n\n");
}

fn render_heading_and_overview(output: &mut String, plan: &Value) {
    let name = text(&plan["metadata"]["name"]);
    let title = if name.trim().is_empty() {
        text(&plan["id"])
    } else {
        name
    };
    output.push_str("# ");
    output.push_str(&escape_inline(title));
    output.push_str(
        "\n\n> Managed by Mino. Manual editing is prohibited; update the source plan instead.\n\n",
    );
    write_table(
        output,
        &["Field", "Value"],
        vec![
            row("Plan ID", text(&plan["id"])),
            row("Status", text(&plan["status"])),
            row("Resume Status", &scalar(&plan["resume_status"])),
            row("Blocker", &scalar(&plan["blocker"])),
            row("Revision", &scalar(&plan["revision"])),
            row("Schema Version", &scalar(&plan["schema_version"])),
            row(
                "Protocol",
                &format!(
                    "{} / {}",
                    text(&plan["protocol_version"]["version"]),
                    text(&plan["protocol_version"]["revision"])
                ),
            ),
        ],
    );
    output.push_str("\n## Metadata\n\n");
    let metadata = &plan["metadata"];
    write_table(
        output,
        &["Field", "Value"],
        vec![
            row("Name", text(&metadata["name"])),
            row("Priority", text(&metadata["priority"])),
            row("Plan Type", text(&metadata["plan_type"])),
            row("Area", text(&metadata["area"])),
            row("Owner", text(&metadata["owner"])),
            row("Created At", text(&metadata["created_at"])),
            row("Updated At", text(&metadata["updated_at"])),
            row("Branch", &scalar(&metadata["branch"])),
            row("Markdown Path", &scalar(&metadata["markdown_path"])),
        ],
    );
}

fn render_original_request(output: &mut String, plan: &Value) {
    output.push_str("\n## Original Request\n\n");
    write_paragraph(output, text(&plan["original_request"]));
}

fn render_summary(output: &mut String, plan: &Value) {
    output.push_str("\n## Summary\n\n");
    write_paragraph(output, text(&plan["summary"]));
}

fn render_context(output: &mut String, plan: &Value) {
    output.push_str("\n## Context\n\n");
    let rows = array(&plan["context"])
        .iter()
        .map(|item| {
            vec![
                scalar(&item["reference"]),
                scalar(&item["fact"]),
                scalar(&item["implication"]),
            ]
        })
        .collect::<Vec<_>>();
    write_optional_table(output, &["Reference", "Fact", "Implication"], rows);
}

fn render_scope(output: &mut String, plan: &Value) {
    output.push_str("\n## Scope\n\n### Goal\n\n");
    write_paragraph(output, text(&plan["scope"]["goal"]));
    write_list_section(output, 3, "Deliverables", &plan["scope"]["deliverables"]);
    write_list_section(output, 3, "In Scope", &plan["scope"]["in_scope"]);
    write_list_section(output, 3, "Out of Scope", &plan["scope"]["out_of_scope"]);
}

fn render_decisions(output: &mut String, plan: &Value) {
    output.push_str("\n## Decisions, Assumptions, and Questions\n\n");
    let rows = array(&plan["decisions"])
        .iter()
        .map(|item| {
            vec![
                scalar(&item["item"]),
                scalar(&item["type"]),
                scalar(&item["decision"]),
                scalar(&item["reason"]),
                scalar(&item["status"]),
            ]
        })
        .collect::<Vec<_>>();
    write_optional_table(
        output,
        &["Item", "Type", "Decision", "Reason", "Status"],
        rows,
    );
}

fn render_approach(output: &mut String, plan: &Value) {
    output.push_str("\n## Approach\n\n");
    write_paragraph(output, text(&plan["approach"]["summary"]));
    output.push_str("\n### File Map\n\n");
    render_file_map(output, &plan["approach"]["file_map"]);
}

fn render_interfaces(output: &mut String, plan: &Value) {
    output.push_str("\n## Interfaces and Data Flow\n\n");
    write_paragraph(output, text(&plan["interfaces"]));
}

fn render_edge_cases(output: &mut String, plan: &Value) {
    output.push_str("\n## Edge Cases\n\n");
    let rows = array(&plan["edge_cases"])
        .iter()
        .map(|item| {
            vec![
                scalar(&item["case"]),
                scalar(&item["expected_behavior"]),
                joined(&item["covered_by"]),
            ]
        })
        .collect::<Vec<_>>();
    write_optional_table(output, &["Case", "Expected Behavior", "Covered By"], rows);
}

fn render_standards(output: &mut String, plan: &Value) {
    output.push_str("\n## Standards\n\n");
    let rows = array(&plan["standards"])
        .iter()
        .map(|item| {
            vec![
                scalar(&item["package_id"]),
                scalar(&item["version"]),
                scalar(&item["digest"]),
                scalar(&item["source"]),
            ]
        })
        .collect::<Vec<_>>();
    write_optional_table(output, &["Package", "Version", "Digest", "Source"], rows);
}

fn render_standards_conflicts(output: &mut String, plan: &Value) {
    let state = &plan["extensions"]["standards_conflicts"];
    let records = array(&state["records"]);
    if records.is_empty() {
        return;
    }
    output.push_str("\n## Standards Conflicts\n");
    for record in records {
        let conflict = &record["conflict"];
        output.push_str("\n### `");
        output.push_str(&escape_code(text(&conflict["id"])));
        output.push_str("`\n\n");
        write_table(
            output,
            &["Field", "Value"],
            vec![
                row("Rule", &scalar(&conflict["rule_id"])),
                row("Fingerprint", &scalar(&conflict["fingerprint"])),
                row(
                    "Default Candidate",
                    &scalar(&conflict["default_candidate_id"]),
                ),
            ],
        );
        output.push_str("\n#### Candidates\n\n");
        let candidates = array(&conflict["candidates"])
            .iter()
            .map(|candidate| {
                vec![
                    scalar(&candidate["id"]),
                    scalar(&candidate["value"]),
                    scalar(&candidate["source_kind"]),
                    scalar(&candidate["precedence"]),
                    scalar(&candidate["source"]),
                    scalar(&candidate["source_digest"]),
                ]
            })
            .collect::<Vec<_>>();
        write_table(
            output,
            &[
                "Candidate",
                "Value",
                "Source Kind",
                "Precedence",
                "Source",
                "Source Digest",
            ],
            candidates,
        );
        output.push_str("\n#### Explicit Decision\n\n");
        if record["decision"].is_null() {
            output.push_str("_Unresolved._\n");
        } else {
            let decision = &record["decision"];
            write_table(
                output,
                &["Field", "Value"],
                vec![
                    row(
                        "Selected Candidate",
                        &scalar(&decision["selected_candidate_id"]),
                    ),
                    row("Rationale", &scalar(&decision["rationale"])),
                    row("Reference", &scalar(&decision["reference"])),
                    row("Actor", &scalar(&decision["actor"])),
                    row("Decided At", &scalar(&decision["decided_at"])),
                    row(
                        "Conflict Fingerprint",
                        &scalar(&decision["conflict_fingerprint"]),
                    ),
                ],
            );
        }
    }
}

fn render_git_readiness(output: &mut String, plan: &Value) {
    output.push_str("\n## Git Readiness\n\n");
    let git = &plan["git_readiness"];
    write_table(
        output,
        &["Field", "Value"],
        vec![
            row("Repository", &scalar(&git["repository"])),
            row("Working Tree", &scalar(&git["working_tree"])),
            row("Branch", &scalar(&git["branch"])),
            row("Base Commit", &scalar(&git["base_commit"])),
            row("Base Status", &scalar(&git["base_status"])),
            row("Git Flow Enabled", &scalar(&git["git_flow_enabled"])),
            row("Git Flow Consent", &scalar(&git["git_flow_consent"])),
            row("Approved At", &scalar(&git["approved_at"])),
        ],
    );
}

fn render_task_order(output: &mut String, plan: &Value) {
    write_list_section(output, 2, "Implementation Task Order", &plan["task_order"]);
}

fn render_tasks(output: &mut String, plan: &Value) {
    output.push_str("\n## Tasks\n");
    if array(&plan["tasks"]).is_empty() {
        output.push_str("\n_None._\n");
        return;
    }
    for task in array(&plan["tasks"]) {
        output.push_str("\n### `");
        output.push_str(&escape_code(text(&task["id"])));
        output.push_str("`: ");
        output.push_str(&escape_inline(text(&task["title"])));
        output.push_str("\n\n");
        write_table(
            output,
            &["Field", "Value"],
            vec![
                row("Status", &scalar(&task["status"])),
                row("Resume Status", &scalar(&task["resume_status"])),
                row("Depends On", &joined(&task["depends_on"])),
                row("Blocker", &scalar(&task["blocker"])),
                row("Evidence", &joined(&task["evidence_refs"])),
            ],
        );
        write_list_section(output, 4, "Steps", &task["steps"]);
        write_list_section(
            output,
            4,
            "Implementation Notes",
            &task["implementation_notes"],
        );
        output.push_str("\n#### File Map\n\n");
        render_file_map(output, &task["file_map"]);
        output.push_str("\n#### Acceptance Criteria\n\n");
        render_acceptance_criteria(output, &task["acceptance_criteria"]);
        output.push_str("\n#### Verification Checks\n\n");
        render_checks(output, &task["verification_checks"]);
        output.push_str("\n#### Commit Gate\n\n");
        render_commit_gate(output, &task["commit_gate"]);
    }
}

fn render_file_map(output: &mut String, file_map: &Value) {
    let rows = array(file_map)
        .iter()
        .map(|item| {
            vec![
                scalar(&item["path"]),
                scalar(&item["change"]),
                scalar(&item["reason"]),
                scalar(&item["task_id"]),
            ]
        })
        .collect::<Vec<_>>();
    write_optional_table(output, &["Path", "Change", "Reason", "Task"], rows);
}

fn render_acceptance_criteria(output: &mut String, criteria: &Value) {
    let rows = array(criteria)
        .iter()
        .map(|item| {
            vec![
                scalar(&item["id"]),
                scalar(&item["description"]),
                scalar(&item["status"]),
                joined(&item["evidence_refs"]),
            ]
        })
        .collect::<Vec<_>>();
    write_optional_table(output, &["ID", "Description", "Status", "Evidence"], rows);
}

fn render_checks(output: &mut String, checks: &Value) {
    let rows = array(checks)
        .iter()
        .map(|item| {
            vec![
                scalar(&item["id"]),
                format_command(&item["command"]),
                scalar(&item["cwd"]),
                scalar(&item["expected_exit_code"]),
                scalar(&item["required"]),
                scalar(&item["status"]),
                joined(&item["evidence_refs"]),
            ]
        })
        .collect::<Vec<_>>();
    write_optional_table(
        output,
        &[
            "ID",
            "Command",
            "CWD",
            "Expected Exit",
            "Required",
            "Status",
            "Evidence",
        ],
        rows,
    );
}

fn render_commit_gate(output: &mut String, gate: &Value) {
    if gate.is_null() {
        output.push_str("_None._\n");
        return;
    }
    write_table(
        output,
        &["Field", "Value"],
        vec![
            row("Required", &scalar(&gate["required"])),
            row("Status", &scalar(&gate["status"])),
            row("Planned Message", &scalar(&gate["planned_message"])),
            row("Scope", &joined(&gate["scope"])),
            row("Actual Commit", &scalar(&gate["actual_commit"])),
            row("Committed Files", &joined(&gate["committed_files"])),
            row("Evidence", &joined(&gate["evidence_refs"])),
        ],
    );
}

fn render_global_verification(output: &mut String, plan: &Value) {
    output.push_str("\n## Verification Plan\n\n");
    render_checks(output, &plan["verification_plan"]);
}

fn render_approvals(output: &mut String, plan: &Value) {
    output.push_str("\n## Approvals\n\n");
    let rows = array(&plan["approvals"])
        .iter()
        .map(|item| {
            vec![
                scalar(&item["kind"]),
                scalar(&item["actor"]),
                scalar(&item["reference"]),
                scalar(&item["recorded_at"]),
                scalar(&item["git_flow_consent"]),
            ]
        })
        .collect::<Vec<_>>();
    write_optional_table(
        output,
        &[
            "Kind",
            "Actor",
            "Reference",
            "Recorded At",
            "Git Flow Consent",
        ],
        rows,
    );
}

fn render_amendments(output: &mut String, plan: &Value) {
    output.push_str("\n## Protected Amendments\n\n");
    let rows = array(&plan["amendments"])
        .iter()
        .map(|item| {
            vec![
                scalar(&item["id"]),
                scalar(&item["reason"]),
                scalar(&item["minimum_classification"]),
                scalar(&item["classification"]),
                scalar(&item["status"]),
                scalar(&item["base_revision"]),
                scalar(&item["base_state_hash"]),
                joined(&item["impact"]["affected_fields"]),
                joined(&item["impact"]["affected_tasks"]),
                joined(&item["impact"]["affected_checks"]),
                joined(&item["impact"]["stale_evidence"]),
                scalar(&item["proposer"]),
                scalar(&item["proposed_at"]),
                scalar(&item["approval_actor"]),
                scalar(&item["approval_reference"]),
                scalar(&item["approved_at"]),
                scalar(&item["applied_at"]),
                scalar(&item["operations"]),
            ]
        })
        .collect::<Vec<_>>();
    write_optional_table(
        output,
        &[
            "ID",
            "Reason",
            "Minimum",
            "Classification",
            "Status",
            "Base Revision",
            "Base State Hash",
            "Affected Fields",
            "Affected Tasks",
            "Affected Checks",
            "Stale Evidence",
            "Proposer",
            "Proposed At",
            "Approval Actor",
            "Approval Reference",
            "Approved At",
            "Applied At",
            "Operations",
        ],
        rows,
    );
}

fn render_review_items(output: &mut String, plan: &Value) {
    output.push_str("\n## Review Feedback\n\n");
    let rows = array(&plan["review_items"])
        .iter()
        .map(|item| {
            vec![
                scalar(&item["id"]),
                scalar(&item["reviewer"]),
                scalar(&item["feedback"]),
                scalar(&item["classification"]),
                scalar(&item["action"]),
                scalar(&item["linked_task"]),
                scalar(&item["origin_task"]),
                scalar(&item["status"]),
                scalar(&item["recorded_at"]),
                scalar(&item["approval_reference"]),
                scalar(&item["superseded_by_change"]),
            ]
        })
        .collect::<Vec<_>>();
    write_optional_table(
        output,
        &[
            "ID",
            "Reviewer",
            "Feedback",
            "Classification",
            "Action",
            "Task",
            "Origin Task",
            "Status",
            "Recorded At",
            "Approval Reference",
            "Superseded By Change",
        ],
        rows,
    );
}

fn render_follow_ups(output: &mut String, plan: &Value) {
    write_list_section(output, 2, "Follow-Ups", &plan["follow_ups"]);
}

fn render_lineage(output: &mut String, plan: &Value) {
    output.push_str("\n## Lineage\n\n");
    let lineage = &plan["lineage"];
    if lineage.is_null() {
        output.push_str("_None._\n");
        return;
    }
    write_table(
        output,
        &["Field", "Value"],
        vec![
            row("Parent Plan", &scalar(&lineage["parent_plan_id"])),
            row(
                "Forked From Revision",
                &scalar(&lineage["forked_from_revision"]),
            ),
            row("Fork Reason", &scalar(&lineage["fork_reason"])),
            row("Source State Hash", &scalar(&lineage["source_state_hash"])),
            row("Forked At", &scalar(&lineage["forked_at"])),
        ],
    );
}

fn render_archive(output: &mut String, plan: &Value) {
    let archive = &plan["archive"];
    if archive.is_null() {
        return;
    }
    output.push_str("\n## Archive\n\n");
    write_table(
        output,
        &["Field", "Value"],
        vec![
            row("Reason", &scalar(&archive["reason"])),
            row("Actor", &scalar(&archive["actor"])),
            row(
                "Approval Reference",
                &scalar(&archive["approval_reference"]),
            ),
            row("Archived At", &scalar(&archive["archived_at"])),
        ],
    );
}

fn render_final_outcome(output: &mut String, plan: &Value) {
    output.push_str("\n## Final Outcome\n\n");
    let outcome = &plan["final_outcome"];
    output.push_str("### Summary\n\n");
    write_paragraph(output, text(&outcome["summary"]));
    output.push_str("\n### Remaining Risk\n\n");
    write_paragraph(output, text(&outcome["remaining_risk"]));
    write_list_section(
        output,
        3,
        "Outcome Follow-Up Tasks",
        &outcome["follow_up_tasks"],
    );
}

fn render_extensions(output: &mut String, plan: &Value) {
    output.push_str("\n## Extensions\n\n");
    let mut extensions = plan["extensions"].as_object().cloned().unwrap_or_default();
    extensions.remove("standards_conflicts");
    if extensions.is_empty() {
        output.push_str("_None._\n");
        return;
    }
    let json = serde_json::to_string_pretty(&extensions)
        .expect("a previously serialized JSON value must serialize again");
    let fence = code_fence(&json);
    output.push_str(&fence);
    output.push_str("json\n");
    output.push_str(&json.replace("\r\n", "\n").replace('\r', "\n"));
    output.push('\n');
    output.push_str(&fence);
    output.push('\n');
}

fn write_list_section(output: &mut String, heading_level: usize, title: &str, values: &Value) {
    output.push('\n');
    output.push_str(&"#".repeat(heading_level));
    output.push(' ');
    output.push_str(title);
    output.push_str("\n\n");
    if array(values).is_empty() {
        output.push_str("_None._\n");
        return;
    }
    for value in array(values) {
        output.push_str("- ");
        output.push_str(&escape_inline(&scalar(value)));
        output.push('\n');
    }
}

fn write_optional_table(output: &mut String, headers: &[&str], rows: Vec<Vec<String>>) {
    if rows.is_empty() {
        output.push_str("_None._\n");
    } else {
        write_table(output, headers, rows);
    }
}

fn write_table(output: &mut String, headers: &[&str], rows: Vec<Vec<String>>) {
    output.push('|');
    for header in headers {
        output.push(' ');
        output.push_str(&escape_table(header));
        output.push_str(" |");
    }
    output.push('\n');
    output.push('|');
    for _ in headers {
        output.push_str("---|");
    }
    output.push('\n');
    for values in rows {
        output.push('|');
        for value in values {
            output.push(' ');
            output.push_str(&escape_table(&value));
            output.push_str(" |");
        }
        output.push('\n');
    }
}

fn write_paragraph(output: &mut String, value: &str) {
    if value.trim().is_empty() {
        output.push_str("_None._\n");
    } else {
        output.push_str(&escape_inline(value));
        output.push('\n');
    }
}

fn row(field: &str, value: &str) -> Vec<String> {
    vec![field.to_owned(), value.to_owned()]
}

fn array(value: &Value) -> &[Value] {
    value.as_array().map_or(&[], Vec::as_slice)
}

fn text(value: &Value) -> &str {
    value.as_str().unwrap_or_default()
}

fn scalar(value: &Value) -> String {
    match value {
        Value::Null => "N/A".to_owned(),
        Value::String(value) => value.clone(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::Array(_) => joined(value),
        Value::Object(_) => serde_json::to_string(value)
            .expect("a previously serialized JSON value must serialize again"),
    }
}

fn joined(value: &Value) -> String {
    let values = array(value);
    if values.is_empty() {
        "None".to_owned()
    } else {
        values.iter().map(scalar).collect::<Vec<_>>().join(", ")
    }
}

fn format_command(value: &Value) -> String {
    array(value)
        .iter()
        .map(|argument| {
            serde_json::to_string(text(argument))
                .expect("a command argument must serialize as a JSON string")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn escape_table(value: &str) -> String {
    escape_inline(value).replace('|', "\\|")
}

fn escape_inline(value: &str) -> String {
    let normalized = value.replace("\r\n", "\n").replace('\r', "\n");
    let mut escaped = String::with_capacity(normalized.len());
    for character in normalized.chars() {
        match character {
            '\\' | '`' | '*' | '_' | '{' | '}' | '[' | ']' | '<' | '>' | '#' | '+' | '!' => {
                escaped.push('\\');
                escaped.push(character);
            }
            '\n' => escaped.push_str("<br>"),
            _ => escaped.push(character),
        }
    }
    escaped
}

fn escape_code(value: &str) -> String {
    value.replace('`', "``").replace(['\r', '\n'], " ")
}

fn code_fence(value: &str) -> String {
    let longest_run = value
        .split(|character| character != '`')
        .map(str::len)
        .max()
        .unwrap_or_default();
    "`".repeat(longest_run.saturating_add(1).max(3))
}
