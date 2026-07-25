//! Typed execution checkpoints stored in the plan extension namespace.

use std::collections::BTreeSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{DomainError, DomainErrorKind, TaskId, Timestamp};

pub(crate) const EXECUTION_EXTENSION_KEY: &str = "execution";

/// Stable human-meaningful checkpoint classifications.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum CheckpointKind {
    /// Repository and caller inspection completed.
    Inspection,
    /// An implementation approach was selected.
    Approach,
    /// A declared implementation milestone completed.
    Implementation,
    /// Verification reached a meaningful milestone.
    Verification,
    /// A blocking condition was observed.
    Blocker,
    /// Execution departed from the approved plan.
    Deviation,
}

/// One immutable execution milestone attached to a task.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Checkpoint {
    sequence: u64,
    task_id: TaskId,
    kind: CheckpointKind,
    summary: String,
    actor: String,
    recorded_at: Timestamp,
}

impl Checkpoint {
    /// Returns the monotonic checkpoint sequence.
    #[must_use]
    pub const fn sequence(&self) -> u64 {
        self.sequence
    }

    /// Returns the task owning the checkpoint.
    #[must_use]
    pub const fn task_id(&self) -> &TaskId {
        &self.task_id
    }

    /// Returns the fixed checkpoint classification.
    #[must_use]
    pub const fn kind(&self) -> CheckpointKind {
        self.kind
    }

    /// Returns the human-meaningful milestone summary.
    #[must_use]
    pub fn summary(&self) -> &str {
        &self.summary
    }

    /// Returns the actor that recorded the checkpoint.
    #[must_use]
    pub fn actor(&self) -> &str {
        &self.actor
    }

    /// Returns the checkpoint timestamp.
    #[must_use]
    pub const fn recorded_at(&self) -> &Timestamp {
        &self.recorded_at
    }
}

/// Versioned execution state stored under the plan extension key.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExecutionState {
    checkpoints: Vec<Checkpoint>,
}

impl ExecutionState {
    /// Returns checkpoints in monotonic sequence order.
    #[must_use]
    pub fn checkpoints(&self) -> &[Checkpoint] {
        &self.checkpoints
    }

    /// Returns whether no execution-only state has been recorded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.checkpoints.is_empty()
    }

    pub(crate) fn record_checkpoint(
        &mut self,
        task_id: TaskId,
        kind: CheckpointKind,
        summary: String,
        actor: String,
        recorded_at: Timestamp,
    ) -> Result<(), DomainError> {
        if summary.trim().is_empty() || actor.trim().is_empty() {
            return Err(invariant(
                "A checkpoint requires a non-empty summary and actor",
            ));
        }
        let sequence = u64::try_from(self.checkpoints.len())
            .ok()
            .and_then(|count| count.checked_add(1))
            .ok_or_else(|| invariant("Checkpoint sequence overflowed"))?;
        self.checkpoints.push(Checkpoint {
            sequence,
            task_id,
            kind,
            summary,
            actor,
            recorded_at,
        });
        Ok(())
    }

    pub(crate) fn validate(&self, task_ids: &BTreeSet<&TaskId>) -> Result<(), DomainError> {
        for (index, checkpoint) in self.checkpoints.iter().enumerate() {
            let expected = u64::try_from(index)
                .ok()
                .and_then(|value| value.checked_add(1))
                .ok_or_else(|| invariant("Checkpoint sequence overflowed"))?;
            if checkpoint.sequence != expected
                || !task_ids.contains(&checkpoint.task_id)
                || checkpoint.summary.trim().is_empty()
                || checkpoint.actor.trim().is_empty()
            {
                return Err(invariant(
                    "Execution checkpoints must be contiguous, task-bound, and complete",
                ));
            }
        }
        Ok(())
    }
}

fn invariant(message: impl Into<String>) -> DomainError {
    DomainError::new(DomainErrorKind::InvariantViolation, message)
}
