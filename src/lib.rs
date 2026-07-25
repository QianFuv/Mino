//! Core library for the Mino local plan protocol engine.

/// Versioned plan protocol entities and legal state transitions.
pub mod domain;
/// Error categories and process exit-code mappings.
pub mod error;
/// Stable human-readable and machine-readable output contracts.
pub mod output;
/// Deterministic managed Markdown projection and drift detection.
pub mod render;
/// Recoverable revisioned storage, locking, hashing, and audit contracts.
pub mod store;

pub use error::{ErrorCategory, MinoError};
pub use output::{EmptyPayload, MinoResult, NextAction, OutputFormat};
