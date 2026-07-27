//! Black-box proof for the complete local Mino v0.2 lifecycle.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use mino::domain::PlanId;
use mino::evidence::EvidenceStore;
use mino::store::PlanStore;
use serde_json::Value;

static NEXT_TEMP_DIRECTORY: AtomicU64 = AtomicU64::new(1);

struct TestProject {
    root: PathBuf,
}

struct TaskExecution<'a> {
    task_id: &'a str,
    criterion_id: &'a str,
    check_id: &'a str,
    file: &'a str,
    contents: &'a str,
}

impl TestProject {
    fn new() -> Self {
        let sequence = NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("mino-e2e-v0-2-{}-{sequence}", std::process::id()));
        fs::create_dir(&root).expect("temporary E2E project should be created");
        fs::write(root.join("seed.txt"), "baseline\n").expect("baseline file should be written");
        fs::write(
            root.join("AGENTS.md"),
            "Repository functions must describe their approval boundary.\n",
        )
        .expect("repository policy source should be written");
        fs::write(
            root.join("project.conf"),
            "approval_boundary = \"project configuration\"\n",
        )
        .expect("project configuration source should be written");
        Self {
            root: root.canonicalize().expect("E2E project should resolve"),
        }
    }

    fn root(&self) -> &Path {
        &self.root
    }

    fn prepare_inputs(&self) {
        fs::write(self.root.join(".gitignore"), "/.mino/\n/docs/plan/\n")
            .expect("runtime ignore rules should be written");
        fs::create_dir_all(self.root.join(".mino/flow-inputs"))
            .expect("ignored flow input directory should be created");
        fs::write(
            self.root.join(".mino/standards.local.toml"),
            local_standards_source(),
        )
        .expect("local standards source should be written");
        fs::write(
            self.root.join(".mino/flow-inputs/request.md"),
            "Implement and review the complete Mino v0.2 lifecycle.\n",
        )
        .expect("request should be written");
        fs::write(self.root.join(".mino/flow-inputs/plan.yaml"), plan_source())
            .expect("plan input should be written");
        fs::write(
            self.root.join(".mino/flow-inputs/amendment.yaml"),
            amendment_source(),
        )
        .expect("amendment input should be written");
        fs::write(
            self.root.join(".mino/flow-inputs/rework.yaml"),
            rework_source(),
        )
        .expect("rework input should be written");
    }
}

