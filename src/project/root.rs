//! Deterministic Git-first project-root discovery.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;

use crate::{ErrorCategory, MinoError};

const AUTHORITATIVE_MARKERS: &[&str] = &[
    "Cargo.toml",
    "package.json",
    "pyproject.toml",
    "setup.py",
    "go.mod",
    "pom.xml",
    "build.gradle",
    "build.gradle.kts",
];

/// Evidence used to select a project root.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RootSource {
    /// Git reported the repository top level.
    Git,
    /// An existing `.mino` directory was found while walking upward.
    MinoDirectory,
    /// A supported authoritative project manifest was found.
    Manifest,
    /// Initialization explicitly fell back to the supplied directory.
    InitializationFallback,
}

/// A normalized project root and the evidence that selected it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProjectRoot {
    path: PathBuf,
    source: RootSource,
}

impl ProjectRoot {
    /// Returns the normalized project-root path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the evidence that selected this root.
    #[must_use]
    pub const fn source(&self) -> RootSource {
        self.source
    }
}

/// Discovers an existing project using Git first and filesystem markers second.
///
/// # Errors
///
/// Returns an environment-unavailable error when no supported root is found or
/// the starting path cannot be normalized.
pub fn discover(start: &Path) -> Result<ProjectRoot, MinoError> {
    discover_inner(start, false)
}

/// Discovers a project for initialization, falling back to the start directory.
///
/// # Errors
///
/// Returns an environment-unavailable error when the starting path cannot be
/// normalized to a directory.
pub fn discover_for_init(start: &Path) -> Result<ProjectRoot, MinoError> {
    discover_inner(start, true)
}

fn discover_inner(start: &Path, allow_fallback: bool) -> Result<ProjectRoot, MinoError> {
    let normalized_start = normalize_start(start)?;
    if let Some(path) = discover_git_root(&normalized_start) {
        return Ok(ProjectRoot {
            path,
            source: RootSource::Git,
        });
    }
    for ancestor in normalized_start.ancestors() {
        if ancestor.join(".mino").is_dir() {
            return Ok(ProjectRoot {
                path: ancestor.to_path_buf(),
                source: RootSource::MinoDirectory,
            });
        }
        if AUTHORITATIVE_MARKERS
            .iter()
            .any(|marker| ancestor.join(marker).is_file())
        {
            return Ok(ProjectRoot {
                path: ancestor.to_path_buf(),
                source: RootSource::Manifest,
            });
        }
    }
    if allow_fallback {
        Ok(ProjectRoot {
            path: normalized_start,
            source: RootSource::InitializationFallback,
        })
    } else {
        Err(MinoError::new(
            ErrorCategory::EnvironmentUnavailable,
            format!(
                "No Git repository, .mino directory, or supported project manifest was found from {}",
                normalized_start.display()
            ),
        ))
    }
}

fn normalize_start(start: &Path) -> Result<PathBuf, MinoError> {
    let canonical = start.canonicalize().map_err(|error| {
        MinoError::new(
            ErrorCategory::EnvironmentUnavailable,
            format!(
                "Failed to resolve project start {}: {error}",
                start.display()
            ),
        )
    })?;
    if canonical.is_dir() {
        Ok(canonical)
    } else if canonical.is_file() {
        canonical.parent().map(Path::to_path_buf).ok_or_else(|| {
            MinoError::new(
                ErrorCategory::EnvironmentUnavailable,
                format!(
                    "Project start {} has no parent directory",
                    canonical.display()
                ),
            )
        })
    } else {
        Err(MinoError::new(
            ErrorCategory::EnvironmentUnavailable,
            format!(
                "Project start {} is not a file or directory",
                canonical.display()
            ),
        ))
    }
}

fn discover_git_root(start: &Path) -> Option<PathBuf> {
    let output = Command::new("git")
        .arg("-C")
        .arg(start)
        .args(["rev-parse", "--show-toplevel"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let path = String::from_utf8(output.stdout).ok()?;
    let path = PathBuf::from(path.trim());
    path.canonicalize().ok().filter(|path| path.is_dir())
}
