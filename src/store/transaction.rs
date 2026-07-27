//! Revision commits, write-ahead recovery, idempotency, and audits.

use std::collections::BTreeSet;
use std::io::{BufReader, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::domain::{DomainError, Event, EventResult, Plan, PlanId, RequestId, Timestamp};
use crate::managed_fs::{
    ManagedEntryKind, ManagedFsError, ManagedPath, ProjectFs, read_bounded_line,
};

use super::canonical::{canonical_json_bytes, sha256_digest};
use super::lock::PlanLock;
use super::{LockOptions, StoreError, StoreErrorKind, StorePaths};

const TRANSACTION_JOURNAL_VERSION: u32 = 1;
const MAX_PLAN_STATE_BYTES: u64 = 8 * 1_024 * 1_024;
const MAX_EVENT_RECORD_BYTES: usize = 1_024 * 1_024;

/// Publication boundaries supported by deterministic failure injection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FailurePoint {
    /// Interrupt before the durable write-ahead journal is published.
    BeforeJournal,
    /// Interrupt after the durable write-ahead journal is published.
    AfterJournal,
    /// Interrupt before the immutable snapshot is published.
    BeforeSnapshot,
    /// Interrupt after the immutable snapshot is published.
    AfterSnapshot,
    /// Interrupt before the append-only event is published.
    BeforeEvent,
    /// Interrupt after the append-only event is published.
    AfterEvent,
    /// Interrupt before the prior plan is moved to the transaction backup.
    BeforePlanBackup,
    /// Interrupt after the prior plan is moved to the transaction backup.
    AfterPlanBackup,
    /// Interrupt before the new current plan is published.
    BeforePlanPublish,
    /// Interrupt after the new current plan is published.
    AfterPlanPublish,
}

/// Optional deterministic controls for a single storage commit.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CommitOptions {
    failure_point: Option<FailurePoint>,
}

impl CommitOptions {
    /// Creates options that interrupt at one publication boundary.
    #[must_use]
    pub const fn fail_at(failure_point: FailurePoint) -> Self {
        Self {
            failure_point: Some(failure_point),
        }
    }

    fn inject(self, point: FailurePoint) -> Result<(), StoreError> {
        if self.failure_point == Some(point) {
            Err(StoreError::new(
                StoreErrorKind::InjectedFailure,
                format!("Injected storage interruption at {point:?}"),
            ))
        } else {
            Ok(())
        }
    }
}

/// Validated metadata accompanying one expected-revision domain mutation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MutationRequest {
    expected_revision: u64,
    request_id: RequestId,
    actor: String,
    command: Vec<String>,
    changed_fields: Vec<String>,
}

impl MutationRequest {
    /// Creates validated mutation metadata.
    ///
    /// # Errors
    ///
    /// Returns an error when the actor or command is empty.
    pub fn new(
        expected_revision: u64,
        request_id: RequestId,
        actor: impl Into<String>,
        command: Vec<String>,
        changed_fields: Vec<String>,
    ) -> Result<Self, StoreError> {
        let actor = actor.into();
        validate_request(&actor, &command)?;
        validate_changed_fields(&changed_fields)?;
        Ok(Self {
            expected_revision,
            request_id,
            actor,
            command,
            changed_fields,
        })
    }

    /// Returns the optimistic-concurrency revision supplied by the caller.
    #[must_use]
    pub const fn expected_revision(&self) -> u64 {
        self.expected_revision
    }

    /// Returns the idempotency identifier supplied by the caller.
    #[must_use]
    pub const fn request_id(&self) -> &RequestId {
        &self.request_id
    }
}

/// Durable result of a successful or idempotently replayed commit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommitReceipt {
    plan_id: PlanId,
    revision: u64,
    event_sequence: u64,
    state_hash: String,
    snapshot_digest: String,
    is_replay: bool,
}

impl CommitReceipt {
    /// Returns the committed plan identifier.
    #[must_use]
    pub const fn plan_id(&self) -> &PlanId {
        &self.plan_id
    }

    /// Returns the committed revision.
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// Returns the append-only event sequence.
    #[must_use]
    pub const fn event_sequence(&self) -> u64 {
        self.event_sequence
    }

    /// Returns the digest of the canonical current state.
    #[must_use]
    pub fn state_hash(&self) -> &str {
        &self.state_hash
    }

    /// Returns the digest of the immutable snapshot bytes.
    #[must_use]
    pub fn snapshot_digest(&self) -> &str {
        &self.snapshot_digest
    }

    /// Returns whether an earlier result was replayed without mutation.
    #[must_use]
    pub const fn is_replay(&self) -> bool {
        self.is_replay
    }

    fn from_event(plan_id: PlanId, event: &Event, is_replay: bool) -> Self {
        Self {
            plan_id,
            revision: event.revision_after,
            event_sequence: event.sequence,
            state_hash: event.state_hash.clone(),
            snapshot_digest: event.snapshot_digest.clone(),
            is_replay,
        }
    }
}

/// Result of checking and recovering one per-plan transaction journal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RecoveryReport {
    was_recovered: bool,
    revision: u64,
}

impl RecoveryReport {
    /// Returns whether a prepared transaction was completed.
    #[must_use]
    pub const fn was_recovered(self) -> bool {
        self.was_recovered
    }

    /// Returns the current revision after recovery.
    #[must_use]
    pub const fn revision(self) -> u64 {
        self.revision
    }
}

/// Verified summary of current state, events, and immutable snapshots.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoreAudit {
    revision: u64,
    event_count: usize,
    snapshot_count: usize,
    state_hash: String,
}

