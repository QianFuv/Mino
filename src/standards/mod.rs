//! Embedded inert standards catalog, detection, recommendation, and application.

mod apply;
mod catalog;
mod detect;
mod recommend;
mod sync;

pub use apply::{
    CommandSource, ResolvedCheck, ResolvedCheckStatus, StandardsApplication, SystemToolProbe,
    ToolProbe, apply_recommendation,
};
pub use catalog::{CheckTemplate, EmbeddedCatalog, RuleLevel, StandardRule, StandardsPackage};
pub use detect::{detected_languages, language_for_path, languages_for_paths};
pub use recommend::{
    RecommendationStage, RecommendedPackage, StandardsRecommendation, recommend_for_paths,
    recommend_initial,
};
pub use sync::{
    SourcePolicy, StandardsSyncReport, SyncLimits, SyncOptions, SyncedPackage, synchronize_all,
    synchronize_all_with_options,
};
