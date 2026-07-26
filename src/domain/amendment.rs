//! Typed protected-plan amendment records and classification policy.

use std::collections::BTreeSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::task::is_safe_repository_path;
use super::{CheckId, DomainError, DomainErrorKind, EvidenceId, FileChange, TaskId, Timestamp};

/// Minimum protection class required by an amendment operation.
#[derive(
    Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize, JsonSchema,
)]
pub enum AmendmentClassification {
    /// Task-local allowlisted support that preserves behavior and scope.
    Minor,
    /// A protected change to behavior, scope, security, compatibility, or order.
    Material,
}

/// Lifecycle state of one immutable amendment proposal.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub enum AmendmentStatus {
    /// A Minor proposal is ready for deterministic application.
    Proposed,
    /// A Material proposal is blocked at its explicit approval boundary.
    #[serde(rename = "Approval Required")]
    ApprovalRequired,
    /// A Material proposal has an auditable approval declaration.
    Approved,
    /// The typed operations were applied and their invalidations recorded.
    Applied,
}

/// Allowlisted task-local file role for a Minor amendment.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub enum MinorFileKind {
    /// A test-only fixture.
    #[serde(rename = "Test Fixture")]
    TestFixture,
    /// A barrel export required by the existing task contract.
    #[serde(rename = "Barrel Export")]
    BarrelExport,
    /// A test or UI snapshot.
    Snapshot,
    /// A task-local support file that preserves approved behavior and scope.
    #[serde(rename = "Support File")]
    SupportFile,
}

/// Protected material decision category.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub enum ProtectedChangeCategory {
    /// User-observable behavior.
    #[serde(rename = "User-Visible Behavior")]
    UserVisibleBehavior,
    /// A public programming interface.
    #[serde(rename = "Public API")]
    PublicApi,
    /// Persistent or exchanged data shape.
    #[serde(rename = "Data or Schema")]
    DataOrSchema,
    /// A project or runtime dependency.
    Dependency,
    /// A compatibility commitment.
    Compatibility,
    /// A security or permission boundary.
    Security,
}

impl ProtectedChangeCategory {
    /// Returns the stable human-readable category label.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UserVisibleBehavior => "User-Visible Behavior",
            Self::PublicApi => "Public API",
            Self::DataOrSchema => "Data or Schema",
            Self::Dependency => "Dependency",
            Self::Compatibility => "Compatibility",
            Self::Security => "Security",
        }
    }
}

/// One typed semantic operation accepted by the amendment protocol.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "operation", rename_all = "kebab-case", deny_unknown_fields)]
pub enum AmendmentOperation {
    /// Adds one explicitly allowlisted task-local file responsibility.
    AddTaskFile {
        /// Allowlisted file role.
        kind: MinorFileKind,
        /// Existing Ready or In Progress task.
        task_id: TaskId,
        /// Project-relative file path or narrow glob.
        path: String,
        /// Planned change kind.
        change: FileChange,
        /// Task-local reason for the file.
        reason: String,
    },
    /// Replaces one task verification definition while retaining its stable ID.
    ReplaceTaskVerification {
        /// Existing Ready or In Progress task.
        task_id: TaskId,
        /// Existing verification check identifier.
        check_id: CheckId,
        /// Exact replacement executable and arguments.
        command: Vec<String>,
        /// Project-relative working directory.
        cwd: String,
        /// Expected replacement exit code.
        expected_exit_code: i32,
        /// Whether the replacement check gates completion.
        required: bool,
    },
    /// Appends a non-behavioral implementation note to one task.
    AddImplementationNote {
        /// Existing Ready or In Progress task.
        task_id: TaskId,
        /// Non-empty implementation note.
        note: String,
    },
    /// Replaces the plan summary and therefore its approved outcome description.
    ReplaceSummary {
        /// Complete replacement summary.
        summary: String,
    },
    /// Replaces every scope boundary as one protected operation.
    ReplaceScope {
        /// Complete goal.
        goal: String,
        /// Complete deliverable list.
        deliverables: Vec<String>,
        /// Complete in-scope list.
        in_scope: Vec<String>,
        /// Complete out-of-scope list.
        out_of_scope: Vec<String>,
    },
    /// Replaces the implementation approach.
    ReplaceApproach {
        /// Complete replacement approach.
        approach: String,
    },
    /// Replaces the public interfaces and data-flow description.
    ReplaceInterfaces {
        /// Complete replacement interface description.
        interfaces: String,
    },
    /// Records a protected behavior/API/data/dependency/compatibility/security decision.
    RecordProtectedDecision {
        /// Protected decision category.
        category: ProtectedChangeCategory,
        /// Decision subject.
        item: String,
        /// Selected value.
        decision: String,
        /// Reason for the protected decision.
        reason: String,
    },
    /// Replaces the complete core task order.
    ReplaceTaskOrder {
        /// Complete unique task order.
        task_order: Vec<TaskId>,
    },
    /// Expands one task File Map outside the Minor allowlist.
    ExpandTaskFile {
        /// Existing task whose approved scope expands.
        task_id: TaskId,
        /// Project-relative path or narrow glob.
        path: String,
        /// Planned change kind.
        change: FileChange,
        /// Reason for the material scope expansion.
        reason: String,
    },
}

