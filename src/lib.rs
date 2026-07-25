//! Core library for the Mino local plan protocol engine.

mod commands;

/// Command-line parsing, dispatch, and output routing.
pub mod cli;
/// Versioned plan protocol entities and legal state transitions.
pub mod domain;
/// Error categories and process exit-code mappings.
pub mod error;
/// Stable human-readable and machine-readable output contracts.
pub mod output;
/// Project discovery, initialization, inspection, and diagnosis services.
pub mod project;
/// Deterministic managed Markdown projection and drift detection.
pub mod render;
/// Recoverable revisioned storage, locking, hashing, and audit contracts.
pub mod store;

pub use error::{ErrorCategory, MinoError};
pub use output::{EmptyPayload, MinoResult, NextAction, OutputFormat};