impl StoreAudit {
    /// Returns the audited current revision.
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// Returns the number of verified append-only events.
    #[must_use]
    pub const fn event_count(&self) -> usize {
        self.event_count
    }

    /// Returns the number of verified immutable snapshots.
    #[must_use]
    pub const fn snapshot_count(&self) -> usize {
        self.snapshot_count
    }

    /// Returns the digest of the audited current state.
    #[must_use]
    pub fn state_hash(&self) -> &str {
        &self.state_hash
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TransactionJournal {
    version: u32,
    plan_id: PlanId,
    request_id: RequestId,
    expected_revision: u64,
    target_revision: u64,
    previous_state_hash: Option<String>,
    state_hash: String,
    snapshot_digest: String,
    event: Event,
}

/// Project-local store for revisioned plan aggregates.
#[derive(Clone, Debug)]
pub struct PlanStore {
    paths: StorePaths,
    lock_options: LockOptions,
    filesystem: Result<ProjectFs, ManagedFsError>,
}

impl PlanStore {
    /// Creates a store with the default bounded-lock policy.
    #[must_use]
    pub fn new(project_root: impl Into<std::path::PathBuf>) -> Self {
        let project_root = project_root.into();
        let filesystem = ProjectFs::open(&project_root);
        let layout_root = filesystem.as_ref().map_or_else(
            |_| project_root.clone(),
            |filesystem| filesystem.root().to_path_buf(),
        );
        Self {
            paths: StorePaths::new(layout_root),
            lock_options: LockOptions::default(),
            filesystem,
        }
    }

    /// Creates a store with an explicit bounded-lock policy.
    #[must_use]
    pub fn with_lock_options(
        project_root: impl Into<std::path::PathBuf>,
        lock_options: LockOptions,
    ) -> Self {
        let project_root = project_root.into();
        let filesystem = ProjectFs::open(&project_root);
        let layout_root = filesystem.as_ref().map_or_else(
            |_| project_root.clone(),
            |filesystem| filesystem.root().to_path_buf(),
        );
        Self {
            paths: StorePaths::new(layout_root),
            lock_options,
            filesystem,
        }
    }

    /// Returns the deterministic path resolver used by this store.
    #[must_use]
    pub const fn paths(&self) -> &StorePaths {
        &self.paths
    }

    /// Persists a new revision-one plan with its first event and snapshot.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid plans, duplicate identifiers, reused request
    /// identifiers, lock failures, or filesystem failures.
    pub fn create_plan(
        &self,
        plan: &Plan,
        request_id: RequestId,
        actor: impl Into<String>,
        command: Vec<String>,
    ) -> Result<CommitReceipt, StoreError> {
        if plan.revision() != 1 {
            return Err(StoreError::new(
                StoreErrorKind::InvalidMutation,
                "A newly persisted plan must be revision one",
            ));
        }
        plan.validate_invariants()?;
        let plan_id = plan.id().clone();
        let request = MutationRequest::new(0, request_id, actor, command, vec!["*".to_owned()])?;
        self.prepare_plan_directories(&plan_id)?;
        let _lock = PlanLock::acquire(
            self.filesystem()?,
            &self.paths.lock_managed(&plan_id),
            self.lock_options,
        )?;
        self.recover_locked(&plan_id)?;
        let events = self.read_events(&plan_id)?;
        if let Some(receipt) = replay_receipt(
            &plan_id,
            &events,
            &request.request_id,
            request.expected_revision,
            &request.actor,
            &request.command,
            &request.changed_fields,
        )? {
            let proposed_bytes = canonical_json_bytes(plan)?;
            if sha256_digest(&proposed_bytes) != receipt.state_hash {
                return Err(StoreError::new(
                    StoreErrorKind::RequestConflict,
                    format!(
                        "Request {} was reused with different initial plan bytes",
                        request.request_id
                    ),
                ));
            }
            return Ok(receipt);
        }
        if self.managed_exists(&self.paths.current_plan_managed(&plan_id))? {
            return Err(StoreError::new(
                StoreErrorKind::PlanAlreadyExists,
                format!("Plan {plan_id} already exists"),
            ));
        }
        self.persist_transaction(plan, request, &events, CommitOptions::default())
    }

    /// Applies and durably commits one semantic domain mutation.
    ///
    /// # Errors
    ///
    /// Returns an error for stale revisions, request conflicts, illegal domain
    /// transitions, lock failures, corruption, or filesystem failures.
    pub fn commit<F>(
        &self,
        plan_id: &PlanId,
        request: MutationRequest,
        mutation: F,
    ) -> Result<CommitReceipt, StoreError>
    where
        F: FnOnce(&mut Plan) -> Result<(), DomainError>,
    {
        self.commit_with_options(plan_id, request, CommitOptions::default(), mutation)
    }

    /// Verifies and returns an already committed idempotent mutation.
    ///
    /// # Errors
    ///
    /// Returns an error when the request was not committed, its identity was
    /// reused with different inputs, or the plan store cannot be recovered.
    pub fn replay(
        &self,
        plan_id: &PlanId,
        request: &MutationRequest,
    ) -> Result<CommitReceipt, StoreError> {
        self.require_plan_directory(plan_id)?;
        let _lock = PlanLock::acquire(
            self.filesystem()?,
            &self.paths.lock_managed(plan_id),
            self.lock_options,
        )?;
        self.recover_locked(plan_id)?;
        let events = self.read_events(plan_id)?;
        replay_receipt(
            plan_id,
            &events,
            &request.request_id,
            request.expected_revision,
            &request.actor,
            &request.command,
            &request.changed_fields,
        )?
        .ok_or_else(|| {
            StoreError::new(
                StoreErrorKind::InvalidMutation,
                format!("Request {} has not been committed", request.request_id),
            )
        })
    }

    /// Applies a semantic mutation with an optional injected publication failure.
    ///
    /// # Errors
    ///
    /// Returns the same failures as [`Self::commit`], including the configured
    /// deterministic injected failure.
    pub fn commit_with_options<F>(
        &self,
        plan_id: &PlanId,
        request: MutationRequest,
        options: CommitOptions,
        mutation: F,
    ) -> Result<CommitReceipt, StoreError>
    where
        F: FnOnce(&mut Plan) -> Result<(), DomainError>,
    {
        self.require_plan_directory(plan_id)?;
        let _lock = PlanLock::acquire(
            self.filesystem()?,
            &self.paths.lock_managed(plan_id),
            self.lock_options,
        )?;
        self.recover_locked(plan_id)?;
        let events = self.read_events(plan_id)?;
        if let Some(receipt) = replay_receipt(
            plan_id,
            &events,
            &request.request_id,
            request.expected_revision,
            &request.actor,
            &request.command,
            &request.changed_fields,
        )? {
            return Ok(receipt);
        }
        let current_plan = self.read_current_plan(plan_id)?;
        if current_plan.is_archived() {
            return Err(StoreError::new(
                StoreErrorKind::InvalidMutation,
                format!("Plan {plan_id} is archived and cannot be mutated"),
            ));
        }
        if current_plan.revision() != request.expected_revision {
            return Err(StoreError::new(
                StoreErrorKind::StaleRevision,
                format!(
                    "Plan {plan_id} is revision {}, not expected revision {}",
                    current_plan.revision(),
                    request.expected_revision
                ),
            ));
        }
        let mut next_plan = current_plan;
        mutation(&mut next_plan)?;
        let target_revision = request.expected_revision.checked_add(1).ok_or_else(|| {
            StoreError::new(StoreErrorKind::InvalidMutation, "Plan revision overflowed")
        })?;
        if next_plan.id() != plan_id || next_plan.revision() != target_revision {
            return Err(StoreError::new(
                StoreErrorKind::InvalidMutation,
                "A committed semantic mutation must retain the plan ID and advance one revision",
            ));
        }
        next_plan.validate_invariants()?;
        self.persist_transaction(&next_plan, request, &events, options)
    }

    /// Loads the current plan after completing any prepared transaction.
    ///
    /// # Errors
    ///
    /// Returns an error when the plan is missing, locked, corrupt, or unreadable.
    pub fn load_plan(&self, plan_id: &PlanId) -> Result<Plan, StoreError> {
        self.require_plan_directory(plan_id)?;
        let _lock = PlanLock::acquire(
            self.filesystem()?,
            &self.paths.lock_managed(plan_id),
            self.lock_options,
        )?;
        self.recover_locked(plan_id)?;
        self.read_current_plan(plan_id)
    }

    /// Loads and verifies one immutable historical plan snapshot.
    ///
    /// # Errors
    ///
    /// Returns an error when the plan or revision is missing, storage is locked
    /// or corrupt, or snapshot bytes are not canonical for the requested identity.
    pub fn load_snapshot(&self, plan_id: &PlanId, revision: u64) -> Result<Plan, StoreError> {
        self.require_plan_directory(plan_id)?;
        let _lock = PlanLock::acquire(
            self.filesystem()?,
            &self.paths.lock_managed(plan_id),
            self.lock_options,
        )?;
        self.recover_locked(plan_id)?;
        let path = self.paths.snapshot_managed(plan_id, revision);
        let display_path = self.filesystem()?.display_path(&path);
        if !self.managed_exists(&path)? {
            return Err(StoreError::new(
                StoreErrorKind::PlanNotFound,
                format!(
                    "Plan {plan_id} revision {revision} does not exist at {}",
                    display_path.display()
                ),
            ));
        }
        let bytes = self.read_managed(&path).map_err(|error| {
            StoreError::new(
                error.kind(),
                format!(
                    "Failed to read plan {plan_id} revision {revision} at {}: {error}",
                    display_path.display()
                ),
            )
        })?;
        let plan: Plan = serde_json::from_slice(&bytes)?;
        if plan.id() != plan_id
            || plan.revision() != revision
            || canonical_json_bytes(&plan)? != bytes
        {
            return Err(StoreError::new(
                StoreErrorKind::CorruptState,
                format!(
                    "Snapshot {revision} for plan {plan_id} is not canonical or has the wrong identity"
                ),
            ));
        }
        Ok(plan)
    }

    /// Loads the verified append-only event sequence after recovery.
    ///
    /// # Errors
    ///
    /// Returns an error when the plan is missing, locked, corrupt, or unreadable.
    pub fn events(&self, plan_id: &PlanId) -> Result<Vec<Event>, StoreError> {
        self.require_plan_directory(plan_id)?;
        let _lock = PlanLock::acquire(
            self.filesystem()?,
            &self.paths.lock_managed(plan_id),
            self.lock_options,
        )?;
        self.recover_locked(plan_id)?;
        self.read_events(plan_id)
    }

    /// Completes a prepared transaction and reports the resulting revision.
    ///
    /// # Errors
    ///
    /// Returns an error when recovery cannot prove a single complete revision.
    pub fn recover(&self, plan_id: &PlanId) -> Result<RecoveryReport, StoreError> {
        self.require_plan_directory(plan_id)?;
        let _lock = PlanLock::acquire(
            self.filesystem()?,
            &self.paths.lock_managed(plan_id),
            self.lock_options,
        )?;
        let was_recovered = self.recover_locked(plan_id)?;
        let plan = self.read_current_plan(plan_id)?;
        Ok(RecoveryReport {
            was_recovered,
            revision: plan.revision(),
        })
    }

    /// Verifies current state, event continuity, snapshots, and all recorded digests.
    ///
    /// # Errors
    ///
    /// Returns an error for any missing, extra, malformed, or digest-mismatched artifact.
    pub fn audit(&self, plan_id: &PlanId) -> Result<StoreAudit, StoreError> {
        self.require_plan_directory(plan_id)?;
        let _lock = PlanLock::acquire(
            self.filesystem()?,
            &self.paths.lock_managed(plan_id),
            self.lock_options,
        )?;
        self.recover_locked(plan_id)?;
        self.audit_locked(plan_id)
    }

    fn persist_transaction(
        &self,
        plan: &Plan,
        request: MutationRequest,
        existing_events: &[Event],
        options: CommitOptions,
    ) -> Result<CommitReceipt, StoreError> {
        let MutationRequest {
            expected_revision,
            request_id,
            actor,
            command,
            changed_fields,
        } = request;
        let plan_id = plan.id().clone();
        let plan_bytes = canonical_json_bytes(plan)?;
        require_generated_size(&plan_bytes, MAX_PLAN_STATE_BYTES, "Plan state")?;
        let state_hash = sha256_digest(&plan_bytes);
        let previous_state_hash = if expected_revision == 0 {
            None
        } else {
            Some(sha256_digest(
                &self.read_managed(&self.paths.current_plan_managed(&plan_id))?,
            ))
        };
        let sequence = u64::try_from(existing_events.len())
            .ok()
            .and_then(|length| length.checked_add(1))
            .ok_or_else(|| {
                StoreError::new(StoreErrorKind::InvalidMutation, "Event sequence overflowed")
            })?;
        let event = Event {
            sequence,
            timestamp: Timestamp::now_utc(),
            actor,
            command,
            request_id: request_id.clone(),
            revision_before: expected_revision,
            revision_after: plan.revision(),
            changed_fields,
            result: EventResult::Succeeded,
            state_hash: state_hash.clone(),
            snapshot_digest: state_hash.clone(),
        };
        require_generated_size(
            &canonical_json_bytes(&event)?,
            u64::try_from(MAX_EVENT_RECORD_BYTES).unwrap_or(u64::MAX),
            "Event record",
        )?;
        let journal = TransactionJournal {
            version: TRANSACTION_JOURNAL_VERSION,
            plan_id: plan_id.clone(),
            request_id,
            expected_revision,
            target_revision: plan.revision(),
            previous_state_hash,
            state_hash: state_hash.clone(),
            snapshot_digest: state_hash,
            event: event.clone(),
        };
        options.inject(FailurePoint::BeforeJournal)?;
        self.prepare_transaction(&journal, &plan_bytes)?;
        self.publish_transaction(&journal, options)?;
        Ok(CommitReceipt::from_event(plan_id, &event, false))
    }

    fn prepare_transaction(
        &self,
        journal: &TransactionJournal,
        plan_bytes: &[u8],
    ) -> Result<(), StoreError> {
        let transaction_directory = self.paths.transaction_managed(&journal.plan_id);
        if self.managed_exists(&transaction_directory)? {
            return Err(StoreError::new(
                StoreErrorKind::CorruptState,
                format!(
                    "Transaction directory {} remained after recovery",
                    self.filesystem()?
                        .display_path(&transaction_directory)
                        .display()
                ),
            ));
        }
        self.filesystem()?
            .ensure_directory(&transaction_directory)
            .map_err(managed_store_error)?;
        self.write_new_file(&self.paths.next_plan_managed(&journal.plan_id), plan_bytes)?;
        let journal_bytes = canonical_json_bytes(journal)?;
        require_generated_size(&journal_bytes, MAX_PLAN_STATE_BYTES, "Transaction journal")?;
        self.write_new_file(
            &self.paths.pending_journal_managed(&journal.plan_id),
            &journal_bytes,
        )?;
        self.filesystem()?
            .rename(
                &self.paths.pending_journal_managed(&journal.plan_id),
                &self.paths.journal_managed(&journal.plan_id),
            )
            .map_err(managed_store_error)?;
        self.filesystem()?
            .sync_directory(Some(&transaction_directory))
            .map_err(managed_store_error)?;
        self.filesystem()?
            .sync_directory(Some(&self.paths.plan_managed(&journal.plan_id)))
            .map_err(managed_store_error)?;
        Ok(())
    }

    fn publish_transaction(
        &self,
        journal: &TransactionJournal,
        options: CommitOptions,
    ) -> Result<(), StoreError> {
        options.inject(FailurePoint::AfterJournal)?;
        options.inject(FailurePoint::BeforeSnapshot)?;
        let plan_bytes = self.transaction_plan_bytes(journal)?;
        self.publish_snapshot(journal, &plan_bytes)?;
        options.inject(FailurePoint::AfterSnapshot)?;
        options.inject(FailurePoint::BeforeEvent)?;
        self.repair_partial_event_tail(&journal.plan_id)?;
        self.publish_event(journal)?;
        options.inject(FailurePoint::AfterEvent)?;
        options.inject(FailurePoint::BeforePlanBackup)?;
        self.backup_current_plan(journal)?;
        options.inject(FailurePoint::AfterPlanBackup)?;
        options.inject(FailurePoint::BeforePlanPublish)?;
        self.publish_next_plan(journal)?;
        options.inject(FailurePoint::AfterPlanPublish)?;
        self.cleanup_transaction(journal)?;
        Ok(())
    }

    fn recover_locked(&self, plan_id: &PlanId) -> Result<bool, StoreError> {
        let transaction_directory = self.paths.transaction_managed(plan_id);
        if !self.managed_exists(&transaction_directory)? {
            return Ok(false);
        }
        let journal_path = self.paths.journal_managed(plan_id);
        if !self.managed_exists(&journal_path)? {
            self.cleanup_uncommitted_preparation(plan_id)?;
            return Ok(false);
        }
        let journal: TransactionJournal =
            serde_json::from_slice(&self.read_managed(&journal_path)?)?;
        self.validate_journal(plan_id, &journal)?;
        self.publish_transaction(&journal, CommitOptions::default())?;
        Ok(true)
    }

    fn validate_journal(
        &self,
        plan_id: &PlanId,
        journal: &TransactionJournal,
    ) -> Result<(), StoreError> {
        let event = &journal.event;
        if journal.version != TRANSACTION_JOURNAL_VERSION
            || &journal.plan_id != plan_id
            || journal.request_id != event.request_id
            || journal.target_revision != journal.expected_revision.saturating_add(1)
            || journal.target_revision != event.revision_after
            || journal.expected_revision != event.revision_before
            || journal.state_hash != event.state_hash
            || journal.snapshot_digest != event.snapshot_digest
            || journal.state_hash != journal.snapshot_digest
            || event.result != EventResult::Succeeded
        {
            return Err(StoreError::new(
                StoreErrorKind::CorruptState,
                format!("Transaction journal for plan {plan_id} is internally inconsistent"),
            ));
        }
        let plan_bytes = self.transaction_plan_bytes(journal)?;
        let plan: Plan = serde_json::from_slice(&plan_bytes)?;
        if plan.id() != plan_id || plan.revision() != journal.target_revision {
            return Err(StoreError::new(
                StoreErrorKind::CorruptState,
                format!("Transaction state for plan {plan_id} has the wrong identity or revision"),
            ));
        }
        Ok(())
    }

    fn transaction_plan_bytes(&self, journal: &TransactionJournal) -> Result<Vec<u8>, StoreError> {
        let next_path = self.paths.next_plan_managed(&journal.plan_id);
        let bytes = if self.managed_exists(&next_path)? {
            self.read_managed(&next_path)?
        } else {
            self.read_managed(&self.paths.current_plan_managed(&journal.plan_id))
                .map_err(|error| {
                    StoreError::new(
                        StoreErrorKind::CorruptState,
                        format!(
                            "Transaction for plan {} lost both next and current state: {error}",
                            journal.plan_id
                        ),
                    )
                })?
        };
        if sha256_digest(&bytes) != journal.state_hash {
            return Err(StoreError::new(
                StoreErrorKind::CorruptState,
                format!(
                    "Transaction state digest for plan {} does not match",
                    journal.plan_id
                ),
            ));
        }
        Ok(bytes)
    }

    fn publish_snapshot(
        &self,
        journal: &TransactionJournal,
        plan_bytes: &[u8],
    ) -> Result<(), StoreError> {
        let snapshot = self
            .paths
            .snapshot_managed(&journal.plan_id, journal.target_revision);
        self.publish_immutable(&snapshot, plan_bytes, &journal.snapshot_digest)
    }

    fn publish_event(&self, journal: &TransactionJournal) -> Result<(), StoreError> {
        let events = self.read_events(&journal.plan_id)?;
        let matching = events
            .iter()
            .filter(|event| event.request_id == journal.request_id)
            .collect::<Vec<_>>();
        match matching.as_slice() {
            [] => {
                let event_bytes = canonical_json_bytes(&journal.event)?;
                let event_log = self.paths.event_log_managed(&journal.plan_id);
                let mut file = self
                    .filesystem()?
                    .open_append_file(&event_log)
                    .map_err(managed_store_error)?;
                file.write_all(&event_bytes)?;
                file.sync_all()?;
                self.filesystem()?
                    .sync_parent(&event_log)
                    .map_err(managed_store_error)?;
                Ok(())
            }
            [event] if **event == journal.event => Ok(()),
            [_] => Err(StoreError::new(
                StoreErrorKind::RequestConflict,
                format!(
                    "Request {} already identifies a different event",
                    journal.request_id
                ),
            )),
            _ => Err(StoreError::new(
                StoreErrorKind::CorruptState,
                format!("Request {} appears more than once", journal.request_id),
            )),
        }
    }

    fn backup_current_plan(&self, journal: &TransactionJournal) -> Result<(), StoreError> {
        let current_path = self.paths.current_plan_managed(&journal.plan_id);
        let previous_path = self.paths.previous_plan_managed(&journal.plan_id);
        if self.managed_exists(&previous_path)? {
            let expected = journal.previous_state_hash.as_deref().ok_or_else(|| {
                StoreError::new(
                    StoreErrorKind::CorruptState,
                    "An initial plan transaction unexpectedly has a previous-state backup",
                )
            })?;
            self.verify_file_digest(&previous_path, expected)?;
        }
        if !self.managed_exists(&current_path)? {
            if journal.previous_state_hash.is_some() && !self.managed_exists(&previous_path)? {
                return Err(StoreError::new(
                    StoreErrorKind::CorruptState,
                    format!(
                        "Plan {} lost its current and previous state",
                        journal.plan_id
                    ),
                ));
            }
            return Ok(());
        }
        let current_digest = sha256_digest(&self.read_managed(&current_path)?);
        if current_digest == journal.state_hash {
            return Ok(());
        }
        if journal.previous_state_hash.as_deref() != Some(current_digest.as_str())
            || self.managed_exists(&previous_path)?
        {
            return Err(StoreError::new(
                StoreErrorKind::CorruptState,
                format!(
                    "Current plan {} is not the journal's prior state",
                    journal.plan_id
                ),
            ));
        }
        self.filesystem()?
            .rename(&current_path, &previous_path)
            .map_err(managed_store_error)?;
        self.filesystem()?
            .sync_directory(Some(&self.paths.plan_managed(&journal.plan_id)))
            .map_err(managed_store_error)?;
        Ok(())
    }

    fn publish_next_plan(&self, journal: &TransactionJournal) -> Result<(), StoreError> {
        let current_path = self.paths.current_plan_managed(&journal.plan_id);
        if self.managed_exists(&current_path)? {
            self.verify_file_digest(&current_path, &journal.state_hash)?;
            return Ok(());
        }
        let next_path = self.paths.next_plan_managed(&journal.plan_id);
        if !self.managed_exists(&next_path)? {
            return Err(StoreError::new(
                StoreErrorKind::CorruptState,
                format!(
                    "Transaction for plan {} lost its next state",
                    journal.plan_id
                ),
            ));
        }
        self.filesystem()?
            .rename(&next_path, &current_path)
            .map_err(managed_store_error)?;
        self.filesystem()?
            .sync_directory(Some(&self.paths.plan_managed(&journal.plan_id)))
            .map_err(managed_store_error)?;
        self.verify_file_digest(&current_path, &journal.state_hash)
    }

    fn cleanup_transaction(&self, journal: &TransactionJournal) -> Result<(), StoreError> {
        self.verify_file_digest(
            &self.paths.current_plan_managed(&journal.plan_id),
            &journal.state_hash,
        )?;
        self.remove_file_if_exists(&self.paths.previous_plan_managed(&journal.plan_id))?;
        self.remove_file_if_exists(&self.paths.next_plan_managed(&journal.plan_id))?;
        self.remove_file_if_exists(&self.paths.pending_journal_managed(&journal.plan_id))?;
        self.remove_file_if_exists(&self.paths.journal_managed(&journal.plan_id))?;
        self.filesystem()?
            .remove_directory(&self.paths.transaction_managed(&journal.plan_id))
            .map_err(managed_store_error)?;
        self.filesystem()?
            .sync_directory(Some(&self.paths.plan_managed(&journal.plan_id)))
            .map_err(managed_store_error)?;
        Ok(())
    }

    fn cleanup_uncommitted_preparation(&self, plan_id: &PlanId) -> Result<(), StoreError> {
        let transaction_directory = self.paths.transaction_managed(plan_id);
        let previous_path = self.paths.previous_plan_managed(plan_id);
        if self.managed_exists(&previous_path)? {
            return Err(StoreError::new(
                StoreErrorKind::CorruptState,
                format!("Plan {plan_id} has a backup without a durable journal"),
            ));
        }
        self.remove_file_if_exists(&self.paths.next_plan_managed(plan_id))?;
        self.remove_file_if_exists(&self.paths.pending_journal_managed(plan_id))?;
        if !self
            .filesystem()?
            .read_directory(&transaction_directory)
            .map_err(managed_store_error)?
            .is_empty()
        {
            return Err(StoreError::new(
                StoreErrorKind::CorruptState,
                format!("Plan {plan_id} has unknown unjournaled transaction files"),
            ));
        }
        self.filesystem()?
            .remove_directory(&transaction_directory)
            .map_err(managed_store_error)?;
        Ok(())
    }

    fn repair_partial_event_tail(&self, plan_id: &PlanId) -> Result<(), StoreError> {
        let event_log = self.paths.event_log_managed(plan_id);
        if !self.managed_exists(&event_log)? {
            return Ok(());
        }
        let file = self
            .filesystem()?
            .open_read_write_file(&event_log)
            .map_err(managed_store_error)?;
        let original_length = file.metadata()?.len();
        let mut reader = BufReader::new(&file);
        let mut line = Vec::new();
        let mut valid_length = 0_u64;
        loop {
            let bytes_read = read_bounded_line(&mut reader, &mut line, MAX_EVENT_RECORD_BYTES)
                .map_err(|error| {
                    StoreError::new(
                        StoreErrorKind::CorruptState,
                        format!("Failed to read event log for plan {plan_id}: {error}"),
                    )
                })?;
            if bytes_read == 0 || !line.ends_with(b"\n") {
                break;
            }
            valid_length = valid_length
                .checked_add(u64::try_from(bytes_read).map_err(|_| {
                    StoreError::new(StoreErrorKind::CorruptState, "Event log length overflowed")
                })?)
                .ok_or_else(|| {
                    StoreError::new(StoreErrorKind::CorruptState, "Event log length overflowed")
                })?;
        }
        drop(reader);
        if valid_length == original_length {
            return Ok(());
        }
        file.set_len(valid_length)?;
        file.sync_all()?;
        Ok(())
    }

    fn read_current_plan(&self, plan_id: &PlanId) -> Result<Plan, StoreError> {
        let path = self.paths.current_plan_managed(plan_id);
        let display_path = self.filesystem()?.display_path(&path);
        if !self.managed_exists(&path)? {
            return Err(StoreError::new(
                StoreErrorKind::PlanNotFound,
                format!(
                    "Plan {plan_id} does not exist at {}",
                    display_path.display()
                ),
            ));
        }
        let bytes = self.read_managed(&path).map_err(|error| {
            StoreError::new(
                error.kind(),
                format!(
                    "Failed to read plan {plan_id} at {}: {error}",
                    display_path.display()
                ),
            )
        })?;
        let plan: Plan = serde_json::from_slice(&bytes)?;
        if plan.id() != plan_id || canonical_json_bytes(&plan)? != bytes {
            return Err(StoreError::new(
                StoreErrorKind::CorruptState,
                format!("Current plan {plan_id} is not canonical or has the wrong identifier"),
            ));
        }
        Ok(plan)
    }

    fn read_events(&self, plan_id: &PlanId) -> Result<Vec<Event>, StoreError> {
        let path = self.paths.event_log_managed(plan_id);
        if !self.managed_exists(&path)? {
            return Ok(Vec::new());
        }
        let file = self
            .filesystem()?
            .open_read_file(&path)
            .map_err(managed_store_error)?;
        let mut reader = BufReader::new(file);
        let mut line = Vec::new();
        let mut events = Vec::new();
        loop {
            let bytes_read = read_bounded_line(&mut reader, &mut line, MAX_EVENT_RECORD_BYTES)
                .map_err(|error| {
                    StoreError::new(
                        StoreErrorKind::CorruptState,
                        format!("Failed to read event log for plan {plan_id}: {error}"),
                    )
                })?;
            if bytes_read == 0 {
                break;
            }
            if !line.ends_with(b"\n") {
                return Err(StoreError::new(
                    StoreErrorKind::CorruptState,
                    format!("Event log for plan {plan_id} has an incomplete final record"),
                ));
            }
            if line != b"\n" {
                events.push(serde_json::from_slice(&line)?);
            }
        }
        validate_event_sequence(plan_id, &events)?;
        Ok(events)
    }

    fn audit_locked(&self, plan_id: &PlanId) -> Result<StoreAudit, StoreError> {
        let plan = self.read_current_plan(plan_id)?;
        let current_bytes = self.read_managed(&self.paths.current_plan_managed(plan_id))?;
        let state_hash = sha256_digest(&current_bytes);
        let events = self.read_events(plan_id)?;
        let last_event = events.last().ok_or_else(|| {
            StoreError::new(
                StoreErrorKind::CorruptState,
                format!("Plan {plan_id} has state without an event"),
            )
        })?;
        if last_event.revision_after != plan.revision() || last_event.state_hash != state_hash {
            return Err(StoreError::new(
                StoreErrorKind::CorruptState,
                format!("Current state for plan {plan_id} does not match its final event"),
            ));
        }
        for event in &events {
            self.verify_file_digest(
                &self.paths.snapshot_managed(plan_id, event.revision_after),
                &event.snapshot_digest,
            )?;
        }
        let snapshot_count = self.count_snapshot_files(&self.paths.snapshots_managed(plan_id))?;
        if snapshot_count != events.len() {
            return Err(StoreError::new(
                StoreErrorKind::CorruptState,
                format!("Plan {plan_id} has extra or missing immutable snapshots"),
            ));
        }
        Ok(StoreAudit {
            revision: plan.revision(),
            event_count: events.len(),
            snapshot_count,
            state_hash,
        })
    }

    fn prepare_plan_directories(&self, plan_id: &PlanId) -> Result<(), StoreError> {
        let snapshots = self.paths.snapshots_managed(plan_id);
        self.filesystem()?
            .ensure_directory(&snapshots)
            .map_err(managed_store_error)?;
        for directory in [
            snapshots,
            self.paths.plan_managed(plan_id),
            self.paths.plans_managed(),
            self.paths.mino_managed(),
        ] {
            self.filesystem()?
                .sync_directory(Some(&directory))
                .map_err(managed_store_error)?;
        }
        self.filesystem()?
            .sync_directory(None)
            .map_err(managed_store_error)?;
        Ok(())
    }

    fn require_plan_directory(&self, plan_id: &PlanId) -> Result<(), StoreError> {
        if self
            .filesystem()?
            .is_directory(&self.paths.plan_managed(plan_id))
            .map_err(managed_store_error)?
        {
            Ok(())
        } else {
            Err(StoreError::new(
                StoreErrorKind::PlanNotFound,
                format!("Plan {plan_id} does not exist"),
            ))
        }
    }

    fn filesystem(&self) -> Result<&ProjectFs, StoreError> {
        self.filesystem.as_ref().map_err(|error| {
            StoreError::new(
                match error.kind() {
                    crate::managed_fs::ManagedFsErrorKind::InvalidPath
                    | crate::managed_fs::ManagedFsErrorKind::UnsafeComponent => {
                        StoreErrorKind::CorruptState
                    }
                    crate::managed_fs::ManagedFsErrorKind::Io => StoreErrorKind::Io,
                },
                error.to_string(),
            )
        })
    }

    fn read_managed(&self, path: &ManagedPath) -> Result<Vec<u8>, StoreError> {
        self.filesystem()?
            .read_bounded(path, MAX_PLAN_STATE_BYTES)
            .map_err(managed_store_error)
    }

    fn managed_exists(&self, path: &ManagedPath) -> Result<bool, StoreError> {
        self.filesystem()?.exists(path).map_err(managed_store_error)
    }

    fn write_new_file(&self, path: &ManagedPath, bytes: &[u8]) -> Result<(), StoreError> {
        let mut file = self
            .filesystem()?
            .create_new_file(path)
            .map_err(managed_store_error)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(())
    }

    fn publish_immutable(
        &self,
        path: &ManagedPath,
        bytes: &[u8],
        digest: &str,
    ) -> Result<(), StoreError> {
        if self.managed_exists(path)? {
            self.verify_file_digest(path, digest)?;
            if self.read_managed(path)? != bytes {
                return Err(StoreError::new(
                    StoreErrorKind::CorruptState,
                    format!(
                        "Immutable artifact {} has conflicting bytes",
                        self.filesystem()?.display_path(path).display()
                    ),
                ));
            }
            return Ok(());
        }
        self.write_new_file(path, bytes)?;
        self.filesystem()?
            .sync_parent(path)
            .map_err(managed_store_error)
    }

    fn verify_file_digest(&self, path: &ManagedPath, expected: &str) -> Result<(), StoreError> {
        let display_path = self.filesystem()?.display_path(path);
        let actual = sha256_digest(&self.read_managed(path).map_err(|error| {
            StoreError::new(
                StoreErrorKind::CorruptState,
                format!(
                    "Failed to read required artifact {}: {error}",
                    display_path.display()
                ),
            )
        })?);
        if actual == expected {
            Ok(())
        } else {
            Err(StoreError::new(
                StoreErrorKind::CorruptState,
                format!(
                    "Digest mismatch for {}: expected {expected}, got {actual}",
                    display_path.display()
                ),
            ))
        }
    }

    fn remove_file_if_exists(&self, path: &ManagedPath) -> Result<(), StoreError> {
        self.filesystem()?
            .remove_file_if_exists(path)
            .map_err(managed_store_error)
    }

    fn count_snapshot_files(&self, directory: &ManagedPath) -> Result<usize, StoreError> {
        let mut count = 0_usize;
        for entry in self
            .filesystem()?
            .read_directory(directory)
            .map_err(managed_store_error)?
        {
            if entry.kind == ManagedEntryKind::File
                && Path::new(&entry.name)
                    .extension()
                    .is_some_and(|extension| extension == "json")
            {
                count = count.checked_add(1).ok_or_else(|| {
                    StoreError::new(StoreErrorKind::CorruptState, "Snapshot count overflowed")
                })?;
            }
        }
        Ok(count)
    }
}

fn managed_store_error(error: ManagedFsError) -> StoreError {
    StoreError::new(
        match error.kind() {
            crate::managed_fs::ManagedFsErrorKind::InvalidPath
            | crate::managed_fs::ManagedFsErrorKind::UnsafeComponent => {
                StoreErrorKind::CorruptState
            }
            crate::managed_fs::ManagedFsErrorKind::Io => StoreErrorKind::Io,
        },
        error.into_message(),
    )
}

fn require_generated_size(bytes: &[u8], maximum_bytes: u64, label: &str) -> Result<(), StoreError> {
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) <= maximum_bytes {
        Ok(())
    } else {
        Err(StoreError::new(
            StoreErrorKind::InvalidMutation,
            format!("{label} exceeds the {maximum_bytes}-byte managed-state limit"),
        ))
    }
}

fn validate_request(actor: &str, command: &[String]) -> Result<(), StoreError> {
    if actor.trim().is_empty()
        || command.is_empty()
        || command.iter().any(|part| part.trim().is_empty())
    {
        Err(StoreError::new(
            StoreErrorKind::InvalidMutation,
            "A storage request requires an actor and a non-empty command",
        ))
    } else {
        Ok(())
    }
}

fn validate_changed_fields(changed_fields: &[String]) -> Result<(), StoreError> {
    if changed_fields.is_empty() || changed_fields.iter().any(|field| field.trim().is_empty()) {
        Err(StoreError::new(
            StoreErrorKind::InvalidMutation,
            "A storage mutation requires at least one non-empty changed field",
        ))
    } else {
        Ok(())
    }
}

fn replay_receipt(
    plan_id: &PlanId,
    events: &[Event],
    request_id: &RequestId,
    expected_revision: u64,
    actor: &str,
    command: &[String],
    changed_fields: &[String],
) -> Result<Option<CommitReceipt>, StoreError> {
    let matching = events
        .iter()
        .filter(|event| &event.request_id == request_id)
        .collect::<Vec<_>>();
    match matching.as_slice() {
        [] => Ok(None),
        [event]
            if event.revision_before == expected_revision
                && event.actor == actor
                && event.command == command
                && event.changed_fields == changed_fields =>
        {
            Ok(Some(CommitReceipt::from_event(
                plan_id.clone(),
                event,
                true,
            )))
        }
        [_] => Err(StoreError::new(
            StoreErrorKind::RequestConflict,
            format!("Request {request_id} was reused for a different operation"),
        )),
        _ => Err(StoreError::new(
            StoreErrorKind::CorruptState,
            format!("Request {request_id} appears more than once"),
        )),
    }
}

fn validate_event_sequence(plan_id: &PlanId, events: &[Event]) -> Result<(), StoreError> {
    let mut request_ids = BTreeSet::new();
    for (index, event) in events.iter().enumerate() {
        let expected_sequence = u64::try_from(index)
            .ok()
            .and_then(|value| value.checked_add(1))
            .ok_or_else(|| {
                StoreError::new(StoreErrorKind::CorruptState, "Event sequence overflowed")
            })?;
        let expected_before = u64::try_from(index).map_err(|_| {
            StoreError::new(StoreErrorKind::CorruptState, "Event revision overflowed")
        })?;
        if event.sequence != expected_sequence
            || event.revision_before != expected_before
            || event.revision_after != expected_sequence
            || event.state_hash != event.snapshot_digest
            || event.result != EventResult::Succeeded
            || !request_ids.insert(&event.request_id)
        {
            return Err(StoreError::new(
                StoreErrorKind::CorruptState,
                format!("Event sequence for plan {plan_id} is inconsistent at {expected_sequence}"),
            ));
        }
    }
    Ok(())
}
