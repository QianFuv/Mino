//! Core library for the Mino local plan protocol engine.

mod commands;

/// Application services that coordinate domain state, storage, and projections.
pub mod application;
/// Command-line parsing, dispatch, and output routing.
pub mod cli;
/// Versioned plan protocol entities and legal state transitions.
pub mod domain;
/// Error categories and process exit-code mappings.
pub mod error;
/// Bounded strict and guided authoring input adapters.
pub mod input;
/// Stable human-readable and machine-readable output contracts.
pub mod output;
/// Project discovery, initialization, inspection, and diagnosis services.
pub mod project;
/// Deterministic managed Markdown projection and drift detection.
pub mod render;
/// Embedded inert standards catalog, recommendations, and check resolution.
pub mod standards;
/// Recoverable revisioned storage, locking, hashing, and audit contracts.
pub mod store;
/// Fixed-order plan validation and structured remediation contracts.
pub mod validation;

pub use error::{ErrorCategory, MinoError};
pub use output::{EmptyPayload, MinoResult, NextAction, OutputFormat};
