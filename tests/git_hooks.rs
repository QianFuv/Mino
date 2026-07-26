//! Contracts for optional approval-bound hooks and read-only hook runtime advice.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use mino::ErrorCategory;
use mino::application::git_hooks::GitHookService;
use mino::git::{GitAdapter, GitHookName, GitHookState};
use mino::project::initialize;
use serde_json::Value;

static NEXT_TEMP_DIRECTORY: AtomicU64 = AtomicU64::new(1);

struct TestProject {
    path: PathBuf,
}

impl TestProject {
    fn new(label: &str) -> Self {
        let sequence = NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "mino-git-hooks-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("temporary repository should be created");
        git(&path, &["init"]);
        git(&path, &["config", "user.name", "Mino Test"]);
        git(&path, &["config", "user.email", "mino@example.invalid"]);
        fs::write(path.join("tracked.txt"), "initial\n").expect("tracked file should write");
        git(&path, &["add", "tracked.txt"]);
        git(&path, &["commit", "-m", "initial"]);
        initialize(&path).expect("Mino project should initialize");
        Self {
            path: path.canonicalize().expect("project root should resolve"),
        }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn hook_path(&self, name: &str) -> PathBuf {
        GitAdapter::new(&self.path)
            .inspect()
            .expect("Git facts should inspect")
            .common_dir
            .expect("common directory should exist")
            .join("hooks")
            .join(name)
    }
}

impl Drop for TestProject {
    fn drop(&mut self) {
        let temporary_root = std::env::temp_dir();
        if self.path.starts_with(&temporary_root)
            && self
                .path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("mino-git-hooks-"))
        {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

fn git(root: &Path, arguments: &[&str]) -> Output {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(arguments)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .output()
        .expect("Git should run");
    assert!(
        output.status.success(),
        "git {arguments:?}\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn repository_bytes(project: &TestProject) -> BTreeMap<String, Vec<u8>> {
    let mut snapshot = BTreeMap::new();
    collect_files(project.path(), &project.path().join(".mino"), &mut snapshot);
    collect_files(project.path(), &project.path().join(".git"), &mut snapshot);
    snapshot
}

fn collect_files(root: &Path, directory: &Path, snapshot: &mut BTreeMap<String, Vec<u8>>) {
    if !directory.exists() {
        return;
    }
    let mut entries = fs::read_dir(directory)
        .expect("snapshot directory should read")
        .map(|entry| entry.expect("snapshot entry should read"))
        .collect::<Vec<_>>();
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).expect("snapshot metadata should read");
        if metadata.is_dir() {
            collect_files(root, &path, snapshot);
        } else if metadata.is_file() {
            let relative = path
                .strip_prefix(root)
                .expect("snapshot path should be relative")
                .to_string_lossy()
                .replace('\\', "/");
            snapshot.insert(relative, fs::read(path).expect("snapshot file should read"));
        }
    }
}

fn run_mino(project: &TestProject, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mino"))
        .arg("--root")
        .arg(project.path())
        .args(["--format", "json", "--no-input"])
        .args(arguments)
        .stdin(Stdio::null())
        .output()
        .expect("Mino should run")
}

fn parse_success(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    serde_json::from_slice(&output.stdout).expect("success should be JSON")
}

fn parse_failure(output: &Output, exit_code: i32) -> Value {
    assert_eq!(output.status.code(), Some(exit_code));
    assert!(output.stderr.is_empty());
    serde_json::from_slice(&output.stdout).expect("failure should be JSON")
}

#[test]
fn absent_and_mino_owned_hooks_install_repair_and_replay_only_after_approval() {
    let project = TestProject::new("install");
    let service = GitHookService::discover(project.path()).expect("service should discover");
    let before = repository_bytes(&project);
    let status = service.status().expect("status should inspect");
    let proposal = service.propose().expect("proposal should inspect");
    assert!(status.installable);
    assert_eq!(status.hooks.len(), 2);
    assert!(
        status
            .hooks
            .iter()
            .all(|hook| hook.state == GitHookState::Absent)
    );
    assert_eq!(proposal.status, status);
    assert_eq!(repository_bytes(&project), before);

    let approval_error = service
        .install(&proposal.proposal_hash, "")
        .expect_err("approval reference should be required");
    assert_eq!(approval_error.category(), ErrorCategory::ApprovalRequired);
    let stale_error = service
        .install("sha256:stale", "chat:hook-approval")
        .expect_err("stale proposal should fail");
    assert_eq!(stale_error.category(), ErrorCategory::DriftDetected);
    assert_eq!(repository_bytes(&project), before);

    let installed = service
        .install(&proposal.proposal_hash, "chat:hook-approval")
        .expect("approved install should succeed");
    assert!(installed.changed);
    assert_eq!(installed.approval_reference, "chat:hook-approval");
    assert!(
        installed
            .status
            .hooks
            .iter()
            .all(|hook| hook.state == GitHookState::Current)
    );
    assert_eq!(
        fs::read(project.hook_path("pre-commit")).expect("pre-commit should read"),
        include_bytes!("../assets/hooks/pre-commit")
    );
    assert_eq!(
        fs::read(project.hook_path("post-commit")).expect("post-commit should read"),
        include_bytes!("../assets/hooks/post-commit")
    );
    assert_hook_is_executable(&project.hook_path("pre-commit"));
    assert_hook_is_executable(&project.hook_path("post-commit"));
    let current_proposal = service.propose().expect("current proposal should inspect");
    assert_eq!(current_proposal.proposal_hash, proposal.proposal_hash);
    let replay = service
        .install(&proposal.proposal_hash, "chat:hook-approval")
        .expect("exact install retry should replay");
    assert!(!replay.changed);

    fs::write(
        project.hook_path("pre-commit"),
        b"#!/bin/sh\n# mino-managed-hook:v1\nexit 0\n",
    )
    .expect("managed drift should be injected");
    let drifted = service.propose().expect("drifted proposal should inspect");
    assert_eq!(drifted.proposal_hash, proposal.proposal_hash);
    assert_eq!(
        drifted.status.hooks[0].state,
        GitHookState::MinoOwnedDrifted
    );
    let repaired = service
        .install(&drifted.proposal_hash, "chat:hook-repair")
        .expect("owned drift should repair");
    assert!(repaired.changed);
    assert_eq!(
        fs::read(project.hook_path("pre-commit")).expect("repaired hook should read"),
        include_bytes!("../assets/hooks/pre-commit")
    );
}

#[test]
fn user_hooks_and_custom_hook_paths_are_preserved_with_manual_snippets() {
    let user_project = TestProject::new("user-owned");
    let user_hook = b"#!/bin/sh\necho user-hook\n";
    fs::write(user_project.hook_path("pre-commit"), user_hook).expect("user hook should write");
    let user_before = repository_bytes(&user_project);
    let service = GitHookService::discover(user_project.path()).expect("service should discover");
    let proposal = service.propose().expect("proposal should inspect");
    assert!(!proposal.status.installable);
    assert_eq!(proposal.status.hooks[0].state, GitHookState::UserOwned);
    assert!(
        proposal.status.hooks[0]
            .integration_snippet
            .as_deref()
            .is_some_and(|snippet| snippet.contains("git hook run --hook pre-commit"))
    );
    let error = service
        .install(&proposal.proposal_hash, "chat:user-hook")
        .expect_err("user hook must never be overwritten");
    assert_eq!(error.category(), ErrorCategory::PolicyViolation);
    assert_eq!(repository_bytes(&user_project), user_before);
    assert!(!user_project.hook_path("post-commit").exists());

    let custom_project = TestProject::new("custom-path");
    git(
        custom_project.path(),
        &["config", "core.hooksPath", "user-hooks"],
    );
    let custom_before = repository_bytes(&custom_project);
    let custom_service =
        GitHookService::discover(custom_project.path()).expect("service should discover");
    let custom = custom_service
        .propose()
        .expect("custom status should inspect");
    assert!(!custom.status.installable);
    assert_eq!(
        custom.status.custom_hooks_path.as_deref(),
        Some("user-hooks")
    );
    let error = custom_service
        .install(&custom.proposal_hash, "chat:custom-hooks")
        .expect_err("custom hooks path must remain user-managed");
    assert_eq!(error.category(), ErrorCategory::PolicyViolation);
    assert_eq!(repository_bytes(&custom_project), custom_before);
}

#[test]
fn runtime_observes_staged_and_head_facts_without_any_repository_or_mino_write() {
    let project = TestProject::new("runtime");
    fs::write(project.path().join("tracked.txt"), "changed\n").expect("tracked file should change");
    git(project.path(), &["add", "tracked.txt"]);
    let before = repository_bytes(&project);
    let service = GitHookService::discover(project.path()).expect("service should discover");
    let pre_commit = service
        .run(GitHookName::PreCommit)
        .expect("pre-commit observation should succeed");
    assert_eq!(pre_commit.staged_paths, ["tracked.txt"]);
    assert!(
        pre_commit
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "git.hook.pre-commit.staged-paths")
    );
    assert_eq!(pre_commit.next_actions[0].id, "agent.context");
    let post_commit = service
        .run(GitHookName::PostCommit)
        .expect("post-commit observation should succeed");
    assert!(post_commit.head.is_some());
    assert!(
        post_commit
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "git.hook.post-commit.head")
    );
    assert_eq!(repository_bytes(&project), before);

