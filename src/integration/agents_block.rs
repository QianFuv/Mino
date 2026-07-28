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

const PLANNING_SUPERSESSION_BODY: &str = "For durable plans, Mino supersedes the legacy template and execution workflow.\nThe remaining repository, coding, MCP, and Git rules continue to apply.";

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

pub(super) fn is_workflow_active(text: &str) -> bool {
    let mut active_fence: Option<(u8, usize)> = None;
    let mut start_lines = Vec::new();
    let mut end_lines = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        let fence = fence_marker(trimmed);
        let was_fenced = active_fence.is_some();
        if let Some((marker, count)) = fence {
            match active_fence {
                None => active_fence = Some((marker, count)),
                Some((active_marker, active_count))
                    if marker == active_marker
                        && count >= active_count
                        && trimmed[count..].trim().is_empty() =>
                {
                    active_fence = None;
                }
                Some(_) => {}
            }
        }
        if was_fenced || fence.is_some() {
            continue;
        }
        let marker = trimmed.trim_end();
        if marker == SPEC.start_marker {
            start_lines.push(index);
        } else if marker == SPEC.end_marker {
            end_lines.push(index);
        }
    }
    start_lines.len() == 1 && end_lines.len() == 1 && start_lines[0] < end_lines[0]
}

fn fence_marker(value: &str) -> Option<(u8, usize)> {
    let marker = *value.as_bytes().first()?;
    if !matches!(marker, b'`' | b'~') {
        return None;
    }
    let count = value.bytes().take_while(|byte| *byte == marker).count();
    (count >= 3).then_some((marker, count))
}

pub(super) fn planning_supersession(line_ending: &str) -> String {
    format!(
        "## Planning Documents{line_ending}{line_ending}{}{line_ending}{line_ending}",
        PLANNING_SUPERSESSION_BODY.replace('\n', line_ending)
    )
}
