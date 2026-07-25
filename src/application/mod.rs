//! Application services coordinating project facts, domain mutations, storage, and projections.

/// Stable machine context, next-action, capability, and active-plan service.
pub mod agent;
/// Plan finalization, review, show, and explicit approval service.
pub mod approval;
/// Plan authoring and read-side application service.
pub mod plan;
