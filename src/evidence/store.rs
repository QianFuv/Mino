//! Append-only evidence index, immutable records, replay, recovery, and audit.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use fs4::{FileExt, TryLockError};
use serde::{Deserialize, Serialize};

use crate::domain::{
    Evidence, EvidenceFields, EvidenceId, EvidenceType, PlanId, Redaction, RequestId,
};
use crate::runner::Redactor;
use crate::store::{PlanStore, StoreError, StoreErrorKind, canonical_json_bytes, sha256_digest};

use super::blob::{self, PreparedArtifact};
use super::policy::{AddEvidenceRequest, EvidenceSource};
use super::{EvidenceError, EvidenceErrorKind};

const EVIDENCE_STORAGE_VERSION: u32 = 1;
const DEFAULT_LOCK_TIMEOUT: Duration = Duration::from_secs(2);
const LOCK_RETRY_INTERVAL: Duration = Duration::from_millis(10);

/// Result of an idempotent evidence-add operation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct EvidenceAddReport {
    evidence: Evidence,
    replayed: bool,
    blob_reused: bool,
}

impl EvidenceAddReport {
    /// Returns the immutable evidence record.
    #[must_use]
    pub const fn evidence(&self) -> &Evidence {
        &self.evidence
    }

    /// Returns whether an identical request reused an existing record.
    #[must_use]
    pub const fn replayed(&self) -> bool {
        self.replayed
    }

    /// Returns whether artifact bytes already existed under the same digest.
    #[must_use]
    pub const fn blob_reused(&self) -> bool {
        self.blob_reused
    }
}

/// One deterministic evidence-storage audit finding.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct EvidenceFinding {
    code: String,
    message: String,
    evidence_id: Option<EvidenceId>,
    path: String,
}

impl EvidenceFinding {
    /// Returns the stable finding code.
    #[must_use]
    pub fn code(&self) -> &str {
        &self.code
    }

    /// Returns the explanatory finding message.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Returns the affected evidence identifier when applicable.
    #[must_use]
    pub const fn evidence_id(&self) -> Option<&EvidenceId> {
        self.evidence_id.as_ref()
    }

    /// Returns the project-relative affected storage path.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }
}

/// Reproducible summary of evidence records, blobs, and integrity findings.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct EvidenceAudit {
    plan_id: PlanId,
    record_count: usize,
    blob_count: usize,
    findings: Vec<EvidenceFinding>,
}

impl EvidenceAudit {
    /// Returns the audited plan identifier.
    #[must_use]
    pub const fn plan_id(&self) -> &PlanId {
        &self.plan_id
    }

    /// Returns the number of indexed immutable evidence records.
    #[must_use]
    pub const fn record_count(&self) -> usize {
        self.record_count
    }

    /// Returns the number of content-addressed blob files.
    #[must_use]
    pub const fn blob_count(&self) -> usize {
        self.blob_count
    }

    /// Returns sorted integrity findings.
    #[must_use]
    pub fn findings(&self) -> &[EvidenceFinding] {
        &self.findings
    }

