//! Git repository facts, worktree bindings, and narrow policy primitives.

mod branch;
mod changes;
mod command;
mod commit;
mod inspect;
mod intent;
mod policy;
mod porcelain;
mod worktree;

pub use branch::{
    GitBranchCommandResult, GitBranchCompletion, GitBranchIntent, GitBranchJournal,
    GitBranchJournalLock, GitBranchJournalStore, create_and_switch_branch, local_branch_target,
    proposed_branch_name, validate_branch_name,
};
pub use changes::{
    ChangedFile, GitChangeError, GitChangeErrorKind, GitChangeSet, inspect_changes,
    matches_file_map_path,
};
pub use command::{GitError, GitErrorKind};
pub use commit::{
    GitCommitObject, GitMutationResult, ensure_no_clean_filters, inspect_commit, run_task_commit,
    stage_commit_paths, write_index_tree,
};
pub use inspect::{GitAdapter, GitFacts, GitHeadState};
pub use intent::{
    CommitFileSnapshot, CommitFileSnapshotKind, GitCommitCompletion, GitCommitCompletionInput,
    GitCommitIntent, GitCommitJournal, GitCommitJournalLock, GitCommitJournalStore,
    GitStagedCommit,
};
pub use policy::{
    capture_commit_snapshots, task_commit_entries, validate_commit_snapshot_scope,
    verify_commit_snapshots,
};
pub use porcelain::{GitStatusEntry, GitStatusKind, PorcelainStatus, parse_porcelain_v2};
pub use worktree::{
    ActiveBindingResolution, ActiveBindingStatus, ActiveBindingStore, ActiveBindingWriteReport,
    ActivePlanBinding,
};