impl AmendmentOperation {
    /// Returns the minimum classification required by this operation.
    #[must_use]
    pub const fn minimum_classification(&self) -> AmendmentClassification {
        match self {
            Self::AddTaskFile { .. }
            | Self::ReplaceTaskVerification { .. }
            | Self::AddImplementationNote { .. } => AmendmentClassification::Minor,
            Self::ReplaceSummary { .. }
            | Self::ReplaceScope { .. }
            | Self::ReplaceApproach { .. }
            | Self::ReplaceInterfaces { .. }
            | Self::RecordProtectedDecision { .. }
            | Self::ReplaceTaskOrder { .. }
            | Self::ExpandTaskFile { .. } => AmendmentClassification::Material,
        }
    }

    pub(crate) fn validate(&self) -> Result<(), DomainError> {
        let valid = match self {
            Self::AddTaskFile {
                kind,
                path,
                change,
                reason,
                ..
            } => {
                is_safe_repository_path(path)
                    && !reason.trim().is_empty()
                    && minor_file_change_is_valid(*kind, *change)
            }
            Self::ReplaceTaskVerification { command, cwd, .. } => {
                !command.is_empty()
                    && command.iter().all(|part| !part.trim().is_empty())
                    && !cwd.trim().is_empty()
            }
            Self::AddImplementationNote { note, .. } => !note.trim().is_empty(),
            Self::ReplaceSummary { summary } => !summary.trim().is_empty(),
            Self::ReplaceScope {
                goal,
                deliverables,
                in_scope,
                out_of_scope,
            } => {
                !goal.trim().is_empty()
                    && complete_text_list(deliverables)
                    && complete_text_list(in_scope)
                    && complete_text_list(out_of_scope)
            }
            Self::ReplaceApproach { approach } => !approach.trim().is_empty(),
            Self::ReplaceInterfaces { interfaces } => !interfaces.trim().is_empty(),
            Self::RecordProtectedDecision {
                item,
                decision,
                reason,
                ..
            } => {
                !item.trim().is_empty() && !decision.trim().is_empty() && !reason.trim().is_empty()
            }
            Self::ReplaceTaskOrder { task_order } => {
                !task_order.is_empty()
                    && task_order.iter().collect::<BTreeSet<_>>().len() == task_order.len()
            }
            Self::ExpandTaskFile { path, reason, .. } => {
                is_safe_repository_path(path) && !reason.trim().is_empty()
            }
        };
        if valid {
            Ok(())
        } else {
            Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                "Amendment operation is incomplete or violates its typed allowlist",
            ))
        }
    }
}

/// Strict YAML document containing only supported semantic operations.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AmendmentPatch {
    operations: Vec<AmendmentOperation>,
}

impl AmendmentPatch {
    /// Returns typed operations in document order.
    #[must_use]
    pub fn operations(&self) -> &[AmendmentOperation] {
        &self.operations
    }

    /// Consumes the patch and returns its typed operations.
    #[must_use]
    pub fn into_operations(self) -> Vec<AmendmentOperation> {
        self.operations
    }

    /// Returns the minimum classification required by every operation together.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty or malformed operation list.
    pub fn minimum_classification(&self) -> Result<AmendmentClassification, DomainError> {
        if self.operations.is_empty() {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                "Amendment patch requires at least one typed operation",
            ));
        }
        let mut classification = AmendmentClassification::Minor;
        for operation in &self.operations {
            operation.validate()?;
            classification = classification.max(operation.minimum_classification());
        }
        Ok(classification)
    }
}

/// Deterministically computed fields and evidence affected by a proposal.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AmendmentImpact {
    affected_fields: Vec<String>,
    affected_tasks: Vec<TaskId>,
    affected_checks: Vec<CheckId>,
    stale_evidence: Vec<EvidenceId>,
}

impl AmendmentImpact {
    pub(crate) fn new(
        affected_fields: Vec<String>,
        affected_tasks: Vec<TaskId>,
        affected_checks: Vec<CheckId>,
        stale_evidence: Vec<EvidenceId>,
    ) -> Result<Self, DomainError> {
        let impact = Self {
            affected_fields,
            affected_tasks,
            affected_checks,
            stale_evidence,
        };
        impact.validate()?;
        Ok(impact)
    }

