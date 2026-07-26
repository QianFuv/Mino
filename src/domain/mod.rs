//! Versioned domain model for plans, tasks, evidence, and audit events.

mod authoring;
mod check_run;
mod error;
mod event;
mod evidence;
mod execution;
mod id;
mod plan;
mod review;
mod state;
mod task;
mod timestamp;
mod version;

pub use authoring::{
    DraftCommitGateInput, DraftContextInput, DraftCriterionInput, DraftDecisionInput,
    DraftEdgeCaseInput, DraftFileInput, DraftMetadataInput, DraftPlanInput, DraftScopeInput,
    DraftTaskInput, DraftVerificationInput, PlanDraftSeed,
};
pub(crate) use check_run::CheckRunCompletion;
pub use check_run::{
    AppliedRedaction, CheckRunContext, CheckRunLease, CheckRunLimits, CheckRunOutcome,
    CheckRunResult,
};
pub use error::{DomainError, DomainErrorKind};
pub use event::{Event, EventResult};
pub(crate) use evidence::EvidenceFields;
pub use evidence::{Evidence, EvidenceType, Redaction};
pub use execution::{Checkpoint, CheckpointKind, ExecutionState};
pub use id::{CheckId, CriterionId, EvidenceId, PlanId, RequestId, TaskId};
pub use plan::{
    Approach, Approval, ApprovalKind, ContextReference, Decision, EdgeCase, FinalOutcome,
    GitReadiness, Lineage, Plan, PlanMetadata, PlanScope, StandardSelection,
};
pub use review::ReviewItem;
pub use state::{
    CheckStatus, CommitStatus, CriterionStatus, GitFlowConsent, PlanStatus, ReviewClassification,
    ReviewStatus, TaskStatus,
};
pub use task::{
    AcceptanceCriterion, CommitGate, FileChange, FileMapEntry, Task, VerificationCheck,
};
pub use timestamp::Timestamp;
pub use version::{
    CURRENT_PROTOCOL_REVISION, CURRENT_PROTOCOL_VERSION, CURRENT_SCHEMA_VERSION, ProtocolVersion,
    SchemaVersion,
};
