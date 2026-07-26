//! Deterministic semantic comparison for authored plan alternatives.

mod plan;

pub use plan::{DiffCategory, PLAN_DIFF_KIND, PlanChange, PlanDiff, PlanDiffReference, diff_plans};
