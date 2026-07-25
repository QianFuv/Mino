//! Supplemental evidence request types and compatibility validation.

use std::path::PathBuf;

use crate::domain::{
    CriterionId, EvidenceId, EvidenceType, Plan, PlanId, RequestId, TaskId, Timestamp,
};

use super::{EvidenceError, EvidenceErrorKind};

/// Immutable request identity and optimistic-concurrency facts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EvidenceRequestContext {
    plan_id: PlanId,
    expected_revision: u64,
    request_id: RequestId,
    actor: String,
    command: Vec<String>,
    captured_at: Timestamp,
}

impl EvidenceRequestContext {
    /// Creates context for one idempotent evidence mutation.
    ///
    /// # Errors
    ///
    /// Returns an invalid-request error for revision zero or an empty actor.
    pub fn new(
        plan_id: PlanId,
        expected_revision: u64,
        request_id: RequestId,
        actor: impl Into<String>,
        command: Vec<String>,
        captured_at: Timestamp,
    ) -> Result<Self, EvidenceError> {
        let context = Self {
            plan_id,
            expected_revision,
            request_id,
            actor: actor.into(),
            command,
            captured_at,
        };
        if context.expected_revision == 0
            || context.actor.trim().is_empty()
            || context.command.is_empty()
            || context.command.iter().any(|part| part.trim().is_empty())
        {
            return Err(invalid(
                "Evidence context requires a positive revision, actor, and canonical command",
            ));
        }
        Ok(context)
    }

    pub(crate) const fn plan_id(&self) -> &PlanId {
        &self.plan_id
    }

    pub(crate) const fn expected_revision(&self) -> u64 {
        self.expected_revision
    }

    pub(crate) const fn request_id(&self) -> &RequestId {
        &self.request_id
    }

    pub(crate) fn actor(&self) -> &str {
        &self.actor
    }

    pub(crate) fn command(&self) -> &[String] {
        &self.command
    }

    pub(crate) const fn captured_at(&self) -> &Timestamp {
        &self.captured_at
    }
}

/// Source supplied for one supplemental evidence record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EvidenceSource {
    /// A project-relative file copied into content-addressed storage.
    Artifact(PathBuf),
    /// An immutable URL, commit, or approval reference.
    Reference(String),
    /// A human observation represented entirely by its description.
    Observation,
}

/// Validated request to add one supplemental evidence record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AddEvidenceRequest {
    context: EvidenceRequestContext,
    kind: EvidenceType,
    source: EvidenceSource,
    description: String,
    task_id: Option<TaskId>,
    criterion_id: Option<CriterionId>,
    supersedes: Option<EvidenceId>,
}

impl AddEvidenceRequest {
    /// Creates an unbound supplemental evidence request.
    ///
    /// # Errors
    ///
    /// Returns an invalid-request error for command evidence, an empty
    /// description, or an incompatible kind/source pair.
    pub fn new(
        context: EvidenceRequestContext,
        kind: EvidenceType,
        source: EvidenceSource,
        description: impl Into<String>,
    ) -> Result<Self, EvidenceError> {
        let request = Self {
            context,
            kind,
            source,
            description: description.into(),
            task_id: None,
            criterion_id: None,
            supersedes: None,
        };
        request.validate_shape()?;
        Ok(request)
    }

    /// Binds the evidence to one task.
    #[must_use]
    pub fn with_task(mut self, task_id: TaskId) -> Self {
        self.task_id = Some(task_id);
        self
    }

    /// Binds the evidence to one acceptance criterion and its task.
    #[must_use]
    pub fn with_criterion(mut self, task_id: TaskId, criterion_id: CriterionId) -> Self {
        self.task_id = Some(task_id);
        self.criterion_id = Some(criterion_id);
        self
    }

    /// Marks this record as the immutable correction of prior evidence.
    #[must_use]
    pub fn superseding(mut self, evidence_id: EvidenceId) -> Self {
        self.supersedes = Some(evidence_id);
        self
    }

    pub(crate) const fn context(&self) -> &EvidenceRequestContext {
        &self.context
    }

    pub(crate) const fn kind(&self) -> EvidenceType {
        self.kind
    }

    pub(crate) const fn source(&self) -> &EvidenceSource {
        &self.source
    }

    pub(crate) fn description(&self) -> &str {
        &self.description
    }

    pub(crate) const fn task_id(&self) -> Option<&TaskId> {
        self.task_id.as_ref()
    }