    /// Returns exact semantic fields affected by the operations.
    #[must_use]
    pub fn affected_fields(&self) -> &[String] {
        &self.affected_fields
    }

    /// Returns tasks whose execution inputs are affected.
    #[must_use]
    pub fn affected_tasks(&self) -> &[TaskId] {
        &self.affected_tasks
    }

    /// Returns verification checks whose prior results are invalidated.
    #[must_use]
    pub fn affected_checks(&self) -> &[CheckId] {
        &self.affected_checks
    }

    /// Returns evidence made stale by application of the proposal.
    #[must_use]
    pub fn stale_evidence(&self) -> &[EvidenceId] {
        &self.stale_evidence
    }

    fn validate(&self) -> Result<(), DomainError> {
        if self.affected_fields.is_empty()
            || self
                .affected_fields
                .iter()
                .any(|field| field.trim().is_empty())
            || !strictly_sorted(&self.affected_fields)
            || !strictly_sorted(&self.affected_tasks)
            || !strictly_sorted(&self.affected_checks)
            || !strictly_sorted(&self.stale_evidence)
        {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                "Amendment impact must contain sorted unique affected fields and identifiers",
            ));
        }
        Ok(())
    }
}

/// Auditable typed amendment proposal and its approval/application history.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Amendment {
    id: String,
    reason: String,
    minimum_classification: AmendmentClassification,
    classification: AmendmentClassification,
    status: AmendmentStatus,
    operations: Vec<AmendmentOperation>,
    base_revision: u64,
    base_state_hash: String,
    impact: AmendmentImpact,
    proposer: String,
    proposed_at: Timestamp,
    approval_actor: Option<String>,
    approval_reference: Option<String>,
    approved_at: Option<Timestamp>,
    applied_at: Option<Timestamp>,
}

impl Amendment {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn proposed(
        id: String,
        reason: String,
        minimum_classification: AmendmentClassification,
        classification: AmendmentClassification,
        operations: Vec<AmendmentOperation>,
        base_revision: u64,
        base_state_hash: String,
        impact: AmendmentImpact,
        proposer: String,
        proposed_at: Timestamp,
    ) -> Result<Self, DomainError> {
        let amendment = Self {
            id,
            reason,
            minimum_classification,
            classification,
            status: if classification == AmendmentClassification::Material {
                AmendmentStatus::ApprovalRequired
            } else {
                AmendmentStatus::Proposed
            },
            operations,
            base_revision,
            base_state_hash,
            impact,
            proposer,
            proposed_at,
            approval_actor: None,
            approval_reference: None,
            approved_at: None,
            applied_at: None,
        };
        amendment.validate()?;
        Ok(amendment)
    }

    /// Returns the monotonic change identifier.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns the exact reason supplied for the proposal.
    #[must_use]
    pub fn reason(&self) -> &str {
        &self.reason
    }

    /// Returns the classifier-derived minimum protection class.
    #[must_use]
    pub const fn minimum_classification(&self) -> AmendmentClassification {
        self.minimum_classification
    }

    /// Returns the selected protection class, which may only raise the minimum.
    #[must_use]
    pub const fn classification(&self) -> AmendmentClassification {
        self.classification
    }

    /// Returns the current amendment lifecycle state.
    #[must_use]
    pub const fn status(&self) -> AmendmentStatus {
        self.status
    }

    /// Returns immutable typed operations in proposal order.
    #[must_use]
    pub fn operations(&self) -> &[AmendmentOperation] {
        &self.operations
    }

    /// Returns the exact source plan revision preserved by the store snapshot.
    #[must_use]
    pub const fn base_revision(&self) -> u64 {
        self.base_revision
    }

    /// Returns the canonical source-state digest at proposal time.
    #[must_use]
    pub fn base_state_hash(&self) -> &str {
        &self.base_state_hash
    }

    /// Returns the classifier-derived impact and stale-evidence set.
    #[must_use]
    pub const fn impact(&self) -> &AmendmentImpact {
        &self.impact
    }

    /// Returns the approval reference for an approved Material change.
    #[must_use]
    pub fn approval_reference(&self) -> Option<&str> {
        self.approval_reference.as_deref()
    }

    /// Returns the actor who proposed the change.
    #[must_use]
    pub fn proposer(&self) -> &str {
        &self.proposer
    }

    /// Returns when the proposal was recorded.
    #[must_use]
    pub const fn proposed_at(&self) -> &Timestamp {
        &self.proposed_at
    }

    /// Returns the actor who approved a Material proposal.
    #[must_use]
    pub fn approval_actor(&self) -> Option<&str> {
        self.approval_actor.as_deref()
    }

    /// Returns when a Material proposal was approved.
    #[must_use]
    pub const fn approved_at(&self) -> Option<&Timestamp> {
        self.approved_at.as_ref()
    }

