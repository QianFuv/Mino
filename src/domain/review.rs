//! Classified review records and their constrained resolution lifecycle.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{DomainError, DomainErrorKind, ReviewClassification, ReviewStatus, TaskId, Timestamp};

/// One immutable review request plus its constrained processing status.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReviewItem {
    id: String,
    reviewer: String,
    feedback: String,
    classification: ReviewClassification,
    action: String,
    linked_task: Option<TaskId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    origin_task: Option<TaskId>,
    status: ReviewStatus,
    recorded_at: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    approval_reference: Option<String>,
}

impl ReviewItem {
    pub(crate) fn acceptance_defect(
        id: String,
        reviewer: String,
        feedback: String,
        task_id: TaskId,
        recorded_at: Timestamp,
    ) -> Result<Self, DomainError> {
        Self::new(
            id,
            reviewer,
            feedback,
            ReviewClassification::AcceptanceDefect,
            format!("Re-run acceptance and verification for {task_id}"),
            Some(task_id),
            None,
            ReviewStatus::Open,
            recorded_at,
            None,
        )
    }

    pub(crate) fn in_scope_rework(
        id: String,
        reviewer: String,
        feedback: String,
        origin_task: TaskId,
        reserved_task: TaskId,
        recorded_at: Timestamp,
    ) -> Result<Self, DomainError> {
        Self::new(
            id,
            reviewer,
            feedback,
            ReviewClassification::InScopeRework,
            format!("Implement approved in-scope rework as {reserved_task}"),
            Some(reserved_task),
            Some(origin_task),
            ReviewStatus::Open,
            recorded_at,
            None,
        )
    }

    pub(crate) fn material_change(
        id: String,
        reviewer: String,
        feedback: String,
        recorded_at: Timestamp,
    ) -> Result<Self, DomainError> {
        Self::new(
            id,
            reviewer,
            feedback,
            ReviewClassification::MaterialChange,
            "Pause for a protected material amendment".to_owned(),
            None,
            None,
            ReviewStatus::Blocked,
            recorded_at,
            None,
        )
    }

    pub(crate) fn follow_up(
        id: String,
        reviewer: String,
        feedback: String,
        recorded_at: Timestamp,
    ) -> Result<Self, DomainError> {
        Self::new(
            id,
            reviewer,
            feedback,
            ReviewClassification::FollowUp,
            "Record outside the active implementation order".to_owned(),
            None,
            None,
            ReviewStatus::Deferred,
            recorded_at,
            None,
        )
    }

