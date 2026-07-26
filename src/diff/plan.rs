//! Typed plan normalization and stable semantic change classification.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use serde_json::Value;

use crate::domain::Plan;

/// Stable schema discriminator for machine-readable semantic plan diffs.
pub const PLAN_DIFF_KIND: &str = "mino.plan-diff/v1";

/// Semantic relationship between one authored value on the left and right.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffCategory {
    /// A stable path exists only on the right.
    Added,
    /// A stable path exists only on the left.
    Removed,
    /// The same stable path has different authored values.
    Changed,
    /// A stable identified item occupies a different authored position.
    Moved,
}

/// One deterministic authored-plan change.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PlanChange {
    /// Change classification.
    pub category: DiffCategory,
    /// Stable dot-separated semantic path.
    pub path: String,
    /// Left-side value or position when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before: Option<Value>,
    /// Right-side value or position when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<Value>,
}

/// Identity and protocol header for one compared plan revision.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PlanDiffReference {
    /// Stable plan identifier.
    pub plan_id: crate::domain::PlanId,
    /// Exact compared revision.
    pub revision: u64,
    /// Calendar protocol version.
    pub protocol_version: String,
    /// Protocol revision name.
    pub protocol_revision: String,
}

/// Complete stable semantic comparison of two authored plan revisions.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PlanDiff {
    /// Stable result schema discriminator.
    pub diff_kind: &'static str,
    /// Left plan identity.
    pub left: PlanDiffReference,
    /// Right plan identity.
    pub right: PlanDiffReference,
    /// Whether both inputs use the same supported protocol pair.
    pub protocol_compatible: bool,
    /// Changes sorted by semantic path and category.
    pub changes: Vec<PlanChange>,
}

impl PlanDiff {
    /// Renders a deterministic concise human comparison.
    #[must_use]
    pub fn render_human(&self) -> String {
        let compatibility = if self.protocol_compatible {
            "compatible"
        } else {
            "different protocols"
        };
        let mut rendered = format!(
            "Plan diff {}@{} -> {}@{} ({compatibility})",
            self.left.plan_id, self.left.revision, self.right.plan_id, self.right.revision
        );
        if self.changes.is_empty() {
            rendered.push_str("\nNo authored differences.");
            return rendered;
        }
        for change in &self.changes {
            rendered.push('\n');
            rendered.push_str(match change.category {
                DiffCategory::Added => "Added",
                DiffCategory::Removed => "Removed",
                DiffCategory::Changed => "Changed",
                DiffCategory::Moved => "Moved",
            });
            rendered.push(' ');
            rendered.push_str(&change.path);
            if let Some(before) = &change.before {
                rendered.push_str(": ");
                rendered.push_str(&display_value(before));
            }
            if let Some(after) = &change.after {
                rendered.push_str(" -> ");
                rendered.push_str(&display_value(after));
            }
        }
        rendered
    }
}

/// Compares normalized authored fields without considering lifecycle or trust state.
///
/// # Errors
///
/// Returns a serialization error if a validated plan cannot be represented as JSON.
pub fn diff_plans(left: &Plan, right: &Plan) -> Result<PlanDiff, serde_json::Error> {
    let left_reference = reference(left);
    let right_reference = reference(right);
    let protocol_compatible = left_reference.protocol_version == right_reference.protocol_version
        && left_reference.protocol_revision == right_reference.protocol_revision;
    let left_value = normalized_authored_value(left)?;
    let right_value = normalized_authored_value(right)?;
    let mut changes = Vec::new();
    compare_value("", &left_value, &right_value, &mut changes);
    changes.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then(left.category.cmp(&right.category))
    });
    Ok(PlanDiff {
        diff_kind: PLAN_DIFF_KIND,
        left: left_reference,
        right: right_reference,
        protocol_compatible,
        changes,
    })
}

fn reference(plan: &Plan) -> PlanDiffReference {
    PlanDiffReference {
        plan_id: plan.id().clone(),
        revision: plan.revision(),
        protocol_version: plan.protocol_version().version().to_owned(),
        protocol_revision: plan.protocol_version().revision().to_owned(),
    }
}

fn normalized_authored_value(plan: &Plan) -> Result<Value, serde_json::Error> {
    let mut value = serde_json::to_value(plan)?;
    let Some(object) = value.as_object_mut() else {
        return Ok(value);
    };
    for field in [
        "id",
        "schema_version",
        "protocol_version",
        "revision",
        "status",
        "resume_status",
        "blocker",
        "git_readiness",
        "approvals",
        "amendments",
        "review_items",
        "follow_ups",
        "lineage",
        "archive",
        "final_outcome",
        "extensions",
    ] {
        object.remove(field);
    }
    if let Some(metadata) = object.get_mut("metadata").and_then(Value::as_object_mut) {
        for field in ["created_at", "updated_at", "branch", "markdown_path"] {
            metadata.remove(field);
        }
    }
    if let Some(tasks) = object.get_mut("tasks").and_then(Value::as_array_mut) {
        for task in tasks {
            normalize_task(task);
        }
    }
    if let Some(checks) = object
        .get_mut("verification_plan")
        .and_then(Value::as_array_mut)
    {
        for check in checks {
            normalize_check(check);
        }
    }
    Ok(value)
}

