//! Application services coordinating project facts, domain mutations, storage, and projections.

/// Stable machine context, next-action, capability, and active-plan service.
pub mod agent;
/// Plan finalization, review, show, and explicit approval service.
pub mod approval;
/// Evidence binding, File Map policy, and completion transitions.
pub mod completion;
/// Ordered execution, checkpointing, planned checks, and evidence attachment.
pub mod execution;
/// Plan authoring and read-side application service.
pub mod plan;
