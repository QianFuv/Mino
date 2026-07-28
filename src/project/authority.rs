//! Auditable resolution of legacy and Mino durable-planning authority.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use fs4::{FileExt, TryLockError};
use serde::{Deserialize, Serialize};

use crate::domain::{RequestId, Timestamp};
use crate::integration::{
    IntegrationFailurePoint, agents_workflow_is_active, guarded_agents_rewrite,
    planning_supersession, recover_repository_transactions,
};
use crate::managed_fs::{
    ManagedEntryKind, ManagedFsError, ManagedFsErrorKind, ManagedPath, ProjectFs,
};
use crate::store::sha256_digest;
use crate::{ErrorCategory, MinoError, NextAction};

use super::ProjectLayout;
use super::migrate::{
    LegacyPlanningClause, LegacyPlanningDetection, detect_legacy_planning_authority,
};

const AUTHORITY_KIND: &str = "mino.planning-authority/v1";
const AUTHORITY_SCHEMA_VERSION: u32 = 1;
const MAX_AUTHORITY_BYTES: u64 = 1024 * 1024;
const MAX_AGENTS_BYTES: u64 = 1024 * 1024;
const LOCK_TIMEOUT: Duration = Duration::from_secs(2);
const LOCK_RETRY: Duration = Duration::from_millis(10);
static NEXT_AUTHORITY_FILE: AtomicU64 = AtomicU64::new(1);

/// Durable-planning authority decision bound to exact repository instructions.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PlanningAuthorityDecision {
    /// No explicit authority outcome has been recorded.
    #[default]
    Pending,
    /// The guarded rewrite replaced the active legacy planning section.
    Superseded,
    /// Mino owns durable execution while legacy text remains inert reference.
    CoexistenceApproved,
    /// Mino durable-plan creation was explicitly declined.
    Declined,
}

/// Current authority facts and effective durable-plan gate.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "the wire report exposes independent observed and effective gate facts"
)]
pub struct PlanningAuthorityStatus {
    /// Stable authority report kind.
    pub authority_kind: &'static str,
    /// Persisted authority revision, or zero before the first write.
    pub authority_revision: u64,
    /// Whether current active text contains legacy durable-planning clauses.
    pub legacy_planning_rules_detected: bool,
    /// Exact current `AGENTS.md` digest when the file exists.
    pub source_digest: Option<String>,
    /// Active clause locations outside fenced examples.
    pub detected_clauses: Vec<LegacyPlanningClause>,
    /// Whether one well-formed Mino workflow block is active.
    pub mino_workflow_active: bool,
    /// Persisted or derived authority decision.
    pub decision: PlanningAuthorityDecision,
    /// Actor who recorded the current terminal decision.
    pub decided_by: Option<String>,
    /// Auditable reference for the current terminal decision.
    pub decision_reference: Option<String>,
    /// Timestamp of the current terminal decision.
    pub decided_at: Option<Timestamp>,
    /// Digest of the successfully published supersession rewrite.
    pub applied_rewrite_digest: Option<String>,
    /// Whether current source bytes invalidate the persisted decision.
    pub decision_is_stale: bool,
    /// Whether a recoverable guarded rewrite remains incomplete.
    pub rewrite_pending: bool,
    /// Exact approved apply retry when a guarded rewrite remains incomplete.
    pub recovery_action: Option<NextAction>,
    /// Canonical state refresh when changed source bytes invalidate a decision.
    pub state_refresh_action: Option<NextAction>,
    /// Whether new durable Mino plans must be refused.
    pub blocks_durable_planning: bool,
    /// Stable reason for a durable-planning refusal.
    pub block_reason: Option<String>,
}

/// Deterministic rewrite proposal for one exact Planning Documents section.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PlanningAuthorityProposal {
    /// Stable authority proposal kind.
    pub proposal_kind: &'static str,
    /// Current authority revision to use for apply.
    pub authority_revision: u64,
    /// Digest of the exact source bytes inspected.
    pub source_digest: String,
    /// Digest of the complete proposed `AGENTS.md` replacement bytes.
    pub replacement_digest: String,
    /// First one-based line replaced.
    pub start_line: usize,
    /// Last one-based line replaced.
    pub end_line: usize,
    /// Exact replacement section text.
    pub replacement: String,
    /// Active legacy clauses covered by the replacement.
    pub detected_clauses: Vec<LegacyPlanningClause>,
}

/// Approval-bound request for coexistence or refusal.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlanningAuthorityDecisionRequest {
    /// Required authority revision.
    pub expected_revision: u64,
    /// Exact current `AGENTS.md` digest.
    pub expected_source_digest: String,
    /// Explicit coexistence or declined outcome.
    pub decision: PlanningAuthorityDecision,
    /// Idempotency identifier for this exact decision.
    pub request_id: RequestId,
    /// Actor recorded in the authority state.
    pub actor: String,
    /// Auditable approval reference.
    pub approval_reference: String,
    /// Complete canonical invoking command.
    pub command: Vec<String>,
    /// Timestamp captured once for the decision.
    pub decided_at: Timestamp,
}

