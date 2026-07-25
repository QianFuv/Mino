//! Execution-graph checks for order, dependencies, file ownership, and task boundaries.

use std::collections::{BTreeMap, BTreeSet};

use crate::domain::{FileMapEntry, Plan, TaskId};

use super::{ValidationFinding, ValidationLayer};

pub(crate) fn validate(plan: &Plan, findings: &mut Vec<ValidationFinding>) {
    let task_ids = plan
        .tasks()
        .iter()
        .map(|task| task.id().clone())
        .collect::<Vec<_>>();
    if plan.task_order() != task_ids.as_slice() {
        findings.push(ValidationFinding::error(
            "GRAPH-TASK-ORDER-MISMATCH",
            ValidationLayer::Graph,
            "task_order",
            "Task order must contain every task exactly once in stored order",
        ));
    }
    let positions = plan
        .task_order()
        .iter()
        .enumerate()
        .map(|(index, task_id)| (task_id, index))
        .collect::<BTreeMap<_, _>>();
    let known_ids = plan
        .tasks()
        .iter()
        .map(crate::domain::Task::id)
        .collect::<BTreeSet<_>>();
    for task in plan.tasks() {
        let mut dependencies = BTreeSet::new();
        for dependency in task.dependencies() {
            if !dependencies.insert(dependency) {
                findings.push(ValidationFinding::error(
                    "GRAPH-DEPENDENCY-DUPLICATE",
                    ValidationLayer::Graph,
                    format!("tasks.{}.depends_on", task.id()),
                    format!("Task {} repeats dependency {dependency}", task.id()),
                ));
            }
            if dependency == task.id() {
                findings.push(ValidationFinding::error(
                    "GRAPH-SELF-DEPENDENCY",
                    ValidationLayer::Graph,
                    format!("tasks.{}.depends_on", task.id()),
                    format!("Task {} depends on itself", task.id()),
                ));
            } else if !known_ids.contains(dependency) {
                findings.push(ValidationFinding::error(
                    "GRAPH-DEPENDENCY-MISSING",
                    ValidationLayer::Graph,
                    format!("tasks.{}.depends_on", task.id()),
                    format!("Task {} depends on missing task {dependency}", task.id()),
                ));
            } else if positions[dependency] >= positions[task.id()] {
                findings.push(ValidationFinding::error(
                    "GRAPH-DEPENDENCY-ORDER",
                    ValidationLayer::Graph,
                    format!("tasks.{}.depends_on", task.id()),
                    format!("Dependency {dependency} must precede task {}", task.id()),
                ));
            }
        }
        if task.steps().is_empty() || task.file_map().is_empty() {
            findings.push(ValidationFinding::error(
                "GRAPH-TASK-BOUNDARY-MISSING",
                ValidationLayer::Graph,
                format!("tasks.{}", task.id()),
                format!(
                    "Task {} requires steps and an owned file boundary",
                    task.id()
                ),
            ));
        }
    }
    if contains_cycle(plan) {
        findings.push(ValidationFinding::error(
            "GRAPH-DEPENDENCY-CYCLE",
            ValidationLayer::Graph,
            "tasks.depends_on",
            "Task dependencies contain a cycle",
        ));
    }
    validate_file_map(plan, findings);
}

fn validate_file_map(plan: &Plan, findings: &mut Vec<ValidationFinding>) {
    for entry in plan.approach().file_map() {
        let Some(task) = plan.task(entry.task_id()) else {
            findings.push(ValidationFinding::error(
                "GRAPH-FILE-TASK-MISSING",
                ValidationLayer::Graph,
                format!("approach.file_map.{}", entry.path()),
                format!(
                    "File map path {} references missing task {}",
                    entry.path(),
                    entry.task_id()
                ),
            ));
            continue;
        };
        if !task
            .file_map()
            .iter()
            .any(|task_entry| same_file_entry(entry, task_entry))
        {
            findings.push(ValidationFinding::error(
                "GRAPH-FILE-TASK-MISMATCH",
                ValidationLayer::Graph,
                format!("approach.file_map.{}", entry.path()),
                format!(
                    "File map path {} is absent from task {}",
                    entry.path(),
                    entry.task_id()
                ),
            ));
        }
    }
    for task in plan.tasks() {
        for entry in task.file_map() {
            if !plan
                .approach()
                .file_map()
                .iter()
                .any(|plan_entry| same_file_entry(entry, plan_entry))
            {
                findings.push(ValidationFinding::error(
                    "GRAPH-TASK-FILE-MISSING",
                    ValidationLayer::Graph,
                    format!("tasks.{}.file_map.{}", task.id(), entry.path()),
                    format!(
                        "Task file {} is absent from the complete plan file map",
                        entry.path()
                    ),
                ));
            }
        }
    }
}

fn same_file_entry(left: &FileMapEntry, right: &FileMapEntry) -> bool {
    left.path() == right.path()
        && left.change() == right.change()
        && left.reason() == right.reason()
        && left.task_id() == right.task_id()
}

fn contains_cycle(plan: &Plan) -> bool {
    let dependencies = plan
        .tasks()
        .iter()
        .map(|task| (task.id().clone(), task.dependencies().to_vec()))
        .collect::<BTreeMap<_, _>>();
    let mut visiting = BTreeSet::new();
    let mut visited = BTreeSet::new();
    dependencies
        .keys()
        .any(|task_id| visit(task_id, &dependencies, &mut visiting, &mut visited))
}

fn visit(
    task_id: &TaskId,
    dependencies: &BTreeMap<TaskId, Vec<TaskId>>,
    visiting: &mut BTreeSet<TaskId>,
    visited: &mut BTreeSet<TaskId>,
) -> bool {
    if visited.contains(task_id) {
        return false;
    }
    if !visiting.insert(task_id.clone()) {
        return true;
    }
    let has_cycle = dependencies.get(task_id).is_some_and(|task_dependencies| {
        task_dependencies.iter().any(|dependency| {
            dependencies.contains_key(dependency)
                && visit(dependency, dependencies, visiting, visited)
        })
    });
    visiting.remove(task_id);
    visited.insert(task_id.clone());
    has_cycle
}