impl Drop for TestProject {
    fn drop(&mut self) {
        let temporary_root = std::env::temp_dir();
        if self.root.starts_with(&temporary_root)
            && self
                .root
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("mino-e2e-v0-2-"))
        {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

#[test]
#[allow(clippy::too_many_lines)]
fn full_v0_2_cli_lifecycle_reaches_done_after_conflict_amendment_and_rework() {
    let project = TestProject::new();
    parse_success(&run_mino(project.root(), &arguments(&["project", "init"])));
    project.prepare_inputs();
    initialize_git(project.root());

    let mut request_number = 1;
    let created = parse_success(&run_mino(
        project.root(),
        &[
            arguments(&[
                "plan",
                "create",
                "--name",
                "Complete v0.2 lifecycle",
                "--trigger",
                "durable",
            ]),
            vec![
                "--request-file".to_owned(),
                project
                    .root()
                    .join(".mino/flow-inputs/request.md")
                    .to_string_lossy()
                    .into_owned(),
                "--request-id".to_owned(),
                request_id(next_request(&mut request_number)),
                "--actor".to_owned(),
                "codex".to_owned(),
            ],
        ]
        .concat(),
    ));
    let plan_id = created["plan_id"]
        .as_str()
        .expect("created plan ID should be text")
        .to_owned();
    let mut revision = result_revision(&created);
    assert_eq!(revision, 1);

    let applied = parse_success(&run_mino(
        project.root(),
        &mutation_arguments(
            &["plan", "apply"],
            &plan_id,
            revision,
            next_request(&mut request_number),
            vec![
                "--file".to_owned(),
                project
                    .root()
                    .join(".mino/flow-inputs/plan.yaml")
                    .to_string_lossy()
                    .into_owned(),
            ],
        ),
    ));
    revision = result_revision(&applied);
    assert_eq!(revision, 2);

    let untracked = parse_failure(
        &run_mino(
            project.root(),
            &arguments(&["plan", "validate", "--plan", &plan_id]),
        ),
        2,
        "incomplete_or_validation",
    );
    assert_finding(&untracked, "POLICY-STANDARD-CONFLICT-UNTRACKED");

    let refreshed = parse_success(&run_mino(
        project.root(),
        &mutation_arguments(
            &["standards", "conflict", "refresh"],
            &plan_id,
            revision,
            next_request(&mut request_number),
            Vec::new(),
        ),
    ));
    revision = result_revision(&refreshed);
    assert_eq!(revision, 3);
    assert_eq!(
        refreshed["standards_conflicts"]["conflicts"][0]["status"],
        "unresolved"
    );

    let listed = parse_success(&run_mino(
        project.root(),
        &arguments(&["standards", "conflict", "list", "--plan", &plan_id]),
    ));
    let live_conflict = &listed["conflicts"][0]["conflict"];
    let conflict_id = live_conflict["id"]
        .as_str()
        .expect("conflict ID should be text");
    let candidates = live_conflict["candidates"]
        .as_array()
        .expect("conflict candidates should be an array");
    assert_eq!(
        candidates
            .iter()
            .map(|candidate| candidate["precedence"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        [5, 4, 3, 1]
    );
    let common_candidate = candidates
        .iter()
        .find(|candidate| candidate["source_kind"] == "common_default")
        .and_then(|candidate| candidate["id"].as_str())
        .expect("Common candidate should be displayed");
    assert_ne!(
        live_conflict["default_candidate_id"],
        Value::from(common_candidate)
    );

    let resolved = parse_success(&run_mino(
        project.root(),
        &mutation_arguments(
            &["standards", "conflict", "resolve"],
            &plan_id,
            revision,
            next_request(&mut request_number),
            vec![
                "--conflict".to_owned(),
                conflict_id.to_owned(),
                "--candidate".to_owned(),
                common_candidate.to_owned(),
                "--rationale".to_owned(),
                "Choose the Common package after explicitly reviewing every higher-precedence source."
                    .to_owned(),
                "--decision-ref".to_owned(),
                "chat:e2e-standards-choice".to_owned(),
            ],
        ),
    ));
    revision = result_revision(&resolved);
    assert_eq!(revision, 4);
    assert_eq!(resolved["complete"], true);
    assert_eq!(
        resolved["standards_conflicts"]["conflicts"][0]["decision"]["reference"],
        "chat:e2e-standards-choice"
    );
    let valid = parse_success(&run_mino(
        project.root(),
        &arguments(&["plan", "validate", "--plan", &plan_id]),
    ));
    assert_eq!(valid["valid"], true);

    let finalized = parse_success(&run_mino(
        project.root(),
        &mutation_arguments(
            &["plan", "finalize"],
            &plan_id,
            revision,
            next_request(&mut request_number),
            Vec::new(),
        ),
    ));
    revision = result_revision(&finalized);
    assert_eq!(finalized["status"], "Ready");

    let approved = parse_success(&run_mino(
        project.root(),
        &mutation_arguments(
            &["plan", "approve"],
            &plan_id,
            revision,
            next_request(&mut request_number),
            vec![
                "--approval-ref".to_owned(),
                "chat:e2e-initial-plan-approval".to_owned(),
                "--git-flow-consent".to_owned(),
                "approved".to_owned(),
            ],
        ),
    ));
    revision = result_revision(&approved);

    let proposed = parse_success(&run_mino(
        project.root(),
        &mutation_arguments(
            &["plan", "amend", "propose"],
            &plan_id,
            revision,
            next_request(&mut request_number),
            vec![
                "--reason".to_owned(),
                "Record the reviewed v0.2 outcome before execution.".to_owned(),
                "--patch-file".to_owned(),
                project
                    .root()
                    .join(".mino/flow-inputs/amendment.yaml")
                    .to_string_lossy()
                    .into_owned(),
            ],
        ),
    ));
    revision = result_revision(&proposed);
    assert_eq!(proposed["assigned_id"], "C1");
    assert_eq!(proposed["status"], "Blocked");
    let amendment_context =
        parse_agent_success(&run_mino(project.root(), &arguments(&["agent", "context"])));
    assert_eq!(amendment_context["approval_required"], true);
    assert_eq!(amendment_context["next_actions"], Value::Array(Vec::new()));
    assert!(
        amendment_context["blocked_actions"]
            .as_array()
            .is_some_and(|actions| actions
                .iter()
                .any(|action| action["action"] == "plan.amend.apply"))
    );
    parse_failure(
        &run_mino(
            project.root(),
            &mutation_arguments(
                &["exec", "start"],
                &plan_id,
                revision,
                next_request(&mut request_number),
                vec!["--task".to_owned(), "T1".to_owned()],
            ),
        ),
        4,
        "approval_required",
    );

    let amendment_approved = parse_success(&run_mino(
        project.root(),
        &mutation_arguments(
            &["plan", "amend", "approve"],
            &plan_id,
            revision,
            next_request(&mut request_number),
            vec![
                "--change".to_owned(),
                "C1".to_owned(),
                "--approval-ref".to_owned(),
                "chat:e2e-material-approval".to_owned(),
            ],
        ),
    ));
    revision = result_revision(&amendment_approved);
    let amendment_applied = parse_success(&run_mino(
        project.root(),
        &mutation_arguments(
            &["plan", "amend", "apply"],
            &plan_id,
            revision,
            next_request(&mut request_number),
            vec!["--change".to_owned(), "C1".to_owned()],
        ),
    ));
    revision = result_revision(&amendment_applied);
    assert_eq!(amendment_applied["status"], "Ready");
    let amended = parse_success(&run_mino(
        project.root(),
        &arguments(&["plan", "show", "--plan", &plan_id]),
    ));
    assert_eq!(amended["approvals"], Value::Array(Vec::new()));
    assert_eq!(amended["amendments"][0]["classification"], "Material");
    assert_eq!(amended["amendments"][0]["status"], "Applied");
    assert_eq!(
        amended["amendments"][0]["approval_reference"],
        "chat:e2e-material-approval"
    );
    assert_eq!(
        parse_success(&run_mino(
            project.root(),
            &arguments(&["plan", "validate", "--plan", &plan_id]),
        ))["valid"],
        true
    );

    let renewed = parse_success(&run_mino(
        project.root(),
        &mutation_arguments(
            &["plan", "approve"],
            &plan_id,
            revision,
            next_request(&mut request_number),
            vec![
                "--approval-ref".to_owned(),
                "chat:e2e-renewed-plan-approval".to_owned(),
                "--git-flow-consent".to_owned(),
                "approved".to_owned(),
            ],
        ),
    ));
    revision = result_revision(&renewed);
    let bound = parse_success(&run_mino(
        project.root(),
        &arguments(&["git", "bind", "--plan", &plan_id, "--current"]),
    ));
    assert_eq!(bound["binding"]["plan_id"], plan_id);

    revision = execute_task(
        &project,
        &plan_id,
        &TaskExecution {
            task_id: "T1",
            criterion_id: "T1-A1",
            check_id: "T1-V1",
            file: "feature.txt",
            contents: "implemented feature\n",
        },
        revision,
        &mut request_number,
    );
    assert_eq!(
        parse_success(&run_mino(
            project.root(),
            &arguments(&["plan", "show", "--plan", &plan_id]),
        ))["revision"],
        revision
    );
    let first_commit = parse_success(&run_mino(
        project.root(),
        &arguments(&["git", "commit", "--plan", &plan_id, "--task", "T1"]),
    ));
    revision = result_revision(&first_commit);
    assert_eq!(
        first_commit["completion"]["message"],
        "feat(fixture): implement v0 two flow"
    );
    assert_eq!(
        first_commit["completion"]["files"],
        serde_json::json!(["feature.txt"])
    );
    revision = run_check(
        &project,
        &plan_id,
        "GLOBAL-V1",
        revision,
        &mut request_number,
    )
    .0;
    let outcome = parse_success(&run_mino(
        project.root(),
        &mutation_arguments(
            &["plan", "outcome", "set"],
            &plan_id,
            revision,
            next_request(&mut request_number),
            vec![
                "--summary".to_owned(),
                "The initial reviewed implementation is verified".to_owned(),
                "--remaining-risk".to_owned(),
                "N/A".to_owned(),
            ],
        ),
    ));
    revision = result_revision(&outcome);
    let first_review = parse_success(&run_mino(
        project.root(),
        &mutation_arguments(
            &["exec", "finish"],
            &plan_id,
            revision,
            next_request(&mut request_number),
            Vec::new(),
        ),
    ));
    revision = result_revision(&first_review);
    assert_eq!(first_review["status"], "Review");

    let recorded = parse_success(&run_mino(
        project.root(),
        &mutation_arguments(
            &["review", "record"],
            &plan_id,
            revision,
            next_request(&mut request_number),
            vec![
                "--classification".to_owned(),
                "in-scope-rework".to_owned(),
                "--feedback".to_owned(),
                "Add the reviewed in-scope correction.".to_owned(),
                "--task".to_owned(),
                "T1".to_owned(),
            ],
        ),
    ));
    revision = result_revision(&recorded);
    assert_eq!(recorded["assigned_id"], "REV-1");
    parse_failure(
        &run_mino(
            project.root(),
            &mutation_arguments(
                &["review", "accept"],
                &plan_id,
                revision,
                next_request(&mut request_number),
                vec![
                    "--approval-ref".to_owned(),
                    "chat:e2e-premature-acceptance".to_owned(),
                ],
            ),
        ),
        5,
        "policy_violation",
    );

    let rework = parse_success(&run_mino(
        project.root(),
        &mutation_arguments(
            &["review", "rework"],
            &plan_id,
            revision,
            next_request(&mut request_number),
            vec![
                "--review".to_owned(),
                "REV-1".to_owned(),
                "--file".to_owned(),
                project
                    .root()
                    .join(".mino/flow-inputs/rework.yaml")
                    .to_string_lossy()
                    .into_owned(),
            ],
        ),
    ));
    revision = result_revision(&rework);
    assert_eq!(rework["status"], "In Progress");
    revision = execute_task(
        &project,
        &plan_id,
        &TaskExecution {
            task_id: "R1",
            criterion_id: "R1-A1",
            check_id: "R1-V1",
            file: "rework.txt",
            contents: "reviewed correction\n",
        },
        revision,
        &mut request_number,
    );
    assert_eq!(
        parse_success(&run_mino(
            project.root(),
            &arguments(&["plan", "show", "--plan", &plan_id]),
        ))["revision"],
        revision
    );
    let rework_commit = parse_success(&run_mino(
        project.root(),
        &arguments(&["git", "commit", "--plan", &plan_id, "--task", "R1"]),
    ));
    revision = result_revision(&rework_commit);
    assert_eq!(
        rework_commit["completion"]["message"],
        "fix(review): complete v0 two rework"
    );
    assert_eq!(
        rework_commit["completion"]["files"],
        serde_json::json!(["rework.txt"])
    );
    revision = run_check(
        &project,
        &plan_id,
        "GLOBAL-V1",
        revision,
        &mut request_number,
    )
    .0;
    let outcome = parse_success(&run_mino(
        project.root(),
        &mutation_arguments(
            &["plan", "outcome", "set"],
            &plan_id,
            revision,
            next_request(&mut request_number),
            vec![
                "--summary".to_owned(),
                "The reviewed correction is verified".to_owned(),
                "--remaining-risk".to_owned(),
                "N/A".to_owned(),
            ],
        ),
    ));
    revision = result_revision(&outcome);
    let second_review = parse_success(&run_mino(
        project.root(),
        &mutation_arguments(
            &["exec", "finish"],
            &plan_id,
            revision,
            next_request(&mut request_number),
            Vec::new(),
        ),
    ));
    revision = result_revision(&second_review);
    assert_eq!(second_review["status"], "Review");

    let resolved_review = parse_success(&run_mino(
        project.root(),
        &mutation_arguments(
            &["review", "resolve"],
            &plan_id,
            revision,
            next_request(&mut request_number),
            vec!["--review".to_owned(), "REV-1".to_owned()],
        ),
    ));
    revision = result_revision(&resolved_review);
    let accepted = parse_success(&run_mino(
        project.root(),
        &mutation_arguments(
            &["review", "accept"],
            &plan_id,
            revision,
            next_request(&mut request_number),
            vec![
                "--approval-ref".to_owned(),
                "chat:e2e-final-acceptance".to_owned(),
            ],
        ),
    ));
    revision = result_revision(&accepted);
    assert_eq!(accepted["status"], "Done");

    verify_final_state(&project, &plan_id, revision);
}

fn execute_task(
    project: &TestProject,
    plan_id: &str,
    task: &TaskExecution<'_>,
    mut revision: u64,
    request_number: &mut u64,
) -> u64 {
    let started = parse_success(&run_mino(
        project.root(),
        &mutation_arguments(
            &["exec", "start"],
            plan_id,
            revision,
            next_request(request_number),
            vec!["--task".to_owned(), task.task_id.to_owned()],
        ),
    ));
    revision = result_revision(&started);
    fs::write(project.root().join(task.file), task.contents).expect("task file should be written");
    let (check_revision, evidence_id) =
        run_check(project, plan_id, task.check_id, revision, request_number);
    revision = check_revision;
    let criterion = parse_success(&run_mino(
        project.root(),
        &mutation_arguments(
            &["exec", "criterion", "pass"],
            plan_id,
            revision,
            next_request(request_number),
            vec![
                "--criterion".to_owned(),
                task.criterion_id.to_owned(),
                "--evidence".to_owned(),
                evidence_id,
            ],
        ),
    ));
    revision = result_revision(&criterion);
    let completed = parse_success(&run_mino(
        project.root(),
        &mutation_arguments(
            &["exec", "complete"],
            plan_id,
            revision,
            next_request(request_number),
            vec!["--task".to_owned(), task.task_id.to_owned()],
        ),
    ));
    result_revision(&completed)
}

fn run_check(
    project: &TestProject,
    plan_id: &str,
    check_id: &str,
    revision: u64,
    request_number: &mut u64,
) -> (u64, String) {
    let checked = parse_success(&run_mino(
        project.root(),
        &mutation_arguments(
            &["exec", "check", "run"],
            plan_id,
            revision,
            next_request(request_number),
            vec!["--check".to_owned(), check_id.to_owned()],
        ),
    ));
    assert_eq!(checked["run"]["outcome"], "passed");
    (
        result_revision(&checked),
        checked["evidence"]["id"]
            .as_str()
            .expect("check evidence ID should be text")
            .to_owned(),
    )
}

fn verify_final_state(project: &TestProject, plan_id: &str, revision: u64) {
    let shown = parse_success(&run_mino(
        project.root(),
        &arguments(&["plan", "show", "--plan", plan_id]),
    ));
    assert_eq!(shown["status"], "Done");
    assert_eq!(shown["revision"], revision);
    assert_eq!(shown["tasks"].as_array().map(Vec::len), Some(2));
    assert!(shown["tasks"].as_array().is_some_and(|tasks| {
        tasks
            .iter()
            .all(|task| task["status"] == "Done" && task["commit_gate"]["status"] == "Committed")
    }));
    assert_eq!(shown["amendments"][0]["status"], "Applied");
    assert_eq!(
        shown["review_items"][0]["classification"],
        "In-Scope Rework"
    );
    assert_eq!(shown["review_items"][0]["status"], "Resolved");
    assert_eq!(shown["review_items"][1]["classification"], "Accepted");
    assert_eq!(
        shown["review_items"][1]["approval_reference"],
        "chat:e2e-final-acceptance"
    );
    assert_eq!(
        shown["extensions"]["standards_conflicts"]["records"][0]["decision"]["reference"],
        "chat:e2e-standards-choice"
    );

    let typed_plan = PlanId::parse(plan_id).expect("plan ID should parse");
    let audit = PlanStore::new(project.root())
        .audit(&typed_plan)
        .expect("plan store should audit");
    let revision_count = usize::try_from(revision).expect("revision should fit in usize");
    assert_eq!(audit.revision(), revision);
    assert_eq!(audit.event_count(), revision_count);
    assert_eq!(audit.snapshot_count(), revision_count);
    let evidence = EvidenceStore::new(project.root());
    assert!(
        evidence
            .audit(&typed_plan)
            .expect("evidence should audit")
            .is_healthy()
    );
    assert!(
        evidence
            .list(&typed_plan)
            .expect("evidence should list")
            .len()
            >= 6
    );
    assert!(git_text(project.root(), &["status", "--short"]).is_empty());
    let messages = git_text(project.root(), &["log", "--format=%s"]);
    let messages = messages.lines().collect::<Vec<_>>();
    assert_eq!(messages[0], "fix(review): complete v0 two rework");
    assert_eq!(messages[1], "feat(fixture): implement v0 two flow");
    assert_eq!(messages[2], "chore: establish v0 two baseline");
}

fn run_mino(root: &Path, command: &[String]) -> Output {
    let mut arguments = vec![
        "--root".to_owned(),
        root.to_string_lossy().into_owned(),
        "--format".to_owned(),
        "json".to_owned(),
        "--no-input".to_owned(),
    ];
    arguments.extend(command.iter().cloned());
    Command::new(env!("CARGO_BIN_EXE_mino"))
        .args(arguments)
        .stdin(Stdio::null())
        .output()
        .expect("Mino binary should run")
}

fn parse_success(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "Mino command failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let value: Value = serde_json::from_slice(&output.stdout).expect("stdout should be JSON");
    assert_eq!(value["kind"], "mino.result/v1");
    assert_eq!(value["ok"], true);
    value
}

fn parse_agent_success(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "Mino Agent command failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    serde_json::from_slice(&output.stdout).expect("Agent stdout should be JSON")
}

fn parse_failure(output: &Output, exit_code: i32, code: &str) -> Value {
    assert_eq!(
        output.status.code(),
        Some(exit_code),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let value: Value = serde_json::from_slice(&output.stdout).expect("failure should be JSON");
    assert_eq!(value["kind"], "mino.result/v1");
    assert_eq!(value["ok"], false);
    assert_eq!(value["error"]["code"], code);
    value
}

fn mutation_arguments(
    path: &[&str],
    plan_id: &str,
    revision: u64,
    request_number: u64,
    extra: Vec<String>,
) -> Vec<String> {
    let mut command = path
        .iter()
        .map(|part| (*part).to_owned())
        .collect::<Vec<_>>();
    command.extend([
        "--plan".to_owned(),
        plan_id.to_owned(),
        "--expect-revision".to_owned(),
        revision.to_string(),
        "--request-id".to_owned(),
        request_id(request_number),
        "--actor".to_owned(),
        "codex".to_owned(),
    ]);
    command.extend(extra);
    command
}

fn result_revision(value: &Value) -> u64 {
    value["revision"]
        .as_u64()
        .or_else(|| value["plan"]["revision"].as_u64())
        .or_else(|| value["plan_revision"].as_u64())
        .expect("result should expose the current plan revision")
}

fn assert_finding(value: &Value, finding_id: &str) {
    assert!(
        value["findings"]
            .as_array()
            .is_some_and(|findings| { findings.iter().any(|finding| finding["id"] == finding_id) })
    );
}

fn next_request(request_number: &mut u64) -> u64 {
    let current = *request_number;
    *request_number += 1;
    current
}

fn request_id(number: u64) -> String {
    format!("a0000000-0000-0000-0000-{number:012}")
}

fn arguments(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

fn initialize_git(root: &Path) {
    git(root, &["init", "--quiet", "--initial-branch", "main"]);
    git(root, &["config", "user.name", "Mino v0.2 E2E"]);
    git(root, &["config", "user.email", "mino-v0-2@example.invalid"]);
    git(
        root,
        &[
            "add",
            "--",
            ".gitignore",
            ".agents",
            "AGENTS.md",
            "project.conf",
            "seed.txt",
        ],
    );
    git(
        root,
        &[
            "commit",
            "--quiet",
            "-m",
            "chore: establish v0 two baseline",
        ],
    );
    assert!(git_text(root, &["status", "--short"]).is_empty());
}

fn git(root: &Path, arguments: &[&str]) {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .expect("Git should run");
    assert!(
        output.status.success(),
        "git {arguments:?} failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_text(root: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .expect("Git should run");
    assert!(
        output.status.success(),
        "git {arguments:?} failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("Git output should be UTF-8")
}

fn local_standards_source() -> &'static str {
    "format_version = 1\n\n[[rules]]\nrule_id = \"common.confirm-intent\"\nvalue = \"The current user explicitly selected an end-to-end approval flow.\"\nsource_kind = \"user_requirement\"\nsource = \"plan.original_request\"\n\n[[rules]]\nrule_id = \"common.confirm-intent\"\nvalue = \"Repository approval boundaries override implicit execution.\"\nsource_kind = \"repository_rule\"\nsource = \"AGENTS.md\"\n\n[[rules]]\nrule_id = \"common.confirm-intent\"\nvalue = \"Project configuration requires an explicit approval marker.\"\nsource_kind = \"project_configuration\"\nsource = \"project.conf\"\n"
}

fn plan_source() -> &'static str {
    "metadata:\n  priority: P1\n  plan_type: backend\n  area: e2e\n  owner: codex\nsummary: Complete the audited Mino v0.2 lifecycle.\ncontext:\n  - reference: AGENTS.md\n    fact: Approval boundaries are repository policy.\n    implication: Every protected decision remains explicit.\nscope:\n  goal: Prove the complete v0.2 lifecycle.\n  deliverables:\n    - Audited feature and review correction\n  in_scope:\n    - Conflict decisions, amendments, Git commits, and review\n  out_of_scope:\n    - Remote Git operations\ndecisions:\n  - item: Conflict value\n    type: Decision\n    decision: Require an explicit candidate choice\n    reason: Precedence cannot silently merge values\n    status: Accepted\napproach: Execute each protected transition through the public CLI.\ninterfaces: CLI results drive exact revision-checked follow-up commands.\nedge_cases:\n  - case: A material amendment is proposed after approval\n    expected_behavior: Execution blocks until approval, apply, validation, and reapproval\n    covered_by:\n      - T1-A1\ntasks:\n  - id: T1\n    title: Implement the initial feature\n    depends_on: []\n    steps:\n      - Create the planned feature artifact\n    files:\n      - path: feature.txt\n        change: Create\n        reason: Record the initial implemented feature\n    acceptance_criteria:\n      - id: T1-A1\n        description: The initial feature is implemented and verified\n    verification:\n      - id: T1-V1\n        command:\n          - git\n          - --version\n        cwd: .\n        expected_exit_code: 0\n        required: true\n    commit_gate:\n      required: true\n      planned_message: \"feat(fixture): implement v0 two flow\"\n      scope:\n        - feature.txt\nverification_plan:\n  - id: GLOBAL-V1\n    command:\n      - git\n      - --version\n    cwd: .\n    expected_exit_code: 0\n    required: true\n"
}

fn amendment_source() -> &'static str {
    "operations:\n  - operation: replace-summary\n    summary: Complete the explicitly amended and reviewed Mino v0.2 lifecycle.\n"
}

fn rework_source() -> &'static str {
    "id: R1\ntitle: Implement the reviewed correction\ndepends_on:\n  - T1\nsteps:\n  - Create the in-scope reviewed artifact\nfiles:\n  - path: rework.txt\n    change: Create\n    reason: Record the reviewed correction\nacceptance_criteria:\n  - id: R1-A1\n    description: The reviewed correction is implemented and verified\nverification:\n  - id: R1-V1\n    command:\n      - git\n      - --version\n    cwd: .\n    expected_exit_code: 0\n    required: true\ncommit_gate:\n  required: true\n  planned_message: \"fix(review): complete v0 two rework\"\n  scope:\n    - rework.txt\n"
}