    /// Returns whether every referenced blob is present and valid.
    #[must_use]
    pub fn is_healthy(&self) -> bool {
        self.findings.is_empty()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredEvidence {
    storage_version: u32,
    request_id: RequestId,
    request_command: Vec<String>,
    request_fingerprint: String,
    evidence: Evidence,
}

#[derive(Serialize)]
struct FingerprintInput<'a> {
    plan_id: &'a PlanId,
    expected_revision: u64,
    actor: &'a str,
    command: &'a [String],
    kind: EvidenceType,
    task_id: Option<&'a crate::domain::TaskId>,
    criterion_id: Option<&'a crate::domain::CriterionId>,
    description: &'a str,
    source: Option<&'a str>,
    artifact_digest: Option<&'a str>,
    redactions: &'a [Redaction],
    supersedes: Option<&'a EvidenceId>,
}

struct PreparedInput {
    description: String,
    source: Option<String>,
    artifact: Option<PreparedArtifact>,
    redactions: Vec<Redaction>,
    request_command: Vec<String>,
}

impl PreparedInput {
    fn capture(
        project_root: &Path,
        request: &AddEvidenceRequest,
        redactor: &Redactor,
    ) -> Result<Self, EvidenceError> {
        let (description, description_redactions) = redact_text(redactor, request.description());
        let mut redactions = description_redactions;
        let mut request_command = Vec::with_capacity(request.context().command().len());
        for argument in request.context().command() {
            let (argument, argument_redactions) = redact_text(redactor, argument);
            merge_redactions(&mut redactions, argument_redactions);
            request_command.push(argument);
        }
        let (source, artifact) = match request.source() {
            EvidenceSource::Artifact(path) => {
                let artifact =
                    blob::prepare_artifact(project_root, path, request.kind(), redactor)?;
                merge_redactions(&mut redactions, artifact.redactions.clone());
                (Some(artifact.protocol_path.clone()), Some(artifact))
            }
            EvidenceSource::Reference(reference) if request.kind() == EvidenceType::Url => {
                let (reference, reference_redactions) = redact_text(redactor, reference);
                merge_redactions(&mut redactions, reference_redactions);
                (Some(reference), None)
            }
            EvidenceSource::Reference(reference) if request.kind() == EvidenceType::Commit => {
                (Some(reference.to_ascii_lowercase()), None)
            }
            EvidenceSource::Reference(reference) => (Some(reference.clone()), None),
            EvidenceSource::Observation => (None, None),
        };
        Ok(Self {
            description,
            source,
            artifact,
            redactions,
            request_command,
        })
    }

    fn fingerprint(&self, request: &AddEvidenceRequest) -> Result<String, EvidenceError> {
        let input = FingerprintInput {
            plan_id: request.context().plan_id(),
            expected_revision: request.context().expected_revision(),
            actor: request.context().actor(),
            command: &self.request_command,
            kind: request.kind(),
            task_id: request.task_id(),
            criterion_id: request.criterion_id(),
            description: &self.description,
            source: self.source.as_deref(),
            artifact_digest: self
                .artifact
                .as_ref()
                .map(|artifact| artifact.digest.as_str()),
            redactions: &self.redactions,
            supersedes: request.supersedes(),
        };
        canonical_json_bytes(&input)
            .map(|bytes| sha256_digest(&bytes))
            .map_err(|error| serialization_error("encode evidence request", &error))
    }
}

/// Project-local immutable evidence store.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EvidenceStore {
    project_root: PathBuf,
    lock_timeout: Duration,
}

impl EvidenceStore {
    /// Creates a store with the default bounded lock timeout.
    #[must_use]
    pub fn new(project_root: impl Into<PathBuf>) -> Self {
        Self {
            project_root: project_root.into(),
            lock_timeout: DEFAULT_LOCK_TIMEOUT,
        }
    }

    /// Creates a store with an explicit positive lock timeout.
    ///
    /// # Errors
    ///
    /// Returns an invalid-request error for a zero timeout.
    pub fn with_lock_timeout(
        project_root: impl Into<PathBuf>,
        lock_timeout: Duration,
    ) -> Result<Self, EvidenceError> {
        if lock_timeout.is_zero() {
            return Err(invalid("Evidence lock timeout must be positive"));
        }
        Ok(Self {
            project_root: project_root.into(),
            lock_timeout,
        })
    }

