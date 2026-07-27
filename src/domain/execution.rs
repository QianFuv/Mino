//! Typed execution checkpoints stored in the plan extension namespace.

use std::collections::BTreeSet;
use std::path::{Component, Path};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{DomainError, DomainErrorKind, EvidenceId, TaskId, Timestamp};

pub(crate) const EXECUTION_EXTENSION_KEY: &str = "execution";

/// Stable human-meaningful checkpoint classifications.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum CheckpointKind {
    /// Repository and caller inspection completed.
    Inspection,
    /// An implementation approach was selected.
    Approach,
    /// A declared implementation milestone completed.
    Implementation,
    /// Verification reached a meaningful milestone.
    Verification,
    /// A blocking condition was observed.
    Blocker,
    /// Execution departed from the approved plan.
    Deviation,
}

/// Protection class assigned to one departure from the approved plan.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub enum DeviationClassification {
    /// A legacy checkpoint or deliberately unclassified departure.
    Unclassified,
    /// A task-local departure that does not change the approved product contract.
    Minor,
    /// A departure that changes scope, behavior, compatibility, or another protected boundary.
    Material,
}

/// Lifecycle state of one identified execution deviation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub enum DeviationStatus {
    /// The departure still blocks task and plan completion.
    Open,
    /// Evidence demonstrates that the departure was resolved in scope.
    Resolved,
    /// A protected decision rejected the departure without changing the plan.
    Rejected,
    /// An applied amendment replaced the affected approved contract.
    Superseded,
}

/// One immutable execution milestone attached to a task.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Checkpoint {
    sequence: u64,
    task_id: TaskId,
    kind: CheckpointKind,
    summary: String,
    actor: String,
    recorded_at: Timestamp,
}

impl Checkpoint {
    /// Returns the monotonic checkpoint sequence.
    #[must_use]
    pub const fn sequence(&self) -> u64 {
        self.sequence
    }

    /// Returns the task owning the checkpoint.
    #[must_use]
    pub const fn task_id(&self) -> &TaskId {
        &self.task_id
    }

    /// Returns the fixed checkpoint classification.
    #[must_use]
    pub const fn kind(&self) -> CheckpointKind {
        self.kind
    }

    /// Returns the human-meaningful milestone summary.
    #[must_use]
    pub fn summary(&self) -> &str {
        &self.summary
    }

    /// Returns the actor that recorded the checkpoint.
    #[must_use]
    pub fn actor(&self) -> &str {
        &self.actor
    }

    /// Returns the checkpoint timestamp.
    #[must_use]
    pub const fn recorded_at(&self) -> &Timestamp {
        &self.recorded_at
    }
}

/// One identified and auditable departure from the approved execution plan.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Deviation {
    id: String,
    task_id: TaskId,
    classification: DeviationClassification,
    status: DeviationStatus,
    summary: String,
    #[serde(default)]
    affected_paths: Vec<String>,
    actor: String,
    recorded_at: Timestamp,
    #[serde(default)]
    legacy_checkpoint_sequence: Option<u64>,
    #[serde(default)]
    resolution: Option<String>,
    #[serde(default)]
    disposition_actor: Option<String>,
    #[serde(default)]
    disposition_reference: Option<String>,
    #[serde(default)]
    amendment_id: Option<String>,
    #[serde(default)]
    evidence_refs: Vec<EvidenceId>,
    #[serde(default)]
    disposed_at: Option<Timestamp>,
}

impl Deviation {
    fn open(
        id: String,
        task_id: TaskId,
        classification: DeviationClassification,
        summary: String,
        affected_paths: Vec<String>,
        actor: String,
        recorded_at: Timestamp,
        legacy_checkpoint_sequence: Option<u64>,
    ) -> Result<Self, DomainError> {
        let deviation = Self {
            id,
            task_id,
            classification,
            status: DeviationStatus::Open,
            summary,
            affected_paths,
            actor,
            recorded_at,
            legacy_checkpoint_sequence,
            resolution: None,
            disposition_actor: None,
            disposition_reference: None,
            amendment_id: None,
            evidence_refs: Vec::new(),
            disposed_at: None,
        };
        deviation.validate()?;
        Ok(deviation)
    }

