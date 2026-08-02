//! Contracts for every shipped strict YAML input example.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use mino::domain::{AmendmentClassification, PrePlanCleanupItem, Task};
use mino::input::yaml;

const EXAMPLES: &[&str] = &[
    "draft-plan.yaml",
    "git-cleanup-proposal.yaml",
    "amendment-patch.yaml",
    "review-rework-task.yaml",
];

fn repository_path(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
}

fn example_source(name: &str) -> String {
    fs::read_to_string(repository_path("assets/skill/mino/references/examples").join(name))
        .expect("canonical structured-input example should be readable")
}

fn help(arguments: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_mino"))
        .args(arguments)
        .stdin(Stdio::null())
        .output()
        .expect("Mino help should run");
    assert!(
        output.status.success(),
        "help stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("help should be UTF-8")
}

#[test]
fn canonical_examples_parse_strictly_and_are_semantically_complete() {
    let draft_source = example_source("draft-plan.yaml");
    let draft = yaml::parse_draft(&draft_source).expect("Draft example should parse");
    assert!(draft.metadata.is_some());
    assert!(
        draft
            .summary
            .as_ref()
            .is_some_and(|value| !value.is_empty())
    );
    assert!(!draft.context.is_empty());
    assert!(draft.scope.is_some());
    assert!(!draft.decisions.is_empty());
    assert!(
        draft
            .approach
            .as_ref()
            .is_some_and(|value| !value.is_empty())
    );
    assert!(
        draft
            .interfaces
            .as_ref()
            .is_some_and(|value| !value.is_empty())
    );
    assert!(!draft.edge_cases.is_empty());
    assert!(!draft.tasks.is_empty());
    assert!(!draft.verification_plan.is_empty());
    for task in &draft.tasks {
        let task_id = task
            .id
            .as_ref()
            .expect("Draft task should have an explicit ID");
        assert!(!task.steps.is_empty());
        assert!(!task.files.is_empty());
        assert!(!task.acceptance_criteria.is_empty());
        assert!(!task.verification.is_empty());
        assert!(task.commit_gate.is_some());
        Task::from_draft(task_id, task.clone()).expect("Draft task should satisfy domain rules");
    }

    let cleanup_source = example_source("git-cleanup-proposal.yaml");
    let cleanup = yaml::parse_cleanup_proposal(&cleanup_source)
        .expect("cleanup proposal example should parse");
    assert!(!cleanup.is_empty());
    let mut assigned_files = BTreeSet::new();
    for (index, item) in cleanup.into_iter().enumerate() {
        assert!(
            item.files
                .iter()
                .all(|path| assigned_files.insert(path.clone()))
        );
        PrePlanCleanupItem::new(
            format!("C{}", index + 1),
            item.logical_change,
            item.files,
            item.planned_commit_message,
        )
        .expect("cleanup item should satisfy domain rules");
    }

    let amendment_source = example_source("amendment-patch.yaml");
    let amendment = yaml::parse_amendment_patch(&amendment_source)
        .expect("amendment patch example should parse");
    assert!(!amendment.operations().is_empty());
    assert_eq!(
        amendment
            .minimum_classification()
            .expect("amendment operations should be complete"),
        AmendmentClassification::Minor
    );

    let rework_source = example_source("review-rework-task.yaml");
    let rework =
        yaml::parse_review_rework_task(&rework_source).expect("review rework example should parse");
    let rework_id = rework
        .id
        .clone()
        .expect("rework task should have an explicit ID");
    assert!(rework_id.as_str().starts_with('R'));
    assert!(!rework.depends_on.is_empty());
    assert!(!rework.steps.is_empty());
    assert!(!rework.files.is_empty());
    assert!(!rework.acceptance_criteria.is_empty());
    assert!(!rework.verification.is_empty());
    assert!(rework.commit_gate.is_some());
    Task::from_draft(&rework_id, rework).expect("rework task should satisfy domain rules");

    assert!(yaml::parse_draft(&format!("{draft_source}\nunknown_field: true\n")).is_err());
    assert!(
        yaml::parse_cleanup_proposal(&format!("{cleanup_source}\nunknown_field: true\n")).is_err()
    );
    assert!(
        yaml::parse_amendment_patch(&format!("{amendment_source}\nunknown_field: true\n")).is_err()
    );
    assert!(
        yaml::parse_review_rework_task(&format!("{rework_source}\nunknown_field: true\n")).is_err()
    );
}

#[test]
fn examples_are_distributed_byte_for_byte_and_contain_no_placeholders() {
    for name in EXAMPLES {
        let canonical =
            fs::read(repository_path("assets/skill/mino/references/examples").join(name))
                .expect("canonical example should be readable");
        let plugin =
            fs::read(repository_path("plugins/mino/skills/mino/references/examples").join(name))
                .expect("plugin example should be readable");
        assert_eq!(canonical, plugin, "plugin example {name} should match");
        let source = String::from_utf8(canonical).expect("example should be UTF-8");
        let normalized = source.to_ascii_lowercase();
        assert!(!normalized.contains("todo"));
        assert!(!normalized.contains("tbd"));
        assert!(!source.contains(['<', '>']));
    }
}

#[test]
fn every_file_option_help_identifies_its_installed_example() {
    for (arguments, name) in [
        (&["plan", "apply", "--help"][..], "draft-plan.yaml"),
        (
            &["git", "cleanup", "propose", "--help"][..],
            "git-cleanup-proposal.yaml",
        ),
        (
            &["plan", "amend", "propose", "--help"][..],
            "amendment-patch.yaml",
        ),
        (
            &["review", "rework", "--help"][..],
            "review-rework-task.yaml",
        ),
    ] {
        let output = help(arguments);
        assert!(output.contains(&format!(".agents/skills/mino/references/examples/{name}")));
    }
}
