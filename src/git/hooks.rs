//! Approval-ready repository hook inspection, owned installation, and read-only runtime advice.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;

use super::command::run_read_only;
use super::{
    ActiveBindingStatus, ActiveBindingStore, GitAdapter, GitError, GitErrorKind, GitFacts,
};
use crate::NextAction;
use crate::domain::PlanId;
use crate::store::sha256_digest;

/// Stable schema identifier for hook status payloads.
pub const GIT_HOOK_STATUS_KIND: &str = "mino.git-hook-status/v1";
/// Stable schema identifier for approval-bound hook proposals.
pub const GIT_HOOK_PROPOSAL_KIND: &str = "mino.git-hook-proposal/v1";
/// Stable schema identifier for hook runtime observations.
pub const GIT_HOOK_RUNTIME_KIND: &str = "mino.git-hook-runtime/v1";

const PRE_COMMIT_TEMPLATE: &[u8] = include_bytes!("../../assets/hooks/pre-commit");
const POST_COMMIT_TEMPLATE: &[u8] = include_bytes!("../../assets/hooks/post-commit");
const OWNERSHIP_MARKER: &str = "# mino-managed-hook:v1";
const MAX_HOOK_BYTES: u64 = 1024 * 1024;
static NEXT_HOOK_FILE: AtomicU64 = AtomicU64::new(1);

/// Supported advisory repository hook names.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum GitHookName {
    /// Observe staged paths immediately before a commit.
    PreCommit,
    /// Observe the resulting HEAD immediately after a commit.
    PostCommit,
}

impl GitHookName {
    /// Returns the Git hook filename and CLI spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::PreCommit => "pre-commit",
            Self::PostCommit => "post-commit",
        }
    }
}

/// Ownership and content relationship for one hook path.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GitHookState {
    /// No hook exists at the default path.
    Absent,
    /// Exact current Mino-owned bytes are installed.
    Current,
    /// A Mino marker exists but the managed bytes differ.
    MinoOwnedDrifted,
    /// A regular hook exists without Mino ownership.
    UserOwned,
    /// The path kind or hooks directory cannot be safely managed.
    Unsupported,
}

impl GitHookState {
    const fn is_installable(self) -> bool {
        matches!(self, Self::Absent | Self::Current | Self::MinoOwnedDrifted)
    }

    const fn fingerprint_class(self) -> &'static str {
        if self.is_installable() {
            "installable"
        } else {
            match self {
                Self::UserOwned => "user_owned",
                Self::Unsupported => "unsupported",
                Self::Absent | Self::Current | Self::MinoOwnedDrifted => unreachable!(),
            }
        }
    }
}

/// Read-only status for one default hook path.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GitHookStatus {
    /// Hook identity.
    pub hook: GitHookName,
    /// Absolute normalized target path.
    pub path: String,
    /// Current ownership/content state.
    pub state: GitHookState,
    /// Digest of the current managed template.
    pub template_digest: String,
    /// Digest of existing bounded regular-file bytes when available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual_digest: Option<String>,
    /// Snippet a user may integrate manually when Mino cannot own the path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub integration_snippet: Option<String>,
}

/// Complete read-only repository hook status.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GitHookStatusReport {
    /// Stable result discriminator.
    pub hook_status_kind: &'static str,
    /// Canonical worktree root.
    pub worktree: String,
    /// Canonical shared Git directory that owns default hooks.
    pub common_dir: String,
    /// Configured non-default hooks path when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_hooks_path: Option<String>,
    /// Whether both default paths are safe for managed installation.
    pub installable: bool,
    /// Deterministic blockers that prevent managed installation.
    pub blockers: Vec<String>,
    /// Pre-commit then post-commit status.
    pub hooks: Vec<GitHookStatus>,
}

/// Hash-bound proposal presented before explicit hook installation approval.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GitHookProposal {
    /// Stable result discriminator.
    pub hook_proposal_kind: &'static str,
    /// Digest binding repository identity, templates, and ownership classes.
    pub proposal_hash: String,
    /// Whether installation requires an explicit external approval reference.
    pub approval_required: bool,
    /// Complete current hook status covered by the proposal.
    pub status: GitHookStatusReport,
}

