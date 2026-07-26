//! Core library for the Mino local plan protocol engine.

mod commands;

/// Application services that coordinate domain state, storage, and projections.
pub mod application;
/// Command-line parsing, dispatch, and output routing.
pub mod cli;
/// Deterministic semantic comparison for authored plan alternatives.
pub mod diff;
/// Canonical plugin source and native distribution contracts.
pub mod distribution;
/// Versioned plan protocol entities and legal state transitions.
pub mod domain;
/// Error categories and process exit-code mappings.
pub mod error;
/// Immutable evidence capture, content-addressed blobs, and audit queries.
pub mod evidence;
/// Read-only Git observations and File Map matching.
pub mod git;
/// Bounded strict and guided authoring input adapters.
pub mod input;
/// Repository Skill installation and owned integration block management.
pub mod integration;
/// Stable human-readable and machine-readable output contracts.
pub mod output;
/// Project discovery, initialization, inspection, and diagnosis services.
pub mod project;
/// Embedded planning protocol resources and explicit migration registry.
pub mod protocol;
/// Deterministic managed Markdown projection and drift detection.
pub mod render;
/// Bounded no-shell process execution and recoverable run journals.
pub mod runner;
/// Scheduler-neutral, side-effect-free task handoff specifications.
pub mod schedule;
/// Embedded inert standards catalog, recommendations, and check resolution.
pub mod standards;
/// Recoverable revisioned storage, locking, hashing, and audit contracts.
pub mod store;
/// Fixed-order plan validation and structured remediation contracts.
pub mod validation;

pub use error::{ErrorCategory, MinoError};
pub use output::{EmptyPayload, MinoResult, NextAction, OutputFormat};
