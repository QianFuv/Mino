//! Ignore-aware workspace discovery and evidence-based language scoring.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::{ErrorCategory, MinoError};

const MANIFEST_WEIGHT: u16 = 5_000;
const SOURCE_WEIGHT: u16 = 2_500;
const LOCKFILE_WEIGHT: u16 = 800;
const TOOL_CONFIG_WEIGHT: u16 = 700;
const CI_WEIGHT: u16 = 500;
const BUILD_SCRIPT_WEIGHT: u16 = 500;
const SCORE_DENOMINATOR: u16 = 10_000;
const MAX_CI_BYTES: u64 = 256 * 1024;

const EXCLUDED_DIRECTORIES: &[&str] = &[
    ".git",
    ".mino",
    ".cache",
    ".next",
    ".pytest_cache",
    ".ruff_cache",
    ".tox",
    ".venv",
    "__pycache__",
    "build",
    "cache",
    "dist",
    "generated",
    "node_modules",
    "target",
    "vendor",
];

/// Languages recognized by the v0.1 standards engine.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Language {
    /// Rust source and Cargo projects.
    Rust,
    /// TypeScript and JavaScript source and package-manager projects.
    TypeScriptJavaScript,
    /// Python source and Python packaging projects.
    Python,
}

impl Language {
    /// Returns the stable user-facing language name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Rust => "Rust",
            Self::TypeScriptJavaScript => "TypeScript/JavaScript",
            Self::Python => "Python",
        }
    }
}

/// One deterministic scoring signal and its exact contribution.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ScanEvidence {
    /// Stable signal identifier.
    pub code: String,
    /// Contribution in ten-thousandths of full confidence.
    pub weight_basis_points: u16,
    /// Repository-relative evidence path when applicable.
    pub path: Option<PathBuf>,
    /// Concise explanation of the signal.
    pub detail: String,
}

/// A language score with source metrics and ordered contributing evidence.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct LanguageScore {
    /// Scored language.
    pub language: Language,
    /// Confidence in ten-thousandths, from 0 through 10,000.
    pub score_basis_points: u16,
    /// Number of attributed source files.
    pub source_files: u64,
    /// Number of attributed source lines.
    pub source_lines: u64,
    /// Ordered evidence contributing to the score.
    pub evidence: Vec<ScanEvidence>,
}

impl LanguageScore {
    /// Returns confidence normalized to the inclusive range 0.0 through 1.0.
    #[must_use]
    pub fn confidence(&self) -> f64 {
        f64::from(self.score_basis_points) / f64::from(SCORE_DENOMINATOR)
    }
}

/// One discovered workspace and its independently attributed rankings.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct WorkspaceScan {
    /// Repository-relative workspace root, or `.` for the project root.
    pub root: PathBuf,
    /// Supported manifest paths at this workspace root.
    pub manifests: Vec<PathBuf>,
    /// Stable descending language rankings.
    pub languages: Vec<LanguageScore>,
}

/// Complete deterministic project scan result.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProjectScan {
    /// Normalized scanned root.
    pub root: PathBuf,
    /// Number of regular non-generated files considered.
    pub files_scanned: u64,
    /// Number of excluded directories.
    pub directories_excluded: u64,
    /// Number of skipped symbolic links.
    pub symlinks_skipped: u64,
    /// Workspace scans in ascending root order.
    pub workspaces: Vec<WorkspaceScan>,
    /// Aggregate stable descending project rankings.
    pub languages: Vec<LanguageScore>,
}

#[derive(Clone, Debug)]
struct FileFact {
    relative_path: PathBuf,
    language: Option<Language>,
    source_lines: u64,
}

#[derive(Default)]
struct TraversalFacts {
    files: Vec<FileFact>,
    directories_excluded: u64,
    symlinks_skipped: u64,
}