    /// Adds one supplemental evidence record or replays the original result.
    ///
    /// # Errors
    ///
    /// Returns an error for stale revisions, unsafe inputs, request conflicts,
    /// missing bindings, lock failures, or corrupt immutable state.
    pub fn add(
        &self,
        request: &AddEvidenceRequest,
        redactor: &Redactor,
    ) -> Result<EvidenceAddReport, EvidenceError> {
        self.require_plan(request.context().plan_id())?;
        let prepared = PreparedInput::capture(&self.project_root, request, redactor)?;
        let paths = EvidencePaths::new(&self.project_root, request.context().plan_id());
        paths.prepare()?;
        let _lock = EvidenceLock::acquire(&paths.lock_file(), self.lock_timeout)?;
        let envelopes = Self::recover_locked(&paths)?;
        let fingerprint = prepared.fingerprint(request)?;
        if let Some(envelope) = envelopes
            .iter()
            .find(|envelope| envelope.request_id == *request.context().request_id())
        {
            if envelope.request_fingerprint != fingerprint {
                return Err(EvidenceError::new(
                    EvidenceErrorKind::RequestConflict,
                    format!(
                        "Request {} was reused with different evidence input",
                        request.context().request_id()
                    ),
                ));
            }
            return Ok(EvidenceAddReport {
                evidence: envelope.evidence.clone(),
                replayed: true,
                blob_reused: envelope.evidence.artifact_digest().is_some(),
            });
        }
        let plan = PlanStore::new(&self.project_root)
            .load_plan(request.context().plan_id())
            .map_err(|error| map_store_error(&error))?;
        let existing = envelopes
            .iter()
            .map(|envelope| envelope.evidence.clone())
            .collect::<Vec<_>>();
        request.validate_against(&plan, &existing)?;
        let evidence_id = next_evidence_id(envelopes.len())?;
        let artifact_digest = prepared
            .artifact
            .as_ref()
            .map(|artifact| artifact.digest.clone());
        let evidence = Evidence::new(EvidenceFields {
            id: evidence_id,
            plan_id: plan.id().clone(),
            captured_revision: plan.revision(),
            task_id: request.task_id().cloned(),
            criterion_id: request.criterion_id().cloned(),
            check_id: None,
            kind: request.kind(),
            command: Vec::new(),
            cwd: None,
            exit_code: None,
            duration_milliseconds: None,
            output_summary: Some(prepared.description.clone()),
            output_digest: Some(sha256_digest(prepared.description.as_bytes())),
            artifact_path: prepared.source.clone(),
            artifact_digest,
            actor: request.context().actor().to_owned(),
            captured_at: request.context().captured_at().clone(),
            redactions: prepared.redactions,
            supersedes: request.supersedes().cloned(),
        })
        .map_err(|error| invalid(error.to_string()))?;
        let request_command = prepared.request_command;
        let blob_reused = match &prepared.artifact {
            Some(artifact) => blob::publish_blob(&paths.blob_directory(), artifact)?,
            None => false,
        };
        let envelope = StoredEvidence {
            storage_version: EVIDENCE_STORAGE_VERSION,
            request_id: request.context().request_id().clone(),
            request_command,
            request_fingerprint: fingerprint,
            evidence: evidence.clone(),
        };
        let record_bytes = canonical_envelope(&envelope)?;
        blob::publish_immutable(&paths.record(&evidence), &record_bytes)?;
        append_index(&paths.index_file(), &record_bytes)?;
        Ok(EvidenceAddReport {
            evidence,
            replayed: false,
            blob_reused,
        })
    }

    /// Lists immutable evidence in monotonic identifier order.
    ///
    /// # Errors
    ///
    /// Returns an error for a missing plan, lock failure, or corrupt state.
    pub fn list(&self, plan_id: &PlanId) -> Result<Vec<Evidence>, EvidenceError> {
        self.require_plan(plan_id)?;
        let paths = EvidencePaths::new(&self.project_root, plan_id);
        paths.prepare()?;
        let _lock = EvidenceLock::acquire(&paths.lock_file(), self.lock_timeout)?;
        Self::recover_locked(&paths).map(|envelopes| {
            envelopes
                .into_iter()
                .map(|envelope| envelope.evidence)
                .collect()
        })
    }

    /// Loads one immutable evidence record.
    ///
    /// # Errors
    ///
    /// Returns an error for a missing plan/evidence ID, lock failure, or corrupt
    /// state.
    pub fn show(
        &self,
        plan_id: &PlanId,
        evidence_id: &EvidenceId,
    ) -> Result<Evidence, EvidenceError> {
        self.list(plan_id)?
            .into_iter()
            .find(|evidence| evidence.id() == evidence_id)
            .ok_or_else(|| {
                EvidenceError::new(
                    EvidenceErrorKind::EvidenceNotFound,
                    format!("Evidence {evidence_id} does not exist in plan {plan_id}"),
                )
            })
    }

