//! Initial and file-map-complete standards recommendations.

use std::collections::BTreeSet;
use std::path::PathBuf;

use serde::Serialize;

use crate::project::{Language, ProjectScan};
use crate::{ErrorCategory, MinoError};

use super::catalog::EmbeddedCatalog;
use super::detect::languages_for_paths;

/// Stage that produced a standards recommendation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecommendationStage {
    /// Initial project recommendation limited to two languages.
    Initial,
    /// File-map validation including every touched language.
    FileMap,
}

/// One exact recommended package with its explanation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RecommendedPackage {
    /// Stable package identifier.
    pub package_id: String,
    /// Exact embedded package version.
    pub version: String,
    /// Exact package digest.
    pub digest: String,
    /// Concise recommendation reason.
    pub reason: String,
    /// Scanner confidence when the package came from detection.
    pub score_basis_points: Option<u16>,
}

/// Deterministic Common-plus-language package recommendation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct StandardsRecommendation {
    /// Recommendation stage.
    pub stage: RecommendationStage,
    /// Exact packages in stable package-ID order.
    pub packages: Vec<RecommendedPackage>,
}

/// Recommends Common plus at most the two highest-ranked project languages.
///
/// # Errors
///
/// Returns a validation error when the embedded catalog is incomplete.
pub fn recommend_initial(
    catalog: &EmbeddedCatalog,
    scan: &ProjectScan,
) -> Result<StandardsRecommendation, MinoError> {
    let ranked = scan
        .languages
        .iter()
        .take(2)
        .map(|score| (score.language, Some(score.score_basis_points)))
        .collect::<Vec<_>>();
    build_recommendation(catalog, RecommendationStage::Initial, ranked)
}

/// Recommends Common plus every language actually touched by a file map.
///
/// # Errors
///
/// Returns a validation error when the embedded catalog is incomplete.
pub fn recommend_for_paths(
    catalog: &EmbeddedCatalog,
    scan: &ProjectScan,
    paths: &[PathBuf],
) -> Result<StandardsRecommendation, MinoError> {
    let scores = scan
        .languages
        .iter()
        .map(|score| (score.language, score.score_basis_points))
        .collect::<std::collections::BTreeMap<_, _>>();
    let languages = languages_for_paths(paths.iter().map(PathBuf::as_path))
        .into_iter()
        .map(|language| (language, scores.get(&language).copied()))
        .collect::<Vec<_>>();
    build_recommendation(catalog, RecommendationStage::FileMap, languages)
}

fn build_recommendation(
    catalog: &EmbeddedCatalog,
    stage: RecommendationStage,
    languages: Vec<(Language, Option<u16>)>,
) -> Result<StandardsRecommendation, MinoError> {
    let mut selected = BTreeSet::from(["common".to_owned()]);
    let mut scores = std::collections::BTreeMap::new();
    for (language, score) in languages {
        let package = catalog.package_for_language(language).ok_or_else(|| {
            MinoError::new(
                ErrorCategory::IncompleteOrValidation,
                format!("Embedded catalog has no package for {}", language.name()),
            )
        })?;
        selected.insert(package.package_id().to_owned());
        scores.insert(package.package_id().to_owned(), score);
    }
    let packages = selected
        .into_iter()
        .map(|package_id| {
            let package = catalog.package(&package_id).ok_or_else(|| {
                MinoError::new(
                    ErrorCategory::IncompleteOrValidation,
                    format!("Embedded catalog has no package {package_id}"),
                )
            })?;
            let score = scores.get(&package_id).copied().flatten();
            Ok(RecommendedPackage {
                package_id: package_id.clone(),
                version: package.version().to_owned(),
                digest: package.digest().to_owned(),
                reason: if package_id == "common" {
                    "Common rules apply to every Mino plan".to_owned()
                } else if let Some(score) = score {
                    format!("Scanner confidence is {score} basis points")
                } else {
                    "The completed file map touches this language".to_owned()
                },
                score_basis_points: score,
            })
        })
        .collect::<Result<Vec<_>, MinoError>>()?;
    Ok(StandardsRecommendation { stage, packages })
}
