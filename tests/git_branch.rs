//! Contract tests for proposal-only and recoverable approval-gated branch creation.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(unix)]
use std::os::unix::fs::symlink;
#[cfg(windows)]
use std::os::windows::fs::{symlink_dir, symlink_file};

use fs4::FileExt;
use mino::domain::{GitReadiness, Plan, PlanDraftSeed, PlanId, RequestId, Timestamp};
use mino::git::{ActiveBindingStore, GitAdapter, GitBranchJournalStore, GitErrorKind};
use mino::integration::IntegrationOptions;
use mino::project::initialize_with_options;
use mino::render::{render_plan, write_projection};
use mino::store::PlanStore;
use serde_json::Value;

const PLAN_ID: &str = "2026-07-26-branch-gate";
const APPROVAL_REFERENCE: &str = "chat:branch-approval";
static NEXT_TEMP_DIRECTORY: AtomicU64 = AtomicU64::new(1);

struct TestArea {
    root: PathBuf,
}

impl TestArea {
    fn new(label: &str) -> Self {
        let sequence = NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "mino-git-branch-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&root).expect("temporary branch test area should be created");
        Self { root }
    }

    fn repository(&self, name: &str) -> (PathBuf, PlanId) {
        let root = self.root.join(name);
        fs::create_dir(&root).expect("repository directory should be created");
        fs::write(root.join("seed.txt"), "seed\n").expect("seed file should be written");
        initialize_with_options(
            &root,
            IntegrationOptions {
                apply_agents_block: true,
                apply_gitignore_block: true,
            },
        )
        .expect("Mino project should initialize");
        git(&root, &["init", "--quiet", "--initial-branch", "main"]);
        git(
            &root,
            &[
                "add",
                "--",
                "seed.txt",
                ".gitignore",
                "AGENTS.md",
                ".agents",
            ],
        );
        commit(&root, "chore: establish branch fixture");
        let plan_id = create_plan(&root);
        (
            root.canonicalize().expect("repository should resolve"),
            plan_id,
        )
    }
}