    /// Audits record/index agreement and every content-addressed blob.
    ///
    /// # Errors
    ///
    /// Returns an error for a missing plan, lock failure, or corrupt record/index
    /// structure. Missing, mismatched, and orphan blobs are returned as findings.
    pub fn audit(&self, plan_id: &PlanId) -> Result<EvidenceAudit, EvidenceError> {
        self.require_plan(plan_id)?;
        let paths = EvidencePaths::new(&self.project_root, plan_id);
        paths.prepare()?;
        let _lock = EvidenceLock::acquire(&paths.lock_file(), self.lock_timeout)?;
        let envelopes = Self::recover_locked(&paths)?;
        let mut findings = Vec::new();
        let mut expected_blobs = BTreeMap::<String, EvidenceId>::new();
        for envelope in &envelopes {
            let Some(digest) = envelope.evidence.artifact_digest() else {
                continue;
            };
            expected_blobs
                .entry(digest.to_owned())
                .or_insert_with(|| envelope.evidence.id().clone());
            let path = blob::blob_path(&paths.blob_directory(), digest)?;
            if !path.exists() {
                findings.push(finding(
                    "evidence_blob_missing",
                    format!(
                        "Evidence {} references a missing blob {digest}",
                        envelope.evidence.id()
                    ),
                    Some(envelope.evidence.id().clone()),
                    &self.project_relative(&path),
                ));
                continue;
            }
            let bytes =
                fs::read(&path).map_err(|error| io_error("read evidence blob", &path, &error))?;
            if sha256_digest(&bytes) != digest {
                findings.push(finding(
                    "evidence_blob_digest_mismatch",
                    format!(
                        "Evidence {} blob bytes do not match {digest}",
                        envelope.evidence.id()
                    ),
                    Some(envelope.evidence.id().clone()),
                    &self.project_relative(&path),
                ));
            }
        }
        let blob_files = list_blob_files(&paths.blob_directory())?;
        for path in &blob_files {
            let digest = digest_from_blob_path(path);
            if digest
                .as_ref()
                .is_none_or(|digest| !expected_blobs.contains_key(digest))
            {
                findings.push(finding(
                    "evidence_blob_orphaned",
                    format!(
                        "Blob {} is not referenced by indexed evidence",
                        path.display()
                    ),
                    None,
                    &self.project_relative(path),
                ));
            }
        }
        findings.sort_by(|left, right| {
            (
                left.code.as_str(),
                left.evidence_id.as_ref(),
                left.path.as_str(),
            )
                .cmp(&(
                    right.code.as_str(),
                    right.evidence_id.as_ref(),
                    right.path.as_str(),
                ))
        });
        Ok(EvidenceAudit {
            plan_id: plan_id.clone(),
            record_count: envelopes.len(),
            blob_count: blob_files.len(),
            findings,
        })
    }

    fn recover_locked(paths: &EvidencePaths) -> Result<Vec<StoredEvidence>, EvidenceError> {
        truncate_partial_index(&paths.index_file())?;
        let mut indexed = read_index(&paths.index_file())?;
        validate_sequence(paths.plan_id(), &indexed)?;
        let records = list_record_files(&paths.record_directory())?;
        if records.len() < indexed.len() {
            return Err(corrupt(format!(
                "Evidence index has {} entries but only {} immutable records",
                indexed.len(),
                records.len()
            )));
        }
        for (index, envelope) in indexed.iter().enumerate() {
            let (_, path) = &records[index];
            if envelope.evidence.id() != &records[index].0 {
                return Err(corrupt(format!(
                    "Evidence record {} does not match index position {}",
                    path.display(),
                    index + 1
                )));
            }
            let record: StoredEvidence = read_canonical(path)?;
            if &record != envelope {
                return Err(corrupt(format!(
                    "Evidence record {} differs from its index entry",
                    path.display()
                )));
            }
        }
        for (_, path) in records.into_iter().skip(indexed.len()) {
            let envelope: StoredEvidence = read_canonical(&path)?;
            indexed.push(envelope);
            validate_sequence(paths.plan_id(), &indexed)?;
            let bytes = canonical_envelope(
                indexed
                    .last()
                    .expect("a recovered record was just appended"),
            )?;
            append_index(&paths.index_file(), &bytes)?;
        }
        Ok(indexed)
    }

    fn require_plan(&self, plan_id: &PlanId) -> Result<(), EvidenceError> {
        PlanStore::new(&self.project_root)
            .load_plan(plan_id)
            .map(|_| ())
            .map_err(|error| map_store_error(&error))
    }

