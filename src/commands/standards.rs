//! Standards detection, recommendation, and application CLI adapter.

use std::path::{Path, PathBuf};

use clap::Subcommand;
use serde::Serialize;

use crate::commands::CommandResponse;
use crate::project;
use crate::standards::{
    EmbeddedCatalog, SystemToolProbe, apply_recommendation, detected_languages,
    recommend_for_paths, recommend_initial,
};
use crate::{ErrorCategory, MinoError};

#[derive(Clone, Debug, Subcommand)]
pub(crate) enum StandardsAction {
    /// Detect supported languages from project scanner evidence.
    Detect,
    /// Recommend Common and applicable language packages.
    Recommend {
        /// File-map path used for second-stage complete recommendations.
        #[arg(long = "path")]
        paths: Vec<PathBuf>,
    },
    /// Resolve recommended packages, rules, and verification commands.
    Apply {
        /// Apply the deterministic recommendation rather than manual package IDs.
        #[arg(long)]
        recommended: bool,
        /// Include project-resolved verification checks in the result.
        #[arg(long)]
        seed_verification: bool,
        /// File-map path used for second-stage complete recommendations.
        #[arg(long = "path")]
        paths: Vec<PathBuf>,
    },
    /// Explicitly download and verify every configured catalog package.
    Sync {
        /// Synchronize every package listed by the configured catalog.
        #[arg(long)]
        all: bool,
    },
}

pub(crate) fn execute(start: &Path, action: StandardsAction) -> Result<CommandResponse, MinoError> {
    match action {
        StandardsAction::Detect => {
            let scan = project::scan(start)?;
            response(
                "Standards detection completed.",
                serde_json::json!({ "languages": detected_languages(&scan) }),
            )
        }
        StandardsAction::Recommend { paths } => {
            let catalog = EmbeddedCatalog::load()?;
            let scan = project::scan(start)?;
            let recommendation = if paths.is_empty() {
                recommend_initial(&catalog, &scan)?
            } else {
                recommend_for_paths(&catalog, &scan, &paths)?
            };
            response("Standards recommendation completed.", recommendation)
        }
        StandardsAction::Apply {
            recommended,
            seed_verification,
            paths,
        } => {
            if !recommended || !seed_verification {
                return Err(MinoError::new(
                    ErrorCategory::IncompleteOrValidation,
                    "v0.1 standards apply requires --recommended and --seed-verification",
                ));
            }
            let catalog = EmbeddedCatalog::load()?;
            let scan = project::scan(start)?;
            let recommendation = if paths.is_empty() {
                recommend_initial(&catalog, &scan)?
            } else {
                recommend_for_paths(&catalog, &scan, &paths)?
            };
            let application =
                apply_recommendation(&scan.root, &catalog, &recommendation, &SystemToolProbe)?;
            response("Standards application resolved.", application)
        }
        StandardsAction::Sync { all } => {
            if !all {
                return Err(MinoError::new(
                    ErrorCategory::IncompleteOrValidation,
                    "v0.1 standards sync requires --all",
                ));
            }
            response(
                "Standards catalog synchronized.",
                crate::standards::synchronize_all(start)?,
            )
        }
    }
}

fn response<T: Serialize>(
    message: impl Into<String>,
    payload: T,
) -> Result<CommandResponse, MinoError> {
    let payload = serde_json::to_value(payload).map_err(|error| {
        MinoError::new(
            ErrorCategory::EnvironmentUnavailable,
            format!("Failed to serialize standards result: {error}"),
        )
    })?;
    Ok(CommandResponse {
        message: message.into(),
        complete: true,
        payload,
        missing: Vec::new(),
        next_actions: Vec::new(),
    })
}
