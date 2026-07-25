//! Versioned domain model for plans, tasks, evidence, and audit events.

mod authoring;
mod error;
mod event;
mod evidence;
mod id;
mod plan;
mod state;
mod task;
mod timestamp;
mod version;

pub use authoring::{
    DraftCommitGateInput, DraftContextInput, DraftCriterionInput, DraftDecisionInput,
    DraftEdgeCaseInput, DraftFileInput, DraftMetadataInput, DraftPlanInput, DraftScopeInput,
    DraftTaskInput, DraftVerificationInput, PlanDraftSeed,
};
pub use error::{DomainError, DomainErrorKind};
pub use event::{Event, EventResult};
pub use evidence::{Evidence, EvidenceType, Redaction};
pub use id::{CheckId, CriterionId, EvidenceId, PlanId, RequestId, TaskId};
pub use plan::{
    Approach, Approval, ApprovalKind, ContextReference, Decision, EdgeCase, FinalOutcome,
    GitReadiness, Lineage, Plan, PlanMetadata, PlanScope, ReviewItem, StandardSelection,
};
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