    /// Returns the stable monotonic deviation identifier.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns the task that owns the deviation.
    #[must_use]
    pub const fn task_id(&self) -> &TaskId {
        &self.task_id
    }

    /// Returns the deviation protection class.
    #[must_use]
    pub const fn classification(&self) -> DeviationClassification {
        self.classification
    }

    /// Returns the deviation lifecycle state.
    #[must_use]
    pub const fn status(&self) -> DeviationStatus {
        self.status
    }

    /// Returns the observed departure summary.
    #[must_use]
    pub fn summary(&self) -> &str {
        &self.summary
    }

    /// Returns exact normalized project paths affected by the departure.
    #[must_use]
    pub fn affected_paths(&self) -> &[String] {
        &self.affected_paths
    }

    /// Returns the actor who recorded the departure.
    #[must_use]
    pub fn actor(&self) -> &str {
        &self.actor
    }

    /// Returns when the departure was recorded.
    #[must_use]
    pub const fn recorded_at(&self) -> &Timestamp {
        &self.recorded_at
    }

    /// Returns the originating legacy checkpoint sequence when one exists.
    #[must_use]
    pub const fn legacy_checkpoint_sequence(&self) -> Option<u64> {
        self.legacy_checkpoint_sequence
    }

    /// Returns the terminal resolution or disposition reason.
    #[must_use]
    pub fn resolution(&self) -> Option<&str> {
        self.resolution.as_deref()
    }

    /// Returns the actor who recorded the terminal disposition.
    #[must_use]
    pub fn disposition_actor(&self) -> Option<&str> {
        self.disposition_actor.as_deref()
    }

    /// Returns the protected decision reference for a rejected deviation.
    #[must_use]
    pub fn disposition_reference(&self) -> Option<&str> {
        self.disposition_reference.as_deref()
    }

    /// Returns the applied amendment that superseded the deviation.
    #[must_use]
    pub fn amendment_id(&self) -> Option<&str> {
        self.amendment_id.as_deref()
    }

    /// Returns immutable evidence supporting a resolution.
    #[must_use]
    pub fn evidence_refs(&self) -> &[EvidenceId] {
        &self.evidence_refs
    }

    /// Returns when the terminal disposition was recorded.
    #[must_use]
    pub const fn disposed_at(&self) -> Option<&Timestamp> {
        self.disposed_at.as_ref()
    }

