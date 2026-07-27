//! Acceptance tests for immutable supplemental evidence storage and CLI queries.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(unix)]
use std::os::unix::fs::symlink;
#[cfg(windows)]
use std::os::windows::fs::symlink_dir;

use mino::domain::{
    AcceptanceCriterion, CriterionId, EvidenceId, EvidenceType, Plan, PlanId, RequestId, Task,
    TaskId, Timestamp,
};
use mino::evidence::{
    AddEvidenceRequest, EvidenceErrorKind, EvidenceRequestContext, EvidenceSource, EvidenceStore,
};
use mino::runner::{RedactionRule, Redactor};
use mino::store::{MutationRequest, PlanStore};
use serde_json::Value;

static NEXT_TEMP_DIRECTORY: AtomicU64 = AtomicU64::new(1);

struct TestProject {
    path: PathBuf,
}

impl TestProject {
    fn new(label: &str, with_task: bool) -> Self {
        let sequence = NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "mino-evidence-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("temporary project should be created");
        fs::write(
            path.join("Cargo.toml"),
            "[package]\nname = \"evidence-fixture\"\nversion = \"0.1.0\"\n",
        )
        .expect("fixture manifest should be written");
        let project = Self { path };
        create_plan(project.path(), with_task);
        project
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TestProject {
    fn drop(&mut self) {
        let temporary_root = std::env::temp_dir();
        if self.path.starts_with(&temporary_root)
            && self
                .path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("mino-evidence-"))
        {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

fn plan_id() -> PlanId {
    PlanId::parse("2026-07-25-evidence-contract").expect("plan ID should be valid")
}

fn task_id() -> TaskId {
    TaskId::parse("T1").expect("task ID should be valid")
}

fn criterion_id() -> CriterionId {
    CriterionId::parse("T1-A1").expect("criterion ID should be valid")
}

fn evidence_id(number: usize) -> EvidenceId {
    EvidenceId::parse(format!("E{number:04}")).expect("evidence ID should be valid")
}

fn request_id(sequence: u64) -> RequestId {
    RequestId::parse(format!("00000000-0000-0000-0000-{sequence:012x}"))
        .expect("request ID should be valid")
}

fn timestamp() -> Timestamp {
    Timestamp::parse("2026-07-25T12:00:00Z").expect("timestamp should be valid")
}

fn create_plan(root: &Path, with_task: bool) {
    let store = PlanStore::new(root);
    store
        .create_plan(
            &Plan::new(plan_id(), "Capture immutable evidence.", timestamp()),
            request_id(1),
            "codex",
            vec!["mino".to_owned(), "plan".to_owned(), "create".to_owned()],
        )
        .expect("plan should be created");
    if with_task {
        let mut task = Task::new(task_id(), "Capture evidence", Vec::new());
        task.add_acceptance_criterion(AcceptanceCriterion::new(
            criterion_id(),
            "Evidence is immutable",
        ))
        .expect("criterion should be added");
        store
            .commit(
                &plan_id(),
                MutationRequest::new(
                    1,
                    request_id(2),
                    "codex",
                    vec!["mino".to_owned(), "plan".to_owned(), "task".to_owned()],
                    vec!["tasks".to_owned(), "task_order".to_owned()],
                )
                .expect("mutation should be valid"),
                |plan| plan.add_task(task, timestamp()),
            )
            .expect("task should be committed");
    }
}

fn redactor() -> Redactor {
    Redactor::new(vec![RedactionRule::literal("fixture-secret", "top-secret")])
        .expect("redactor should compile")
}

fn add_request(
    sequence: u64,
    revision: u64,
    kind: EvidenceType,
    source: EvidenceSource,
    description: &str,
) -> AddEvidenceRequest {
    AddEvidenceRequest::new(
        EvidenceRequestContext::new(
            plan_id(),
            revision,
            request_id(sequence),
            "codex",
            vec![
                "mino".to_owned(),
                "evidence".to_owned(),
                "add".to_owned(),
                "--description".to_owned(),
                description.to_owned(),
            ],
            timestamp(),
        )
        .expect("context should be valid"),
        kind,
        source,
        description,
    )
    .expect("evidence request should be valid")
}

fn evidence_directory(root: &Path) -> PathBuf {
    root.join(".mino")
        .join("plans")
        .join(plan_id().as_str())
        .join("evidence")
}

fn record_path(root: &Path, id: &EvidenceId) -> PathBuf {
    evidence_directory(root)
        .join("records")
        .join(format!("{id}.json"))
}

fn blob_path(root: &Path, digest: &str) -> PathBuf {
    evidence_directory(root).join("blobs").join(format!(
        "{}.blob",
        digest
            .strip_prefix("sha256:")
            .expect("digest should be prefixed")
    ))
}

#[cfg(any(unix, windows))]
fn create_directory_symlink(target: &Path, link: &Path) -> bool {
    #[cfg(unix)]
    let result = symlink(target, link);
    #[cfg(windows)]
    let result = symlink_dir(target, link);
    match result {
        Ok(()) => true,
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => false,
        Err(error) => panic!("evidence symlink should be created: {error}"),
    }
}

#[cfg(any(unix, windows))]
#[test]
fn symlinked_blob_directory_cannot_publish_outside_the_project() {
    let project = TestProject::new("blob-symlink", false);
    let external = TestProject::new("blob-symlink-external", false);
    let evidence = evidence_directory(project.path());
    fs::create_dir_all(&evidence).expect("evidence directory should be created");
    let sentinel = external.path().join("sentinel.txt");
    fs::write(&sentinel, b"outside\n").expect("outside sentinel should be written");
    if !create_directory_symlink(external.path(), &evidence.join("blobs")) {
        return;
    }
    fs::write(project.path().join("artifact.txt"), b"artifact\n")
        .expect("artifact should be written");
    let request = add_request(
        10,
        1,
        EvidenceType::File,
        EvidenceSource::Artifact(PathBuf::from("artifact.txt")),
        "Capture a file",
    );

    let error = EvidenceStore::new(project.path())
        .add(&request, &redactor())
        .expect_err("symlinked blob directory must be rejected");
    assert_eq!(error.kind(), EvidenceErrorKind::CorruptStore);
    assert_eq!(
        fs::read(&sentinel).expect("outside sentinel should remain readable"),
        b"outside\n"
    );
    assert!(
        fs::read_dir(external.path())
            .expect("outside directory should remain readable")
            .all(|entry| entry
                .expect("outside entry should be readable")
                .path()
                .extension()
                .is_none_or(|extension| extension != "blob"))
    );
    assert!(!evidence.join("index.jsonl").exists());
    assert!(
        fs::read_dir(evidence.join("records"))
            .expect("record directory should remain empty")
            .next()
            .is_none()
    );
}

#[test]
fn supplemental_types_are_monotonic_queryable_deduplicated_and_redacted() {
    let project = TestProject::new("types", true);
    fs::write(project.path().join("artifact.txt"), "top-secret\n")
        .expect("text artifact should be written");
    fs::write(project.path().join("artifact.log"), "top-secret\n")
        .expect("log artifact should be written");
    fs::write(project.path().join("artifact.diff"), "top-secret\n")
        .expect("diff artifact should be written");
    fs::write(project.path().join("screenshot.bin"), [0_u8, 159, 146, 150])
        .expect("binary artifact should be written");
    let store = EvidenceStore::new(project.path());
    let redactor = redactor();
    let reports = supplemental_requests()
        .into_iter()
        .map(|request| {
            store
                .add(&request, &redactor)
                .expect("evidence should be captured")
        })
        .collect::<Vec<_>>();
    assert_eq!(
        reports
            .iter()
            .map(|report| report.evidence().id().clone())
            .collect::<Vec<_>>(),
        (1..=8).map(evidence_id).collect::<Vec<_>>()
    );
    assert!(
        reports
            .iter()
            .all(|report| report.evidence().captured_revision() == Some(2))
    );
    assert_eq!(reports[0].evidence().artifact_path(), Some("artifact.txt"));
    let text_digests = [0, 1, 4].map(|index| {
        reports[index]
            .evidence()
            .artifact_digest()
            .expect("text artifact should have a digest")
    });
    assert!(text_digests.windows(2).all(|pair| pair[0] == pair[1]));
    let serialized = serde_json::to_string(&reports).expect("reports should serialize");
    assert!(!serialized.contains("top-secret"));
    assert!(!serialized.contains("hidden"));
    assert!(serialized.contains("[REDACTED]"));
    let text_blob = fs::read_to_string(blob_path(project.path(), text_digests[0]))
        .expect("redacted text blob should be readable");
    assert_eq!(text_blob, "[REDACTED]\n");
    assert_eq!(
        store
            .show(&plan_id(), &evidence_id(1))
            .expect("evidence should load"),
        reports[0].evidence().clone()
    );
    assert_eq!(
        store.list(&plan_id()).expect("evidence should list").len(),
        8
    );
    let audit = store.audit(&plan_id()).expect("evidence should audit");
    assert!(audit.is_healthy());
    assert_eq!(audit.record_count(), 8);
    assert_eq!(audit.blob_count(), 2);
}

fn supplemental_requests() -> [AddEvidenceRequest; 8] {
    [
        add_request(
            100,
            2,
            EvidenceType::File,
            EvidenceSource::Artifact(PathBuf::from("./artifact.txt")),
            "File contains top-secret",
        )
        .with_criterion(task_id(), criterion_id()),
        add_request(
            101,
            2,
            EvidenceType::GitDiff,
            EvidenceSource::Artifact(PathBuf::from("artifact.diff")),
            "Captured diff",
        )
        .with_task(task_id()),
        add_request(
            102,
            2,
            EvidenceType::Commit,
            EvidenceSource::Reference("abcdef1234567".to_owned()),
            "Recorded commit",
        )
        .with_task(task_id()),
        add_request(
            103,
            2,
            EvidenceType::Url,
            EvidenceSource::Reference("https://example.test/report?token=hidden".to_owned()),
            "External report",
        ),
        add_request(
            104,
            2,
            EvidenceType::Log,
            EvidenceSource::Artifact(PathBuf::from("artifact.log")),
            "Captured log",
        )
        .with_task(task_id()),
        add_request(
            105,
            2,
            EvidenceType::Screenshot,
            EvidenceSource::Artifact(PathBuf::from("screenshot.bin")),
            "Captured screenshot",
        ),
        add_request(
            106,
            2,
            EvidenceType::ManualObservation,
            EvidenceSource::Observation,
            "Observed behavior manually",
        )
        .with_task(task_id()),
        add_request(
            107,
            2,
            EvidenceType::AcceptedException,
            EvidenceSource::Reference("approval-reference-42".to_owned()),
            "Approved exception",
        )
        .with_criterion(task_id(), criterion_id()),
    ]
}

#[test]
fn replay_and_supersession_preserve_historical_bytes() {
    let project = TestProject::new("supersession", false);
    fs::write(project.path().join("result.txt"), "first\n").expect("artifact should be written");
    let store = EvidenceStore::new(project.path());
    let redactor = redactor();
    let request = add_request(
        200,
        1,
        EvidenceType::File,
        EvidenceSource::Artifact(PathBuf::from("result.txt")),
        "Original result",
    );
    let first = store
        .add(&request, &redactor)
        .expect("original should be stored");
    let first_record_path = record_path(project.path(), first.evidence().id());
    let first_record = fs::read(&first_record_path).expect("record should be readable");
    let index_path = evidence_directory(project.path()).join("index.jsonl");
    let first_index = fs::read(&index_path).expect("index should be readable");
    let replay = store
        .add(&request, &redactor)
        .expect("same request should replay");
    assert!(replay.replayed());
    assert_eq!(replay.evidence(), first.evidence());

    fs::write(project.path().join("result.txt"), "changed\n").expect("artifact should change");
    let conflict = store
        .add(&request, &redactor)
        .expect_err("changed bytes must conflict under the same request");
    assert_eq!(conflict.kind(), EvidenceErrorKind::RequestConflict);
    fs::write(project.path().join("result.txt"), "first\n").expect("artifact should be restored");
    let correction = store
        .add(
            &add_request(
                201,
                1,
                EvidenceType::File,
                EvidenceSource::Artifact(PathBuf::from("result.txt")),
                "Corrected description",
            )
            .superseding(first.evidence().id().clone()),
            &redactor,
        )
        .expect("correction should be stored");
    assert_eq!(correction.evidence().id(), &evidence_id(2));
    assert_eq!(
        correction.evidence().supersedes(),
        Some(first.evidence().id())
    );
    assert!(correction.blob_reused());
    assert_eq!(
        fs::read(&first_record_path).expect("historical record should remain"),
        first_record
    );
    assert!(
        fs::read(&index_path)
            .expect("index should remain readable")
            .starts_with(&first_index)
    );
    let duplicate_correction = store
        .add(
            &add_request(
                202,
                1,
                EvidenceType::File,
                EvidenceSource::Artifact(PathBuf::from("result.txt")),
                "Second correction",
            )
            .superseding(first.evidence().id().clone()),
            &redactor,
        )
        .expect_err("one record cannot have two direct corrections");
    assert_eq!(
        duplicate_correction.kind(),
        EvidenceErrorKind::InvalidRequest
    );
    assert_eq!(
        store.list(&plan_id()).expect("evidence should list").len(),
        2
    );
}

#[test]
fn unsafe_paths_and_blob_damage_are_reported_without_record_mutation() {
    let project = TestProject::new("audit", false);
    let outside = std::env::temp_dir().join(format!(
        "mino-evidence-outside-{}-{}",
        std::process::id(),
        NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed)
    ));
    fs::write(&outside, "outside\n").expect("outside file should be written");
    let store = EvidenceStore::new(project.path());
    let redactor = redactor();
    let traversal = store
        .add(
            &add_request(
                300,
                1,
                EvidenceType::File,
                EvidenceSource::Artifact(PathBuf::from("../outside.txt")),
                "Unsafe path",
            ),
            &redactor,
        )
        .expect_err("parent traversal must be rejected");
    assert_eq!(traversal.kind(), EvidenceErrorKind::InvalidRequest);

    let link = project.path().join("outside-link.txt");
    if create_file_symlink(&outside, &link) {
        let escaped = store
            .add(
                &add_request(
                    301,
                    1,
                    EvidenceType::File,
                    EvidenceSource::Artifact(PathBuf::from("outside-link.txt")),
                    "Escaping link",
                ),
                &redactor,
            )
            .expect_err("outside symlink must be rejected");
        assert_eq!(escaped.kind(), EvidenceErrorKind::InvalidRequest);
    }

    fs::write(project.path().join("inside.txt"), "inside\n")
        .expect("inside artifact should be written");
    let report = store
        .add(
            &add_request(
                302,
                1,
                EvidenceType::File,
                EvidenceSource::Artifact(PathBuf::from("inside.txt")),
                "Inside artifact",
            ),
            &redactor,
        )
        .expect("inside artifact should store");
    let record = fs::read(record_path(project.path(), report.evidence().id()))
        .expect("record should be readable");
    let digest = report
        .evidence()
        .artifact_digest()
        .expect("artifact should have digest");
    let blob = blob_path(project.path(), digest);
    fs::remove_file(&blob).expect("blob damage should be injected");
    let missing = store.audit(&plan_id()).expect("missing blob should audit");
    assert_eq!(missing.findings()[0].code(), "evidence_blob_missing");
    fs::write(&blob, "tampered\n").expect("tampered blob should be written");
    let mismatched = store.audit(&plan_id()).expect("bad blob should audit");
    assert_eq!(
        mismatched.findings()[0].code(),
        "evidence_blob_digest_mismatch"
    );
    let orphan = evidence_directory(project.path())
        .join("blobs")
        .join(format!("{}.blob", "a".repeat(64)));
    fs::write(orphan, "orphan\n").expect("orphan blob should be written");
    let findings = store.audit(&plan_id()).expect("orphan blob should audit");
    assert!(
        findings
            .findings()
            .iter()
            .any(|finding| finding.code() == "evidence_blob_orphaned")
    );
    assert_eq!(
        fs::read(record_path(project.path(), report.evidence().id()))
            .expect("record should remain readable"),
        record
    );
    let _ = fs::remove_file(outside);
}

#[test]
fn index_recovery_restores_only_the_missing_or_partial_tail() {
    let project = TestProject::new("recovery", false);
    fs::write(project.path().join("result.txt"), "result\n").expect("artifact should be written");
    let store = EvidenceStore::new(project.path());
    store
        .add(
            &add_request(
                400,
                1,
                EvidenceType::File,
                EvidenceSource::Artifact(PathBuf::from("result.txt")),
                "Recoverable result",
            ),
            &redactor(),
        )
        .expect("evidence should store");
    let index = evidence_directory(project.path()).join("index.jsonl");
    let complete = fs::read(&index).expect("index should be readable");
    OpenOptions::new()
        .append(true)
        .open(&index)
        .and_then(|mut file| file.write_all(b"{\"partial\""))
        .expect("partial index tail should be injected");
    assert_eq!(
        store
            .list(&plan_id())
            .expect("partial tail should recover")
            .len(),
        1
    );
    assert_eq!(fs::read(&index).expect("index should recover"), complete);
    fs::write(&index, []).expect("missing index publication should be injected");
    assert_eq!(
        store
            .list(&plan_id())
            .expect("orphan record should roll forward")
            .len(),
        1
    );
    assert_eq!(
        fs::read(&index).expect("index should roll forward"),
        complete
    );
}

#[test]
fn evidence_index_rejects_a_record_one_byte_over_the_managed_limit() {
    let project = TestProject::new("oversized-index-record", false);
    let store = EvidenceStore::new(project.path());
    assert!(
        store
            .list(&plan_id())
            .expect("empty evidence store should initialize")
            .is_empty()
    );
    let index = evidence_directory(project.path()).join("index.jsonl");
    fs::write(&index, vec![b'x'; 4 * 1_024 * 1_024 + 1])
        .expect("oversized evidence record should be injected");

    let error = store
        .list(&plan_id())
        .expect_err("oversized evidence record must be rejected before parsing");
    assert_eq!(error.kind(), EvidenceErrorKind::CorruptStore);
    assert!(error.message().contains("exceeds the 4194304-byte limit"));
}

#[test]
fn evidence_cli_add_list_show_and_retry_are_strict() {
    let project = TestProject::new("cli", false);
    fs::write(project.path().join("report.txt"), "report\n").expect("report should be written");
    let request = "00000000-0000-0000-0000-000000000500";
    let plan_identifier = plan_id();
    let add_arguments = [
        "evidence",
        "add",
        "--plan",
        plan_identifier.as_str(),
        "--type",
        "file",
        "--path",
        "report.txt",
        "--description",
        "CLI report",
        "--expect-revision",
        "1",
        "--request-id",
        request,
        "--actor",
        "codex",
        "--format",
        "json",
        "--no-input",
    ];
    let first = run_mino(project.path(), &add_arguments);
    assert!(first.status.success(), "{}", stderr(&first));
    let first_json = stdout_json(&first);
    assert_eq!(first_json["evidence"]["id"], "E0001");
    assert_eq!(first_json["replayed"], false);
    let replay = run_mino(project.path(), &add_arguments);
    assert!(replay.status.success(), "{}", stderr(&replay));
    assert_eq!(stdout_json(&replay)["replayed"], true);

    let list = run_mino(
        project.path(),
        &[
            "evidence",
            "list",
            "--plan",
            plan_identifier.as_str(),
            "--format",
            "json",
            "--no-input",
        ],
    );
    assert!(list.status.success(), "{}", stderr(&list));
    assert_eq!(
        stdout_json(&list)["evidence"]
            .as_array()
            .expect("list payload should be an array")
            .len(),
        1
    );
    let show = run_mino(
        project.path(),
        &[
            "evidence",
            "show",
            "--plan",
            plan_identifier.as_str(),
            "--evidence",
            "E0001",
            "--format",
            "json",
            "--no-input",
        ],
    );
    assert!(show.status.success(), "{}", stderr(&show));
    assert_eq!(stdout_json(&show)["id"], "E0001");

    let stale = run_mino(
        project.path(),
        &[
            "evidence",
            "add",
            "--plan",
            plan_identifier.as_str(),
            "--type",
            "manual-observation",
            "--description",
            "Stale",
            "--expect-revision",
            "2",
            "--request-id",
            "00000000-0000-0000-0000-000000000501",
            "--format",
            "json",
            "--no-input",
        ],
    );
    assert_eq!(stale.status.code(), Some(3));
    assert_eq!(stdout_json(&stale)["error"]["code"], "revision_conflict");
}

fn mino_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_mino"))
}

fn run_mino(root: &Path, arguments: &[&str]) -> Output {
    Command::new(mino_binary())
        .arg("--root")
        .arg(root)
        .args(arguments)
        .output()
        .expect("Mino CLI should run")
}

fn stdout_json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "stdout should contain JSON: {error}; stdout={}",
            String::from_utf8_lossy(&output.stdout)
        )
    })
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[cfg(unix)]
fn create_file_symlink(target: &Path, link: &Path) -> bool {
    std::os::unix::fs::symlink(target, link).is_ok()
}

#[cfg(windows)]
fn create_file_symlink(target: &Path, link: &Path) -> bool {
    std::os::windows::fs::symlink_file(target, link).is_ok()
}

#[cfg(not(any(unix, windows)))]
fn create_file_symlink(_target: &Path, _link: &Path) -> bool {
    false
}