    let implementation = include_str!("../src/git/hooks.rs");
    assert!(!implementation.contains("run_mutating"));
    assert!(!implementation.contains("PlanStore"));
    for template in [
        include_str!("../assets/hooks/pre-commit"),
        include_str!("../assets/hooks/post-commit"),
    ] {
        assert!(template.contains("# mino-managed-hook:v1"));
        assert!(template.contains("|| true"));
        assert!(template.ends_with("exit 0\n"));
        assert!(!template.contains('\r'));
        assert!(!template.contains("git add"));
        assert!(!template.contains("git commit"));
    }
    assert!(
        include_str!("../.gitattributes")
            .lines()
            .any(|line| line == "assets/hooks/* text eol=lf")
    );
}

#[cfg(unix)]
fn assert_hook_is_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;

    let mode = fs::metadata(path)
        .expect("hook metadata should read")
        .permissions()
        .mode();
    assert_ne!(mode & 0o111, 0);
}

#[cfg(not(unix))]
fn assert_hook_is_executable(path: &Path) {
    assert!(path.is_file());
}

#[test]
fn cli_hook_contract_is_hash_bound_approval_gated_and_read_only_at_runtime() {
    let project = TestProject::new("cli");
    let proposal = parse_success(&run_mino(&project, &["git", "hook", "propose"]));
    assert_eq!(proposal["kind"], "mino.result/v1");
    assert_eq!(proposal["hook_proposal_kind"], "mino.git-hook-proposal/v1");
    let proposal_hash = proposal["proposal_hash"]
        .as_str()
        .expect("proposal hash should be text")
        .to_owned();
    let approval = parse_failure(
        &run_mino(
            &project,
            &[
                "git",
                "hook",
                "install",
                "--proposal-hash",
                &proposal_hash,
                "--approval-ref",
                "",
            ],
        ),
        4,
    );
    assert_eq!(approval["error"]["code"], "approval_required");
    let installed = parse_success(&run_mino(
        &project,
        &[
            "git",
            "hook",
            "install",
            "--proposal-hash",
            &proposal_hash,
            "--approval-ref",
            "chat:cli-hook-approval",
        ],
    ));
    assert_eq!(installed["hook_install_kind"], "mino.git-hook-install/v1");
    assert_eq!(installed["changed"], true);
    let status = parse_success(&run_mino(&project, &["git", "hook", "status"]));
    assert_eq!(status["hook_status_kind"], "mino.git-hook-status/v1");
    assert!(
        status["hooks"]
            .as_array()
            .is_some_and(|hooks| hooks.iter().all(|hook| hook["state"] == "current"))
    );

    fs::write(project.path().join("tracked.txt"), "cli change\n")
        .expect("tracked file should change");
    git(project.path(), &["add", "tracked.txt"]);
    let before = repository_bytes(&project);
    let runtime = parse_success(&run_mino(
        &project,
        &["git", "hook", "run", "--hook", "pre-commit"],
    ));
    assert_eq!(runtime["hook_runtime_kind"], "mino.git-hook-runtime/v1");
    assert_eq!(runtime["staged_paths"], serde_json::json!(["tracked.txt"]));
    assert_eq!(repository_bytes(&project), before);
}