/// Scans an explicitly selected root without running root discovery.
///
/// # Errors
///
/// Returns an environment-unavailable error when the root or a traversed
/// directory cannot be read.
pub fn scan_root(root: &Path) -> Result<ProjectScan, MinoError> {
    let root = root.canonicalize().map_err(|error| {
        MinoError::new(
            ErrorCategory::EnvironmentUnavailable,
            format!("Failed to resolve scan root {}: {error}", root.display()),
        )
    })?;
    if !root.is_dir() {
        return Err(MinoError::new(
            ErrorCategory::EnvironmentUnavailable,
            format!("Scan root {} is not a directory", root.display()),
        ));
    }
    let mut traversal = TraversalFacts::default();
    collect_files(&root, Path::new(""), &mut traversal)?;
    traversal
        .files
        .sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    let workspace_roots = discover_workspace_roots(&traversal.files);
    let ci_files = traversal
        .files
        .iter()
        .filter(|fact| is_ci_file(&fact.relative_path))
        .collect::<Vec<_>>();
    let assignments = assign_workspaces(&traversal.files, &workspace_roots);
    let workspaces = workspace_roots
        .iter()
        .map(|workspace_root| {
            let files = assignments
                .get(workspace_root)
                .map_or(&[][..], Vec::as_slice);
            WorkspaceScan {
                root: display_workspace_root(workspace_root),
                manifests: workspace_manifests(workspace_root, files),
                languages: score_languages(files, workspace_root, &ci_files, &root),
            }
        })
        .collect::<Vec<_>>();
    let languages = score_languages(&traversal.files, Path::new(""), &ci_files, &root);
    Ok(ProjectScan {
        root,
        files_scanned: u64::try_from(traversal.files.len()).unwrap_or(u64::MAX),
        directories_excluded: traversal.directories_excluded,
        symlinks_skipped: traversal.symlinks_skipped,
        workspaces,
        languages,
    })
}

fn collect_files(
    root: &Path,
    relative_directory: &Path,
    facts: &mut TraversalFacts,
) -> Result<(), MinoError> {
    let directory = root.join(relative_directory);
    let entries = fs::read_dir(&directory).map_err(|error| scan_io_error(&directory, &error))?;
    let mut entries = entries.collect::<Result<Vec<_>, _>>().map_err(|error| {
        MinoError::new(
            ErrorCategory::EnvironmentUnavailable,
            format!("Failed to enumerate {}: {error}", directory.display()),
        )
    })?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let relative_path = relative_directory.join(entry.file_name());
        let metadata = fs::symlink_metadata(entry.path())
            .map_err(|error| scan_io_error(&entry.path(), &error))?;
        if metadata.file_type().is_symlink() {
            facts.symlinks_skipped = facts.symlinks_skipped.saturating_add(1);
        } else if metadata.is_dir() {
            if is_excluded_directory(&entry.file_name()) {
                facts.directories_excluded = facts.directories_excluded.saturating_add(1);
            } else {
                collect_files(root, &relative_path, facts)?;
            }
        } else if metadata.is_file() && !is_generated_file(&relative_path) {
            let language = source_language(&relative_path);
            let source_lines = match language {
                Some(_) => count_lines(&entry.path())
                    .map_err(|error| scan_io_error(&entry.path(), &error))?,
                None => 0,
            };
            facts.files.push(FileFact {
                relative_path,
                language,
                source_lines,
            });
        }
    }
    Ok(())
}

fn is_excluded_directory(name: &std::ffi::OsStr) -> bool {
    let name = name.to_string_lossy().to_ascii_lowercase();
    EXCLUDED_DIRECTORIES.contains(&name.as_str())
}

fn is_generated_file(path: &Path) -> bool {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    name.ends_with(".min.js")
        || name.ends_with(".min.css")
        || name.contains(".generated.")
        || name.ends_with("_pb2.py")
}

fn source_language(path: &Path) -> Option<Language> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    match extension.as_str() {
        "rs" => Some(Language::Rust),
        "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" => Some(Language::TypeScriptJavaScript),
        "py" | "pyi" => Some(Language::Python),
        _ => None,
    }
}

fn count_lines(path: &Path) -> Result<u64, std::io::Error> {
    let mut reader = BufReader::new(File::open(path)?);
    let mut buffer = Vec::new();
    let mut count = 0_u64;
    loop {
        buffer.clear();
        let bytes = reader.read_until(b'\n', &mut buffer)?;
        if bytes == 0 {
            break;
        }
        count = count.saturating_add(1);
    }
    Ok(count)
}

fn discover_workspace_roots(files: &[FileFact]) -> Vec<PathBuf> {
    let mut roots = files
        .iter()
        .filter(|fact| manifest_language(&fact.relative_path).is_some())
        .filter_map(|fact| fact.relative_path.parent().map(Path::to_path_buf))
        .collect::<BTreeSet<_>>();
    if roots.is_empty() {
        roots.insert(PathBuf::new());
    }
    roots.into_iter().collect()
}

