//! Application services coordinating project facts, domain mutations, storage, and projections.

/// Stable machine context, next-action, capability, and active-plan service.
pub mod agent;
/// Typed protected-plan amendment proposal, approval, and application service.
pub mod amendment;
/// Plan finalization, review, show, and explicit approval service.
pub mod approval;
/// Evidence binding, File Map policy, and completion transitions.
pub mod completion;
/// Ordered execution, checkpointing, planned checks, and evidence attachment.
pub mod execution;
/// Git inspection and worktree-aware active-plan binding service.
pub mod git_binding;
/// Approval-gated Git branch proposal, creation, and recovery service.
pub mod git_branch;
/// Recoverable plan-scoped Git task-commit service.
pub mod git_commit;
/// Plan authoring and read-side application service.
pub mod plan;
/// Classified review, rework, resolution, and final acceptance service.
pub mod review;
pub mod standards;
