//! Versioned domain model for plans, tasks, evidence, and audit events.

mod amendment;
mod authoring;
mod check_run;
mod error;
mod event;
mod evidence;
mod execution;
mod git_readiness;
mod id;
mod lineage;
mod plan;
mod review;
mod standards;
mod state;
mod task;
mod timestamp;
mod version;
mod workspace;

pub use amendment::{
    Amendment, AmendmentClassification, AmendmentImpact, AmendmentOperation, AmendmentPatch,
    AmendmentStatus, MinorFileKind, ProtectedChangeCategory,
};
pub use authoring::{
    DraftCommitGateInput, DraftContextInput, DraftCriterionInput, DraftDecisionInput,
    DraftEdgeCaseInput, DraftFileInput, DraftMetadataInput, DraftPlanInput, DraftScopeInput,
    DraftTaskInput, DraftTaskUpdateInput, DraftVerificationInput, PlanDraftSeed,
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
pub use execution::{
    Checkpoint, CheckpointKind, Deviation, DeviationClassification, DeviationStatus, ExecutionState,
};
pub use git_readiness::{
    CleanupConsentStatus, GitReadinessObservation, GitReadinessState, GitRepositoryMode,
    GitSetupDecision, GitSetupState, PrePlanCleanupDecision, PrePlanCleanupItem,
    PrePlanCleanupState,
};
pub(crate) use git_readiness::{GIT_READINESS_EXTENSION_KEY, is_conventional_commit};
pub use id::{CheckId, CriterionId, EvidenceId, PlanId, RequestId, TaskId};
pub use lineage::{Lineage, PlanArchive};
pub use plan::{
    Approach, Approval, ApprovalKind, ContextReference, Decision, EdgeCase, FinalOutcome,
    GitReadiness, OutcomeFollowUpSource, Plan, PlanMetadata, PlanScope, ProjectScanAcceptance,
    ProjectScanSummary, StandardSelection,
};
pub use review::{MaterialReviewDecision, MaterialReviewDisposition, ReviewItem};
pub use standards::{
    StandardConflict, StandardConflictCandidate, StandardConflictDecision, StandardConflictRecord,
    StandardSourceKind, StandardsConflictState,
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
pub(crate) use workspace::WORKSPACE_EXTENSION_KEY;
pub use workspace::{
    WorkspaceFileKind, WorkspaceFileSnapshot, WorkspaceFingerprint, WorkspaceFingerprintScope,
    WorkspaceGitEntry, WorkspaceProtocolState, WorkspaceRepositoryMode, WorkspaceScopeKind,
    WorkspaceStatusEntry,
};
