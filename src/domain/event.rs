//! Append-only audit event entity definitions.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{RequestId, Timestamp};

/// Outcome recorded for an attempted semantic command.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub enum EventResult {
    /// The command produced and committed a new revision.
    Succeeded,
    /// The command was valid to attempt but failed during execution.
    Failed,
    /// Domain or policy validation rejected the command.
    Rejected,
}

/// A single append-only semantic mutation event.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Event {
    pub(crate) sequence: u64,
    pub(crate) timestamp: Timestamp,
    pub(crate) actor: String,
    pub(crate) command: Vec<String>,
    pub(crate) request_id: RequestId,
    pub(crate) revision_before: u64,
    pub(crate) revision_after: u64,
    pub(crate) changed_fields: Vec<String>,
    pub(crate) result: EventResult,
    pub(crate) state_hash: String,
    pub(crate) snapshot_digest: String,
}