impl Drop for TestArea {
    fn drop(&mut self) {
        let temporary_root = std::env::temp_dir();
        if self.root.starts_with(&temporary_root)
            && self
                .root
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("mino-git-branch-"))
        {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

#[cfg(any(unix, windows))]
#[test]
fn branch_journal_rejects_a_symlinked_git_state_directory() {
    let area = TestArea::new("journal-symlink");
    let (root, _) = area.repository("repository");
    let external = TestArea::new("journal-symlink-external");
    let sentinel = external.root.join("sentinel.txt");
    fs::write(&sentinel, b"outside\n").expect("outside sentinel should be written");
    #[cfg(unix)]
    let symlink_result = symlink(&external.root, root.join(".mino/git"));
    #[cfg(windows)]
    let symlink_result = symlink_dir(&external.root, root.join(".mino/git"));
    if symlink_result
        .as_ref()
        .is_err_and(|error| error.kind() == std::io::ErrorKind::PermissionDenied)
    {
        return;
    }
    symlink_result.expect("Git state symlink should be created");

    let error = GitBranchJournalStore::new(&root)
        .lock()
        .expect_err("symlinked Git journal directory must be rejected");
    assert_eq!(error.kind(), GitErrorKind::InvalidOutput);
    assert_eq!(
        fs::read(&sentinel).expect("outside sentinel should remain readable"),
        b"outside\n"
    );
    assert_eq!(
        fs::read_dir(&external.root)
            .expect("outside directory should remain readable")
            .count(),
        1
    );
}

#[cfg(any(unix, windows))]
#[test]
fn active_binding_store_rejects_a_symlinked_target_file() {
    let area = TestArea::new("binding-symlink");
    let (root, plan_id) = area.repository("repository");
    let external = TestArea::new("binding-symlink-external");
    let sentinel = external.root.join("active.json");
    fs::write(&sentinel, b"outside\n").expect("outside sentinel should be written");
    #[cfg(unix)]
    let symlink_result = symlink(&sentinel, root.join(".mino/active.json"));
    #[cfg(windows)]
    let symlink_result = symlink_file(&sentinel, root.join(".mino/active.json"));
    if symlink_result
        .as_ref()
        .is_err_and(|error| error.kind() == std::io::ErrorKind::PermissionDenied)
    {
        return;
    }
    symlink_result.expect("active binding symlink should be created");
    let facts = GitAdapter::new(&root)
        .inspect()
        .expect("Git facts should inspect");

    let error = ActiveBindingStore::new(&root)
        .bind(&facts, plan_id, 1, timestamp())
        .expect_err("symlinked active binding must be rejected");
    assert_eq!(error.kind(), GitErrorKind::InvalidOutput);
    assert_eq!(
        fs::read(&sentinel).expect("outside sentinel should remain readable"),
        b"outside\n"
    );
}

#[test]
fn proposal_is_deterministic_and_preserves_git_and_mino_bytes() {
    let area = TestArea::new("proposal");
    let (root, plan_id) = area.repository("repository");
    let before = protected_git_bytes(&root);
    let output = json_success(&run_mino(
        &root,
        &["git", "branch", "propose", "--plan", plan_id.as_str()],
    ));
    assert_eq!(output["plan_id"], plan_id.as_str());
    assert_eq!(output["branch_name"], proposed_branch(&plan_id));
    assert_eq!(output["can_create"], true);
    assert_eq!(output["blockers"], serde_json::json!([]));
    assert_eq!(protected_git_bytes(&root), before);
    assert!(!active_binding_path(&root).exists());
    assert!(!branch_intent_path(&root, &plan_id).exists());
    assert!(!root.join(".mino/git").exists());
}

#[test]
fn creation_rejects_missing_approval_dirty_existing_invalid_and_stale_sources() {
    let area = TestArea::new("refusals");
    let (root, plan_id) = area.repository("dirty");
    let missing_approval = run_mino(
        &root,
        &["git", "branch", "create", "--plan", plan_id.as_str()],
    );
    assert_eq!(missing_approval.status.code(), Some(2));
    assert_no_branch_operation(&root, &plan_id);

    let empty_approval = run_mino(
        &root,
        &[
            "git",
            "branch",
            "create",
            "--plan",
            plan_id.as_str(),
            "--approval-ref",
            "",
        ],
    );
    assert_json_error(&empty_approval, 4, "approval_required");
    assert_no_branch_operation(&root, &plan_id);

    fs::write(root.join("seed.txt"), "dirty\n").expect("tracked file should become dirty");
    let dirty = approved_create(&root, &plan_id, None);
    assert_json_error(&dirty, 5, "policy_violation");
    assert_no_branch_operation(&root, &plan_id);
    fs::write(root.join("seed.txt"), "seed\n").expect("fixture bytes should be restored");

    let invalid = approved_create(&root, &plan_id, Some("invalid branch"));
    assert_json_error(&invalid, 5, "policy_violation");
    assert_no_branch_operation(&root, &plan_id);

    let (existing_root, existing_plan) = area.repository("existing");
    let branch_name = proposed_branch(&existing_plan);
    git(&existing_root, &["branch", &branch_name]);
    let existing = approved_create(&existing_root, &existing_plan, None);
    assert_json_error(&existing, 5, "policy_violation");
    assert_eq!(current_branch(&existing_root), "main");
    assert!(!branch_intent_path(&existing_root, &existing_plan).exists());

    let (detached_root, detached_plan) = area.repository("detached-mismatch");
    git(&detached_root, &["switch", "--quiet", "--detach", "HEAD"]);
    let detached_head = git_text(&detached_root, &["rev-parse", "HEAD"]);
    let detached = approved_create(&detached_root, &detached_plan, None);
    assert_json_error(&detached, 5, "policy_violation");
    assert_eq!(
        git_text(&detached_root, &["rev-parse", "HEAD"]),
        detached_head
    );
    assert!(!local_branch_exists(
        &detached_root,
        &proposed_branch(&detached_plan)
    ));
    assert!(!branch_intent_path(&detached_root, &detached_plan).exists());
}

#[test]
fn approved_creation_disables_hooks_binds_and_replays_without_mutation() {
    let area = TestArea::new("success");
    let (root, plan_id) = area.repository("repository");
    install_post_checkout_hook(&root);
    let base_head = git_text(&root, &["rev-parse", "HEAD"]);
    let branch_name = proposed_branch(&plan_id);
    let first = json_success(&approved_create(&root, &plan_id, Some(&branch_name)));
    assert_eq!(first["intent"]["approval_reference"], APPROVAL_REFERENCE);
    assert_eq!(first["intent"]["base_head"], base_head);
    assert_eq!(first["completion"]["branch_name"], branch_name);
    assert_eq!(first["active_binding"]["status"], "current");
    assert_eq!(first["replayed"], false);
    assert_eq!(first["reconciled"], false);
    assert_eq!(current_branch(&root), branch_name);
    assert_eq!(git_text(&root, &["rev-parse", "HEAD"]), base_head);
    assert!(!root.join("hook-ran.txt").exists());
    let journal = journal_bytes(&root, &plan_id);
    let git_bytes = protected_git_bytes(&root);

    let replay = json_success(&approved_create(&root, &plan_id, Some(&branch_name)));
    assert_eq!(replay["replayed"], true);
    assert_eq!(replay["reconciled"], false);
    assert_eq!(journal_bytes(&root, &plan_id), journal);
    assert_eq!(protected_git_bytes(&root), git_bytes);
    assert!(!root.join("hook-ran.txt").exists());

    let conflicting = run_mino(
        &root,
        &[
            "git",
            "branch",
            "create",
            "--plan",
            plan_id.as_str(),
            "--approval-ref",
            "chat:different-approval",
        ],
    );
    assert_json_error(&conflicting, 3, "revision_conflict");
    assert_eq!(journal_bytes(&root, &plan_id), journal);
    assert_eq!(protected_git_bytes(&root), git_bytes);
}

#[test]
fn prepared_operations_retry_git_failures_and_reconcile_binding_interruptions() {
    let area = TestArea::new("recovery");
    let (failure_root, failure_plan) = area.repository("git-failure");
    fs::write(failure_root.join(".git/index.lock"), "held\n")
        .expect("fixture index lock should be created");
    let failed = approved_create(&failure_root, &failure_plan, None);
    assert_json_error(&failed, 7, "environment_unavailable");
    assert_eq!(current_branch(&failure_root), "main");
    assert!(!local_branch_exists(
        &failure_root,
        &proposed_branch(&failure_plan)
    ));
    assert!(branch_intent_path(&failure_root, &failure_plan).is_file());
    assert!(!branch_completion_path(&failure_root, &failure_plan).exists());
    assert!(!active_binding_path(&failure_root).exists());
    fs::remove_file(failure_root.join(".git/index.lock"))
        .expect("fixture index lock should be removed");
    let retried = json_success(&approved_create(&failure_root, &failure_plan, None));
    assert_eq!(retried["replayed"], false);
    assert_eq!(retried["reconciled"], false);
    assert_eq!(retried["active_binding"]["status"], "current");

    let (binding_root, binding_plan) = area.repository("binding-interruption");
    let active_lock_path = binding_root.join(".mino/active.lock");
    let active_lock = OpenOptions::new()
        .create(true)
        .read(true)
        .truncate(false)
        .write(true)
        .open(&active_lock_path)
        .expect("active binding lock should open");
    FileExt::lock(&active_lock).expect("active binding lock should be held");
    let interrupted = approved_create(&binding_root, &binding_plan, None);
    assert_json_error(&interrupted, 7, "environment_unavailable");
    assert_eq!(
        current_branch(&binding_root),
        proposed_branch(&binding_plan)
    );
    assert!(branch_intent_path(&binding_root, &binding_plan).is_file());
    assert!(!branch_completion_path(&binding_root, &binding_plan).exists());
    assert!(!active_binding_path(&binding_root).exists());
    FileExt::unlock(&active_lock).expect("active binding lock should release");
    drop(active_lock);

    let reconciled = json_success(&approved_create(&binding_root, &binding_plan, None));
    assert_eq!(reconciled["replayed"], false);
    assert_eq!(reconciled["reconciled"], true);
    assert_eq!(reconciled["active_binding"]["status"], "current");
    assert!(branch_completion_path(&binding_root, &binding_plan).is_file());
}

fn create_plan(root: &Path) -> PlanId {
    let plan_id = PlanId::parse(PLAN_ID).expect("plan ID should parse");
    let plan = Plan::from_draft_seed(
        PlanDraftSeed {
            id: plan_id.clone(),
            name: "Branch gate fixture".to_owned(),
            trigger: "durable".to_owned(),
            original_request: "Create an explicitly approved Git branch.".to_owned(),
            branch: Some("main".to_owned()),
            markdown_path: format!("docs/plan/{plan_id}.md"),
            git_readiness: GitReadiness::detected(
                "Present",
                "Clean",
                Some("main".to_owned()),
                Some(git_text(root, &["rev-parse", "--short", "HEAD"])),
                "Clean: git status --short returned empty",
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
            RequestId::parse("62000000-0000-0000-0000-000000000001")
                .expect("request ID should parse"),
            "codex",
            vec!["test".to_owned(), "create-branch-plan".to_owned()],
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

fn approved_create(root: &Path, plan_id: &PlanId, branch: Option<&str>) -> Output {
    let mut arguments = vec![
        "git",
        "branch",
        "create",
        "--plan",
        plan_id.as_str(),
        "--approval-ref",
        APPROVAL_REFERENCE,
    ];
    if let Some(branch) = branch {
        arguments.extend(["--branch", branch]);
    }
    run_mino(root, &arguments)
}

fn assert_no_branch_operation(root: &Path, plan_id: &PlanId) {
    assert_eq!(current_branch(root), "main");
    assert!(!local_branch_exists(root, &proposed_branch(plan_id)));
    assert!(!active_binding_path(root).exists());
    assert!(!branch_intent_path(root, plan_id).exists());
}

fn protected_git_bytes(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let branch = current_branch(root);
    [
        PathBuf::from(".git/HEAD"),
        PathBuf::from(".git/index"),
        PathBuf::from(format!(".git/refs/heads/{branch}")),
    ]
    .into_iter()
    .map(|relative| {
        (
            relative.clone(),
            fs::read(root.join(&relative)).expect("protected Git file should exist"),
        )
    })
    .collect()
}

fn journal_bytes(root: &Path, plan_id: &PlanId) -> (Vec<u8>, Vec<u8>) {
    (
        fs::read(branch_intent_path(root, plan_id)).expect("intent should be readable"),
        fs::read(branch_completion_path(root, plan_id)).expect("completion should be readable"),
    )
}

fn install_post_checkout_hook(root: &Path) {
    let hook = root.join(".git/hooks/post-checkout");
    fs::write(
        &hook,
        "#!/bin/sh\nprintf 'hook executed' > hook-ran.txt\nexit 1\n",
    )
    .expect("post-checkout hook should be written");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = fs::metadata(&hook)
            .expect("hook metadata should exist")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&hook, permissions).expect("hook should become executable");
    }
}

fn active_binding_path(root: &Path) -> PathBuf {
    root.join(".mino/active.json")
}

fn branch_intent_path(root: &Path, plan_id: &PlanId) -> PathBuf {
    root.join(".mino/git/branches")
        .join(plan_id.as_str())
        .join("intent.json")
}

fn branch_completion_path(root: &Path, plan_id: &PlanId) -> PathBuf {
    root.join(".mino/git/branches")
        .join(plan_id.as_str())
        .join("completion.json")
}

fn proposed_branch(plan_id: &PlanId) -> String {
    format!("mino/{plan_id}")
}

fn current_branch(root: &Path) -> String {
    git_text(root, &["branch", "--show-current"])
}

fn local_branch_exists(root: &Path, branch: &str) -> bool {
    Command::new("git")
        .args([
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ])
        .current_dir(root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .status()
        .expect("Git branch probe should run")
        .success()
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
    serde_json::from_slice(&output.stdout).expect("success output should be JSON")
}

fn assert_json_error(output: &Output, exit_code: i32, code: &str) {
    assert_eq!(
        output.status.code(),
        Some(exit_code),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let value: Value =
        serde_json::from_slice(&output.stdout).expect("failure output should be JSON");
    assert_eq!(value["error"]["code"], code);
}

fn commit(root: &Path, message: &str) {
    git(
        root,
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
    assert!(output.status.success());
    String::from_utf8(output.stdout)
        .expect("Git text should be UTF-8")
        .trim()
        .to_owned()
}

fn timestamp() -> Timestamp {
    Timestamp::parse("2026-07-26T06:00:00Z").expect("timestamp should parse")
}