fn assign_workspaces<'a>(
    files: &'a [FileFact],
    workspace_roots: &[PathBuf],
) -> BTreeMap<PathBuf, Vec<&'a FileFact>> {
    let mut assignments = workspace_roots
        .iter()
        .cloned()
        .map(|root| (root, Vec::new()))
        .collect::<BTreeMap<_, _>>();
    for file in files {
        if let Some(workspace_root) = workspace_roots
            .iter()
            .filter(|root| file.relative_path.starts_with(root))
            .max_by_key(|root| root.components().count())
        {
            assignments
                .entry(workspace_root.clone())
                .or_default()
                .push(file);
        }
    }
    assignments
}

fn display_workspace_root(root: &Path) -> PathBuf {
    if root.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        root.to_path_buf()
    }
}

fn workspace_manifests(root: &Path, files: &[&FileFact]) -> Vec<PathBuf> {
    files
        .iter()
        .filter(|fact| fact.relative_path.parent() == Some(root))
        .filter(|fact| manifest_language(&fact.relative_path).is_some())
        .map(|fact| fact.relative_path.clone())
        .collect()
}

fn score_languages(
    files: &[impl std::borrow::Borrow<FileFact>],
    scope_root: &Path,
    ci_files: &[&FileFact],
    absolute_root: &Path,
) -> Vec<LanguageScore> {
    let total_source_lines = files
        .iter()
        .map(|file| file.borrow().source_lines)
        .sum::<u64>();
    let mut scores = [
        score_language(
            Language::Rust,
            files,
            scope_root,
            ci_files,
            absolute_root,
            total_source_lines,
        ),
        score_language(
            Language::TypeScriptJavaScript,
            files,
            scope_root,
            ci_files,
            absolute_root,
            total_source_lines,
        ),
        score_language(
            Language::Python,
            files,
            scope_root,
            ci_files,
            absolute_root,
            total_source_lines,
        ),
    ]
    .into_iter()
    .filter(|score| score.score_basis_points != 0)
    .collect::<Vec<_>>();
    scores.sort_by(|left, right| {
        right
            .score_basis_points
            .cmp(&left.score_basis_points)
            .then_with(|| left.language.cmp(&right.language))
    });
    scores
}

fn score_language(
    language: Language,
    files: &[impl std::borrow::Borrow<FileFact>],
    scope_root: &Path,
    ci_files: &[&FileFact],
    absolute_root: &Path,
    total_source_lines: u64,
) -> LanguageScore {
    let source_files = files
        .iter()
        .filter(|file| file.borrow().language == Some(language))
        .count();
    let source_lines = files
        .iter()
        .filter(|file| file.borrow().language == Some(language))
        .map(|file| file.borrow().source_lines)
        .sum::<u64>();
    let mut evidence = Vec::new();
    add_first_path_signal(
        &mut evidence,
        files,
        language,
        "manifest",
        MANIFEST_WEIGHT,
        |path| manifest_language(path) == Some(language),
    );
    if source_lines != 0 && total_source_lines != 0 {
        let weight = u16::try_from(
            u64::from(SOURCE_WEIGHT).saturating_mul(source_lines) / total_source_lines,
        )
        .unwrap_or(SOURCE_WEIGHT)
        .clamp(1, SOURCE_WEIGHT);
        evidence.push(ScanEvidence {
            code: "source_share".to_owned(),
            weight_basis_points: weight,
            path: None,
            detail: format!(
                "{source_files} source file(s), {source_lines} of {total_source_lines} supported source line(s)"
            ),
        });
    }
    add_first_path_signal(
        &mut evidence,
        files,
        language,
        "lockfile",
        LOCKFILE_WEIGHT,
        |path| is_lockfile(language, path),
    );
    add_first_path_signal(
        &mut evidence,
        files,
        language,
        "tool_config",
        TOOL_CONFIG_WEIGHT,
        |path| is_tool_config(language, path),
    );
    if let Some(path) = ci_evidence(language, ci_files, absolute_root) {
        evidence.push(ScanEvidence {
            code: "ci".to_owned(),
            weight_basis_points: CI_WEIGHT,
            path: Some(path),
            detail: format!("CI configuration references {} tooling", language.name()),
        });
    }
    add_first_path_signal(
        &mut evidence,
        files,
        language,
        "root_build_script",
        BUILD_SCRIPT_WEIGHT,
        |path| is_root_build_script(language, scope_root, path),
    );
    evidence.sort_by(|left, right| {
        left.code
            .cmp(&right.code)
            .then_with(|| left.path.cmp(&right.path))
    });
    let score_basis_points = evidence
        .iter()
        .map(|item| item.weight_basis_points)
        .sum::<u16>()
        .min(SCORE_DENOMINATOR);
    LanguageScore {
        language,
        score_basis_points,
        source_files: u64::try_from(source_files).unwrap_or(u64::MAX),
        source_lines,
        evidence,
    }
}

