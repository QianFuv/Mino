//! Embedded inert standards catalog, detection, recommendation, and application.

mod apply;
mod catalog;
mod conflict;
mod detect;
mod recommend;
mod sync;

pub use apply::{
    CommandSource, ResolvedCheck, ResolvedCheckStatus, StandardsApplication, SystemToolProbe,
    ToolProbe, ToolProbeOutcome, apply_recommendation,
};
pub use catalog::{
    CheckTemplate, EmbeddedCatalog, RuleLevel, StandardRule, StandardsPackage,
    TEAM_CATALOG_MANIFEST_KIND, TEAM_CATALOG_SOURCE_VERSION, TeamCatalogBuildReport,
    TeamCatalogFileReport, TeamCatalogInitReport, TeamCatalogPackageReport,
    TeamCatalogValidationReport, build_team_catalog, build_team_catalog_with_policy,
    initialize_team_catalog, validate_team_catalog, validate_team_catalog_with_policy,
};
pub use conflict::{
    AssessedStandardConflict, DetectedStandardsConflicts, LocalStandardsSource,
    StandardConflictStatus, StandardsConflictAssessment, assess_standard_conflicts,
    detect_standard_conflicts,
};
pub use detect::{detected_languages, language_for_path, languages_for_paths};
pub use recommend::{
    RecommendationStage, RecommendedPackage, StandardsRecommendation, recommend_for_paths,
    recommend_initial,
};
pub use sync::{
    SourcePolicy, StandardsSyncReport, SyncLimits, SyncOptions, SyncedPackage, synchronize_all,
    synchronize_all_with_options,
};
