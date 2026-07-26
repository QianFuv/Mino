//! Ignore-aware workspace discovery and evidence-based language scoring.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use ignore::WalkBuilder;
use serde::Serialize;

use crate::{ErrorCategory, MinoError};

const MANIFEST_WEIGHT: u16 = 5_000;
const SOURCE_WEIGHT: u16 = 2_500;
const LOCKFILE_WEIGHT: u16 = 800;
const TOOL_CONFIG_WEIGHT: u16 = 700;
const CI_WEIGHT: u16 = 500;
const BUILD_SCRIPT_WEIGHT: u16 = 500;
const SCORE_DENOMINATOR: u16 = 10_000;
const DEFAULT_MAX_DEPTH: usize = 64;
const DEFAULT_MAX_FILES: u64 = 100_000;
const DEFAULT_MAX_TOTAL_BYTES: u64 = 128 * 1024 * 1024;
const DEFAULT_MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;
const READ_BUFFER_BYTES: usize = 64 * 1024;
const DEPTH_LIMIT_REASON: &str = "depth_limit";
const FILE_LIMIT_REASON: &str = "file_limit";
const PER_FILE_BYTE_LIMIT_REASON: &str = "per_file_byte_limit";
const TOTAL_BYTE_LIMIT_REASON: &str = "total_byte_limit";

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
    /// Number of built-in generated or cache directories excluded.
    pub directories_excluded: u64,
    /// Number of skipped symbolic links.
    pub symlinks_skipped: u64,
    /// Number of file-content bytes read for source and CI evidence.
    pub bytes_read: u64,
    /// Whether one or more configured scan budgets truncated the result.
    pub truncated: bool,
    /// Stable sorted codes for every budget that truncated the result.
    pub truncation_reasons: Vec<String>,
    /// Workspace scans in ascending root order.
    pub workspaces: Vec<WorkspaceScan>,
    /// Aggregate stable descending project rankings.
    pub languages: Vec<LanguageScore>,
}

/// Positive resource limits applied to one project scan.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScanLimits {
    /// Maximum path depth below the scan root, with the root at depth zero.
    pub max_depth: usize,
    /// Maximum number of regular files visited before traversal stops.
    pub max_files: u64,
    /// Maximum aggregate bytes read from source and CI files.
    pub max_total_bytes: u64,
    /// Maximum bytes read from any one source or CI file.
    pub max_file_bytes: u64,
}

impl Default for ScanLimits {
    fn default() -> Self {
        Self {
            max_depth: DEFAULT_MAX_DEPTH,
            max_files: DEFAULT_MAX_FILES,
            max_total_bytes: DEFAULT_MAX_TOTAL_BYTES,
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
        }
    }
}

#[derive(Clone, Debug)]
struct FileFact {
    relative_path: PathBuf,
    language: Option<Language>,
    source_lines: u64,
    ci_languages: BTreeSet<Language>,
}

#[derive(Default)]
struct TraversalFacts {
    files: Vec<FileFact>,
    directories_excluded: u64,
    symlinks_skipped: u64,
    files_visited: u64,
    bytes_read: u64,
    truncation_reasons: BTreeSet<&'static str>,
}

#[derive(Default)]
struct ContentFacts {
    source_lines: u64,
    ci_languages: BTreeSet<Language>,
}

#[derive(Default)]
struct CiMatcher {
    languages: BTreeSet<Language>,
    tail: Vec<u8>,
}

/// Scans an explicitly selected root without running root discovery.
///
/// # Errors
///
/// Returns an environment-unavailable error when the root or a traversed
/// directory cannot be read.
pub fn scan_root(root: &Path) -> Result<ProjectScan, MinoError> {
    scan_root_with_limits(root, ScanLimits::default())
}

/// Scans an explicitly selected root with caller-provided positive limits.
///
/// # Errors
///
/// Returns an incomplete-or-validation error when any limit is zero, or an
/// environment-unavailable error when the root or an included path cannot be
/// read.
pub fn scan_root_with_limits(root: &Path, limits: ScanLimits) -> Result<ProjectScan, MinoError> {
    validate_scan_limits(limits)?;
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
    let traversal = collect_files(&root, limits)?;
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
                languages: score_languages(files, workspace_root, &ci_files),
            }
        })
        .collect::<Vec<_>>();
    let languages = score_languages(&traversal.files, Path::new(""), &ci_files);
    let truncation_reasons = traversal
        .truncation_reasons
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    Ok(ProjectScan {
        root,
        files_scanned: u64::try_from(traversal.files.len()).unwrap_or(u64::MAX),
        directories_excluded: traversal.directories_excluded,
        symlinks_skipped: traversal.symlinks_skipped,
        bytes_read: traversal.bytes_read,
        truncated: !truncation_reasons.is_empty(),
        truncation_reasons,
        workspaces,
        languages,
    })
}