    fn project_relative(&self, path: &Path) -> String {
        path.strip_prefix(&self.project_root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    }
}

struct EvidencePaths<'a> {
    project_root: &'a Path,
    plan_id: &'a PlanId,
}

impl<'a> EvidencePaths<'a> {
    fn new(project_root: &'a Path, plan_id: &'a PlanId) -> Self {
        Self {
            project_root,
            plan_id,
        }
    }

    fn directory(&self) -> PathBuf {
        self.project_root
            .join(".mino")
            .join("plans")
            .join(self.plan_id.as_str())
            .join("evidence")
    }

    fn index_file(&self) -> PathBuf {
        self.directory().join("index.jsonl")
    }

    fn lock_file(&self) -> PathBuf {
        self.directory().join("evidence.lock")
    }

    fn record_directory(&self) -> PathBuf {
        self.directory().join("records")
    }

    fn blob_directory(&self) -> PathBuf {
        self.directory().join("blobs")
    }

    fn record(&self, evidence: &Evidence) -> PathBuf {
        self.record_directory()
            .join(format!("{}.json", evidence.id()))
    }

    fn prepare(&self) -> Result<(), EvidenceError> {
        for directory in [
            self.directory(),
            self.record_directory(),
            self.blob_directory(),
        ] {
            fs::create_dir_all(&directory).map_err(|error| {
                io_error("create evidence storage directory", &directory, &error)
            })?;
        }
        Ok(())
    }

    const fn plan_id(&self) -> &PlanId {
        self.plan_id
    }
}

struct EvidenceLock {
    file: File,
}

impl EvidenceLock {
    fn acquire(path: &Path, timeout: Duration) -> Result<Self, EvidenceError> {
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .truncate(false)
            .write(true)
            .open(path)
            .map_err(|error| io_error("open evidence lock", path, &error))?;
        let started = Instant::now();
        loop {
            match FileExt::try_lock(&file) {
                Ok(()) => return Ok(Self { file }),
                Err(TryLockError::WouldBlock) if started.elapsed() < timeout => {
                    thread::sleep(
                        LOCK_RETRY_INTERVAL.min(timeout.saturating_sub(started.elapsed())),
                    );
                }
                Err(TryLockError::WouldBlock) => {
                    return Err(EvidenceError::new(
                        EvidenceErrorKind::LockTimeout,
                        format!(
                            "Timed out after {} ms acquiring evidence lock {}",
                            timeout.as_millis(),
                            path.display()
                        ),
                    ));
                }
                Err(TryLockError::Error(error)) => {
                    return Err(io_error("lock evidence store", path, &error));
                }
            }
        }
    }
}

impl Drop for EvidenceLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

fn next_evidence_id(existing_count: usize) -> Result<EvidenceId, EvidenceError> {
    let number = existing_count
        .checked_add(1)
        .ok_or_else(|| invalid("Evidence identifier sequence overflowed"))?;
    EvidenceId::parse(format!("E{number:04}")).map_err(|error| invalid(error.to_string()))
}

fn validate_sequence(plan_id: &PlanId, envelopes: &[StoredEvidence]) -> Result<(), EvidenceError> {
    let mut requests = BTreeSet::new();
    for (index, envelope) in envelopes.iter().enumerate() {
        if envelope.storage_version != EVIDENCE_STORAGE_VERSION {
            return Err(corrupt(format!(
                "Evidence {} uses unsupported storage version {}",
                envelope.evidence.id(),
                envelope.storage_version
            )));
        }
        if envelope.request_command.is_empty()
            || envelope
                .request_command
                .iter()
                .any(|part| part.trim().is_empty())
        {
            return Err(corrupt(format!(
                "Evidence {} has no canonical request command",
                envelope.evidence.id()
            )));
        }
        envelope
            .evidence
            .validate_invariants()
            .map_err(|error| corrupt(error.to_string()))?;
        let expected = next_evidence_id(index)?;
        if envelope.evidence.id() != &expected || envelope.evidence.plan_id() != plan_id {
            return Err(corrupt(format!(
                "Evidence index position {} has inconsistent identity",
                index + 1
            )));
        }
        if envelope.evidence.captured_revision().is_none() {
            return Err(corrupt(format!(
                "Stored evidence {} has no captured revision",
                envelope.evidence.id()
            )));
        }
        blob::validated_digest(&envelope.request_fingerprint)
            .map_err(|error| corrupt(error.to_string()))?;
        if !requests.insert(envelope.request_id.as_str()) {
            return Err(corrupt(format!(
                "Evidence index repeats request {}",
                envelope.request_id
            )));
        }
    }
    Ok(())
}

