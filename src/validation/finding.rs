//! Stable validation layers, finding records, and report schema.

use serde::Serialize;

use crate::NextAction;
use crate::domain::{Plan, PlanId, PlanStatus};

/// Versioned validation result schema identifier.
pub const VALIDATION_KIND: &str = "mino.validation/v1";

/// Fixed validation layer order.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationLayer {
    /// Typed shape, identifier, placeholder, and path checks.
    Schema,
    /// Required authored meaning and completeness checks.
    Semantic,
    /// Task dependency, ordering, ownership, and boundary checks.
    Graph,
    /// Repository, Git, standards, verification, and approval checks.
    Policy,
}

/// Severity assigned to one validation finding.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationSeverity {
    /// The finding blocks finalization or execution.
    Error,
    /// The finding is informative and does not block.
    Warning,
}

/// One stable, located validation finding.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ValidationFinding {
    /// Stable rule identifier.
    pub id: String,
    /// Validation layer that emitted the finding.
    pub layer: ValidationLayer,
    /// Severity and blocking behavior.
    pub severity: ValidationSeverity,
    /// Stable authored-field, task, or policy location.
    pub location: String,
    /// Concise actionable explanation.
    pub message: String,
    /// Whether finalization is blocked.
    pub blocking: bool,
}

impl ValidationFinding {
    pub(crate) fn error(
        id: impl Into<String>,
        layer: ValidationLayer,
        location: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            layer,
            severity: ValidationSeverity::Error,
            location: location.into(),
            message: message.into(),
            blocking: true,
        }
    }
}

/// Complete fixed-order validation result for one plan revision.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ValidationReport {
    /// Versioned validation schema identifier.
    pub validation_kind: &'static str,
    /// Validated plan identifier.
    pub plan_id: PlanId,
    /// Validated plan status.
    pub status: PlanStatus,
    /// Validated optimistic-concurrency revision.
    pub revision: u64,
    /// Whether no blocking finding exists.
    pub valid: bool,
    /// Findings in fixed layer and deterministic within-layer order.
    pub findings: Vec<ValidationFinding>,
    /// Canonical remediation or finalize actions.
    #[serde(skip)]
    pub next_actions: Vec<NextAction>,
}

impl ValidationReport {
    pub(crate) fn new(
        plan: &Plan,
        findings: Vec<ValidationFinding>,
        next_actions: Vec<NextAction>,
    ) -> Self {
        let valid = findings.iter().all(|finding| !finding.blocking);
        Self {
            validation_kind: VALIDATION_KIND,
            plan_id: plan.id().clone(),
            status: plan.status(),
            revision: plan.revision(),
            valid,
            findings,
            next_actions,
        }
    }

    /// Returns stable unique locations for all blocking findings.
    #[must_use]
    pub fn missing_locations(&self) -> Vec<String> {
        let mut locations = Vec::new();
        for finding in &self.findings {
            if finding.blocking && !locations.contains(&finding.location) {
                locations.push(finding.location.clone());
            }
        }
        locations
    }
}
