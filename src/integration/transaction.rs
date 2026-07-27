//! Recoverable replacement transactions for repository integration files.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use fs4::{FileExt, TryLockError};
use serde::{Deserialize, Serialize};

use crate::managed_fs::{
    ManagedEntryKind, ManagedFsError, ManagedFsErrorKind, ManagedPath, ProjectFs,
};
use crate::store::{canonical_json_bytes, sha256_digest};
use crate::{ErrorCategory, MinoError};

use super::IntegrationStatus;

const TRANSACTION_VERSION: u32 = 1;
const MAX_INTEGRATION_PHASE_BYTES: u64 = 1_024 * 1_024;
const MAX_INTEGRATION_ARTIFACT_BYTES: u64 = 16 * 1_024 * 1_024;
const LOCK_TIMEOUT: Duration = Duration::from_secs(2);
const LOCK_RETRY: Duration = Duration::from_millis(10);
const TRANSACTION_ROOT: &str = ".mino/integration-transactions";
const TRANSACTION_LOCK: &str = ".mino/integration-transactions.lock";
const ALLOWED_TARGETS: &[&str] = &[
    ".agents/skills/mino/SKILL.md",
    ".agents/skills/mino/agents/openai.yaml",
    ".agents/skills/mino/references/approval-boundaries.md",
    ".agents/skills/mino/references/command-contract.md",
    ".gitignore",
    "AGENTS.md",
];

/// Deterministic integration replacement boundary used by recovery tests.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IntegrationFailurePoint {
    /// Interrupt after preparation and immediately before the target backup.
    BeforeBackup,
    /// Interrupt after the target backup is durable.
    AfterBackup,
    /// Interrupt immediately before publishing replacement bytes.
    BeforePublish,
    /// Interrupt after replacement publication is durable.
    AfterPublish,
    /// Interrupt immediately before removing the verified backup.
    BeforeBackupRemoval,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TransactionPhase {
    Prepared,
    BackedUp,
    Published,
    Cleaned,
}

