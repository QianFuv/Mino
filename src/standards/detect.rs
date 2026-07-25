//! Standards package detection from scanner evidence and file-map paths.

use std::collections::BTreeSet;
use std::path::Path;

use crate::project::{Language, ProjectScan};

/// Returns detected languages in scanner ranking order.
#[must_use]
pub fn detected_languages(scan: &ProjectScan) -> Vec<Language> {
    scan.languages.iter().map(|score| score.language).collect()
}

/// Returns every supported language touched by file-map paths.
#[must_use]
pub fn languages_for_paths<'a>(paths: impl IntoIterator<Item = &'a Path>) -> Vec<Language> {
    let mut languages = BTreeSet::new();
    for path in paths {
        if let Some(language) = language_for_path(path) {
            languages.insert(language);
        }
    }
    languages.into_iter().collect()
}

/// Returns the supported language associated with one source path.
#[must_use]
pub fn language_for_path(path: &Path) -> Option<Language> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    match extension.as_str() {
        "rs" => Some(Language::Rust),
        "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" => Some(Language::TypeScriptJavaScript),
        "py" | "pyi" => Some(Language::Python),
        _ => None,
    }
}
