//! Idempotent package application and project-aware check resolution.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;

use crate::project::LockedStandard;
use crate::runner::probe::{BoundedCommandErrorKind, BoundedCommandRunner};
use crate::{ErrorCategory, MinoError};

use super::catalog::{CheckTemplate, EmbeddedCatalog, StandardRule};
use super::recommend::StandardsRecommendation;

const TOOL_ENVIRONMENT_ALLOWLIST: &[&str] = &[
    "CARGO_HOME",
    "HOME",
    "LANG",
    "PATH",
    "PATHEXT",
    "RUSTUP_HOME",
    "SYSTEMROOT",
    "TEMP",
    "TMP",
    "TMPDIR",
    "USERPROFILE",
    "WINDIR",
];

/// Typed terminal result of one automatic tool availability probe.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolProbeOutcome {
    /// The version command exited successfully within every bound.
    Available,
    /// The executable could not start or returned an unsuccessful status.
    Unavailable,
    /// The version command exceeded the tool-probe deadline.
    TimedOut,
    /// Combined stdout and stderr exceeded the capture limit.
    OutputLimitExceeded,
    /// Output capture, observation, or process-tree termination failed.
    Failed,
}

impl ToolProbeOutcome {
    const fn is_available(self) -> bool {
        matches!(self, Self::Available)
    }

    fn unresolved_reason(self, tool: &str) -> Option<String> {
        match self {
            Self::Available => None,
            Self::Unavailable => Some(format!("Required tool {tool} is unavailable")),
            Self::TimedOut => Some("Tool probe timed out".to_owned()),
            Self::OutputLimitExceeded => Some("Tool probe output exceeded 65536 bytes".to_owned()),
            Self::Failed => Some("Tool probe failed".to_owned()),
        }
    }
}

/// Availability probe used to keep command resolution deterministic in tests.
pub trait ToolProbe {
    /// Returns a typed terminal outcome for one required executable or capability.
    fn probe(&self, tool: &str, working_directory: &Path) -> ToolProbeOutcome;
}

/// Host-process tool probe that invokes only `--version` checks.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemToolProbe;

impl ToolProbe for SystemToolProbe {
    fn probe(&self, tool: &str, working_directory: &Path) -> ToolProbeOutcome {
        let mut command = if tool == "cargo-miri" {
            let mut command = Command::new("cargo");
            command.args(["+nightly", "miri", "--version"]);
            command
        } else {
            let mut command = Command::new(tool);
            command.arg("--version");
            command
        };
        command
            .current_dir(working_directory)
            .env_clear()
            .envs(allowed_tool_environment())
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("LC_ALL", "C");
        match BoundedCommandRunner::tool_probe().run(&mut command) {
            Ok(output) if output.status.success() => ToolProbeOutcome::Available,
            Ok(_) => ToolProbeOutcome::Unavailable,
            Err(error) => match error.kind() {
                BoundedCommandErrorKind::Spawn => ToolProbeOutcome::Unavailable,
                BoundedCommandErrorKind::Timeout => ToolProbeOutcome::TimedOut,
                BoundedCommandErrorKind::OutputLimit => ToolProbeOutcome::OutputLimitExceeded,
                BoundedCommandErrorKind::InvalidLimits
                | BoundedCommandErrorKind::Capture
                | BoundedCommandErrorKind::Observe
                | BoundedCommandErrorKind::Terminate => ToolProbeOutcome::Failed,
            },
        }
    }
}

/// Whether a seeded check can be executed in the detected project environment.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolvedCheckStatus {
    /// The selected command's required tool is available.
    Runnable,
    /// The check is retained but cannot currently execute.
    Unresolved,
}

/// Source selected for a resolved command.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandSource {
    /// The inert embedded package template was selected.
    EmbeddedTemplate,
    /// A project-declared package script took precedence.
    ProjectScript,
}

/// One seeded project-specific verification check.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ResolvedCheck {
    /// Stable verification identifier.
    pub id: String,
    /// Resolved argument vector.
    pub argv: Vec<String>,
    /// Project-relative working directory.
    pub cwd: PathBuf,
    /// Whether this check is required.
    pub required: bool,
    /// Executability status.
    pub status: ResolvedCheckStatus,
    /// Resolution source.
    pub source: CommandSource,
    /// Required executable or capability.
    pub tool: String,
    /// Actionable reason when unresolved.
    pub unresolved_reason: Option<String>,
}

/// Exact selected packages, merged rules, and seeded verification checks.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct StandardsApplication {
    /// Exact selected package versions and digests.
    pub standards: Vec<LockedStandard>,
    /// Unique merged rules in stable identifier order.
    pub rules: Vec<StandardRule>,
    /// Unique resolved checks in stable identifier order.
    pub checks: Vec<ResolvedCheck>,
}