/// One read-only hook-runtime diagnostic.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GitHookDiagnostic {
    /// Stable diagnostic identifier.
    pub code: String,
    /// Concise human-readable observation.
    pub message: String,
}

/// Read-only repository facts observed by one advisory hook invocation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GitHookRuntimeReport {
    /// Stable result discriminator.
    pub hook_runtime_kind: &'static str,
    /// Invoked hook identity.
    pub hook: GitHookName,
    /// Canonical worktree root.
    pub worktree: String,
    /// Current branch when attached.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// Current full HEAD when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    /// Sorted staged paths observed without optional Git locks.
    pub staged_paths: Vec<String>,
    /// Current worktree binding relationship.
    pub binding_status: ActiveBindingStatus,
    /// Bound plan when the current/stale relationship exposes one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bound_plan: Option<PlanId>,
    /// Deterministic advisory diagnostics.
    pub diagnostics: Vec<GitHookDiagnostic>,
    /// Read-only commands a caller may choose to run after the hook returns.
    pub next_actions: Vec<NextAction>,
}

#[derive(Serialize)]
struct ProposalFingerprint<'a> {
    worktree: &'a str,
    common_dir: &'a str,
    custom_hooks_path: &'a Option<String>,
    hooks: Vec<HookFingerprint<'a>>,
}

#[derive(Serialize)]
struct HookFingerprint<'a> {
    hook: GitHookName,
    path: &'a str,
    ownership_class: &'static str,
    template_digest: &'a str,
    conflict_digest: Option<&'a str>,
}

/// Returns current default-hook ownership and content status without mutation.
pub(crate) fn hook_status(root: &Path) -> Result<GitHookStatusReport, GitError> {
    let facts = required_worktree(root)?;
    let worktree = path_text(facts.worktree.as_deref().expect("required worktree"))?;
    let common_dir_path = facts
        .common_dir
        .as_deref()
        .expect("required common directory");
    let common_dir = path_text(common_dir_path)?;
    let custom_hooks_path = configured_hooks_path(Path::new(&worktree))?;
    let hooks_directory = common_dir_path.join("hooks");
    let mut blockers = Vec::new();
    if let Some(path) = &custom_hooks_path {
        blockers.push(format!(
            "Repository config core.hooksPath={path} is user-managed"
        ));
    }
    let hooks_directory_is_safe = safe_hooks_directory(&hooks_directory, &mut blockers)?;
    let hooks = [GitHookName::PreCommit, GitHookName::PostCommit]
        .into_iter()
        .map(|hook| inspect_hook(&hooks_directory, hook, hooks_directory_is_safe))
        .collect::<Result<Vec<_>, _>>()?;
    for hook in &hooks {
        match hook.state {
            GitHookState::UserOwned => blockers.push(format!(
                "Existing {} hook is user-owned and must be integrated manually",
                hook.hook.as_str()
            )),
            GitHookState::Unsupported => blockers.push(format!(
                "Existing {} hook path has an unsupported file type",
                hook.hook.as_str()
            )),
            GitHookState::Absent | GitHookState::Current | GitHookState::MinoOwnedDrifted => {}
        }
    }
    let installable = custom_hooks_path.is_none()
        && hooks_directory_is_safe
        && hooks.iter().all(|hook| hook.state.is_installable());
    Ok(GitHookStatusReport {
        hook_status_kind: GIT_HOOK_STATUS_KIND,
        worktree,
        common_dir,
        custom_hooks_path,
        installable,
        blockers,
        hooks,
    })
}

/// Returns a stable approval proposal for the current hook ownership classes.
pub(crate) fn hook_proposal(root: &Path) -> Result<GitHookProposal, GitError> {
    let status = hook_status(root)?;
    let proposal_hash = proposal_hash(&status)?;
    Ok(GitHookProposal {
        hook_proposal_kind: GIT_HOOK_PROPOSAL_KIND,
        proposal_hash,
        approval_required: true,
        status,
    })
}

