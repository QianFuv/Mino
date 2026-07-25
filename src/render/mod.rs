//! Deterministic managed Markdown rendering and drift-safe publication.

mod error;
mod markdown;
mod projection;

pub use error::{RenderError, RenderErrorKind};
pub use markdown::{RENDERER_VERSION, RenderedPlan, render_plan};
pub use projection::{
    ProjectionCheck, ProjectionStatus, ProjectionWriteOutcome, check_projection, write_projection,
};