fn normalize_task(task: &mut Value) {
    let Some(object) = task.as_object_mut() else {
        return;
    };
    for field in ["status", "resume_status", "evidence_refs", "blocker"] {
        object.remove(field);
    }
    if let Some(criteria) = object
        .get_mut("acceptance_criteria")
        .and_then(Value::as_array_mut)
    {
        for criterion in criteria {
            if let Some(criterion) = criterion.as_object_mut() {
                criterion.remove("status");
                criterion.remove("evidence_refs");
            }
        }
    }
    if let Some(checks) = object
        .get_mut("verification_checks")
        .and_then(Value::as_array_mut)
    {
        for check in checks {
            normalize_check(check);
        }
    }
    if let Some(gate) = object.get_mut("commit_gate").and_then(Value::as_object_mut) {
        for field in [
            "status",
            "actual_commit",
            "committed_files",
            "evidence_refs",
        ] {
            gate.remove(field);
        }
    }
}

fn normalize_check(check: &mut Value) {
    if let Some(check) = check.as_object_mut() {
        check.remove("status");
        check.remove("evidence_refs");
    }
}

fn compare_value(path: &str, left: &Value, right: &Value, changes: &mut Vec<PlanChange>) {
    match (left, right) {
        (Value::Object(left), Value::Object(right)) => {
            let keys = left
                .keys()
                .chain(right.keys())
                .cloned()
                .collect::<BTreeSet<_>>();
            for key in keys {
                let child = joined_path(path, &key);
                match (left.get(&key), right.get(&key)) {
                    (Some(left), Some(right)) => compare_value(&child, left, right, changes),
                    (Some(left), None) => removed(child, left.clone(), changes),
                    (None, Some(right)) => added(child, right.clone(), changes),
                    (None, None) => {}
                }
            }
        }
        (Value::Array(left), Value::Array(right)) => compare_array(path, left, right, changes),
        _ if left != right => changed(path.to_owned(), left.clone(), right.clone(), changes),
        _ => {}
    }
}

fn compare_array(path: &str, left: &[Value], right: &[Value], changes: &mut Vec<PlanChange>) {
    if let (Some(left), Some(right)) = (identified_values(left), identified_values(right)) {
        compare_identified(path, &left, &right, changes);
    } else if let (Some(left), Some(right)) = (identified_strings(left), identified_strings(right))
    {
        compare_identified(path, &left, &right, changes);
    } else if left != right {
        changed(
            path.to_owned(),
            Value::Array(left.to_vec()),
            Value::Array(right.to_vec()),
            changes,
        );
    }
}

fn compare_identified(
    path: &str,
    left: &BTreeMap<String, (usize, Value)>,
    right: &BTreeMap<String, (usize, Value)>,
    changes: &mut Vec<PlanChange>,
) {
    let identifiers = left
        .keys()
        .chain(right.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    for identifier in identifiers {
        let child = joined_path(path, &identifier);
        match (left.get(&identifier), right.get(&identifier)) {
            (Some((left_index, left_value)), Some((right_index, right_value))) => {
                if left_index != right_index {
                    changes.push(PlanChange {
                        category: DiffCategory::Moved,
                        path: child.clone(),
                        before: Some(Value::from(*left_index)),
                        after: Some(Value::from(*right_index)),
                    });
                }
                compare_value(&child, left_value, right_value, changes);
            }
            (Some((_, left)), None) => removed(child, left.clone(), changes),
            (None, Some((_, right))) => added(child, right.clone(), changes),
            (None, None) => {}
        }
    }
}

fn identified_values(values: &[Value]) -> Option<BTreeMap<String, (usize, Value)>> {
    let mut identified = BTreeMap::new();
    for (index, value) in values.iter().enumerate() {
        let id = value.get("id")?.as_str()?.to_owned();
        if identified.insert(id, (index, value.clone())).is_some() {
            return None;
        }
    }
    Some(identified)
}

fn identified_strings(values: &[Value]) -> Option<BTreeMap<String, (usize, Value)>> {
    let mut identified = BTreeMap::new();
    for (index, value) in values.iter().enumerate() {
        let id = value.as_str()?.to_owned();
        if identified.insert(id, (index, value.clone())).is_some() {
            return None;
        }
    }
    Some(identified)
}

fn added(path: String, value: Value, changes: &mut Vec<PlanChange>) {
    changes.push(PlanChange {
        category: DiffCategory::Added,
        path,
        before: None,
        after: Some(value),
    });
}

fn removed(path: String, value: Value, changes: &mut Vec<PlanChange>) {
    changes.push(PlanChange {
        category: DiffCategory::Removed,
        path,
        before: Some(value),
        after: None,
    });
}

fn changed(path: String, before: Value, after: Value, changes: &mut Vec<PlanChange>) {
    changes.push(PlanChange {
        category: DiffCategory::Changed,
        path,
        before: Some(before),
        after: Some(after),
    });
}

fn joined_path(parent: &str, child: &str) -> String {
    if parent.is_empty() {
        child.to_owned()
    } else {
        format!("{parent}.{child}")
    }
}

fn display_value(value: &Value) -> String {
    serde_json::to_string(value).expect("a JSON value must serialize")
}
