//! Stable ignore rules for local Mino runtime and generated plan state.

use std::path::Path;

use crate::MinoError;

use super::{IntegrationArtifactKind, IntegrationReport, ManagedBlockSpec, reconcile_block};

const GITIGNORE_BLOCK: &str = r"# mino:runtime:start
/.mino/
/docs/plan/
# mino:runtime:end";

const SPEC: ManagedBlockSpec = ManagedBlockSpec {
    kind: IntegrationArtifactKind::GitignoreBlock,
    relative_path: ".gitignore",
    start_marker: "# mino:runtime:start",
    end_marker: "# mino:runtime:end",
    block: GITIGNORE_BLOCK,
    missing_code: "gitignore_block_missing",
    drift_code: "gitignore_block_drift",
    malformed_code: "gitignore_block_malformed",
    missing_message: ".gitignore lacks the owned Mino runtime block",
    drift_message: "The owned Mino runtime ignore block differs from the supported value",
    malformed_message: ".gitignore contains malformed or duplicate Mino runtime markers and was preserved",
};

pub(super) fn reconcile(
    root: &Path,
    should_apply: bool,
    report: &mut IntegrationReport,
) -> Result<(), MinoError> {
    reconcile_block(root, &SPEC, should_apply, report)
}
