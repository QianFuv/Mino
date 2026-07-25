//! Internal CLI command adapters over public application services.

pub(crate) mod project;
pub(crate) mod standards;

use serde_json::Value;

use crate::{MinoResult, NextAction};

pub(crate) struct CommandResponse {
    pub(crate) message: String,
    pub(crate) complete: bool,
    pub(crate) payload: Value,
    pub(crate) missing: Vec<String>,
    pub(crate) next_actions: Vec<NextAction>,
}

impl CommandResponse {
    pub(crate) fn into_result(self) -> MinoResult<Value> {
        MinoResult::success(self.message, self.complete, self.payload)
            .with_missing(self.missing)
            .with_next_actions(self.next_actions)
    }
}