    /// Returns whether this deviation still blocks completion.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.status == DeviationStatus::Open
    }

    fn resolve(
        &mut self,
        actor: String,
        resolution: String,
        evidence_refs: Vec<EvidenceId>,
        disposed_at: Timestamp,
    ) -> Result<(), DomainError> {
        if !self.is_open()
            || actor.trim().is_empty()
            || resolution.trim().is_empty()
            || evidence_refs.is_empty()
            || !strictly_sorted(&evidence_refs)
        {
            return Err(invariant(format!(
                "Deviation {} is not eligible for evidence-backed resolution",
                self.id
            )));
        }
        self.status = DeviationStatus::Resolved;
        self.resolution = Some(resolution);
        self.disposition_actor = Some(actor);
        self.evidence_refs = evidence_refs;
        self.disposed_at = Some(disposed_at);
        self.validate()
    }

    fn reject(
        &mut self,
        actor: String,
        reference: String,
        reason: String,
        disposed_at: Timestamp,
    ) -> Result<(), DomainError> {
        if !self.is_open()
            || actor.trim().is_empty()
            || reference.trim().is_empty()
            || reason.trim().is_empty()
        {
            return Err(invariant(format!(
                "Deviation {} is not eligible for rejection",
                self.id
            )));
        }
        self.status = DeviationStatus::Rejected;
        self.resolution = Some(reason);
        self.disposition_actor = Some(actor);
        self.disposition_reference = Some(reference);
        self.disposed_at = Some(disposed_at);
        self.validate()
    }

    fn supersede(
        &mut self,
        actor: String,
        amendment_id: String,
        reason: String,
        disposed_at: Timestamp,
    ) -> Result<(), DomainError> {
        if !self.is_open()
            || actor.trim().is_empty()
            || change_number(&amendment_id).is_none()
            || reason.trim().is_empty()
        {
            return Err(invariant(format!(
                "Deviation {} is not eligible for amendment supersession",
                self.id
            )));
        }
        self.status = DeviationStatus::Superseded;
        self.resolution = Some(reason);
        self.disposition_actor = Some(actor);
        self.amendment_id = Some(amendment_id);
        self.disposed_at = Some(disposed_at);
        self.validate()
    }

    fn validate(&self) -> Result<(), DomainError> {
        if deviation_number(&self.id).is_none()
            || self.summary.trim().is_empty()
            || self.actor.trim().is_empty()
            || self.legacy_checkpoint_sequence == Some(0)
            || self
                .affected_paths
                .iter()
                .any(|path| !is_safe_exact_path(path))
            || !strictly_sorted(&self.affected_paths)
            || !strictly_sorted(&self.evidence_refs)
        {
            return Err(invariant("Deviation identity or source is malformed"));
        }
        let resolution = self
            .resolution
            .as_deref()
            .filter(|value| !value.trim().is_empty());
        let disposition_actor = self
            .disposition_actor
            .as_deref()
            .filter(|value| !value.trim().is_empty());
        let disposition_reference = self
            .disposition_reference
            .as_deref()
            .filter(|value| !value.trim().is_empty());
        let amendment_id = self
            .amendment_id
            .as_deref()
            .filter(|value| change_number(value).is_some());
        let valid = match self.status {
            DeviationStatus::Open => {
                resolution.is_none()
                    && disposition_actor.is_none()
                    && disposition_reference.is_none()
                    && amendment_id.is_none()
                    && self.evidence_refs.is_empty()
                    && self.disposed_at.is_none()
            }
            DeviationStatus::Resolved => {
                resolution.is_some()
                    && disposition_actor.is_some()
                    && disposition_reference.is_none()
                    && amendment_id.is_none()
                    && !self.evidence_refs.is_empty()
                    && self.disposed_at.is_some()
            }
            DeviationStatus::Rejected => {
                resolution.is_some()
                    && disposition_actor.is_some()
                    && disposition_reference.is_some()
                    && amendment_id.is_none()
                    && self.evidence_refs.is_empty()
                    && self.disposed_at.is_some()
            }
            DeviationStatus::Superseded => {
                resolution.is_some()
                    && disposition_actor.is_some()
                    && disposition_reference.is_none()
                    && amendment_id.is_some()
                    && self.evidence_refs.is_empty()
                    && self.disposed_at.is_some()
            }
        };
        if valid {
            Ok(())
        } else {
            Err(invariant(format!(
                "Deviation {} has inconsistent lifecycle fields",
                self.id
            )))
        }
    }
}

/// Versioned execution state stored under the plan extension key.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExecutionState {
    checkpoints: Vec<Checkpoint>,
    #[serde(default)]
    deviations: Vec<Deviation>,
}

impl ExecutionState {
    /// Returns checkpoints in monotonic sequence order.
    #[must_use]
    pub fn checkpoints(&self) -> &[Checkpoint] {
        &self.checkpoints
    }

    /// Returns identified deviations in monotonic identifier order.
    #[must_use]
    pub fn deviations(&self) -> &[Deviation] {
        &self.deviations
    }