/// Installs or repairs only installable Mino-owned default hooks.
pub(crate) fn install_hooks(root: &Path) -> Result<(bool, GitHookStatusReport), GitError> {
    let before = hook_status(root)?;
    if !before.installable {
        return Err(policy(format!(
            "Repository hooks are not safely installable: {}",
            before.blockers.join("; ")
        )));
    }
    let hooks_directory = Path::new(&before.common_dir).join("hooks");
    ensure_hooks_directory(&hooks_directory)?;
    let mut changed = false;
    for status in &before.hooks {
        if status.state != GitHookState::Current {
            write_owned_hook(
                &hooks_directory.join(status.hook.as_str()),
                status,
                template(status.hook),
            )?;
            changed = true;
        }
    }
    let after = hook_status(root)?;
    if !after.installable
        || after
            .hooks
            .iter()
            .any(|hook| hook.state != GitHookState::Current)
    {
        return Err(invalid("Installed hook bytes did not verify as current"));
    }
    Ok((changed, after))
}

/// Observes staged/HEAD and binding facts without mutating Git or Mino state.
pub(crate) fn observe_hook(
    root: &Path,
    hook: GitHookName,
) -> Result<GitHookRuntimeReport, GitError> {
    let facts = required_worktree(root)?;
    let binding = ActiveBindingStore::new(root).resolve(&facts)?;
    let bound_plan = binding
        .binding
        .as_ref()
        .map(|binding| binding.plan_id.clone());
    let mut diagnostics = Vec::new();
    match hook {
        GitHookName::PreCommit if facts.staged_paths.is_empty() => diagnostics.push(diagnostic(
            "git.hook.pre-commit.no-staged-paths",
            "No staged paths were visible to the advisory pre-commit hook",
        )),
        GitHookName::PreCommit => diagnostics.push(diagnostic(
            "git.hook.pre-commit.staged-paths",
            format!(
                "Observed {} staged path(s) without modifying the index",
                facts.staged_paths.len()
            ),
        )),
        GitHookName::PostCommit if facts.head.is_none() => diagnostics.push(diagnostic(
            "git.hook.post-commit.no-head",
            "No commit HEAD was visible to the advisory post-commit hook",
        )),
        GitHookName::PostCommit => diagnostics.push(diagnostic(
            "git.hook.post-commit.head",
            format!(
                "Observed commit {} without modifying repository state",
                facts.head.as_deref().expect("guarded HEAD")
            ),
        )),
    }
    diagnostics.push(diagnostic(
        "git.hook.binding-status",
        format!("Active binding status is {:?}", binding.status),
    ));
    let next_actions = if binding.status == ActiveBindingStatus::Current {
        bound_plan
            .as_ref()
            .map(inspect_plan_action)
            .into_iter()
            .collect()
    } else {
        vec![agent_context_action()]
    };
    Ok(GitHookRuntimeReport {
        hook_runtime_kind: GIT_HOOK_RUNTIME_KIND,
        hook,
        worktree: path_text(facts.worktree.as_deref().expect("required worktree"))?,
        branch: facts.branch,
        head: facts.head,
        staged_paths: facts.staged_paths,
        binding_status: binding.status,
        bound_plan,
        diagnostics,
        next_actions,
    })
}

fn required_worktree(root: &Path) -> Result<GitFacts, GitError> {
    let facts = GitAdapter::new(root).inspect()?;
    if !facts.repository
        || !facts.is_worktree
        || facts.worktree.is_none()
        || facts.common_dir.is_none()
    {
        return Err(policy("Repository hooks require a non-bare Git worktree"));
    }
    Ok(facts)
}