fn validate_scan_limits(limits: ScanLimits) -> Result<(), MinoError> {
    let invalid = [
        (
            "max_depth",
            u64::try_from(limits.max_depth).unwrap_or(u64::MAX),
        ),
        ("max_files", limits.max_files),
        ("max_total_bytes", limits.max_total_bytes),
        ("max_file_bytes", limits.max_file_bytes),
    ]
    .into_iter()
    .filter_map(|(name, value)| (value == 0).then_some(name))
    .collect::<Vec<_>>();
    if invalid.is_empty() {
        Ok(())
    } else {
        Err(MinoError::new(
            ErrorCategory::IncompleteOrValidation,
            format!("Scan limits must be positive: {}", invalid.join(", ")),
        ))
    }
}

fn collect_files(root: &Path, limits: ScanLimits) -> Result<TraversalFacts, MinoError> {
    let directories_excluded = Arc::new(AtomicU64::new(0));
    let depth_truncated = Arc::new(AtomicBool::new(false));
    let mut builder = WalkBuilder::new(root);
    builder
        .standard_filters(true)
        .hidden(false)
        .parents(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .require_git(false)
        .follow_links(false)
        .current_dir(root)
        .sort_by_file_path(Path::cmp);
    let excluded_counter = Arc::clone(&directories_excluded);
    let depth_flag = Arc::clone(&depth_truncated);
    builder.filter_entry(move |entry| {
        if entry.depth() == 0
            || !entry
                .file_type()
                .is_some_and(|file_type| file_type.is_dir())
        {
            return true;
        }
        if is_excluded_directory(entry.file_name()) {
            excluded_counter.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        if entry.depth() >= limits.max_depth {
            depth_flag.store(true, Ordering::Relaxed);
            return false;
        }
        true
    });

    let mut facts = TraversalFacts::default();
    for entry in builder.build() {
        let entry = entry.map_err(|error| scan_walk_error(root, &error))?;
        if entry.depth() == 0 {
            continue;
        }
        if entry.path_is_symlink() {
            facts.symlinks_skipped = facts.symlinks_skipped.saturating_add(1);
            continue;
        }
        let Some(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_file() {
            continue;
        }
        if facts.files_visited >= limits.max_files {
            facts.truncation_reasons.insert(FILE_LIMIT_REASON);
            break;
        }
        facts.files_visited = facts.files_visited.saturating_add(1);
        let relative_path = entry.path().strip_prefix(root).map_err(|error| {
            MinoError::new(
                ErrorCategory::EnvironmentUnavailable,
                format!(
                    "Failed to make scan path {} relative to {}: {error}",
                    entry.path().display(),
                    root.display()
                ),
            )
        })?;
        if is_generated_file(relative_path) {
            continue;
        }
        let metadata = entry
            .metadata()
            .map_err(|error| scan_walk_error(entry.path(), &error))?;
        let language = source_language(relative_path);
        let content = inspect_content(
            entry.path(),
            metadata.len(),
            language.is_some(),
            is_ci_file(relative_path),
            &mut facts,
            limits,
        )?;
        facts.files.push(FileFact {
            relative_path: relative_path.to_path_buf(),
            language,
            source_lines: content.source_lines,
            ci_languages: content.ci_languages,
        });
    }
    facts.directories_excluded = directories_excluded.load(Ordering::Relaxed);
    if depth_truncated.load(Ordering::Relaxed) {
        facts.truncation_reasons.insert(DEPTH_LIMIT_REASON);
    }
    Ok(facts)
}

fn inspect_content(
    path: &Path,
    file_size: u64,
    should_count_lines: bool,
    should_match_ci: bool,
    facts: &mut TraversalFacts,
    limits: ScanLimits,
) -> Result<ContentFacts, MinoError> {
    if !should_count_lines && !should_match_ci {
        return Ok(ContentFacts::default());
    }
    let remaining_total = limits.max_total_bytes.saturating_sub(facts.bytes_read);
    if file_size > limits.max_file_bytes {
        facts.truncation_reasons.insert(PER_FILE_BYTE_LIMIT_REASON);
    }
    let per_file_read_limit = file_size.min(limits.max_file_bytes);
    if per_file_read_limit > remaining_total {
        facts.truncation_reasons.insert(TOTAL_BYTE_LIMIT_REASON);
    }
    let read_limit = per_file_read_limit.min(remaining_total);
    if read_limit == 0 {
        return Ok(ContentFacts::default());
    }

    let mut file = File::open(path).map_err(|error| scan_io_error(path, &error))?;
    let mut buffer = vec![0_u8; READ_BUFFER_BYTES].into_boxed_slice();
    let mut remaining_file = read_limit;
    let mut newline_count = 0_u64;
    let mut last_byte = None;
    let mut ci_matcher = CiMatcher::default();
    while remaining_file != 0 {
        let requested = usize::try_from(remaining_file.min(READ_BUFFER_BYTES as u64))
            .unwrap_or(READ_BUFFER_BYTES);
        let read = file
            .read(&mut buffer[..requested])
            .map_err(|error| scan_io_error(path, &error))?;
        if read == 0 {
            break;
        }
        let chunk = &buffer[..read];
        facts.bytes_read = facts
            .bytes_read
            .saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
        remaining_file = remaining_file.saturating_sub(u64::try_from(read).unwrap_or(u64::MAX));
        if should_count_lines {
            let mut chunk_newlines = 0_u64;
            for byte in chunk {
                chunk_newlines = chunk_newlines.saturating_add(u64::from(*byte == b'\n'));
            }
            newline_count = newline_count.saturating_add(chunk_newlines);
            last_byte = chunk.last().copied();
        }
        if should_match_ci {
            ci_matcher.observe(chunk);
        }
    }
    let source_lines = if should_count_lines && last_byte.is_some() {
        newline_count.saturating_add(u64::from(last_byte != Some(b'\n')))
    } else {
        0
    };
    Ok(ContentFacts {
        source_lines,
        ci_languages: ci_matcher.languages,
    })
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

impl CiMatcher {
    fn observe(&mut self, chunk: &[u8]) {
        const MAX_NEEDLE_BYTES: usize = 6;

        let mut searchable = Vec::with_capacity(self.tail.len().saturating_add(chunk.len()));
        searchable.extend_from_slice(&self.tail);
        searchable.extend(chunk.iter().map(u8::to_ascii_lowercase));
        for language in [
            Language::Rust,
            Language::TypeScriptJavaScript,
            Language::Python,
        ] {
            if ci_needles(language).iter().any(|needle| {
                searchable
                    .windows(needle.len())
                    .any(|window| window == *needle)
            }) {
                self.languages.insert(language);
            }
        }
        let tail_start = searchable.len().saturating_sub(MAX_NEEDLE_BYTES - 1);
        self.tail.clear();
        self.tail.extend_from_slice(&searchable[tail_start..]);
    }
}

const fn ci_needles(language: Language) -> &'static [&'static [u8]] {
    match language {
        Language::Rust => &[b"cargo", b"rustup"],
        Language::TypeScriptJavaScript => &[b"node", b"npm", b"pnpm", b"yarn", b"bun"],
        Language::Python => &[b"python", b"pip", b"pytest", b"ruff", b"uv"],
    }
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
            total_source_lines,
        ),
        score_language(
            Language::TypeScriptJavaScript,
            files,
            scope_root,
            ci_files,
            total_source_lines,
        ),
        score_language(
            Language::Python,
            files,
            scope_root,
            ci_files,
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
    if let Some(path) = ci_evidence(language, ci_files) {
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

fn ci_evidence(language: Language, ci_files: &[&FileFact]) -> Option<PathBuf> {
    ci_files
        .iter()
        .find(|fact| fact.ci_languages.contains(&language))
        .map(|fact| fact.relative_path.clone())
}

fn scan_io_error(path: &Path, error: &std::io::Error) -> MinoError {
    MinoError::new(
        ErrorCategory::EnvironmentUnavailable,
        format!("Failed to scan {}: {error}", path.display()),
    )
}

fn scan_walk_error(path: &Path, error: &ignore::Error) -> MinoError {
    MinoError::new(
        ErrorCategory::EnvironmentUnavailable,
        format!("Failed to scan {}: {error}", path.display()),
    )
}