/// Approval-bound request for the guarded supersession rewrite.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlanningAuthorityApplyRequest {
    /// Required authority revision.
    pub expected_revision: u64,
    /// Exact current source digest returned by proposal.
    pub expected_source_digest: String,
    /// Exact replacement digest returned by proposal.
    pub expected_replacement_digest: String,
    /// Explicit command-line confirmation of the external-file rewrite.
    pub is_confirmed: bool,
    /// Idempotency identifier for this exact apply.
    pub request_id: RequestId,
    /// Actor recorded in the authority state.
    pub actor: String,
    /// Auditable approval reference.
    pub approval_reference: String,
    /// Complete canonical invoking command.
    pub command: Vec<String>,
    /// Timestamp captured once for the decision.
    pub decided_at: Timestamp,
}

/// Result of one authority-state mutation or exact replay.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PlanningAuthorityMutationReport {
    /// Current authority status.
    #[serde(flatten)]
    pub status: PlanningAuthorityStatus,
    /// Whether the exact request had already completed.
    pub replayed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum AuthorityRequestKind {
    Decide,
    Apply,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthorityRequestAudit {
    kind: AuthorityRequestKind,
    expected_revision: u64,
    expected_source_digest: String,
    expected_replacement_digest: Option<String>,
    decision: PlanningAuthorityDecision,
    request_id: RequestId,
    actor: String,
    approval_reference: String,
    command: Vec<String>,
    decided_at: Timestamp,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthorityRewriteIntent {
    source_digest: String,
    replacement_digest: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthorityFile {
    kind: String,
    schema_version: u32,
    revision: u64,
    legacy_planning_rules_detected: bool,
    source_digest: String,
    detected_clauses: Vec<LegacyPlanningClause>,
    decision: PlanningAuthorityDecision,
    decided_by: Option<String>,
    decision_reference: Option<String>,
    decided_at: Option<Timestamp>,
    applied_rewrite_digest: Option<String>,
    pending_rewrite: Option<AuthorityRewriteIntent>,
    last_request: Option<AuthorityRequestAudit>,
}

impl AuthorityFile {
    fn pending(source: &AuthoritySource, revision: u64) -> Self {
        Self {
            kind: AUTHORITY_KIND.to_owned(),
            schema_version: AUTHORITY_SCHEMA_VERSION,
            revision,
            legacy_planning_rules_detected: !source.detection.clauses.is_empty(),
            source_digest: source.digest.clone(),
            detected_clauses: source.detection.clauses.clone(),
            decision: PlanningAuthorityDecision::Pending,
            decided_by: None,
            decision_reference: None,
            decided_at: None,
            applied_rewrite_digest: None,
            pending_rewrite: None,
            last_request: None,
        }
    }
}

#[derive(Clone, Debug)]
struct AuthoritySource {
    bytes: Vec<u8>,
    digest: String,
    detection: LegacyPlanningDetection,
    mino_workflow_active: bool,
}

#[derive(Clone, Debug)]
struct RewriteCandidate {
    report: PlanningAuthorityProposal,
    replacement: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AuthorityEnsureResult {
    pub(crate) path: PathBuf,
    pub(crate) created: bool,
}

/// Service for inspecting and resolving repository planning authority.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlanningAuthorityService {
    root: PathBuf,
}

impl PlanningAuthorityService {
    /// Discovers an initialized project and creates its authority service.
    ///
    /// # Errors
    ///
    /// Returns an environment error when the project cannot be discovered.
    pub fn discover(start: &Path) -> Result<Self, MinoError> {
        let project = super::discover(start)?;
        Ok(Self::new(project.path()))
    }

    #[must_use]
    pub(crate) fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Returns current detection, persisted decision, staleness, and gate facts.
    ///
    /// # Errors
    ///
    /// Returns a drift or environment error for unsafe, oversized, malformed,
    /// non-canonical, or unreadable source/state bytes.
    pub fn status(&self) -> Result<PlanningAuthorityStatus, MinoError> {
        let filesystem = filesystem(&self.root)?;
        let source = read_source(&filesystem)?;
        let state = load_file(&filesystem)?;
        Ok(build_status(source.as_ref(), state.as_ref()))
    }

    /// Builds the exact bounded Planning Documents replacement without writing.
    ///
    /// # Errors
    ///
    /// Returns a policy or drift error unless one active legacy planning section
    /// can be replaced deterministically.
    pub fn propose(&self) -> Result<PlanningAuthorityProposal, MinoError> {
        let filesystem = filesystem(&self.root)?;
        let source = required_source(read_source(&filesystem)?)?;
        let state = load_file(&filesystem)?;
        require_no_pending_rewrite(state.as_ref())?;
        let revision = state.as_ref().map_or(0, |file| file.revision);
        Ok(rewrite_candidate(&source, revision)?.report)
    }

    /// Records explicit coexistence or refusal against exact source bytes.
    ///
    /// # Errors
    ///
    /// Returns an approval, revision, policy, drift, or persistence error for an
    /// invalid/stale decision or unsafe authority state.
    pub fn decide(
        &self,
        request: PlanningAuthorityDecisionRequest,
    ) -> Result<PlanningAuthorityMutationReport, MinoError> {
        validate_decision_request(&request)?;
        let filesystem = filesystem(&self.root)?;
        let _lock = AuthorityLock::acquire(&filesystem)?;
        let source = required_source(read_source(&filesystem)?)?;
        let current = load_file(&filesystem)?;
        let audit = decision_audit(&request);
        if let Some(file) = &current
            && file.revision == request.expected_revision.saturating_add(1)
            && file
                .last_request
                .as_ref()
                .is_some_and(|persisted| same_authority_request(persisted, &audit))
        {
            return Ok(PlanningAuthorityMutationReport {
                status: build_status(Some(&source), Some(file)),
                replayed: true,
            });
        }
        require_no_pending_rewrite(current.as_ref())?;
        require_revision(current.as_ref(), request.expected_revision)?;
        require_source_digest(&source, &request.expected_source_digest)?;
        require_active_conflict(&source)?;
        let mut candidate =
            AuthorityFile::pending(&source, next_revision(request.expected_revision)?);
        candidate.decision = request.decision;
        candidate.decided_by = Some(request.actor);
        candidate.decision_reference = Some(request.approval_reference);
        candidate.decided_at = Some(request.decided_at);
        candidate.last_request = Some(audit);
        validate_file(&candidate)?;
        publish_file(&filesystem, &candidate)?;
        Ok(PlanningAuthorityMutationReport {
            status: build_status(Some(&source), Some(&candidate)),
            replayed: false,
        })
    }

    /// Applies the exact approved rewrite through the recoverable integration journal.
    ///
    /// # Errors
    ///
    /// Returns an approval, revision, policy, drift, interruption, or persistence
    /// error without accepting changed source or proposal bytes.
    pub fn apply(
        &self,
        request: &PlanningAuthorityApplyRequest,
    ) -> Result<PlanningAuthorityMutationReport, MinoError> {
        self.apply_internal(request, None)
    }

    /// Applies with one deterministic injected integration interruption.
    ///
    /// # Errors
    ///
    /// Returns the injected interruption or any normal apply error.
    pub fn apply_with_failure(
        &self,
        request: &PlanningAuthorityApplyRequest,
        failure_point: IntegrationFailurePoint,
    ) -> Result<PlanningAuthorityMutationReport, MinoError> {
        self.apply_internal(request, Some(failure_point))
    }

    fn apply_internal(
        &self,
        request: &PlanningAuthorityApplyRequest,
        failure_point: Option<IntegrationFailurePoint>,
    ) -> Result<PlanningAuthorityMutationReport, MinoError> {
        validate_apply_request(request)?;
        let filesystem = filesystem(&self.root)?;
        let _lock = AuthorityLock::acquire(&filesystem)?;
        recover_repository_transactions(&self.root)?;
        let mut current = load_file(&filesystem)?;
        let requested_audit = apply_audit(request);
        let source = required_source(read_source(&filesystem)?)?;
        if let Some(report) =
            completed_apply_report(current.as_ref(), &source, request, &requested_audit)?
        {
            return Ok(report);
        }
        let is_recovery = current.as_ref().is_some_and(|file| {
            file.revision == request.expected_revision.saturating_add(1)
                && file
                    .last_request
                    .as_ref()
                    .is_some_and(|persisted| same_authority_request(persisted, &requested_audit))
                && file.pending_rewrite.is_some()
        });
        if !is_recovery {
            require_revision(current.as_ref(), request.expected_revision)?;
            require_source_digest(&source, &request.expected_source_digest)?;
            require_active_conflict(&source)?;
            let rewrite = rewrite_candidate(&source, request.expected_revision)?;
            if rewrite.report.replacement_digest != request.expected_replacement_digest {
                return Err(drift(
                    "Authority proposal digest does not match the current rewrite",
                ));
            }
            let mut intent =
                AuthorityFile::pending(&source, next_revision(request.expected_revision)?);
            intent.pending_rewrite = Some(AuthorityRewriteIntent {
                source_digest: source.digest.clone(),
                replacement_digest: rewrite.report.replacement_digest,
            });
            intent.last_request = Some(requested_audit);
            validate_file(&intent)?;
            publish_file(&filesystem, &intent)?;
            current = Some(intent);
        }
        let mut intent_state =
            current.ok_or_else(|| drift("Authority rewrite intent is missing"))?;
        let audit = intent_state
            .last_request
            .clone()
            .ok_or_else(|| drift("Authority rewrite audit is missing"))?;
        let intent = intent_state
            .pending_rewrite
            .clone()
            .ok_or_else(|| drift("Authority rewrite intent is missing"))?;
        if intent.source_digest != request.expected_source_digest
            || intent.replacement_digest != request.expected_replacement_digest
        {
            return Err(drift(
                "Authority rewrite retry does not match the persisted intent",
            ));
        }
        let source = required_source(read_source(&filesystem)?)?;
        if source.digest == intent.source_digest {
            let rewrite = rewrite_candidate(&source, request.expected_revision)?;
            if rewrite.report.replacement_digest != intent.replacement_digest {
                return Err(drift(
                    "Persisted authority rewrite no longer matches its source",
                ));
            }
            guarded_agents_rewrite(
                &self.root,
                &source.bytes,
                &rewrite.replacement,
                failure_point,
            )?;
        } else if source.digest != intent.replacement_digest {
            return Err(drift(
                "AGENTS.md changed outside the guarded authority rewrite",
            ));
        }
        let published = required_source(read_source(&filesystem)?)?;
        if published.digest != intent.replacement_digest {
            return Err(drift(
                "Guarded authority rewrite did not publish the expected bytes",
            ));
        }
        intent_state.decision = PlanningAuthorityDecision::Superseded;
        intent_state.decided_by = Some(audit.actor.clone());
        intent_state.decision_reference = Some(audit.approval_reference.clone());
        intent_state.decided_at = Some(audit.decided_at.clone());
        intent_state.applied_rewrite_digest = Some(intent.replacement_digest);
        intent_state.pending_rewrite = None;
        intent_state.last_request = Some(audit);
        validate_file(&intent_state)?;
        publish_file(&filesystem, &intent_state)?;
        Ok(PlanningAuthorityMutationReport {
            status: build_status(Some(&published), Some(&intent_state)),
            replayed: false,
        })
    }
}

pub(crate) fn ensure_authority_state(
    root: &Path,
) -> Result<Option<AuthorityEnsureResult>, MinoError> {
    let filesystem = filesystem(root)?;
    let _lock = AuthorityLock::acquire(&filesystem)?;
    let Some(source) = read_source(&filesystem)? else {
        return Ok(None);
    };
    let current = load_file(&filesystem)?;
    let path = filesystem.display_path(&authority_path());
    let candidate = match current.as_ref() {
        None => Some(AuthorityFile::pending(&source, 1)),
        Some(file) if file.pending_rewrite.is_none() && file.source_digest != source.digest => {
            Some(AuthorityFile::pending(
                &source,
                next_revision(file.revision)?,
            ))
        }
        Some(_) => None,
    };
    if let Some(candidate) = candidate {
        validate_file(&candidate)?;
        publish_file(&filesystem, &candidate)?;
        return Ok(Some(AuthorityEnsureResult {
            path,
            created: current.is_none(),
        }));
    }
    Ok(Some(AuthorityEnsureResult {
        path,
        created: false,
    }))
}

pub(crate) fn require_durable_planning_authority(root: &Path) -> Result<(), MinoError> {
    let status = PlanningAuthorityService::new(root).status()?;
    if !status.blocks_durable_planning {
        return Ok(());
    }
    Err(MinoError::new(
        ErrorCategory::PolicyViolation,
        "Durable Mino plan creation is blocked by unresolved planning authority",
    )
    .with_remediation(
        vec![
            status
                .block_reason
                .clone()
                .unwrap_or_else(|| "planning_authority".to_owned()),
        ],
        vec![authority_status_action()],
    ))
}

pub(crate) fn authority_status_action() -> NextAction {
    NextAction {
        id: "project.authority.status".to_owned(),
        argv: vec![
            "mino".to_owned(),
            "project".to_owned(),
            "authority".to_owned(),
            "status".to_owned(),
            "--format".to_owned(),
            "json".to_owned(),
            "--no-input".to_owned(),
        ],
    }
}

fn build_status(
    source: Option<&AuthoritySource>,
    state: Option<&AuthorityFile>,
) -> PlanningAuthorityStatus {
    let decision = state.map_or(PlanningAuthorityDecision::Pending, |file| file.decision);
    let rewrite_pending = state.is_some_and(|file| file.pending_rewrite.is_some());
    let recovery_action = state.and_then(authority_apply_recovery_action);
    let decision_is_stale = state.is_some_and(|file| state_is_stale(file, source));
    let state_refresh_action = (decision_is_stale && !rewrite_pending && source.is_some())
        .then(authority_state_refresh_action);
    let legacy_detected = source.is_some_and(|source| !source.detection.clauses.is_empty());
    let mino_active = source.is_some_and(|source| source.mino_workflow_active);
    let block_reason = if rewrite_pending {
        Some("planning_authority_rewrite_pending".to_owned())
    } else if decision_is_stale {
        Some("planning_authority_decision_stale".to_owned())
    } else if decision == PlanningAuthorityDecision::Declined {
        Some("mino_durable_planning_declined".to_owned())
    } else if legacy_detected && mino_active && decision == PlanningAuthorityDecision::Pending {
        Some("legacy_planning_authority_conflict".to_owned())
    } else {
        None
    };
    PlanningAuthorityStatus {
        authority_kind: AUTHORITY_KIND,
        authority_revision: state.map_or(0, |file| file.revision),
        legacy_planning_rules_detected: legacy_detected,
        source_digest: source.map(|source| source.digest.clone()),
        detected_clauses: source.map_or_else(Vec::new, |source| source.detection.clauses.clone()),
        mino_workflow_active: mino_active,
        decision,
        decided_by: state.and_then(|file| file.decided_by.clone()),
        decision_reference: state.and_then(|file| file.decision_reference.clone()),
        decided_at: state.and_then(|file| file.decided_at.clone()),
        applied_rewrite_digest: state.and_then(|file| file.applied_rewrite_digest.clone()),
        decision_is_stale,
        rewrite_pending,
        recovery_action,
        state_refresh_action,
        blocks_durable_planning: block_reason.is_some(),
        block_reason,
    }
}

fn authority_state_refresh_action() -> NextAction {
    NextAction {
        id: "project.init".to_owned(),
        argv: vec![
            "mino".to_owned(),
            "project".to_owned(),
            "init".to_owned(),
            "--format".to_owned(),
            "json".to_owned(),
            "--no-input".to_owned(),
        ],
    }
}

fn authority_apply_recovery_action(file: &AuthorityFile) -> Option<NextAction> {
    file.pending_rewrite.as_ref()?;
    let audit = file
        .last_request
        .as_ref()
        .filter(|audit| audit.kind == AuthorityRequestKind::Apply)?;
    Some(NextAction {
        id: "project.authority.apply".to_owned(),
        argv: vec![
            "mino".to_owned(),
            "project".to_owned(),
            "authority".to_owned(),
            "apply".to_owned(),
            "--apply-rewrite".to_owned(),
            "--source-digest".to_owned(),
            audit.expected_source_digest.clone(),
            "--replacement-digest".to_owned(),
            audit.expected_replacement_digest.clone()?,
            "--expect-revision".to_owned(),
            audit.expected_revision.to_string(),
            "--request-id".to_owned(),
            audit.request_id.to_string(),
            "--approval-ref".to_owned(),
            audit.approval_reference.clone(),
            "--actor".to_owned(),
            audit.actor.clone(),
            "--format".to_owned(),
            "json".to_owned(),
            "--no-input".to_owned(),
        ],
    })
}

fn completed_apply_report(
    current: Option<&AuthorityFile>,
    source: &AuthoritySource,
    request: &PlanningAuthorityApplyRequest,
    audit: &AuthorityRequestAudit,
) -> Result<Option<PlanningAuthorityMutationReport>, MinoError> {
    let Some(file) = current.filter(|file| {
        file.revision == request.expected_revision.saturating_add(1)
            && file
                .last_request
                .as_ref()
                .is_some_and(|persisted| same_authority_request(persisted, audit))
            && file.decision == PlanningAuthorityDecision::Superseded
            && file.pending_rewrite.is_none()
    }) else {
        return Ok(None);
    };
    let status = build_status(Some(source), Some(file));
    if status.decision_is_stale {
        return Err(drift(
            "Applied authority rewrite no longer matches AGENTS.md",
        ));
    }
    Ok(Some(PlanningAuthorityMutationReport {
        status,
        replayed: true,
    }))
}

fn same_authority_request(left: &AuthorityRequestAudit, right: &AuthorityRequestAudit) -> bool {
    left.kind == right.kind
        && left.expected_revision == right.expected_revision
        && left.expected_source_digest == right.expected_source_digest
        && left.expected_replacement_digest == right.expected_replacement_digest
        && left.decision == right.decision
        && left.request_id == right.request_id
        && left.actor == right.actor
        && left.approval_reference == right.approval_reference
        && left.command == right.command
}

fn state_is_stale(file: &AuthorityFile, source: Option<&AuthoritySource>) -> bool {
    let Some(source) = source else {
        return true;
    };
    if let Some(intent) = &file.pending_rewrite {
        return source.digest != intent.source_digest && source.digest != intent.replacement_digest;
    }
    if file.decision == PlanningAuthorityDecision::Superseded {
        file.applied_rewrite_digest.as_deref() != Some(source.digest.as_str())
            || !source.detection.clauses.is_empty()
    } else {
        file.source_digest != source.digest
    }
}

fn rewrite_candidate(
    source: &AuthoritySource,
    authority_revision: u64,
) -> Result<RewriteCandidate, MinoError> {
    if source.detection.clauses.is_empty() {
        return Err(MinoError::new(
            ErrorCategory::PolicyViolation,
            "AGENTS.md contains no active legacy durable-planning clauses",
        ));
    }
    if source.detection.sections.len() != 1 {
        return Err(MinoError::new(
            ErrorCategory::PolicyViolation,
            "Authority rewrite requires exactly one Planning Documents section",
        ));
    }
    let section = &source.detection.sections[0];
    let text =
        std::str::from_utf8(&source.bytes).map_err(|_| drift("AGENTS.md is not valid UTF-8"))?;
    let line_ending = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let replacement_section = planning_supersession(line_ending);
    let mut replacement = Vec::with_capacity(
        source.bytes.len() - (section.end - section.start) + replacement_section.len(),
    );
    replacement.extend_from_slice(&source.bytes[..section.start]);
    replacement.extend_from_slice(replacement_section.as_bytes());
    replacement.extend_from_slice(&source.bytes[section.end..]);
    let replacement_digest = sha256_digest(&replacement);
    Ok(RewriteCandidate {
        report: PlanningAuthorityProposal {
            proposal_kind: "mino.planning-authority-proposal/v1",
            authority_revision,
            source_digest: source.digest.clone(),
            replacement_digest,
            start_line: section.start_line,
            end_line: section.end_line,
            replacement: replacement_section,
            detected_clauses: source.detection.clauses.clone(),
        },
        replacement,
    })
}

fn read_source(filesystem: &ProjectFs) -> Result<Option<AuthoritySource>, MinoError> {
    let path = agents_path();
    match filesystem.entry_kind(&path).map_err(managed_error)? {
        None => return Ok(None),
        Some(ManagedEntryKind::File) => {}
        Some(kind) => {
            return Err(drift(format!(
                "Planning authority source {} is {kind:?}, not a regular file",
                filesystem.display_path(&path).display()
            )));
        }
    }
    let bytes = filesystem
        .read_bounded(&path, MAX_AGENTS_BYTES)
        .map_err(managed_error)?;
    let text = std::str::from_utf8(&bytes).map_err(|error| {
        drift(format!(
            "Planning authority source {} is not UTF-8: {error}",
            filesystem.display_path(&path).display()
        ))
    })?;
    if text.contains('\0') {
        return Err(drift("Planning authority source contains NUL bytes"));
    }
    Ok(Some(AuthoritySource {
        digest: sha256_digest(&bytes),
        detection: detect_legacy_planning_authority(text),
        mino_workflow_active: agents_workflow_is_active(text),
        bytes,
    }))
}

fn required_source(source: Option<AuthoritySource>) -> Result<AuthoritySource, MinoError> {
    source.ok_or_else(|| {
        MinoError::new(
            ErrorCategory::PolicyViolation,
            "Planning authority requires a regular AGENTS.md file",
        )
    })
}

fn require_active_conflict(source: &AuthoritySource) -> Result<(), MinoError> {
    if source.detection.clauses.is_empty() || !source.mino_workflow_active {
        Err(MinoError::new(
            ErrorCategory::PolicyViolation,
            "Planning authority resolution requires active legacy clauses and the Mino workflow block",
        ))
    } else {
        Ok(())
    }
}

fn require_no_pending_rewrite(file: Option<&AuthorityFile>) -> Result<(), MinoError> {
    let Some(file) = file.filter(|file| file.pending_rewrite.is_some()) else {
        return Ok(());
    };
    let next_actions = authority_apply_recovery_action(file).into_iter().collect();
    Err(MinoError::new(
        ErrorCategory::PolicyViolation,
        "A pending planning-authority rewrite must be recovered before another decision",
    )
    .with_remediation(
        vec!["planning_authority_rewrite_pending".to_owned()],
        next_actions,
    ))
}

fn require_source_digest(source: &AuthoritySource, expected: &str) -> Result<(), MinoError> {
    if source.digest == expected {
        Ok(())
    } else {
        Err(drift("AGENTS.md digest changed after authority inspection"))
    }
}

fn validate_decision_request(request: &PlanningAuthorityDecisionRequest) -> Result<(), MinoError> {
    if !matches!(
        request.decision,
        PlanningAuthorityDecision::CoexistenceApproved | PlanningAuthorityDecision::Declined
    ) {
        return Err(MinoError::new(
            ErrorCategory::IncompleteOrValidation,
            "Authority decide accepts only coexistence-approved or declined",
        ));
    }
    validate_request_fields(
        &request.expected_source_digest,
        &request.actor,
        &request.approval_reference,
        &request.command,
    )
}

fn validate_apply_request(request: &PlanningAuthorityApplyRequest) -> Result<(), MinoError> {
    if !request.is_confirmed {
        return Err(MinoError::new(
            ErrorCategory::ApprovalRequired,
            "Authority apply requires the explicit --apply-rewrite flag",
        ));
    }
    validate_request_fields(
        &request.expected_source_digest,
        &request.actor,
        &request.approval_reference,
        &request.command,
    )?;
    if !is_sha256(&request.expected_replacement_digest) {
        return Err(MinoError::new(
            ErrorCategory::IncompleteOrValidation,
            "Authority apply requires a valid replacement digest",
        ));
    }
    Ok(())
}

fn validate_request_fields(
    source_digest: &str,
    actor: &str,
    approval_reference: &str,
    command: &[String],
) -> Result<(), MinoError> {
    if !is_sha256(source_digest)
        || actor.trim().is_empty()
        || approval_reference.trim().is_empty()
        || command.is_empty()
        || command.iter().any(|part| part.trim().is_empty())
    {
        return Err(MinoError::new(
            ErrorCategory::ApprovalRequired,
            "Authority mutation requires exact digests, actor, approval reference, and command",
        ));
    }
    Ok(())
}

fn decision_audit(request: &PlanningAuthorityDecisionRequest) -> AuthorityRequestAudit {
    AuthorityRequestAudit {
        kind: AuthorityRequestKind::Decide,
        expected_revision: request.expected_revision,
        expected_source_digest: request.expected_source_digest.clone(),
        expected_replacement_digest: None,
        decision: request.decision,
        request_id: request.request_id.clone(),
        actor: request.actor.clone(),
        approval_reference: request.approval_reference.clone(),
        command: request.command.clone(),
        decided_at: request.decided_at.clone(),
    }
}

fn apply_audit(request: &PlanningAuthorityApplyRequest) -> AuthorityRequestAudit {
    AuthorityRequestAudit {
        kind: AuthorityRequestKind::Apply,
        expected_revision: request.expected_revision,
        expected_source_digest: request.expected_source_digest.clone(),
        expected_replacement_digest: Some(request.expected_replacement_digest.clone()),
        decision: PlanningAuthorityDecision::Superseded,
        request_id: request.request_id.clone(),
        actor: request.actor.clone(),
        approval_reference: request.approval_reference.clone(),
        command: request.command.clone(),
        decided_at: request.decided_at.clone(),
    }
}

fn require_revision(file: Option<&AuthorityFile>, expected: u64) -> Result<(), MinoError> {
    let actual = file.map_or(0, |file| file.revision);
    if actual == expected {
        Ok(())
    } else {
        Err(MinoError::new(
            ErrorCategory::RevisionConflict,
            format!("Planning authority is revision {actual}, not expected revision {expected}"),
        ))
    }
}

fn next_revision(current: u64) -> Result<u64, MinoError> {
    current.checked_add(1).ok_or_else(|| {
        MinoError::new(
            ErrorCategory::RevisionConflict,
            "Planning authority revision overflowed",
        )
    })
}

fn validate_file(file: &AuthorityFile) -> Result<(), MinoError> {
    let mut clause_kinds = std::collections::BTreeSet::new();
    let clauses_valid = file
        .detected_clauses
        .iter()
        .all(|clause| clause.line > 0 && clause_kinds.insert(clause.kind));
    let decision_metadata = file.decided_by.as_deref().is_some_and(valid_text)
        && file.decision_reference.as_deref().is_some_and(valid_text)
        && file.decided_at.is_some();
    let decision_valid = match file.decision {
        PlanningAuthorityDecision::Pending => {
            file.decided_by.is_none()
                && file.decision_reference.is_none()
                && file.decided_at.is_none()
                && file.applied_rewrite_digest.is_none()
        }
        PlanningAuthorityDecision::Superseded => {
            decision_metadata
                && file
                    .applied_rewrite_digest
                    .as_deref()
                    .is_some_and(is_sha256)
                && file.pending_rewrite.is_none()
        }
        PlanningAuthorityDecision::CoexistenceApproved | PlanningAuthorityDecision::Declined => {
            decision_metadata && file.applied_rewrite_digest.is_none()
        }
    };
    let intent_valid = file.pending_rewrite.as_ref().is_none_or(|intent| {
        is_sha256(&intent.source_digest)
            && is_sha256(&intent.replacement_digest)
            && file.last_request.as_ref().is_some_and(|audit| {
                audit.kind == AuthorityRequestKind::Apply
                    && audit.expected_source_digest == intent.source_digest
                    && audit.expected_replacement_digest.as_deref()
                        == Some(intent.replacement_digest.as_str())
            })
    });
    let request_valid = file.last_request.as_ref().is_none_or(|audit| {
        audit.expected_revision.checked_add(1) == Some(file.revision)
            && is_sha256(&audit.expected_source_digest)
            && audit.expected_source_digest == file.source_digest
            && audit
                .expected_replacement_digest
                .as_deref()
                .is_none_or(is_sha256)
            && valid_text(&audit.actor)
            && valid_text(&audit.approval_reference)
            && !audit.command.is_empty()
            && audit.command.iter().all(|part| valid_text(part))
    });
    let decision_audit_valid = match file.decision {
        PlanningAuthorityDecision::Pending => match &file.pending_rewrite {
            Some(_) => file.last_request.as_ref().is_some_and(|audit| {
                audit.kind == AuthorityRequestKind::Apply
                    && audit.decision == PlanningAuthorityDecision::Superseded
            }),
            None => file.last_request.is_none(),
        },
        PlanningAuthorityDecision::Superseded => file.last_request.as_ref().is_some_and(|audit| {
            audit.kind == AuthorityRequestKind::Apply
                && audit.decision == file.decision
                && audit.expected_replacement_digest == file.applied_rewrite_digest
                && audit_matches_decision_metadata(audit, file)
        }),
        PlanningAuthorityDecision::CoexistenceApproved | PlanningAuthorityDecision::Declined => {
            file.pending_rewrite.is_none()
                && file.last_request.as_ref().is_some_and(|audit| {
                    audit.kind == AuthorityRequestKind::Decide
                        && audit.decision == file.decision
                        && audit.expected_replacement_digest.is_none()
                        && audit_matches_decision_metadata(audit, file)
                })
        }
    };
    if file.kind != AUTHORITY_KIND
        || file.schema_version != AUTHORITY_SCHEMA_VERSION
        || file.revision == 0
        || file.legacy_planning_rules_detected == file.detected_clauses.is_empty()
        || !is_sha256(&file.source_digest)
        || !clauses_valid
        || !decision_valid
        || !intent_valid
        || !request_valid
        || !decision_audit_valid
    {
        return Err(drift(
            "Planning authority state is malformed or unsupported",
        ));
    }
    Ok(())
}

fn audit_matches_decision_metadata(audit: &AuthorityRequestAudit, file: &AuthorityFile) -> bool {
    file.decided_by.as_deref() == Some(audit.actor.as_str())
        && file.decision_reference.as_deref() == Some(audit.approval_reference.as_str())
        && file.decided_at.as_ref() == Some(&audit.decided_at)
}

fn load_file(filesystem: &ProjectFs) -> Result<Option<AuthorityFile>, MinoError> {
    let path = authority_path();
    if !filesystem.exists(&path).map_err(managed_error)? {
        return Ok(None);
    }
    let bytes = filesystem
        .read_bounded(&path, MAX_AUTHORITY_BYTES)
        .map_err(managed_error)?;
    let file = serde_json::from_slice::<AuthorityFile>(&bytes).map_err(|error| {
        drift(format!(
            "Failed to parse planning authority {}: {error}",
            filesystem.display_path(&path).display()
        ))
    })?;
    validate_file(&file)?;
    if encode_file(&file)? != bytes {
        return Err(drift(format!(
            "Planning authority {} is not canonical",
            filesystem.display_path(&path).display()
        )));
    }
    Ok(Some(file))
}

fn publish_file(filesystem: &ProjectFs, file: &AuthorityFile) -> Result<(), MinoError> {
    let path = authority_path();
    let bytes = encode_file(file)?;
    let existing_kind = filesystem.entry_kind(&path).map_err(managed_error)?;
    if existing_kind.is_some_and(|kind| kind != ManagedEntryKind::File) {
        return Err(drift(format!(
            "Planning authority {} is not a regular file",
            filesystem.display_path(&path).display()
        )));
    }
    let parent = path
        .parent()
        .ok_or_else(|| drift("Planning authority path has no parent"))?;
    let sequence = NEXT_AUTHORITY_FILE.fetch_add(1, Ordering::Relaxed);
    let temporary = parent
        .join(format!(
            ".authority.json.mino-authority-{}-{sequence}.tmp",
            std::process::id()
        ))
        .map_err(managed_error)?;
    let backup = parent
        .join(format!(
            ".authority.json.mino-authority-{}-{sequence}.bak",
            std::process::id()
        ))
        .map_err(managed_error)?;
    write_new_file(filesystem, &temporary, &bytes)?;
    if existing_kind.is_none() {
        if let Err(error) = filesystem.rename(&temporary, &path) {
            let _ = filesystem.remove_file_if_exists(&temporary);
            return Err(managed_error(error));
        }
        return filesystem.sync_parent(&path).map_err(managed_error);
    }
    if let Err(error) = filesystem.rename(&path, &backup) {
        let _ = filesystem.remove_file_if_exists(&temporary);
        return Err(managed_error(error));
    }
    if let Err(error) = filesystem.rename(&temporary, &path) {
        let restoration = filesystem.rename(&backup, &path);
        let _ = filesystem.remove_file_if_exists(&temporary);
        return match restoration {
            Ok(()) => Err(managed_error(error)),
            Err(restoration_error) => Err(MinoError::new(
                ErrorCategory::EnvironmentUnavailable,
                format!(
                    "Failed to publish planning authority ({error}) and restore its backup ({restoration_error})"
                ),
            )),
        };
    }
    filesystem.remove_file(&backup).map_err(managed_error)?;
    filesystem.sync_parent(&path).map_err(managed_error)
}

fn encode_file(file: &AuthorityFile) -> Result<Vec<u8>, MinoError> {
    let mut bytes = serde_json::to_vec_pretty(file).map_err(|error| {
        MinoError::new(
            ErrorCategory::EnvironmentUnavailable,
            format!("Failed to encode planning authority: {error}"),
        )
    })?;
    bytes.push(b'\n');
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_AUTHORITY_BYTES {
        return Err(MinoError::new(
            ErrorCategory::PolicyViolation,
            "Planning authority state exceeds its stable size limit",
        ));
    }
    Ok(bytes)
}

fn write_new_file(
    filesystem: &ProjectFs,
    path: &ManagedPath,
    bytes: &[u8],
) -> Result<(), MinoError> {
    let mut file = filesystem.create_new_file(path).map_err(managed_error)?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| {
            MinoError::new(
                ErrorCategory::EnvironmentUnavailable,
                format!(
                    "Failed to write planning authority {}: {error}",
                    filesystem.display_path(path).display()
                ),
            )
        })
}

struct AuthorityLock {
    file: std::fs::File,
}

impl AuthorityLock {
    fn acquire(filesystem: &ProjectFs) -> Result<Self, MinoError> {
        let path = authority_lock_path();
        let file = filesystem.open_lock_file(&path).map_err(managed_error)?;
        let started_at = Instant::now();
        loop {
            match FileExt::try_lock(&file) {
                Ok(()) => return Ok(Self { file }),
                Err(TryLockError::WouldBlock) if started_at.elapsed() < LOCK_TIMEOUT => {
                    thread::sleep(LOCK_RETRY);
                }
                Err(TryLockError::WouldBlock) => {
                    return Err(MinoError::new(
                        ErrorCategory::EnvironmentUnavailable,
                        format!(
                            "Timed out acquiring planning authority lock {}",
                            filesystem.display_path(&path).display()
                        ),
                    ));
                }
                Err(TryLockError::Error(error)) => {
                    return Err(MinoError::new(
                        ErrorCategory::EnvironmentUnavailable,
                        format!(
                            "Failed to lock planning authority {}: {error}",
                            filesystem.display_path(&path).display()
                        ),
                    ));
                }
            }
        }
    }
}

impl Drop for AuthorityLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

fn filesystem(root: &Path) -> Result<ProjectFs, MinoError> {
    ProjectFs::open(root).map_err(managed_error)
}

fn authority_path() -> ManagedPath {
    ProjectLayout::authority_managed()
}

fn authority_lock_path() -> ManagedPath {
    ManagedPath::new(".mino/authority.lock").expect("static authority lock path should be valid")
}

fn agents_path() -> ManagedPath {
    ManagedPath::new("AGENTS.md").expect("static AGENTS path should be valid")
}

fn is_sha256(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("sha256:")
        && value["sha256:".len()..]
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

fn valid_text(value: &str) -> bool {
    !value.trim().is_empty() && !value.contains('\0')
}

fn managed_error(error: ManagedFsError) -> MinoError {
    let category = match error.kind() {
        ManagedFsErrorKind::InvalidPath | ManagedFsErrorKind::UnsafeComponent => {
            ErrorCategory::DriftDetected
        }
        ManagedFsErrorKind::Io => ErrorCategory::EnvironmentUnavailable,
    };
    MinoError::new(category, error.into_message())
}

fn drift(message: impl Into<String>) -> MinoError {
    MinoError::new(ErrorCategory::DriftDetected, message)
}
