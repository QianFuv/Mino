//! Git repository facts, worktree bindings, and narrow policy primitives.

mod changes;
mod command;
mod inspect;
mod porcelain;
mod worktree;

pub use changes::{
    ChangedFile, GitChangeError, GitChangeErrorKind, GitChangeSet, inspect_changes,
    matches_file_map_path,
};
pub use command::{GitError, GitErrorKind};
pub use inspect::{GitAdapter, GitFacts, GitHeadState};
pub use porcelain::{GitStatusEntry, GitStatusKind, PorcelainStatus, parse_porcelain_v2};
pub use worktree::{
    ActiveBindingResolution, ActiveBindingStatus, ActiveBindingStore, ActiveBindingWriteReport,
    ActivePlanBinding,
};