fn configured_hooks_path(worktree: &Path) -> Result<Option<String>, GitError> {
    let output = run_read_only(worktree, ["config", "--get", "core.hooksPath"])?;
    if !output.success {
        if output.exit_code == Some(1) && output.stdout.is_empty() {
            return Ok(None);
        }
        return Err(invalid(format!(
            "Git could not read core.hooksPath: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let value = String::from_utf8(output.stdout)
        .map_err(|_| invalid("Git core.hooksPath is not valid UTF-8"))?;
    let value = value.trim();
    if value.is_empty() || value.contains(['\r', '\n', '\0']) {
        return Err(invalid("Git core.hooksPath is empty or malformed"));
    }
    Ok(Some(value.replace('\\', "/")))
}

fn safe_hooks_directory(path: &Path, blockers: &mut Vec<String>) -> Result<bool, GitError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            blockers.push(format!(
                "Default hooks path {} is not a regular directory",
                path.display()
            ));
            Ok(false)
        }
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(path_error("inspect hooks directory", path, &error)),
    }
}

fn inspect_hook(
    directory: &Path,
    hook: GitHookName,
    directory_is_safe: bool,
) -> Result<GitHookStatus, GitError> {
    let path = directory.join(hook.as_str());
    let template = template(hook);
    let template_digest = sha256_digest(template);
    let mut status = GitHookStatus {
        hook,
        path: path_text(&path)?,
        state: GitHookState::Absent,
        template_digest,
        actual_digest: None,
        integration_snippet: None,
    };
    if !directory_is_safe {
        status.state = GitHookState::Unsupported;
        status.integration_snippet = Some(integration_snippet(hook));
        return Ok(status);
    }
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(status),
        Err(error) => return Err(path_error("inspect hook", &path, &error)),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        status.state = GitHookState::Unsupported;
        status.integration_snippet = Some(integration_snippet(hook));
        return Ok(status);
    }
    if metadata.len() > MAX_HOOK_BYTES {
        status.state = GitHookState::UserOwned;
        status.integration_snippet = Some(integration_snippet(hook));
        return Ok(status);
    }
    let bytes = fs::read(&path).map_err(|error| path_error("read hook", &path, &error))?;
    status.actual_digest = Some(sha256_digest(&bytes));
    status.state = if bytes == template {
        GitHookState::Current
    } else if is_mino_owned(&bytes) {
        GitHookState::MinoOwnedDrifted
    } else {
        status.integration_snippet = Some(integration_snippet(hook));
        GitHookState::UserOwned
    };
    Ok(status)
}

fn proposal_hash(status: &GitHookStatusReport) -> Result<String, GitError> {
    let hooks = status
        .hooks
        .iter()
        .map(|hook| HookFingerprint {
            hook: hook.hook,
            path: &hook.path,
            ownership_class: hook.state.fingerprint_class(),
            template_digest: &hook.template_digest,
            conflict_digest: (!hook.state.is_installable())
                .then_some(hook.actual_digest.as_deref())
                .flatten(),
        })
        .collect();
    let fingerprint = ProposalFingerprint {
        worktree: &status.worktree,
        common_dir: &status.common_dir,
        custom_hooks_path: &status.custom_hooks_path,
        hooks,
    };
    serde_json::to_vec(&fingerprint)
        .map(|bytes| sha256_digest(&bytes))
        .map_err(|error| invalid(format!("Failed to encode hook proposal: {error}")))
}

fn ensure_hooks_directory(path: &Path) -> Result<(), GitError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => Err(policy(
            format!("Hooks path {} is not a regular directory", path.display()),
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(path).map_err(|error| path_error("create hooks directory", path, &error))
        }
        Err(error) => Err(path_error("inspect hooks directory", path, &error)),
    }
}

fn write_owned_hook(
    path: &Path,
    expected: &GitHookStatus,
    replacement: &[u8],
) -> Result<(), GitError> {
    let current = inspect_hook(
        path.parent()
            .ok_or_else(|| invalid("Hook path has no parent directory"))?,
        expected.hook,
        true,
    )?;
    if current.state != expected.state || current.actual_digest != expected.actual_digest {
        return Err(invalid(format!(
            "Hook {} changed after proposal preflight",
            expected.hook.as_str()
        )));
    }
    let temporary = temporary_path(path, "tmp");
    write_new_file(&temporary, replacement)?;
    if expected.state == GitHookState::Absent {
        if let Err(error) = fs::rename(&temporary, path) {
            let _ = fs::remove_file(&temporary);
            return Err(path_error("publish hook", path, &error));
        }
    } else {
        let backup = temporary_path(path, "backup");
        fs::rename(path, &backup).map_err(|error| path_error("back up hook", path, &error))?;
        if let Err(error) = fs::rename(&temporary, path) {
            let restoration = fs::rename(&backup, path);
            let _ = fs::remove_file(&temporary);
            return match restoration {
                Ok(()) => Err(path_error("replace hook", path, &error)),
                Err(restoration_error) => Err(unavailable(format!(
                    "Failed to replace hook {}: {error}; restoration failed: {restoration_error}",
                    path.display()
                ))),
            };
        }
        fs::remove_file(&backup)
            .map_err(|error| path_error("remove hook backup", &backup, &error))?;
    }
    let actual = fs::read(path).map_err(|error| path_error("verify hook", path, &error))?;
    if actual != replacement {
        return Err(invalid(format!(
            "Published hook {} differs from the managed template",
            path.display()
        )));
    }
    Ok(())
}