    /// Returns when the operations were atomically applied.
    #[must_use]
    pub const fn applied_at(&self) -> Option<&Timestamp> {
        self.applied_at.as_ref()
    }

    /// Returns whether the proposal still blocks unrelated plan mutations.
    #[must_use]
    pub fn is_pending(&self) -> bool {
        self.status != AmendmentStatus::Applied
    }

    pub(crate) fn approve(
        &mut self,
        actor: String,
        reference: String,
        approved_at: Timestamp,
    ) -> Result<(), DomainError> {
        if self.classification != AmendmentClassification::Material
            || self.status != AmendmentStatus::ApprovalRequired
            || actor.trim().is_empty()
            || reference.trim().is_empty()
        {
            return Err(DomainError::new(
                DomainErrorKind::ApprovalRequired,
                format!(
                    "Amendment {} requires one explicit Material approval",
                    self.id
                ),
            ));
        }
        self.status = AmendmentStatus::Approved;
        self.approval_actor = Some(actor);
        self.approval_reference = Some(reference);
        self.approved_at = Some(approved_at);
        self.validate()
    }

    pub(crate) fn mark_applied(&mut self, applied_at: Timestamp) -> Result<(), DomainError> {
        let is_eligible = match self.classification {
            AmendmentClassification::Minor => self.status == AmendmentStatus::Proposed,
            AmendmentClassification::Material => self.status == AmendmentStatus::Approved,
        };
        if !is_eligible {
            return Err(DomainError::new(
                DomainErrorKind::InvalidTransition,
                format!("Amendment {} is not eligible to apply", self.id),
            ));
        }
        self.status = AmendmentStatus::Applied;
        self.applied_at = Some(applied_at);
        self.validate()
    }

    pub(crate) fn validate(&self) -> Result<(), DomainError> {
        if change_number(&self.id).is_none()
            || self.reason.trim().is_empty()
            || self.proposer.trim().is_empty()
            || self.base_revision == 0
            || !is_sha256(&self.base_state_hash)
            || self.operations.is_empty()
            || self.classification < self.minimum_classification
        {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                "Amendment identity, source, or classification is malformed",
            ));
        }
        let computed_minimum = self.operations.iter().try_fold(
            AmendmentClassification::Minor,
            |current, operation| {
                operation.validate()?;
                Ok::<_, DomainError>(current.max(operation.minimum_classification()))
            },
        )?;
        if computed_minimum > self.minimum_classification {
            return Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                "Amendment minimum classification is below its typed operations",
            ));
        }
        self.impact.validate()?;
        let approval_complete = self
            .approval_actor
            .as_deref()
            .is_some_and(|actor| !actor.trim().is_empty())
            && self
                .approval_reference
                .as_deref()
                .is_some_and(|reference| !reference.trim().is_empty())
            && self.approved_at.is_some();
        let state_valid = match self.status {
            AmendmentStatus::Proposed => {
                self.classification == AmendmentClassification::Minor
                    && !approval_complete
                    && self.applied_at.is_none()
            }
            AmendmentStatus::ApprovalRequired => {
                self.classification == AmendmentClassification::Material
                    && !approval_complete
                    && self.applied_at.is_none()
            }
            AmendmentStatus::Approved => {
                self.classification == AmendmentClassification::Material
                    && approval_complete
                    && self.applied_at.is_none()
            }
            AmendmentStatus::Applied => {
                self.applied_at.is_some()
                    && (self.classification == AmendmentClassification::Minor || approval_complete)
            }
        };
        if state_valid {
            Ok(())
        } else {
            Err(DomainError::new(
                DomainErrorKind::InvariantViolation,
                format!("Amendment {} has inconsistent lifecycle fields", self.id),
            ))
        }
    }
}

pub(crate) fn change_number(id: &str) -> Option<u64> {
    id.strip_prefix('C')
        .filter(|number| !number.starts_with('0'))
        .and_then(|number| number.parse().ok())
        .filter(|number| *number > 0)
}

fn minor_file_change_is_valid(kind: MinorFileKind, change: FileChange) -> bool {
    match kind {
        MinorFileKind::TestFixture | MinorFileKind::SupportFile => {
            matches!(change, FileChange::Create | FileChange::Test)
        }
        MinorFileKind::BarrelExport => {
            matches!(change, FileChange::Create | FileChange::Modify)
        }
        MinorFileKind::Snapshot => matches!(
            change,
            FileChange::Create | FileChange::Modify | FileChange::Test
        ),
    }
}

fn complete_text_list(values: &[String]) -> bool {
    !values.is_empty() && values.iter().all(|value| !value.trim().is_empty())
}

fn strictly_sorted<T: Ord>(values: &[T]) -> bool {
    values.windows(2).all(|pair| pair[0] < pair[1])
}

fn is_sha256(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}
