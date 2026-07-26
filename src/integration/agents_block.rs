//! Stable Mino workflow block for repository `AGENTS.md` files.

use std::path::Path;

use crate::MinoError;

use super::{
    IntegrationArtifactKind, IntegrationReport, IntegrationWriter, ManagedBlockSpec,
    reconcile_block,
};

const AGENTS_BLOCK: &str = r"<!-- mino:workflow:start -->
## Mino Workflow

- Invoke `$mino` for an explicitly requested formal plan or durable planning.
- Never edit `.mino/**` or Mino-managed `docs/plan/*.md` directly.
- Run `mino agent context --format json --no-input` before creating, resuming,
  executing, amending, or reviewing a Mino plan.
- Record plan state, checkpoints, verification evidence, and review feedback
  through Mino, and follow only canonical `next_actions`.
- Stop when Mino reports that explicit user approval is required.
- Treat plan commit gates as scope constraints; all Git mutation still requires
  authorization under repository policy.
<!-- mino:workflow:end -->";

const SPEC: ManagedBlockSpec = ManagedBlockSpec {
    kind: IntegrationArtifactKind::AgentsBlock,
    relative_path: "AGENTS.md",
    start_marker: "<!-- mino:workflow:start -->",
    end_marker: "<!-- mino:workflow:end -->",
    block: AGENTS_BLOCK,
    missing_code: "agents_block_missing",
    drift_code: "agents_block_drift",
    malformed_code: "agents_block_malformed",
    missing_message: "AGENTS.md lacks the stable Mino workflow block",
    drift_message: "The owned Mino workflow block differs from the supported value",
    malformed_message: "AGENTS.md contains malformed or duplicate Mino workflow markers and was preserved",
};

pub(super) fn reconcile(
    root: &Path,
    should_apply: bool,
    writer: Option<&IntegrationWriter>,
    report: &mut IntegrationReport,
) -> Result<(), MinoError> {
    reconcile_block(root, &SPEC, should_apply, writer, report)
}