    pub(crate) const fn criterion_id(&self) -> Option<&CriterionId> {
        self.criterion_id.as_ref()
    }

    pub(crate) const fn supersedes(&self) -> Option<&EvidenceId> {
        self.supersedes.as_ref()
    }

    pub(crate) fn validate_against(
        &self,
        plan: &Plan,
        existing: &[crate::domain::Evidence],
    ) -> Result<(), EvidenceError> {
        self.validate_shape()?;
        if plan.id() != self.context.plan_id() {
            return Err(invalid("Evidence plan does not match the loaded plan"));
        }
        if plan.revision() != self.context.expected_revision() {
            return Err(EvidenceError::new(
                EvidenceErrorKind::RevisionConflict,
                format!(
                    "Expected plan revision {}, found {}",
                    self.context.expected_revision(),
                    plan.revision()
                ),
            ));
        }
        let task = match self.task_id() {
            Some(task_id) => Some(plan.task(task_id).ok_or_else(|| {
                invalid(format!(
                    "Task {task_id} does not exist in plan {}",
                    plan.id()
                ))
            })?),
            None => None,
        };
        if let Some(criterion_id) = self.criterion_id() {
            let task = task.ok_or_else(|| {
                invalid("Criterion-bound evidence requires an existing task binding")
            })?;
            if !task
                .acceptance_criteria()
                .iter()
                .any(|criterion| criterion.id() == criterion_id)
            {
                return Err(invalid(format!(
                    "Criterion {criterion_id} does not exist on task {}",
                    task.id()
                )));
            }
        }
        if let Some(supersedes) = self.supersedes() {
            let prior = existing
                .iter()
                .find(|evidence| evidence.id() == supersedes)
                .ok_or_else(|| {
                    EvidenceError::new(
                        EvidenceErrorKind::EvidenceNotFound,
                        format!("Superseded evidence {supersedes} does not exist"),
                    )
                })?;
            if existing
                .iter()
                .any(|evidence| evidence.supersedes() == Some(supersedes))
            {
                return Err(invalid(format!(
                    "Evidence {supersedes} already has a superseding correction"
                )));
            }
            if prior.kind() != self.kind
                || prior.task_id() != self.task_id()
                || prior.criterion_id() != self.criterion_id()
            {
                return Err(invalid(
                    "A correction must retain the prior kind, task, and criterion bindings",
                ));
            }
        }
        Ok(())
    }

    fn validate_shape(&self) -> Result<(), EvidenceError> {
        if self.description.trim().is_empty() || self.description.len() > 16 * 1_024 {
            return Err(invalid("Evidence description cannot be empty"));
        }
        match (self.kind, &self.source) {
            (EvidenceType::Command, _) => Err(invalid(
                "Command evidence is created only by the planned check runner",
            )),
            (
                EvidenceType::File
                | EvidenceType::GitDiff
                | EvidenceType::Log
                | EvidenceType::Screenshot,
                EvidenceSource::Artifact(_),
            )
            | (
                EvidenceType::Commit | EvidenceType::Url | EvidenceType::AcceptedException,
                EvidenceSource::Reference(_),
            )
            | (EvidenceType::ManualObservation, EvidenceSource::Observation) => {
                self.validate_reference_value()
            }
            _ => Err(invalid("Evidence type and source are incompatible")),
        }
    }

    fn validate_reference_value(&self) -> Result<(), EvidenceError> {
        let EvidenceSource::Reference(reference) = &self.source else {
            return Ok(());
        };
        if reference.trim().is_empty() || reference.len() > 4_096 {
            return Err(invalid("Evidence reference must be non-empty and bounded"));
        }
        match self.kind {
            EvidenceType::Url
                if !(reference.starts_with("https://") || reference.starts_with("http://")) =>
            {
                Err(invalid("URL evidence requires an HTTP or HTTPS reference"))
            }
            EvidenceType::Commit
                if reference.len() < 7
                    || reference.len() > 64
                    || !reference.bytes().all(|byte| byte.is_ascii_hexdigit()) =>
            {
                Err(invalid(
                    "Commit evidence requires a 7-to-64 character hexadecimal reference",
                ))
            }
            _ => Ok(()),
        }
    }
}

fn invalid(message: impl Into<String>) -> EvidenceError {
    EvidenceError::new(EvidenceErrorKind::InvalidRequest, message)
}