    /// Returns one identified deviation when it exists.
    #[must_use]
    pub fn deviation(&self, deviation_id: &str) -> Option<&Deviation> {
        self.deviations
            .iter()
            .find(|deviation| deviation.id == deviation_id)
    }

    /// Returns the next deterministic deviation identifier without reserving it.
    ///
    /// # Errors
    ///
    /// Returns an invariant error when the identifier counter overflows.
    pub fn next_deviation_id(&self) -> Result<String, DomainError> {
        let number = self
            .deviations
            .len()
            .checked_add(1)
            .ok_or_else(|| invariant("Deviation identifier overflowed"))?;
        Ok(format!("D{number}"))
    }

    /// Returns whether no execution-only state has been recorded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.checkpoints.is_empty() && self.deviations.is_empty()
    }

    pub(crate) fn record_checkpoint(
        &mut self,
        task_id: TaskId,
        kind: CheckpointKind,
        summary: String,
        actor: String,
        recorded_at: Timestamp,
    ) -> Result<(), DomainError> {
        self.materialize_legacy_deviations()?;
        if summary.trim().is_empty() || actor.trim().is_empty() {
            return Err(invariant(
                "A checkpoint requires a non-empty summary and actor",
            ));
        }
        let sequence = u64::try_from(self.checkpoints.len())
            .ok()
            .and_then(|count| count.checked_add(1))
            .ok_or_else(|| invariant("Checkpoint sequence overflowed"))?;
        self.checkpoints.push(Checkpoint {
            sequence,
            task_id: task_id.clone(),
            kind,
            summary: summary.clone(),
            actor: actor.clone(),
            recorded_at: recorded_at.clone(),
        });
        if kind == CheckpointKind::Deviation {
            self.push_deviation(
                task_id,
                DeviationClassification::Unclassified,
                summary,
                Vec::new(),
                actor,
                recorded_at,
                Some(sequence),
            )?;
        }
        Ok(())
    }

    pub(crate) fn record_deviation(
        &mut self,
        task_id: TaskId,
        classification: DeviationClassification,
        summary: String,
        affected_paths: Vec<String>,
        actor: String,
        recorded_at: Timestamp,
    ) -> Result<String, DomainError> {
        self.materialize_legacy_deviations()?;
        self.push_deviation(
            task_id,
            classification,
            summary,
            affected_paths,
            actor,
            recorded_at,
            None,
        )
    }

    pub(crate) fn resolve_deviation(
        &mut self,
        deviation_id: &str,
        actor: String,
        resolution: String,
        evidence_refs: Vec<EvidenceId>,
        disposed_at: Timestamp,
    ) -> Result<(), DomainError> {
        self.materialize_legacy_deviations()?;
        self.deviation_mut(deviation_id)?
            .resolve(actor, resolution, evidence_refs, disposed_at)
    }

    pub(crate) fn reject_deviation(
        &mut self,
        deviation_id: &str,
        actor: String,
        reference: String,
        reason: String,
        disposed_at: Timestamp,
    ) -> Result<(), DomainError> {
        self.materialize_legacy_deviations()?;
        self.deviation_mut(deviation_id)?
            .reject(actor, reference, reason, disposed_at)
    }

    pub(crate) fn supersede_deviation(
        &mut self,
        deviation_id: &str,
        actor: String,
        amendment_id: String,
        reason: String,
        disposed_at: Timestamp,
    ) -> Result<(), DomainError> {
        self.materialize_legacy_deviations()?;
        self.deviation_mut(deviation_id)?
            .supersede(actor, amendment_id, reason, disposed_at)
    }

    pub(crate) fn materialize_legacy_deviations(&mut self) -> Result<(), DomainError> {
        let legacy_sequences = self
            .deviations
            .iter()
            .filter_map(|deviation| deviation.legacy_checkpoint_sequence)
            .collect::<BTreeSet<_>>();
        let checkpoints = self
            .checkpoints
            .iter()
            .filter(|checkpoint| {
                checkpoint.kind == CheckpointKind::Deviation
                    && !legacy_sequences.contains(&checkpoint.sequence)
            })
            .cloned()
            .collect::<Vec<_>>();
        for checkpoint in checkpoints {
            self.push_deviation(
                checkpoint.task_id,
                DeviationClassification::Unclassified,
                checkpoint.summary,
                Vec::new(),
                checkpoint.actor,
                checkpoint.recorded_at,
                Some(checkpoint.sequence),
            )?;
        }
        Ok(())
    }

    pub(crate) fn reset_for_material_amendment(&mut self) {
        self.checkpoints.clear();
        for deviation in &mut self.deviations {
            deviation.legacy_checkpoint_sequence = None;
        }
    }

    pub(crate) fn validate(&self, task_ids: &BTreeSet<&TaskId>) -> Result<(), DomainError> {
        for (index, checkpoint) in self.checkpoints.iter().enumerate() {
            let expected = u64::try_from(index)
                .ok()
                .and_then(|value| value.checked_add(1))
                .ok_or_else(|| invariant("Checkpoint sequence overflowed"))?;
            if checkpoint.sequence != expected
                || !task_ids.contains(&checkpoint.task_id)
                || checkpoint.summary.trim().is_empty()
                || checkpoint.actor.trim().is_empty()
            {
                return Err(invariant(
                    "Execution checkpoints must be contiguous, task-bound, and complete",
                ));
            }
        }
        let mut legacy_sequences = BTreeSet::new();
        for (index, deviation) in self.deviations.iter().enumerate() {
            let expected = index
                .checked_add(1)
                .ok_or_else(|| invariant("Deviation identifier overflowed"))?;
            deviation.validate()?;
            if deviation.id != format!("D{expected}")
                || !task_ids.contains(&deviation.task_id)
                || deviation
                    .legacy_checkpoint_sequence
                    .is_some_and(|sequence| !legacy_sequences.insert(sequence))
            {
                return Err(invariant(
                    "Execution deviations must be ordered, task-bound, and uniquely sourced",
                ));
            }
        }
        Ok(())
    }

    fn push_deviation(
        &mut self,
        task_id: TaskId,
        classification: DeviationClassification,
        summary: String,
        affected_paths: Vec<String>,
        actor: String,
        recorded_at: Timestamp,
        legacy_checkpoint_sequence: Option<u64>,
    ) -> Result<String, DomainError> {
        let id = self.next_deviation_id()?;
        self.deviations.push(Deviation::open(
            id.clone(),
            task_id,
            classification,
            summary,
            affected_paths,
            actor,
            recorded_at,
            legacy_checkpoint_sequence,
        )?);
        Ok(id)
    }

    fn deviation_mut(&mut self, deviation_id: &str) -> Result<&mut Deviation, DomainError> {
        self.deviations
            .iter_mut()
            .find(|deviation| deviation.id == deviation_id)
            .ok_or_else(|| invariant(format!("Deviation {deviation_id} does not exist")))
    }
}

fn deviation_number(id: &str) -> Option<u64> {
    prefixed_number(id, 'D')
}

fn change_number(id: &str) -> Option<u64> {
    prefixed_number(id, 'C')
}

fn prefixed_number(id: &str, prefix: char) -> Option<u64> {
    id.strip_prefix(prefix)
        .filter(|number| !number.starts_with('0'))
        .and_then(|number| number.parse().ok())
        .filter(|number| *number > 0)
}

fn strictly_sorted<T: Ord>(values: &[T]) -> bool {
    values.windows(2).all(|pair| pair[0] < pair[1])
}

fn is_safe_exact_path(value: &str) -> bool {
    let path = Path::new(value);
    !value.is_empty()
        && !value.contains(['\\', '*', '?', '[', ']'])
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn invariant(message: impl Into<String>) -> DomainError {
    DomainError::new(DomainErrorKind::InvariantViolation, message)
}