    pub(crate) fn accepted(
        id: String,
        reviewer: String,
        approval_reference: String,
        recorded_at: Timestamp,
    ) -> Result<Self, DomainError> {
        Self::new(
            id,
            reviewer,
            "Reviewed result accepted".to_owned(),
            ReviewClassification::Accepted,
            "Record final review acceptance".to_owned(),
            None,
            None,
            ReviewStatus::Resolved,
            recorded_at,
            Some(approval_reference),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn new(
        id: String,
        reviewer: String,
        feedback: String,
        classification: ReviewClassification,
        action: String,
        linked_task: Option<TaskId>,
        origin_task: Option<TaskId>,
        status: ReviewStatus,
        recorded_at: Timestamp,
        approval_reference: Option<String>,
    ) -> Result<Self, DomainError> {
        let item = Self {
            id,
            reviewer,
            feedback,
            classification,
            action,
            linked_task,
            origin_task,
            status,
            recorded_at,
            approval_reference,
        };
        item.validate()?;
        Ok(item)
    }

    /// Returns the monotonic review identifier.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns the actor who supplied the review feedback.
    #[must_use]
    pub fn reviewer(&self) -> &str {
        &self.reviewer
    }

    /// Returns the exact review feedback.
    #[must_use]
    pub fn feedback(&self) -> &str {
        &self.feedback
    }

    /// Returns the minimum review classification.
    #[must_use]
    pub const fn classification(&self) -> ReviewClassification {
        self.classification
    }

    /// Returns the protocol-selected action for the feedback.
    #[must_use]
    pub fn action(&self) -> &str {
        &self.action
    }

    /// Returns the existing or reserved task associated with the feedback.
    #[must_use]
    pub const fn linked_task(&self) -> Option<&TaskId> {
        self.linked_task.as_ref()
    }

    /// Returns the completed task that originated an in-scope rework task.
    #[must_use]
    pub const fn origin_task(&self) -> Option<&TaskId> {
        self.origin_task.as_ref()
    }

    /// Returns the current review-item status.
    #[must_use]
    pub const fn status(&self) -> ReviewStatus {
        self.status
    }

    /// Returns when the feedback was first recorded.
    #[must_use]
    pub const fn recorded_at(&self) -> &Timestamp {
        &self.recorded_at
    }

    /// Returns the explicit final-acceptance reference when present.
    #[must_use]
    pub fn approval_reference(&self) -> Option<&str> {
        self.approval_reference.as_deref()
    }

    pub(crate) fn begin_rework(&mut self) -> Result<(), DomainError> {
        if self.status != ReviewStatus::Open
            || !matches!(
                self.classification,
                ReviewClassification::AcceptanceDefect | ReviewClassification::InScopeRework
            )
        {
            return Err(DomainError::new(
                DomainErrorKind::InvalidTransition,
                format!("Review item {} cannot begin rework", self.id),
            ));
        }
        self.status = ReviewStatus::InProgress;
        Ok(())
    }

    pub(crate) fn resolve(&mut self) -> Result<(), DomainError> {
        if self.status != ReviewStatus::InProgress
            || !matches!(
                self.classification,
                ReviewClassification::AcceptanceDefect | ReviewClassification::InScopeRework
            )
        {
            return Err(DomainError::new(
                DomainErrorKind::InvalidTransition,
                format!("Review item {} cannot be resolved", self.id),
            ));
        }
        self.status = ReviewStatus::Resolved;
        Ok(())
    }

    pub(crate) fn validate(&self) -> Result<(), DomainError> {
        if review_number(&self.id).is_none()
            || self.reviewer.trim().is_empty()
            || self.feedback.trim().is_empty()
            || self.action.trim().is_empty()
            || self
                .approval_reference
                .as_deref()
                .is_some_and(|reference| reference.trim().is_empty())
        {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                "Review item fields are malformed",
            ));
        }
        let is_valid = match self.classification {
            ReviewClassification::AcceptanceDefect => {
                self.linked_task.is_some()
                    && self.origin_task.is_none()
                    && self.approval_reference.is_none()
                    && self.action
                        == format!(
                            "Re-run acceptance and verification for {}",
                            self.linked_task
                                .as_ref()
                                .expect("validated linked task exists")
                        )
                    && matches!(
                        self.status,
                        ReviewStatus::Open | ReviewStatus::InProgress | ReviewStatus::Resolved
                    )
            }
            ReviewClassification::InScopeRework => {
                self.linked_task
                    .as_ref()
                    .is_some_and(|task| task.as_str().starts_with('R'))
                    && self.origin_task.is_some()
                    && self.linked_task != self.origin_task
                    && self.approval_reference.is_none()
                    && self.action
                        == format!(
                            "Implement approved in-scope rework as {}",
                            self.linked_task
                                .as_ref()
                                .expect("validated linked task exists")
                        )
                    && matches!(
                        self.status,
                        ReviewStatus::Open | ReviewStatus::InProgress | ReviewStatus::Resolved
                    )
            }
            ReviewClassification::MaterialChange => {
                self.linked_task.is_none()
                    && self.origin_task.is_none()
                    && self.approval_reference.is_none()
                    && self.action == "Pause for a protected material amendment"
                    && self.status == ReviewStatus::Blocked
            }
            ReviewClassification::FollowUp => {
                self.linked_task.is_none()
                    && self.origin_task.is_none()
                    && self.approval_reference.is_none()
                    && self.action == "Record outside the active implementation order"
                    && self.status == ReviewStatus::Deferred
            }
            ReviewClassification::Accepted => {
                self.origin_task.is_none()
                    && matches!(
                        self.action.as_str(),
                        "Record final review acceptance" | "Record acceptance"
                    )
                    && self.status == ReviewStatus::Resolved
            }
        };
        if is_valid {
            Ok(())
        } else {
            Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                format!(
                    "Review item {} has inconsistent classification state",
                    self.id
                ),
            ))
        }
    }
}

pub(crate) fn review_number(id: &str) -> Option<u64> {
    id.strip_prefix("REV-")
        .filter(|number| !number.starts_with('0'))
        .and_then(|number| number.parse().ok())
        .filter(|number| *number > 0)
}