impl TransactionPhase {
    const fn file_name(self) -> &'static str {
        match self {
            Self::Prepared => "prepared.json",
            Self::BackedUp => "backed_up.json",
            Self::Published => "published.json",
            Self::Cleaned => "cleaned.json",
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::BackedUp => "backed_up",
            Self::Published => "published",
            Self::Cleaned => "cleaned",
        }
    }

    fn from_file_name(value: &str) -> Option<Self> {
        match value {
            "prepared.json" => Some(Self::Prepared),
            "backed_up.json" => Some(Self::BackedUp),
            "published.json" => Some(Self::Published),
            "cleaned.json" => Some(Self::Cleaned),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TransactionRecord {
    version: u32,
    target: String,
    backup: String,
    temporary: String,
    expected_digest: Option<String>,
    replacement_digest: String,
    phase: TransactionPhase,
}

impl TransactionRecord {
    fn with_phase(&self, phase: TransactionPhase) -> Self {
        let mut record = self.clone();
        record.phase = phase;
        record
    }

    fn has_same_operation(&self, other: &Self) -> bool {
        self.version == other.version
            && self.target == other.target
            && self.backup == other.backup
            && self.temporary == other.temporary
            && self.expected_digest == other.expected_digest
            && self.replacement_digest == other.replacement_digest
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct IntegrationTransactionInspection {
    pub(crate) path: PathBuf,
    pub(crate) message: String,
    pub(crate) is_corrupt: bool,
}

pub(super) struct IntegrationWriter {
    filesystem: ProjectFs,
    failure_point: Option<IntegrationFailurePoint>,
    _lock: IntegrationLock,
}

impl IntegrationWriter {
    pub(super) fn open(
        root: &Path,
        failure_point: Option<IntegrationFailurePoint>,
    ) -> Result<Self, MinoError> {
        let filesystem = ProjectFs::open(root).map_err(managed_error)?;
        let lock = IntegrationLock::acquire(&filesystem)?;
        let writer = Self {
            filesystem,
            failure_point,
            _lock: lock,
        };
        writer.recover_all()?;
        Ok(writer)
    }

    pub(super) fn root(&self) -> &Path {
        self.filesystem.root()
    }

    pub(super) fn guarded_write(
        &self,
        target: &Path,
        expected: Option<&[u8]>,
        replacement: &[u8],
    ) -> Result<IntegrationStatus, MinoError> {
        let target = target_managed_path(&self.filesystem, target)?;
        let actual = optional_file_bytes(&self.filesystem, &target)?;
        if actual.as_deref() == Some(replacement) {
            return Ok(IntegrationStatus::Current);
        }
        match (actual.as_deref(), expected) {
            (Some(actual), Some(expected)) if actual == expected => {}
            (None, None) => {}
            (Some(_), _) => return Err(changed_bytes_error(&self.filesystem, &target)),
            (None, Some(_)) => return Err(disappeared_error(&self.filesystem, &target)),
        }
        let status = if actual.is_some() {
            IntegrationStatus::Updated
        } else {
            IntegrationStatus::Created
        };
        let record = build_record(&target, expected, replacement)?;
        self.prepare_transaction(&record, replacement)?;
        self.inject(IntegrationFailurePoint::BeforeBackup)?;
        self.publish_backup(&record)?;
        self.publish_phase(&record, TransactionPhase::BackedUp)?;
        self.inject(IntegrationFailurePoint::AfterBackup)?;
        self.inject(IntegrationFailurePoint::BeforePublish)?;
        self.publish_target(&record)?;
        self.publish_phase(&record, TransactionPhase::Published)?;
        self.inject(IntegrationFailurePoint::AfterPublish)?;
        self.inject(IntegrationFailurePoint::BeforeBackupRemoval)?;
        self.finish_published(&record)?;
        Ok(status)
    }

    fn prepare_transaction(
        &self,
        record: &TransactionRecord,
        replacement: &[u8],
    ) -> Result<(), MinoError> {
        let transaction_root = transaction_root();
        self.filesystem
            .ensure_directory(&transaction_root)
            .map_err(managed_error)?;
        let directory = transaction_directory(record);
        if self.filesystem.exists(&directory).map_err(managed_error)? {
            return Err(corrupt_error(format!(
                "Integration transaction {} already exists",
                self.filesystem.display_path(&directory).display()
            )));
        }
        self.filesystem
            .create_directory(&directory)
            .map_err(managed_error)?;
        if let Err(error) = self.publish_phase(record, TransactionPhase::Prepared) {
            let _ = self.filesystem.remove_directory_all(&directory);
            return Err(error);
        }
        let temporary = managed_path(&record.temporary)?;
        if self.filesystem.exists(&temporary).map_err(managed_error)? {
            return Err(corrupt_error(format!(
                "Integration temporary {} already exists",
                self.filesystem.display_path(&temporary).display()
            )));
        }
        write_new_synced(&self.filesystem, &temporary, replacement)
    }

    fn publish_backup(&self, record: &TransactionRecord) -> Result<(), MinoError> {
        let target = managed_path(&record.target)?;
        let backup = managed_path(&record.backup)?;
        require_digest_state(
            &self.filesystem,
            &backup,
            None,
            "integration backup before replacement",
        )?;
        if let Some(expected) = record.expected_digest.as_deref() {
            require_digest_state(
                &self.filesystem,
                &target,
                Some(expected),
                "integration target before backup",
            )?;
            self.filesystem
                .rename(&target, &backup)
                .map_err(managed_error)?;
            self.filesystem
                .sync_parent(&target)
                .map_err(managed_error)?;
        } else {
            require_digest_state(
                &self.filesystem,
                &target,
                None,
                "new integration target before publication",
            )?;
        }
        Ok(())
    }

    fn publish_target(&self, record: &TransactionRecord) -> Result<(), MinoError> {
        let target = managed_path(&record.target)?;
        let temporary = managed_path(&record.temporary)?;
        require_digest_state(
            &self.filesystem,
            &target,
            None,
            "integration target before publication",
        )?;
        require_digest_state(
            &self.filesystem,
            &temporary,
            Some(&record.replacement_digest),
            "integration temporary before publication",
        )?;
        self.filesystem
            .rename(&temporary, &target)
            .map_err(managed_error)?;
        self.filesystem.sync_parent(&target).map_err(managed_error)
    }

    fn finish_published(&self, record: &TransactionRecord) -> Result<(), MinoError> {
        self.remove_backup(record)?;
        self.remove_temporary(record)?;
        self.publish_phase(record, TransactionPhase::Cleaned)?;
        self.cleanup_transaction(record)
    }

    fn recover_all(&self) -> Result<(), MinoError> {
        let root = transaction_root();
        match self.filesystem.entry_kind(&root).map_err(managed_error)? {
            None => return Ok(()),
            Some(ManagedEntryKind::Directory) => {}
            Some(kind) => {
                return Err(corrupt_error(format!(
                    "Integration transaction root {} is {kind:?}, not a directory",
                    self.filesystem.display_path(&root).display()
                )));
            }
        }
        for entry in self
            .filesystem
            .read_directory(&root)
            .map_err(managed_error)?
        {
            let name = entry.name.to_str().ok_or_else(|| {
                corrupt_error("Integration transaction directory name is not UTF-8")
            })?;
            if entry.kind != ManagedEntryKind::Directory {
                return Err(corrupt_error(format!(
                    "Integration transaction entry {} is not a directory",
                    self.filesystem.display_path(&root).join(name).display()
                )));
            }
            let directory = root.join(name).map_err(managed_error)?;
            let record = load_transaction(&self.filesystem, &directory)?;
            self.recover_record(&record)?;
        }
        Ok(())
    }

    fn recover_record(&self, record: &TransactionRecord) -> Result<(), MinoError> {
        match record.phase {
            TransactionPhase::Prepared => self.recover_prepared(record),
            TransactionPhase::BackedUp => self.recover_backed_up(record),
            TransactionPhase::Published => self.recover_published(record),
            TransactionPhase::Cleaned => self.recover_cleaned(record),
        }
    }

    fn recover_prepared(&self, record: &TransactionRecord) -> Result<(), MinoError> {
        let state = OperationState::read(&self.filesystem, record)?;
        match record.expected_digest.as_deref() {
            Some(expected)
                if state.target.as_deref() == Some(expected) && state.backup.is_none() =>
            {
                self.remove_temporary(record)?;
                self.cleanup_transaction(record)
            }
            Some(expected)
                if state.target.is_none() && state.backup.as_deref() == Some(expected) =>
            {
                self.restore_backup(record)?;
                self.remove_temporary(record)?;
                self.cleanup_transaction(record)
            }
            None if state.target.is_none() && state.backup.is_none() => {
                self.remove_temporary(record)?;
                self.cleanup_transaction(record)
            }
            _ => Err(unexpected_state_error(&self.filesystem, record, &state)),
        }
    }

    fn recover_backed_up(&self, record: &TransactionRecord) -> Result<(), MinoError> {
        let state = OperationState::read(&self.filesystem, record)?;
        if let Some(expected) = record.expected_digest.as_deref()
            && state.target.as_deref() == Some(expected)
            && state.backup.is_none()
        {
            self.remove_temporary(record)?;
            return self.cleanup_transaction(record);
        }
        if state.target.is_none() {
            if record.expected_digest.is_some()
                && state.backup.as_deref() != record.expected_digest.as_deref()
            {
                return Err(unexpected_state_error(&self.filesystem, record, &state));
            }
            if record.expected_digest.is_none() && state.backup.is_some() {
                return Err(unexpected_state_error(&self.filesystem, record, &state));
            }
            if state.temporary.as_deref() == Some(record.replacement_digest.as_str()) {
                self.publish_target(record)?;
            } else if state.temporary.is_none() && record.expected_digest.is_some() {
                self.restore_backup(record)?;
                return self.cleanup_transaction(record);
            } else if state.temporary.is_none() {
                return self.cleanup_transaction(record);
            } else {
                return Err(unexpected_state_error(&self.filesystem, record, &state));
            }
        } else if state.target.as_deref() != Some(record.replacement_digest.as_str()) {
            return Err(unexpected_state_error(&self.filesystem, record, &state));
        }
        self.publish_phase(record, TransactionPhase::Published)?;
        self.recover_published(&record.with_phase(TransactionPhase::Published))
    }

    fn recover_published(&self, record: &TransactionRecord) -> Result<(), MinoError> {
        let state = OperationState::read(&self.filesystem, record)?;
        if state.target.as_deref() != Some(record.replacement_digest.as_str()) {
            return Err(unexpected_state_error(&self.filesystem, record, &state));
        }
        if state.backup.is_some() && state.backup.as_deref() != record.expected_digest.as_deref() {
            return Err(unexpected_state_error(&self.filesystem, record, &state));
        }
        if state.temporary.is_some()
            && state.temporary.as_deref() != Some(record.replacement_digest.as_str())
        {
            return Err(unexpected_state_error(&self.filesystem, record, &state));
        }
        self.remove_backup(record)?;
        self.remove_temporary(record)?;
        self.publish_phase(record, TransactionPhase::Cleaned)?;
        self.recover_cleaned(&record.with_phase(TransactionPhase::Cleaned))
    }

    fn recover_cleaned(&self, record: &TransactionRecord) -> Result<(), MinoError> {
        let state = OperationState::read(&self.filesystem, record)?;
        if state.target.as_deref() != Some(record.replacement_digest.as_str())
            || state.backup.is_some()
            || state.temporary.is_some()
        {
            return Err(unexpected_state_error(&self.filesystem, record, &state));
        }
        self.cleanup_transaction(record)
    }

    fn restore_backup(&self, record: &TransactionRecord) -> Result<(), MinoError> {
        let target = managed_path(&record.target)?;
        let backup = managed_path(&record.backup)?;
        require_digest_state(
            &self.filesystem,
            &target,
            None,
            "integration target before restoration",
        )?;
        require_digest_state(
            &self.filesystem,
            &backup,
            record.expected_digest.as_deref(),
            "integration backup before restoration",
        )?;
        self.filesystem
            .rename(&backup, &target)
            .map_err(managed_error)?;
        self.filesystem.sync_parent(&target).map_err(managed_error)
    }

    fn remove_backup(&self, record: &TransactionRecord) -> Result<(), MinoError> {
        remove_matching_file(
            &self.filesystem,
            &managed_path(&record.backup)?,
            record.expected_digest.as_deref(),
            "integration backup",
        )
    }

    fn remove_temporary(&self, record: &TransactionRecord) -> Result<(), MinoError> {
        remove_matching_file(
            &self.filesystem,
            &managed_path(&record.temporary)?,
            Some(&record.replacement_digest),
            "integration temporary",
        )
    }

    fn publish_phase(
        &self,
        record: &TransactionRecord,
        phase: TransactionPhase,
    ) -> Result<(), MinoError> {
        let phase_record = record.with_phase(phase);
        let path = transaction_directory(record)
            .join(phase.file_name())
            .map_err(managed_error)?;
        let bytes = canonical_json_bytes(&phase_record).map_err(|error| {
            MinoError::new(
                ErrorCategory::EnvironmentUnavailable,
                format!("Failed to encode integration transaction: {error}"),
            )
        })?;
        if self.filesystem.exists(&path).map_err(managed_error)? {
            if self
                .filesystem
                .read_bounded(&path, MAX_INTEGRATION_PHASE_BYTES)
                .map_err(managed_error)?
                == bytes
            {
                return Ok(());
            }
            return Err(corrupt_error(format!(
                "Integration phase {} has conflicting bytes",
                self.filesystem.display_path(&path).display()
            )));
        }
        write_new_synced(&self.filesystem, &path, &bytes)
    }

    fn cleanup_transaction(&self, record: &TransactionRecord) -> Result<(), MinoError> {
        let directory = transaction_directory(record);
        self.filesystem
            .remove_directory_all(&directory)
            .map_err(managed_error)?;
        self.filesystem
            .sync_directory(Some(&transaction_root()))
            .map_err(managed_error)
    }

    fn inject(&self, point: IntegrationFailurePoint) -> Result<(), MinoError> {
        if self.failure_point == Some(point) {
            Err(MinoError::new(
                ErrorCategory::EnvironmentUnavailable,
                format!("Injected integration interruption at {point:?}"),
            ))
        } else {
            Ok(())
        }
    }
}

struct IntegrationLock {
    file: File,
}

impl IntegrationLock {
    fn acquire(filesystem: &ProjectFs) -> Result<Self, MinoError> {
        let path = managed_path(TRANSACTION_LOCK)?;
        let file = filesystem.open_lock_file(&path).map_err(managed_error)?;
        let started = Instant::now();
        loop {
            match FileExt::try_lock(&file) {
                Ok(()) => return Ok(Self { file }),
                Err(TryLockError::WouldBlock) if started.elapsed() < LOCK_TIMEOUT => {
                    thread::sleep(LOCK_RETRY.min(LOCK_TIMEOUT.saturating_sub(started.elapsed())));
                }
                Err(TryLockError::WouldBlock) => {
                    return Err(MinoError::new(
                        ErrorCategory::EnvironmentUnavailable,
                        format!(
                            "Timed out acquiring integration lock {}",
                            filesystem.display_path(&path).display()
                        ),
                    ));
                }
                Err(TryLockError::Error(error)) => {
                    return Err(MinoError::new(
                        ErrorCategory::EnvironmentUnavailable,
                        format!(
                            "Failed to lock integration transaction {}: {error}",
                            filesystem.display_path(&path).display()
                        ),
                    ));
                }
            }
        }
    }
}

impl Drop for IntegrationLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

struct OperationState {
    target: Option<String>,
    backup: Option<String>,
    temporary: Option<String>,
}

impl OperationState {
    fn read(filesystem: &ProjectFs, record: &TransactionRecord) -> Result<Self, MinoError> {
        Ok(Self {
            target: optional_digest(filesystem, &managed_path(&record.target)?)?,
            backup: optional_digest(filesystem, &managed_path(&record.backup)?)?,
            temporary: optional_digest(filesystem, &managed_path(&record.temporary)?)?,
        })
    }
}

pub(crate) fn inspect_transactions(
    root: &Path,
) -> Result<Vec<IntegrationTransactionInspection>, MinoError> {
    let filesystem = ProjectFs::open(root).map_err(managed_error)?;
    let transaction_root = transaction_root();
    match filesystem
        .entry_kind(&transaction_root)
        .map_err(managed_error)?
    {
        None => return Ok(Vec::new()),
        Some(ManagedEntryKind::Directory) => {}
        Some(kind) => {
            return Ok(vec![IntegrationTransactionInspection {
                path: filesystem.display_path(&transaction_root),
                message: format!("Integration transaction root is {kind:?}, not a directory"),
                is_corrupt: true,
            }]);
        }
    }
    let mut inspections = Vec::new();
    for entry in filesystem
        .read_directory(&transaction_root)
        .map_err(managed_error)?
    {
        let name = entry.name.to_string_lossy();
        let directory = transaction_root
            .join(name.as_ref())
            .map_err(managed_error)?;
        let path = filesystem.display_path(&directory);
        if entry.kind != ManagedEntryKind::Directory {
            inspections.push(IntegrationTransactionInspection {
                path,
                message: "Integration transaction entry is not a directory".to_owned(),
                is_corrupt: true,
            });
            continue;
        }
        match load_transaction(&filesystem, &directory) {
            Ok(record) => match OperationState::read(&filesystem, &record) {
                Ok(state) if is_recoverable_state(&record, &state) => {
                    inspections.push(IntegrationTransactionInspection {
                        path,
                        message: format!(
                            "Integration replacement for {} is pending at phase {}",
                            record.target,
                            record.phase.label()
                        ),
                        is_corrupt: false,
                    });
                }
                Ok(state) => inspections.push(IntegrationTransactionInspection {
                    path,
                    message: unexpected_state_error(&filesystem, &record, &state).to_string(),
                    is_corrupt: true,
                }),
                Err(error) => inspections.push(IntegrationTransactionInspection {
                    path,
                    message: error.to_string(),
                    is_corrupt: true,
                }),
            },
            Err(error) => inspections.push(IntegrationTransactionInspection {
                path,
                message: error.to_string(),
                is_corrupt: true,
            }),
        }
    }
    Ok(inspections)
}

fn build_record(
    target: &ManagedPath,
    expected: Option<&[u8]>,
    replacement: &[u8],
) -> Result<TransactionRecord, MinoError> {
    let target = protocol_path(target)?;
    if !ALLOWED_TARGETS.contains(&target.as_str()) {
        return Err(MinoError::new(
            ErrorCategory::PolicyViolation,
            format!("Integration transaction target {target} is not owned by Mino"),
        ));
    }
    let id = transaction_id(&target);
    let file_name = Path::new(&target)
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| corrupt_error("Integration target has no UTF-8 file name"))?;
    let parent = Path::new(&target).parent().unwrap_or_else(|| Path::new(""));
    let temporary = parent.join(format!(".{file_name}.mino-integration-{id}.tmp"));
    let backup = parent.join(format!(".{file_name}.mino-integration-{id}.bak"));
    Ok(TransactionRecord {
        version: TRANSACTION_VERSION,
        target,
        backup: protocol_path(&ManagedPath::new(backup).map_err(managed_error)?)?,
        temporary: protocol_path(&ManagedPath::new(temporary).map_err(managed_error)?)?,
        expected_digest: expected.map(sha256_digest),
        replacement_digest: sha256_digest(replacement),
        phase: TransactionPhase::Prepared,
    })
}

fn load_transaction(
    filesystem: &ProjectFs,
    directory: &ManagedPath,
) -> Result<TransactionRecord, MinoError> {
    let mut records = BTreeMap::new();
    for entry in filesystem
        .read_directory(directory)
        .map_err(managed_error)?
    {
        if entry.kind != ManagedEntryKind::File {
            return Err(corrupt_error(format!(
                "Integration transaction {} contains a non-file entry",
                filesystem.display_path(directory).display()
            )));
        }
        let name = entry.name.to_str().ok_or_else(|| {
            corrupt_error("Integration transaction phase name is not valid UTF-8")
        })?;
        let phase = TransactionPhase::from_file_name(name).ok_or_else(|| {
            corrupt_error(format!("Unknown integration transaction phase file {name}"))
        })?;
        let path = directory.join(name).map_err(managed_error)?;
        let bytes = filesystem
            .read_bounded(&path, MAX_INTEGRATION_PHASE_BYTES)
            .map_err(managed_error)?;
        let record: TransactionRecord = serde_json::from_slice(&bytes).map_err(|error| {
            corrupt_error(format!(
                "Failed to decode integration transaction {}: {error}",
                filesystem.display_path(&path).display()
            ))
        })?;
        let canonical = canonical_json_bytes(&record).map_err(|error| {
            corrupt_error(format!("Failed to encode integration transaction: {error}"))
        })?;
        if canonical != bytes || record.phase != phase {
            return Err(corrupt_error(format!(
                "Integration transaction {} is not canonical or has the wrong phase",
                filesystem.display_path(&path).display()
            )));
        }
        validate_record(&record, directory)?;
        if records.insert(phase, record).is_some() {
            return Err(corrupt_error("Duplicate integration transaction phase"));
        }
    }
    validate_phase_sequence(&records)?;
    let prepared = records
        .get(&TransactionPhase::Prepared)
        .ok_or_else(|| corrupt_error("Integration transaction has no prepared phase"))?;
    if records
        .values()
        .any(|record| !record.has_same_operation(prepared))
    {
        return Err(corrupt_error(
            "Integration transaction phase records identify different operations",
        ));
    }
    records
        .into_values()
        .next_back()
        .ok_or_else(|| corrupt_error("Integration transaction is empty"))
}

fn validate_phase_sequence(
    records: &BTreeMap<TransactionPhase, TransactionRecord>,
) -> Result<(), MinoError> {
    if records.contains_key(&TransactionPhase::BackedUp)
        && !records.contains_key(&TransactionPhase::Prepared)
        || records.contains_key(&TransactionPhase::Published)
            && !records.contains_key(&TransactionPhase::BackedUp)
        || records.contains_key(&TransactionPhase::Cleaned)
            && !records.contains_key(&TransactionPhase::Published)
    {
        Err(corrupt_error(
            "Integration transaction phase sequence has a gap",
        ))
    } else {
        Ok(())
    }
}

fn validate_record(record: &TransactionRecord, directory: &ManagedPath) -> Result<(), MinoError> {
    if record.version != TRANSACTION_VERSION
        || !ALLOWED_TARGETS.contains(&record.target.as_str())
        || !is_digest(&record.replacement_digest)
        || record
            .expected_digest
            .as_deref()
            .is_some_and(|digest| !is_digest(digest) || digest == record.replacement_digest)
    {
        return Err(corrupt_error("Integration transaction fields are invalid"));
    }
    for value in [&record.target, &record.backup, &record.temporary] {
        let path = managed_path(value)?;
        if protocol_path(&path)? != *value {
            return Err(corrupt_error(
                "Integration transaction paths are not normalized",
            ));
        }
    }
    let expected = build_record_paths(&record.target)?;
    if record.backup != expected.0 || record.temporary != expected.1 {
        return Err(corrupt_error(
            "Integration transaction temporary or backup path is invalid",
        ));
    }
    let directory_name = directory
        .as_path()
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| corrupt_error("Integration transaction directory has no UTF-8 name"))?;
    if directory_name != transaction_id(&record.target) {
        return Err(corrupt_error(
            "Integration transaction directory does not match its target",
        ));
    }
    Ok(())
}

fn build_record_paths(target: &str) -> Result<(String, String), MinoError> {
    let id = transaction_id(target);
    let path = Path::new(target);
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| corrupt_error("Integration target has no UTF-8 file name"))?;
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    let backup = ManagedPath::new(parent.join(format!(".{file_name}.mino-integration-{id}.bak")))
        .map_err(managed_error)?;
    let temporary =
        ManagedPath::new(parent.join(format!(".{file_name}.mino-integration-{id}.tmp")))
            .map_err(managed_error)?;
    Ok((protocol_path(&backup)?, protocol_path(&temporary)?))
}

fn target_managed_path(filesystem: &ProjectFs, target: &Path) -> Result<ManagedPath, MinoError> {
    let relative = target.strip_prefix(filesystem.root()).map_err(|_| {
        MinoError::new(
            ErrorCategory::PolicyViolation,
            format!(
                "Integration target {} is outside project root {}",
                target.display(),
                filesystem.root().display()
            ),
        )
    })?;
    ManagedPath::new(relative).map_err(managed_error)
}

fn transaction_root() -> ManagedPath {
    ManagedPath::new(TRANSACTION_ROOT).expect("static transaction root should be valid")
}

fn transaction_directory(record: &TransactionRecord) -> ManagedPath {
    transaction_root()
        .join(transaction_id(&record.target))
        .expect("transaction digest should form a managed path")
}

fn transaction_id(target: &str) -> String {
    sha256_digest(target.as_bytes())
        .strip_prefix("sha256:")
        .expect("SHA-256 helper should return a prefixed digest")
        .to_owned()
}

fn managed_path(value: &str) -> Result<ManagedPath, MinoError> {
    ManagedPath::new(value).map_err(managed_error)
}

fn protocol_path(path: &ManagedPath) -> Result<String, MinoError> {
    path.as_path()
        .components()
        .map(|component| {
            component
                .as_os_str()
                .to_str()
                .ok_or_else(|| corrupt_error("Integration transaction path is not valid UTF-8"))
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|components| components.join("/"))
}

fn optional_file_bytes(
    filesystem: &ProjectFs,
    path: &ManagedPath,
) -> Result<Option<Vec<u8>>, MinoError> {
    match filesystem.entry_kind(path).map_err(managed_error)? {
        None => Ok(None),
        Some(ManagedEntryKind::File) => filesystem
            .read_bounded(path, MAX_INTEGRATION_ARTIFACT_BYTES)
            .map(Some)
            .map_err(managed_error),
        Some(kind) => Err(corrupt_error(format!(
            "Integration path {} is {kind:?}, not a regular file",
            filesystem.display_path(path).display()
        ))),
    }
}

fn optional_digest(
    filesystem: &ProjectFs,
    path: &ManagedPath,
) -> Result<Option<String>, MinoError> {
    optional_file_bytes(filesystem, path).map(|bytes| bytes.as_deref().map(sha256_digest))
}

fn require_digest_state(
    filesystem: &ProjectFs,
    path: &ManagedPath,
    expected: Option<&str>,
    description: &str,
) -> Result<(), MinoError> {
    let actual = optional_digest(filesystem, path)?;
    if actual.as_deref() == expected {
        Ok(())
    } else {
        Err(corrupt_error(format!(
            "Unexpected {description} state at {}: expected {expected:?}, found {actual:?}",
            filesystem.display_path(path).display()
        )))
    }
}

fn remove_matching_file(
    filesystem: &ProjectFs,
    path: &ManagedPath,
    expected: Option<&str>,
    description: &str,
) -> Result<(), MinoError> {
    let Some(actual) = optional_digest(filesystem, path)? else {
        return Ok(());
    };
    if Some(actual.as_str()) != expected {
        return Err(corrupt_error(format!(
            "Unexpected {description} bytes at {}",
            filesystem.display_path(path).display()
        )));
    }
    filesystem.remove_file(path).map_err(managed_error)?;
    filesystem.sync_parent(path).map_err(managed_error)
}

fn write_new_synced(
    filesystem: &ProjectFs,
    path: &ManagedPath,
    bytes: &[u8],
) -> Result<(), MinoError> {
    let mut file = filesystem.create_new_file(path).map_err(managed_error)?;
    if let Err(error) = file.write_all(bytes).and_then(|()| file.sync_all()) {
        drop(file);
        let _ = filesystem.remove_file_if_exists(path);
        return Err(MinoError::new(
            ErrorCategory::EnvironmentUnavailable,
            format!(
                "Failed to write integration artifact {}: {error}",
                filesystem.display_path(path).display()
            ),
        ));
    }
    drop(file);
    filesystem.sync_parent(path).map_err(managed_error)
}

fn unexpected_state_error(
    filesystem: &ProjectFs,
    record: &TransactionRecord,
    state: &OperationState,
) -> MinoError {
    corrupt_error(format!(
        "Integration transaction for {} has unexpected bytes at phase {}: target={:?}, backup={:?}, temporary={:?}",
        filesystem
            .display_path(&managed_path(&record.target).expect("validated target path"))
            .display(),
        record.phase.label(),
        state.target,
        state.backup,
        state.temporary
    ))
}

fn changed_bytes_error(filesystem: &ProjectFs, target: &ManagedPath) -> MinoError {
    corrupt_error(format!(
        "Integration bytes changed before writing {}",
        filesystem.display_path(target).display()
    ))
}

fn disappeared_error(filesystem: &ProjectFs, target: &ManagedPath) -> MinoError {
    corrupt_error(format!(
        "Integration path disappeared before writing {}",
        filesystem.display_path(target).display()
    ))
}

fn is_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

fn is_recoverable_state(record: &TransactionRecord, state: &OperationState) -> bool {
    let expected = record.expected_digest.as_deref();
    let replacement = record.replacement_digest.as_str();
    let safe_temporary = state.temporary.is_none()
        || state.temporary.as_deref() == Some(record.replacement_digest.as_str());
    match record.phase {
        TransactionPhase::Prepared => {
            safe_temporary
                && match expected {
                    Some(expected) => {
                        state.target.as_deref() == Some(expected) && state.backup.is_none()
                            || state.target.is_none() && state.backup.as_deref() == Some(expected)
                    }
                    None => state.target.is_none() && state.backup.is_none(),
                }
        }
        TransactionPhase::BackedUp => {
            safe_temporary
                && match expected {
                    Some(expected) => {
                        state.target.as_deref() == Some(expected) && state.backup.is_none()
                            || state.target.is_none() && state.backup.as_deref() == Some(expected)
                            || state.target.as_deref() == Some(replacement)
                                && state.backup.as_deref() == Some(expected)
                    }
                    None => {
                        state.backup.is_none()
                            && (state.target.is_none()
                                || state.target.as_deref() == Some(replacement))
                    }
                }
        }
        TransactionPhase::Published => {
            safe_temporary
                && state.target.as_deref() == Some(replacement)
                && match expected {
                    Some(expected) => {
                        state.backup.is_none() || state.backup.as_deref() == Some(expected)
                    }
                    None => state.backup.is_none(),
                }
        }
        TransactionPhase::Cleaned => {
            state.target.as_deref() == Some(replacement)
                && state.backup.is_none()
                && state.temporary.is_none()
        }
    }
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

fn corrupt_error(message: impl Into<String>) -> MinoError {
    MinoError::new(ErrorCategory::DriftDetected, message)
}
