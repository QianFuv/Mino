//! Contract tests for porcelain-v2 Git facts and worktree-aware active bindings.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use mino::domain::{GitReadiness, Plan, PlanDraftSeed, PlanId, RequestId, Timestamp};
use mino::git::{
    GitAdapter, GitAvailability, GitErrorKind, GitHeadState, GitStatusEntry, GitStatusKind,
    parse_porcelain_v2,
};
use mino::integration::IntegrationOptions;
use mino::project::initialize_with_options;
use mino::render::{render_plan, write_projection};
use mino::store::PlanStore;
use serde_json::Value;

static NEXT_TEMP_DIRECTORY: AtomicU64 = AtomicU64::new(1);

struct TestArea {
    root: PathBuf,
}

impl TestArea {
    fn new(label: &str) -> Self {
        let sequence = NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "mino-git-inspect-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&root).expect("temporary Git test area should be created");
        Self { root }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }
}

impl Drop for TestArea {
    fn drop(&mut self) {
        let temporary_root = std::env::temp_dir();
        if self.root.starts_with(&temporary_root)
            && self
                .root
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("mino-git-inspect-"))
        {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

#[test]
fn porcelain_v2_parser_accepts_every_record_and_rejects_unsafe_bytes() {
    let hash = "1".repeat(40);
    let input = format!(
        "# branch.oid {hash}\0# branch.head main\0# branch.upstream origin/main\0# branch.ab +2 -1\01 MM N... 100644 100644 100644 {hash} {hash} tracked.txt\02 R. N... 100644 100644 100644 {hash} {hash} R100 renamed.txt\0old.txt\0u UU N... 100644 100644 100644 100644 {hash} {hash} {hash} conflicted.txt\0? untracked file.txt\0"
    );
    let parsed = parse_porcelain_v2(input.as_bytes()).expect("porcelain fixture should parse");
    assert_eq!(parsed.branch_oid.as_deref(), Some(hash.as_str()));
    assert_eq!(parsed.branch_head.as_deref(), Some("main"));
    assert_eq!(parsed.branch_upstream.as_deref(), Some("origin/main"));
    assert_eq!(parsed.ahead, Some(2));
    assert_eq!(parsed.behind, Some(1));
    assert_eq!(parsed.entries.len(), 4);
    assert_eq!(parsed.entries[0].kind, GitStatusKind::Unmerged);
    assert_eq!(parsed.entries[1].kind, GitStatusKind::RenamedOrCopied);
    assert_eq!(parsed.entries[1].original_path.as_deref(), Some("old.txt"));
    assert!(parsed.entries[2].is_staged());
    assert!(parsed.entries[2].is_unstaged());
    assert_eq!(parsed.entries[3].kind, GitStatusKind::Untracked);

    for invalid in [
        b"? ../escape\0".as_slice(),
        b"1 M. N... too-few\0".as_slice(),
        b"2 R. N... 1 1 1 a b R100 current\0".as_slice(),
        b"? duplicate\0? duplicate\0".as_slice(),
        b"? \xff\0".as_slice(),
    ] {
        assert_eq!(
            parse_porcelain_v2(invalid)
                .expect_err("invalid porcelain should fail")
                .kind(),
            GitErrorKind::InvalidOutput
        );
    }
}

#[test]
fn inspect_reports_normal_status_submodules_and_preserves_git_bytes() {
    let area = TestArea::new("normal");
    let non_repository = area.path("plain");
    fs::create_dir(&non_repository).expect("plain directory should exist");
    let plain = GitAdapter::new(&non_repository)
        .inspect()
        .expect("plain directory should inspect");
    assert!(!plain.repository);
    assert_eq!(plain.head_state, GitHeadState::NotRepository);
    assert_eq!(
        GitAdapter::new(&non_repository)
            .inspect_availability()
            .expect("plain directory availability should inspect"),
        GitAvailability::NotRepository
    );

    let repository = committed_repository(&area.path("repository"));
    add_status_matrix(&area, &repository);
    let protected = protected_git_bytes(&repository);
    let nested = repository.join("nested/path");
    fs::create_dir_all(&nested).expect("nested inspection path should exist");
    let facts = GitAdapter::new(&nested)
        .inspect()
        .expect("normal repository should inspect");
    assert!(facts.repository);
    assert!(facts.is_worktree);
    assert_eq!(facts.head_state, GitHeadState::Branch);
    assert_eq!(facts.branch.as_deref(), Some("main"));
    assert_eq!(facts.head.as_deref().map(str::len), Some(40));
    assert_eq!(facts.worktree.as_deref(), Some(repository.as_path()));
    assert!(facts.common_dir.as_ref().is_some_and(|path| path.is_dir()));
    assert!(facts.git_dir.as_ref().is_some_and(|path| path.is_dir()));
    assert!(facts.index_file.as_ref().is_some_and(|path| path.is_file()));
    assert!(facts.staged_paths.contains(&"tracked.txt".to_owned()));
    assert!(facts.unstaged_paths.contains(&"tracked.txt".to_owned()));
    assert!(
        facts
            .untracked_paths
            .contains(&"untracked 空间.txt".to_owned())
    );
    assert!(facts.status.iter().any(GitStatusEntry::is_submodule));
    assert!(!facts.is_clean);
    assert_eq!(protected_git_bytes(&repository), protected);
}

#[test]
fn availability_rejects_corrupt_repository_metadata_without_echoing_git_output() {
    let area = TestArea::new("corrupt-metadata");
    let repository = committed_repository(&area.path("repository"));
    let secret = "gitmetadatacredentialvalue";
    fs::write(repository.join(".git/HEAD"), format!("invalid {secret}\n"))
        .expect("corrupt HEAD should be written");

    let error = GitAdapter::new(&repository)
        .inspect_availability()
        .expect_err("corrupt repository metadata must not become NotRepository");
    assert_eq!(error.kind(), GitErrorKind::Unavailable);
    assert!(!error.message().contains(secret));
}

#[test]
fn inspect_distinguishes_unborn_detached_and_linked_worktrees() {
    let area = TestArea::new("identities");
    let unborn = area.path("unborn");
    initialize_repository(&unborn);
    let unborn_facts = GitAdapter::new(&unborn)
        .inspect()
        .expect("unborn repository should inspect");
    assert_eq!(unborn_facts.head_state, GitHeadState::Unborn);
    assert_eq!(unborn_facts.branch.as_deref(), Some("main"));
    assert_eq!(unborn_facts.head, None);

    let detached = committed_repository(&area.path("detached"));
    git(&detached, &["switch", "--quiet", "--detach", "HEAD"]);
    let detached_facts = GitAdapter::new(&detached)
        .inspect()
        .expect("detached repository should inspect");
    assert_eq!(detached_facts.head_state, GitHeadState::Detached);
    assert_eq!(detached_facts.branch, None);
    assert!(detached_facts.head.is_some());

    let main = committed_repository(&area.path("main worktree"));
    let linked = area.path("linked worktree");
    git(
        &main,
        &[
            "worktree",
            "add",
            "--quiet",
            "-b",
            "linked",
            path_text(&linked),
        ],
    );
    let main_facts = GitAdapter::new(&main)
        .inspect()
        .expect("main worktree should inspect");
    let linked_facts = GitAdapter::new(&linked)
        .inspect()
        .expect("linked worktree should inspect");
    assert_eq!(main_facts.branch.as_deref(), Some("main"));
    assert_eq!(linked_facts.branch.as_deref(), Some("linked"));
    assert_eq!(main_facts.common_dir, linked_facts.common_dir);
    assert_ne!(main_facts.worktree, linked_facts.worktree);
    assert_ne!(main_facts.git_dir, linked_facts.git_dir);
    assert_ne!(main_facts.index_file, linked_facts.index_file);
}

#[test]
fn cli_binding_is_idempotent_branch_aware_and_cannot_cross_worktrees() {
    let area = TestArea::new("binding");
    let main = initialized_mino_repository(&area.path("main"));
    let plan_id = create_plan(&main);
    let binding_path = main.join(".mino/active.json");
    let inspected = json_success(&run_mino(
        &main,
        &["git", "inspect", "--plan", plan_id.as_str()],
    ));
    assert_eq!(inspected["active_binding"]["status"], "missing");
    assert_eq!(inspected["is_requested_plan_bound"], false);
    assert!(!binding_path.exists());
    let missing_current = run_mino(&main, &["git", "bind", "--plan", plan_id.as_str()]);
    assert_eq!(missing_current.status.code(), Some(2));
    assert!(!binding_path.exists());

    let bound = json_success(&run_mino(
        &main,
        &["git", "bind", "--plan", plan_id.as_str(), "--current"],
    ));
    assert_eq!(bound["binding"]["plan_id"], plan_id.as_str());
    assert_eq!(bound["replayed"], false);
    let binding_bytes = fs::read(&binding_path).expect("binding should be readable");
    let replay = json_success(&run_mino(
        &main,
        &["git", "bind", "--plan", plan_id.as_str(), "--current"],
    ));
    assert_eq!(replay["replayed"], true);
    assert_eq!(fs::read(&binding_path).unwrap(), binding_bytes);

    let selected_plan_id = assert_explicit_rebinding(&main, &plan_id);
    let context = agent_context(&main);
    assert_eq!(context["active_plan"]["id"], selected_plan_id.as_str());
    assert_eq!(context["git"]["binding_status"], "current");

    git(&main, &["switch", "--quiet", "-c", "other"]);
    let stale = agent_context(&main);
    assert_eq!(stale["active_plan"]["id"], selected_plan_id.as_str());
    assert_eq!(stale["git"]["binding_status"], "stale_branch");
    let doctor = json_success(&run_mino(&main, &["project", "doctor"]));
    assert!(doctor["findings"].as_array().is_some_and(|findings| {
        findings
            .iter()
            .any(|finding| finding["code"] == "active_binding_branch_stale")
    }));
    let rebound = json_success(&run_mino(
        &main,
        &["git", "bind", "--plan", plan_id.as_str(), "--current"],
    ));
    assert_eq!(rebound["binding"]["branch"], "other");

    git(&main, &["switch", "--quiet", "--detach", "HEAD"]);
    let detached_binding = json_success(&run_mino(
        &main,
        &["git", "bind", "--plan", plan_id.as_str(), "--current"],
    ));
    assert_eq!(detached_binding["binding"]["branch"], Value::Null);
    assert!(detached_binding["binding"]["detached_head"].is_string());
    git(&main, &["switch", "--quiet", "other"]);
    let stale_head = agent_context(&main);
    assert_eq!(stale_head["active_plan"]["id"], selected_plan_id.as_str());
    assert_eq!(stale_head["git"]["binding_status"], "stale_head");
    json_success(&run_mino(
        &main,
        &["git", "bind", "--plan", plan_id.as_str(), "--current"],
    ));

    let linked = area.path("linked");
    git(
        &main,
        &[
            "worktree",
            "add",
            "--quiet",
            "-b",
            "linked",
            path_text(&linked),
        ],
    );
    initialize_project(&linked);
    fs::copy(&binding_path, linked.join(".mino/active.json"))
        .expect("foreign binding should be copied");
    let linked_context = agent_context(&linked);
    assert_eq!(linked_context["active_plan"], Value::Null);
    assert_eq!(linked_context["git"]["binding_status"], "foreign_worktree");
    let linked_doctor = json_success(&run_mino(&linked, &["project", "doctor"]));
    assert!(
        linked_doctor["findings"]
            .as_array()
            .is_some_and(|findings| {
                findings
                    .iter()
                    .any(|finding| finding["code"] == "active_binding_worktree_mismatch")
            })
    );
}

fn assert_explicit_rebinding(root: &Path, original_plan_id: &PlanId) -> PlanId {
    let second_plan_id = create_additional_plan(root);
    let replacement = json_success(&run_mino(
        root,
        &[
            "git",
            "bind",
            "--plan",
            second_plan_id.as_str(),
            "--current",
        ],
    ));
    assert_eq!(replacement["binding"]["plan_id"], second_plan_id.as_str());
    let unresolved = agent_context(root);
    assert_eq!(unresolved["active_plan"], Value::Null);
    let alternatives = json_success(&run_mino(root, &["plan", "alternatives"]));
    assert_eq!(alternatives["selection_revision"], 0);
    assert_eq!(alternatives["selected_plan"], Value::Null);
    json_success(&run_mino(
        root,
        &[
            "plan",
            "select",
            "--plan",
            second_plan_id.as_str(),
            "--expect-selection-revision",
            "0",
            "--request-id",
            "61000000-0000-0000-0000-000000000003",
            "--actor",
            "codex",
            "--approval-ref",
            "test:explicit-selection",
            "--reason",
            "Select the second project alternative",
        ],
    ));
    assert_eq!(
        agent_context(root)["active_plan"]["id"],
        second_plan_id.as_str()
    );
    json_success(&run_mino(
        root,
        &[
            "git",
            "bind",
            "--plan",
            original_plan_id.as_str(),
            "--current",
        ],
    ));
    assert_eq!(
        agent_context(root)["active_plan"]["id"],
        second_plan_id.as_str()
    );
    second_plan_id
}

fn add_status_matrix(area: &TestArea, repository: &Path) {
    fs::write(repository.join("tracked.txt"), "staged version\n")
        .expect("tracked file should change");
    git(repository, &["add", "--", "tracked.txt"]);
    fs::write(repository.join("tracked.txt"), "worktree version\n")
        .expect("tracked file should change again");
    fs::write(repository.join("staged.txt"), "staged\n").expect("staged file should exist");
    git(repository, &["add", "--", "staged.txt"]);
    fs::write(repository.join("untracked 空间.txt"), "untracked\n")
        .expect("untracked file should exist");

    let origin = committed_repository(&area.path("submodule origin"));
    let origin_argument = git_path_argument(&origin);
    git(
        repository,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "--quiet",
            &origin_argument,
            "modules/child",
        ],
    );
    fs::write(
        repository.join("modules/child/tracked.txt"),
        "dirty submodule\n",
    )
    .expect("submodule worktree should change");
}

fn initialized_mino_repository(path: &Path) -> PathBuf {
    fs::create_dir_all(path).expect("Mino repository should be created");
    fs::write(path.join("seed.txt"), "seed\n").expect("seed should be written");
    initialize_project(path);
    initialize_repository(path);
    git(
        path,
        &[
            "add",
            "--",
            "seed.txt",
            ".gitignore",
            "AGENTS.md",
            ".agents",
        ],
    );
    commit(path, "chore: establish binding fixture");
    path.canonicalize().expect("Mino repository should resolve")
}

fn initialize_project(path: &Path) {
    initialize_with_options(
        path,
        IntegrationOptions {
            apply_agents_block: true,
            apply_gitignore_block: true,
        },
    )
    .expect("Mino project should initialize");
}

fn create_plan(root: &Path) -> PlanId {
    create_plan_with_identity(
        root,
        "2026-07-26-git-binding",
        "61000000-0000-0000-0000-000000000001",
    )
}

fn create_additional_plan(root: &Path) -> PlanId {
    create_plan_with_identity(
        root,
        "2026-07-26-second-binding",
        "61000000-0000-0000-0000-000000000002",
    )
}

fn create_plan_with_identity(root: &Path, id: &str, request_id: &str) -> PlanId {
    let plan_id = PlanId::parse(id).expect("plan ID should parse");
    let plan = Plan::from_draft_seed(
        PlanDraftSeed {
            id: plan_id.clone(),
            name: "Git binding fixture".to_owned(),
            trigger: "durable".to_owned(),
            original_request: "Bind this plan to one worktree.".to_owned(),
            branch: Some("main".to_owned()),
            markdown_path: format!("docs/plan/{plan_id}.md"),
            git_readiness: GitReadiness::detected(
                "Present",
                "Clean",
                Some("main".to_owned()),
                git_text(root, &["rev-parse", "HEAD"]),
                "Clean",
                true,
            ),
            standards: Vec::new(),
            verification_plan: Vec::new(),
        },
        timestamp(),
    );
    PlanStore::new(root)
        .create_plan(
            &plan,
            RequestId::parse(request_id).expect("request ID should parse"),
            "codex",
            vec!["test".to_owned(), "create-plan".to_owned()],
        )
        .expect("plan should persist");
    let rendered = render_plan(&plan).expect("plan should render");
    write_projection(
        &root.join(format!("docs/plan/{plan_id}.md")),
        &rendered,
        None,
    )
    .expect("projection should publish");
    plan_id
}

fn committed_repository(path: &Path) -> PathBuf {
    initialize_repository(path);
    fs::write(path.join("tracked.txt"), "initial\n").expect("tracked file should be written");
    git(path, &["add", "--", "tracked.txt"]);
    commit(path, "chore: initialize fixture");
    path.canonicalize().expect("repository should resolve")
}

fn initialize_repository(path: &Path) {
    fs::create_dir_all(path).expect("repository directory should be created");
    git(path, &["init", "--quiet", "--initial-branch", "main"]);
}

fn commit(path: &Path, message: &str) {
    git(
        path,
        &[
            "-c",
            "user.name=Mino Tests",
            "-c",
            "user.email=mino-tests@example.invalid",
            "commit",
            "--quiet",
            "-m",
            message,
        ],
    );
}

fn protected_git_bytes(repository: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    [".git/HEAD", ".git/index", ".git/refs/heads/main"]
        .into_iter()
        .map(|relative| {
            (
                PathBuf::from(relative),
                fs::read(repository.join(relative)).expect("protected Git file should exist"),
            )
        })
        .collect()
}

fn agent_context(root: &Path) -> Value {
    json_agent(&run_mino(root, &["agent", "context"]))
}

fn run_mino(root: &Path, command: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mino"))
        .arg("--root")
        .arg(root)
        .args(["--format", "json", "--no-input"])
        .args(command)
        .stdin(Stdio::null())
        .output()
        .expect("Mino binary should run")
}

fn json_success(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let value: Value = serde_json::from_slice(&output.stdout).expect("stdout should be JSON");
    assert_eq!(value["kind"], "mino.result/v1");
    value
}

fn json_agent(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    serde_json::from_slice(&output.stdout).expect("Agent stdout should be JSON")
}

fn git(path: &Path, arguments: &[&str]) {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(path)
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

fn git_text(path: &Path, arguments: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(arguments)
        .current_dir(path)
        .output()
        .expect("Git should run");
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn path_text(path: &Path) -> &str {
    path.to_str().expect("test path should be UTF-8")
}

fn git_path_argument(path: &Path) -> String {
    path_text(path)
        .strip_prefix(r"\\?\")
        .unwrap_or_else(|| path_text(path))
        .to_owned()
}

fn timestamp() -> Timestamp {
    Timestamp::parse("2026-07-26T05:00:00Z").expect("timestamp should parse")
}