fn add_first_path_signal<T, F>(
    evidence: &mut Vec<ScanEvidence>,
    files: &[T],
    language: Language,
    code: &str,
    weight: u16,
    predicate: F,
) where
    T: std::borrow::Borrow<FileFact>,
    F: Fn(&Path) -> bool,
{
    if let Some(file) = files
        .iter()
        .map(std::borrow::Borrow::borrow)
        .find(|file| predicate(&file.relative_path))
    {
        evidence.push(ScanEvidence {
            code: code.to_owned(),
            weight_basis_points: weight,
            path: Some(file.relative_path.clone()),
            detail: format!("{} {code} evidence", language.name()),
        });
    }
}

fn manifest_language(path: &Path) -> Option<Language> {
    match path.file_name()?.to_str()? {
        "Cargo.toml" => Some(Language::Rust),
        "package.json" => Some(Language::TypeScriptJavaScript),
        "pyproject.toml" | "setup.py" => Some(Language::Python),
        _ => None,
    }
}

fn is_lockfile(language: Language, path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    match language {
        Language::Rust => name == "Cargo.lock",
        Language::TypeScriptJavaScript => matches!(
            name,
            "package-lock.json" | "pnpm-lock.yaml" | "yarn.lock" | "bun.lock" | "bun.lockb"
        ),
        Language::Python => matches!(
            name,
            "uv.lock" | "poetry.lock" | "Pipfile.lock" | "requirements.txt"
        ),
    }
}

fn is_tool_config(language: Language, path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    match language {
        Language::Rust => matches!(name, "rustfmt.toml" | ".rustfmt.toml" | "clippy.toml"),
        Language::TypeScriptJavaScript => {
            name == "tsconfig.json"
                || name.starts_with("eslint.config.")
                || name.starts_with(".eslintrc")
                || name.starts_with("prettier.config.")
        }
        Language::Python => matches!(name, "ruff.toml" | "mypy.ini" | "pytest.ini"),
    }
}

fn is_root_build_script(language: Language, scope_root: &Path, path: &Path) -> bool {
    if path.parent() != Some(scope_root) {
        return false;
    }
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    match language {
        Language::Rust => name == "build.rs",
        Language::TypeScriptJavaScript => {
            name.starts_with("vite.config.")
                || name.starts_with("webpack.config.")
                || name.starts_with("next.config.")
        }
        Language::Python => matches!(name, "noxfile.py" | "tox.ini"),
    }
}

fn is_ci_file(path: &Path) -> bool {
    path.starts_with(".github/workflows")
        && path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                extension.eq_ignore_ascii_case("yml") || extension.eq_ignore_ascii_case("yaml")
            })
}

fn ci_evidence(language: Language, ci_files: &[&FileFact], root: &Path) -> Option<PathBuf> {
    let needles: &[&str] = match language {
        Language::Rust => &["cargo", "rustup"],
        Language::TypeScriptJavaScript => &["node", "npm", "pnpm", "yarn", "bun"],
        Language::Python => &["python", "pip", "pytest", "ruff", "uv"],
    };
    ci_files.iter().find_map(|fact| {
        let path = root.join(&fact.relative_path);
        let mut file = File::open(path).ok()?;
        let mut contents = String::new();
        file.by_ref()
            .take(MAX_CI_BYTES)
            .read_to_string(&mut contents)
            .ok()?;
        let contents = contents.to_ascii_lowercase();
        needles
            .iter()
            .any(|needle| contents.contains(needle))
            .then(|| fact.relative_path.clone())
    })
}

fn scan_io_error(path: &Path, error: &std::io::Error) -> MinoError {
    MinoError::new(
        ErrorCategory::EnvironmentUnavailable,
        format!("Failed to scan {}: {error}", path.display()),
    )
}