fn canonical_envelope(envelope: &StoredEvidence) -> Result<Vec<u8>, EvidenceError> {
    canonical_json_bytes(envelope)
        .map_err(|error| serialization_error("encode evidence record", &error))
}

fn read_canonical<T>(path: &Path) -> Result<T, EvidenceError>
where
    T: for<'de> Deserialize<'de> + Serialize,
{
    let bytes = fs::read(path).map_err(|error| io_error("read evidence record", path, &error))?;
    let value = serde_json::from_slice(&bytes).map_err(|error| {
        corrupt(format!(
            "Failed to decode evidence record {}: {error}",
            path.display()
        ))
    })?;
    let canonical = canonical_json_bytes(&value)
        .map_err(|error| serialization_error("re-encode evidence record", &error))?;
    if canonical != bytes {
        return Err(corrupt(format!(
            "Evidence record {} is not canonical",
            path.display()
        )));
    }
    Ok(value)
}

fn read_index(path: &Path) -> Result<Vec<StoredEvidence>, EvidenceError> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let bytes = fs::read(path).map_err(|error| io_error("read evidence index", path, &error))?;
    let mut envelopes = Vec::new();
    for line in bytes.split_inclusive(|byte| *byte == b'\n') {
        if line.is_empty() {
            continue;
        }
        let envelope: StoredEvidence = serde_json::from_slice(line).map_err(|error| {
            corrupt(format!(
                "Failed to decode evidence index {}: {error}",
                path.display()
            ))
        })?;
        if canonical_envelope(&envelope)? != line {
            return Err(corrupt(format!(
                "Evidence index {} contains a non-canonical entry",
                path.display()
            )));
        }
        envelopes.push(envelope);
    }
    Ok(envelopes)
}

fn truncate_partial_index(path: &Path) -> Result<(), EvidenceError> {
    if !path.exists() {
        return Ok(());
    }
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|error| io_error("open evidence index", path, &error))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|error| io_error("read evidence index", path, &error))?;
    let complete_length = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |index| index.saturating_add(1));
    if complete_length != bytes.len() {
        file.set_len(u64::try_from(complete_length).unwrap_or(u64::MAX))
            .and_then(|()| file.seek(SeekFrom::Start(0)).map(|_| ()))
            .and_then(|()| file.sync_all())
            .map_err(|error| io_error("recover evidence index", path, &error))?;
    }
    Ok(())
}

fn append_index(path: &Path, bytes: &[u8]) -> Result<(), EvidenceError> {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| io_error("open evidence index", path, &error))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| io_error("append evidence index", path, &error))
}

fn list_record_files(directory: &Path) -> Result<Vec<(EvidenceId, PathBuf)>, EvidenceError> {
    let mut records = Vec::new();
    for entry in fs::read_dir(directory)
        .map_err(|error| io_error("read evidence record directory", directory, &error))?
    {
        let entry =
            entry.map_err(|error| io_error("read evidence record entry", directory, &error))?;
        if !entry
            .file_type()
            .map_err(|error| io_error("inspect evidence record", &entry.path(), &error))?
            .is_file()
            || entry
                .path()
                .extension()
                .is_none_or(|extension| extension != "json")
        {
            continue;
        }
        let stem = entry
            .path()
            .file_stem()
            .and_then(|stem| stem.to_str())
            .ok_or_else(|| corrupt("Evidence record has a non-UTF-8 file name"))?
            .to_owned();
        let evidence_id = EvidenceId::parse(stem)
            .map_err(|error| corrupt(format!("Invalid evidence record name: {error}")))?;
        records.push((evidence_id, entry.path()));
    }
    records.sort_by_key(|(evidence_id, _)| evidence_number(evidence_id));
    for (index, (evidence_id, _)) in records.iter().enumerate() {
        if evidence_id != &next_evidence_id(index)? {
            return Err(corrupt("Evidence record identifiers are not contiguous"));
        }
    }
    Ok(records)
}