/// Applies a recommendation as inert records and project-aware check commands.
///
/// # Errors
///
/// Returns a validation error for a missing package or conflicting duplicate
/// rule/check identifier.
pub fn apply_recommendation<P: ToolProbe>(
    project_root: &Path,
    catalog: &EmbeddedCatalog,
    recommendation: &StandardsRecommendation,
    probe: &P,
) -> Result<StandardsApplication, MinoError> {
    let mut standards = Vec::new();
    let mut rules = BTreeMap::<String, StandardRule>::new();
    let mut checks = BTreeMap::<String, ResolvedCheck>::new();
    for selected in &recommendation.packages {
        let package = catalog.package(&selected.package_id).ok_or_else(|| {
            MinoError::new(
                ErrorCategory::IncompleteOrValidation,
                format!("Recommended package {} is unavailable", selected.package_id),
            )
        })?;
        if package.version() != selected.version || package.digest() != selected.digest {
            return Err(MinoError::new(
                ErrorCategory::IncompleteOrValidation,
                format!(
                    "Recommended package {} changed version or digest",
                    selected.package_id
                ),
            ));
        }
        standards.push(LockedStandard {
            package_id: package.package_id().to_owned(),
            version: package.version().to_owned(),
            digest: package.digest().to_owned(),
        });
        for rule in package.rules() {
            insert_unique(&mut rules, &rule.id, rule.clone(), "rule")?;
        }
        for template in package.checks() {
            let resolved = resolve_check(project_root, package.package_id(), template, probe);
            insert_unique(&mut checks, &template.id, resolved, "check")?;
        }
    }
    standards.sort_by(|left, right| left.package_id.cmp(&right.package_id));
    Ok(StandardsApplication {
        standards,
        rules: rules.into_values().collect(),
        checks: checks.into_values().collect(),
    })
}

fn insert_unique<T: Eq>(
    values: &mut BTreeMap<String, T>,
    id: &str,
    value: T,
    kind: &str,
) -> Result<(), MinoError> {
    if let Some(existing) = values.get(id) {
        if existing == &value {
            return Ok(());
        }
        return Err(MinoError::new(
            ErrorCategory::PolicyViolation,
            format!("Selected standards contain conflicting {kind} identifier {id}"),
        ));
    }
    values.insert(id.to_owned(), value);
    Ok(())
}

fn resolve_check<P: ToolProbe>(
    project_root: &Path,
    package_id: &str,
    template: &CheckTemplate,
    probe: &P,
) -> ResolvedCheck {
    let (argv, tool, source) = if package_id == "typescript-javascript" {
        resolve_typescript_check(project_root, template)
    } else if package_id == "python" {
        resolve_python_check(project_root, template, probe)
    } else {
        (
            template.argv.clone(),
            template.tool.clone(),
            CommandSource::EmbeddedTemplate,
        )
    };
    let outcome = probe.probe(&tool, project_root);
    ResolvedCheck {
        id: template.id.clone(),
        argv,
        cwd: PathBuf::from("."),
        required: template.required,
        status: if outcome.is_available() {
            ResolvedCheckStatus::Runnable
        } else {
            ResolvedCheckStatus::Unresolved
        },
        source,
        unresolved_reason: outcome.unresolved_reason(&tool),
        tool,
    }
}

fn resolve_typescript_check(
    project_root: &Path,
    template: &CheckTemplate,
) -> (Vec<String>, String, CommandSource) {
    let manager = package_manager(project_root);
    if let Some(script) = template.project_script.as_deref()
        && package_scripts(project_root).contains(script)
    {
        return (
            vec![manager.clone(), "run".to_owned(), script.to_owned()],
            manager,
            CommandSource::ProjectScript,
        );
    }
    let arguments = template.argv.iter().skip(2).cloned().collect::<Vec<_>>();
    let argv = match manager.as_str() {
        "pnpm" | "yarn" => [vec![manager.clone(), "exec".to_owned()], arguments].concat(),
        "npm" => [
            vec![manager.clone(), "exec".to_owned(), "--".to_owned()],
            arguments,
        ]
        .concat(),
        "bun" => [vec!["bunx".to_owned()], arguments].concat(),
        _ => template.argv.clone(),
    };
    let tool = if manager == "bun" {
        "bunx".to_owned()
    } else {
        manager
    };
    (argv, tool, CommandSource::EmbeddedTemplate)
}

fn resolve_python_check<P: ToolProbe>(
    project_root: &Path,
    template: &CheckTemplate,
    probe: &P,
) -> (Vec<String>, String, CommandSource) {
    if project_root.join("uv.lock").is_file() || probe.probe("uv", project_root).is_available() {
        return (
            template.argv.clone(),
            "uv".to_owned(),
            CommandSource::EmbeddedTemplate,
        );
    }
    let tool = template
        .argv
        .get(2)
        .cloned()
        .unwrap_or_else(|| template.tool.clone());
    let argv = template.argv.iter().skip(2).cloned().collect::<Vec<_>>();
    (argv, tool, CommandSource::EmbeddedTemplate)
}

fn allowed_tool_environment() -> Vec<(String, OsString)> {
    TOOL_ENVIRONMENT_ALLOWLIST
        .iter()
        .filter_map(|name| std::env::var_os(name).map(|value| ((*name).to_owned(), value)))
        .collect()
}

fn package_manager(project_root: &Path) -> String {
    for (lockfile, manager) in [
        ("pnpm-lock.yaml", "pnpm"),
        ("yarn.lock", "yarn"),
        ("bun.lock", "bun"),
        ("bun.lockb", "bun"),
        ("package-lock.json", "npm"),
    ] {
        if project_root.join(lockfile).is_file() {
            return manager.to_owned();
        }
    }
    "npm".to_owned()
}

fn package_scripts(project_root: &Path) -> BTreeSet<String> {
    let path = project_root.join("package.json");
    let Ok(contents) = fs::read(&path) else {
        return BTreeSet::new();
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&contents) else {
        return BTreeSet::new();
    };
    value["scripts"]
        .as_object()
        .map(|scripts| scripts.keys().cloned().collect())
        .unwrap_or_default()
}