fn write_new_file(path: &Path, bytes: &[u8]) -> Result<(), GitError> {
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    let mut file = options
        .open(path)
        .map_err(|error| path_error("create temporary hook", path, &error))?;
    let result = file.write_all(bytes).and_then(|()| file.sync_all());
    #[cfg(unix)]
    let result = result.and_then(|()| set_executable(&file));
    drop(file);
    if let Err(error) = result {
        let _ = fs::remove_file(path);
        return Err(path_error("write temporary hook", path, &error));
    }
    Ok(())
}

#[cfg(unix)]
fn set_executable(file: &fs::File) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = file.metadata()?.permissions();
    permissions.set_mode(0o755);
    file.set_permissions(permissions)
}

fn temporary_path(path: &Path, suffix: &str) -> PathBuf {
    let sequence = NEXT_HOOK_FILE.fetch_add(1, Ordering::Relaxed);
    path.parent().expect("hook path has a parent").join(format!(
        ".mino-{}-{}-{sequence}.{suffix}",
        path.file_name()
            .expect("hook path has a file name")
            .to_string_lossy(),
        std::process::id()
    ))
}

fn template(hook: GitHookName) -> &'static [u8] {
    match hook {
        GitHookName::PreCommit => PRE_COMMIT_TEMPLATE,
        GitHookName::PostCommit => POST_COMMIT_TEMPLATE,
    }
}

fn is_mino_owned(bytes: &[u8]) -> bool {
    bytes
        .split(|byte| *byte == b'\n')
        .nth(1)
        .and_then(|line| std::str::from_utf8(line.strip_suffix(b"\r").unwrap_or(line)).ok())
        == Some(OWNERSHIP_MARKER)
}

fn integration_snippet(hook: GitHookName) -> String {
    format!(
        "if command -v mino >/dev/null 2>&1; then\n  mino git hook run --hook {} --format human --no-input || true\nfi",
        hook.as_str()
    )
}

fn inspect_plan_action(plan_id: &PlanId) -> NextAction {
    NextAction {
        id: "git.inspect".to_owned(),
        argv: vec![
            "mino".to_owned(),
            "git".to_owned(),
            "inspect".to_owned(),
            "--plan".to_owned(),
            plan_id.to_string(),
            "--format".to_owned(),
            "json".to_owned(),
            "--no-input".to_owned(),
        ],
    }
}

fn agent_context_action() -> NextAction {
    NextAction {
        id: "agent.context".to_owned(),
        argv: vec![
            "mino".to_owned(),
            "agent".to_owned(),
            "context".to_owned(),
            "--format".to_owned(),
            "json".to_owned(),
            "--no-input".to_owned(),
        ],
    }
}

fn diagnostic(code: &str, message: impl Into<String>) -> GitHookDiagnostic {
    GitHookDiagnostic {
        code: code.to_owned(),
        message: message.into(),
    }
}

fn path_text(path: &Path) -> Result<String, GitError> {
    path.to_str()
        .map(|value| value.replace('\\', "/"))
        .ok_or_else(|| invalid(format!("Path {} is not valid UTF-8", path.display())))
}

fn path_error(action: &str, path: &Path, error: &std::io::Error) -> GitError {
    unavailable(format!("Failed to {action} {}: {error}", path.display()))
}

fn policy(message: impl Into<String>) -> GitError {
    GitError::new(GitErrorKind::PolicyViolation, message)
}

fn invalid(message: impl Into<String>) -> GitError {
    GitError::new(GitErrorKind::InvalidOutput, message)
}

fn unavailable(message: impl Into<String>) -> GitError {
    GitError::new(GitErrorKind::Unavailable, message)
}