fn evidence_number(evidence_id: &EvidenceId) -> u64 {
    evidence_id
        .as_str()
        .strip_prefix('E')
        .and_then(|number| number.parse().ok())
        .unwrap_or(u64::MAX)
}

fn list_blob_files(directory: &Path) -> Result<Vec<PathBuf>, EvidenceError> {
    let mut files = Vec::new();
    for entry in fs::read_dir(directory)
        .map_err(|error| io_error("read evidence blob directory", directory, &error))?
    {
        let entry =
            entry.map_err(|error| io_error("read evidence blob entry", directory, &error))?;
        if entry
            .file_type()
            .map_err(|error| io_error("inspect evidence blob", &entry.path(), &error))?
            .is_file()
            && entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "blob")
        {
            files.push(entry.path());
        }
    }
    files.sort();
    Ok(files)
}

fn digest_from_blob_path(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    (stem.len() == 64
        && stem
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)))
    .then(|| format!("sha256:{stem}"))
}

fn redact_text(redactor: &Redactor, text: &str) -> (String, Vec<Redaction>) {
    let (redacted, applied) = redactor.redact(text);
    let redactions = applied
        .into_iter()
        .map(|redaction| Redaction::new(redaction.rule_id(), redaction.replacements()))
        .collect();
    (redacted, redactions)
}

fn merge_redactions(target: &mut Vec<Redaction>, additional: Vec<Redaction>) {
    let mut counts = target
        .drain(..)
        .map(|redaction| (redaction.rule_id().to_owned(), redaction.replacements()))
        .collect::<BTreeMap<_, _>>();
    for redaction in additional {
        counts
            .entry(redaction.rule_id().to_owned())
            .and_modify(|count| *count = count.saturating_add(redaction.replacements()))
            .or_insert(redaction.replacements());
    }
    target.extend(
        counts
            .into_iter()
            .map(|(id, count)| Redaction::new(id, count)),
    );
}

fn finding(
    code: impl Into<String>,
    message: impl Into<String>,
    evidence_id: Option<EvidenceId>,
    path: &str,
) -> EvidenceFinding {
    EvidenceFinding {
        code: code.into(),
        message: message.into(),
        evidence_id,
        path: path.to_owned(),
    }
}

fn map_store_error(error: &StoreError) -> EvidenceError {
    let kind = match error.kind() {
        StoreErrorKind::PlanNotFound => EvidenceErrorKind::PlanNotFound,
        StoreErrorKind::StaleRevision => EvidenceErrorKind::RevisionConflict,
        StoreErrorKind::RequestConflict => EvidenceErrorKind::RequestConflict,
        StoreErrorKind::CorruptState => EvidenceErrorKind::CorruptStore,
        StoreErrorKind::Serialization => EvidenceErrorKind::Serialization,
        StoreErrorKind::LockTimeout => EvidenceErrorKind::LockTimeout,
        StoreErrorKind::Io
        | StoreErrorKind::PlanAlreadyExists
        | StoreErrorKind::InvalidMutation
        | StoreErrorKind::InjectedFailure => EvidenceErrorKind::Io,
    };
    EvidenceError::new(kind, error.to_string())
}

fn invalid(message: impl Into<String>) -> EvidenceError {
    EvidenceError::new(EvidenceErrorKind::InvalidRequest, message)
}

fn corrupt(message: impl Into<String>) -> EvidenceError {
    EvidenceError::new(EvidenceErrorKind::CorruptStore, message)
}

fn serialization_error(action: &str, error: &StoreError) -> EvidenceError {
    EvidenceError::new(
        EvidenceErrorKind::Serialization,
        format!("Failed to {action}: {error}"),
    )
}

fn io_error(action: &str, path: &Path, error: &std::io::Error) -> EvidenceError {
    EvidenceError::new(
        EvidenceErrorKind::Io,
        format!("Failed to {action} {}: {error}", path.display()),
    )
}
