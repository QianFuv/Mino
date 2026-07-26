//! Contract tests for deterministic, data-only, sync-compatible team catalogs.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use mino::ErrorCategory;
use mino::project::{ProjectConfig, ProjectLayout, StandardsLock, initialize};
use mino::standards::{
    SourcePolicy, SyncLimits, SyncOptions, build_team_catalog, build_team_catalog_with_policy,
    synchronize_all_with_options, validate_team_catalog,
};
use mino::store::sha256_digest;
use serde_json::Value;

static NEXT_TEMP_DIRECTORY: AtomicU64 = AtomicU64::new(1);

struct TestDirectory {
    path: PathBuf,
}

impl TestDirectory {
    fn new(label: &str) -> Self {
        let sequence = NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "mino-catalog-build-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("test directory should be created");
        Self { path }
    }

    fn path(&self, child: &str) -> PathBuf {
        self.path.join(child)
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let temporary_root = std::env::temp_dir();
        if self.path.starts_with(&temporary_root)
            && self
                .path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("mino-catalog-build-"))
        {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

struct StaticServer {
    address: SocketAddr,
    stop: Arc<AtomicBool>,
    requests: Arc<AtomicU64>,
    thread: Option<JoinHandle<()>>,
}

impl StaticServer {
    fn new(root: PathBuf) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("test server should bind");
        listener
            .set_nonblocking(true)
            .expect("test listener should be nonblocking");
        let address = listener
            .local_addr()
            .expect("test server address should resolve");
        let stop = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(AtomicU64::new(0));
        let server_stop = Arc::clone(&stop);
        let server_requests = Arc::clone(&requests);
        let server_root = Arc::new(root);
        let thread = thread::spawn(move || {
            while !server_stop.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let root = Arc::clone(&server_root);
                        let requests = Arc::clone(&server_requests);
                        thread::spawn(move || serve_files(stream, &root, &requests));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(_) if !server_stop.load(Ordering::Acquire) => {
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            address,
            stop,
            requests,
            thread: Some(thread),
        }
    }

    fn base_url(&self) -> String {
        format!("http://{}", self.address)
    }

    fn request_count(&self) -> u64 {
        self.requests.load(Ordering::Relaxed)
    }
}

impl Drop for StaticServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = TcpStream::connect(self.address);
        if let Some(thread) = self.thread.take() {
            thread.join().expect("test server should stop");
        }
    }
}

fn serve_files(mut stream: TcpStream, root: &Path, requests: &AtomicU64) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
    let mut buffer = [0_u8; 1024];
    loop {
        let mut request = Vec::new();
        while request.len() < 16 * 1024 {
            match stream.read(&mut buffer) {
                Ok(0) | Err(_) => return,
                Ok(count) => {
                    request.extend_from_slice(&buffer[..count]);
                    if request.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
            }
        }
        let request_text = String::from_utf8_lossy(&request);
        let path = request_text
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .unwrap_or("/");
        requests.fetch_add(1, Ordering::Relaxed);
        let relative = path.strip_prefix('/').unwrap_or(path);
        let is_safe = !relative.is_empty()
            && !relative.contains(['?', '#', '\\'])
            && relative
                .split('/')
                .all(|segment| !segment.is_empty() && segment != "." && segment != "..");
        let body = is_safe
            .then(|| fs::read(root.join(relative)).ok())
            .flatten();
        let (status, reason, body) = body
            .map_or((404, "Not Found", b"not found".to_vec()), |body| {
                (200, "OK", body)
            });
        let header = format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n",
            body.len()
        );
        if stream.write_all(header.as_bytes()).is_err()
            || stream.write_all(&body).is_err()
            || stream.flush().is_err()
        {
            return;
        }
    }
}

fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/catalog/valid")
}

fn copy_fixture(destination: &Path) {
    copy_tree(&fixture_root(), destination);
}

fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir(destination).expect("fixture destination should be created");
    for entry in fs::read_dir(source).expect("fixture directory should be readable") {
        let entry = entry.expect("fixture entry should be readable");
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        let file_type = entry.file_type().expect("fixture entry should inspect");
        if file_type.is_dir() {
            copy_tree(&source_path, &destination_path);
        } else {
            fs::copy(&source_path, &destination_path).expect("fixture file should copy");
        }
    }
}

fn set_base_url(source: &Path, base_url: &str) {
    fs::write(
        source.join("catalog-source.toml"),
        format!("source_version = 1\nnamespace = \"example.com\"\nbase_url = \"{base_url}\"\n"),
    )
    .expect("source URL should be updated");
}

fn snapshot_files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, directory: &Path, snapshot: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(directory).expect("snapshot directory should be readable") {
            let entry = entry.expect("snapshot entry should be readable");
            let path = entry.path();
            let file_type = entry.file_type().expect("snapshot entry should inspect");
            if file_type.is_dir() {
                visit(root, &path, snapshot);
            } else if file_type.is_file() {
                snapshot.insert(
                    path.strip_prefix(root)
                        .expect("snapshot path should stay below root")
                        .to_path_buf(),
                    fs::read(&path).expect("snapshot file should be readable"),
                );
            }
        }
    }

    let mut snapshot = BTreeMap::new();
    visit(root, root, &mut snapshot);
    snapshot
}

fn configure_project_catalog(project: &Path, catalog_url: &str) {
    let layout = ProjectLayout::new(project);
    let mut config: ProjectConfig = toml::from_str(
        &fs::read_to_string(layout.config()).expect("project config should be readable"),
    )
    .expect("project config should parse");
    config.catalog.url = Some(catalog_url.to_owned());
    let mut rendered = toml::to_string_pretty(&config).expect("project config should serialize");
    if !rendered.ends_with('\n') {
        rendered.push('\n');
    }
    fs::write(layout.config(), rendered).expect("project config should update");
}

fn run_mino(arguments: &[OsString]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mino"))
        .args(arguments)
        .stdin(Stdio::null())
        .output()
        .expect("Mino binary should run")
}

fn catalog_arguments(action: &str, values: &[(&str, &Path)]) -> Vec<OsString> {
    let mut arguments = vec![
        "--format".into(),
        "json".into(),
        "--no-input".into(),
        "standards".into(),
        "catalog".into(),
        action.into(),
    ];
    for (name, value) in values {
        arguments.push((*name).into());
        arguments.push(value.as_os_str().to_owned());
    }
    arguments
}

fn expect_failure_preserving_output(
    source: &Path,
    output: &Path,
    expected: &BTreeMap<PathBuf, Vec<u8>>,
) {
    let error = build_team_catalog(source, output).expect_err("invalid source should fail");
    assert!(matches!(
        error.category(),
        ErrorCategory::IncompleteOrValidation
            | ErrorCategory::PolicyViolation
            | ErrorCategory::DriftDetected
    ));
    assert_eq!(snapshot_files(output), *expected);
}

#[test]
fn cli_initializes_validates_builds_and_never_overwrites_source() {
    let temporary = TestDirectory::new("cli");
    let source = temporary.path("source");
    let output = temporary.path("output");
    let mut init_arguments = catalog_arguments("init", &[("--source", &source)]);
    init_arguments.extend([
        "--namespace".into(),
        "example.com".into(),
        "--base-url".into(),
        "https://standards.example.test/mino".into(),
    ]);
    let initialized = run_mino(&init_arguments);
    assert!(initialized.status.success());
    assert!(initialized.stderr.is_empty());
    let initialized_json: Value =
        serde_json::from_slice(&initialized.stdout).expect("init result should be JSON");
    assert_eq!(initialized_json["kind"], "mino.team-catalog-init/v1");
    let source_snapshot = snapshot_files(&source);
    let duplicate = run_mino(&init_arguments);
    assert_eq!(duplicate.status.code(), Some(2));
    assert_eq!(snapshot_files(&source), source_snapshot);

    let validated = run_mino(&catalog_arguments("validate", &[("--source", &source)]));
    assert!(validated.status.success());
    let validated_json: Value =
        serde_json::from_slice(&validated.stdout).expect("validate result should be JSON");
    assert_eq!(validated_json["kind"], "mino.team-catalog-validation/v1");
    assert_eq!(
        validated_json["packages"][0]["package_id"],
        "example.com.common"
    );

    let built = run_mino(&catalog_arguments(
        "build",
        &[("--source", &source), ("--output", &output)],
    ));
    assert!(built.status.success());
    let built_json: Value =
        serde_json::from_slice(&built.stdout).expect("build result should be JSON");
    assert_eq!(built_json["kind"], "mino.team-catalog-build/v1");
    assert!(output.join("catalog.toml").is_file());
    assert!(output.join("catalog-manifest.json").is_file());
}

#[test]
fn repeated_builds_are_byte_identical_and_sync_consumes_the_output_unchanged() {
    let temporary = TestDirectory::new("sync");
    let source = temporary.path("source");
    let first_output = temporary.path("output-a");
    let second_output = temporary.path("output-b");
    let project = temporary.path("project");
    copy_fixture(&source);
    fs::create_dir(&project).expect("project directory should be created");
    let server = StaticServer::new(first_output.clone());
    set_base_url(&source, &server.base_url());
    let first =
        build_team_catalog_with_policy(&source, &first_output, SourcePolicy::HttpsOrLoopbackHttp)
            .expect("first catalog should build");
    let second =
        build_team_catalog_with_policy(&source, &second_output, SourcePolicy::HttpsOrLoopbackHttp)
            .expect("second catalog should build");
    assert_eq!(first.catalog_digest, second.catalog_digest);
    assert_eq!(first.tree_digest, second.tree_digest);
    assert_eq!(first.manifest_digest, second.manifest_digest);
    assert_eq!(
        snapshot_files(&first_output),
        snapshot_files(&second_output)
    );

    initialize(&project).expect("project should initialize");
    configure_project_catalog(&project, &format!("{}/catalog.toml", server.base_url()));
    let synchronized = synchronize_all_with_options(
        &project,
        SyncOptions::new(SyncLimits::default(), SourcePolicy::HttpsOrLoopbackHttp),
    )
    .expect("generated catalog should synchronize without a special path");
    assert_eq!(synchronized.catalog_digest, first.catalog_digest);
    assert_eq!(
        synchronized
            .packages
            .iter()
            .map(|package| package.package_id.as_str())
            .collect::<Vec<_>>(),
        ["example.com.common", "example.com.rust"]
    );
    let lock: StandardsLock = toml::from_str(
        &fs::read_to_string(ProjectLayout::new(&project).standards_lock())
            .expect("standards lock should be readable"),
    )
    .expect("standards lock should parse");
    assert_eq!(
        lock.catalog_digest.as_deref(),
        Some(first.catalog_digest.as_str())
    );
    assert_eq!(server.request_count(), 7);
}

#[test]
fn semantic_order_and_line_endings_normalize_to_identical_artifacts() {
    let temporary = TestDirectory::new("normalize");
    let first_source = temporary.path("source-a");
    let second_source = temporary.path("source-b");
    let first_output = temporary.path("output-a");
    let second_output = temporary.path("output-b");
    copy_fixture(&first_source);
    copy_fixture(&second_source);
    let package_id = "example.com.common";
    let reordered = format!(
        "[[rules]]\r\nid = \"{package_id}.review\"\r\nlevel = \"recommended\"\r\ntext = \"Record review evidence before completion.\"\r\n\r\n[[rules]]\r\nid = \"{package_id}.scope\"\r\nlevel = \"required\"\r\ntext = \"Keep changes within the declared task scope.\"\r\n"
    );
    fs::write(second_source.join("packages/common/rules.toml"), reordered)
        .expect("reordered fixture should be written");
    let first = build_team_catalog(&first_source, &first_output)
        .expect("first normalized catalog should build");
    let second = build_team_catalog(&second_source, &second_output)
        .expect("second normalized catalog should build");
    assert_eq!(first.tree_digest, second.tree_digest);
    assert_eq!(
        snapshot_files(&first_output),
        snapshot_files(&second_output)
    );
}

#[test]
fn every_invalid_source_fails_before_replacing_a_valid_output() {
    let temporary = TestDirectory::new("preserve");
    let baseline_source = temporary.path("baseline-source");
    let output = temporary.path("output");
    copy_fixture(&baseline_source);
    build_team_catalog(&baseline_source, &output).expect("baseline catalog should build");
    let expected_output = snapshot_files(&output);

    let malformed = temporary.path("malformed");
    copy_fixture(&malformed);
    fs::write(malformed.join("packages/common/rules.toml"), "rules = [")
        .expect("malformed source should be written");
    expect_failure_preserving_output(&malformed, &output, &expected_output);

    let duplicate = temporary.path("duplicate");
    copy_fixture(&duplicate);
    let rule = "[[rules]]\nid = \"example.com.common.duplicate\"\nlevel = \"required\"\ntext = \"Duplicate.\"\n";
    fs::write(
        duplicate.join("packages/common/rules.toml"),
        format!("{rule}\n{rule}"),
    )
    .expect("duplicate source should be written");
    expect_failure_preserving_output(&duplicate, &output, &expected_output);

    let invalid_version = temporary.path("version");
    copy_fixture(&invalid_version);
    let manifest_path = invalid_version.join("packages/rust/manifest.toml");
    let manifest = fs::read_to_string(&manifest_path)
        .expect("version fixture should be readable")
        .replace("2.1.0-beta.1", "2.1");
    fs::write(&manifest_path, manifest).expect("invalid version should be written");
    expect_failure_preserving_output(&invalid_version, &output, &expected_output);

    let unexpected = temporary.path("unexpected");
    copy_fixture(&unexpected);
    fs::write(
        unexpected.join("packages/common/payload.exe"),
        b"not executable data",
    )
    .expect("unexpected payload should be written");
    expect_failure_preserving_output(&unexpected, &output, &expected_output);

    let oversized = temporary.path("oversized");
    copy_fixture(&oversized);
    fs::write(
        oversized.join("packages/common/rules.toml"),
        vec![b'x'; 1024 * 1024 + 1],
    )
    .expect("oversized source should be written");
    expect_failure_preserving_output(&oversized, &output, &expected_output);

    let wrong_identity = temporary.path("identity");
    copy_fixture(&wrong_identity);
    let manifest_path = wrong_identity.join("packages/common/manifest.toml");
    let manifest = fs::read_to_string(&manifest_path)
        .expect("identity fixture should be readable")
        .replace("example.com.common", "../common");
    fs::write(&manifest_path, manifest).expect("unsafe identity should be written");
    expect_failure_preserving_output(&wrong_identity, &output, &expected_output);
}

#[test]
fn valid_rebuild_replaces_only_a_verified_catalog_output() {
    let temporary = TestDirectory::new("replace");
    let source = temporary.path("source");
    let output = temporary.path("output");
    copy_fixture(&source);
    let first = build_team_catalog(&source, &output).expect("first catalog should build");
    assert!(!first.replaced_existing);
    let first_snapshot = snapshot_files(&output);

    let rules_path = source.join("packages/common/rules.toml");
    let rules = fs::read_to_string(&rules_path)
        .expect("rules should be readable")
        .replace(
            "Record review evidence before completion.",
            "Record current review evidence before completion.",
        );
    fs::write(&rules_path, rules).expect("rules should update");
    let second = build_team_catalog(&source, &output).expect("verified output should replace");
    assert!(second.replaced_existing);
    assert_ne!(second.tree_digest, first.tree_digest);
    assert_ne!(snapshot_files(&output), first_snapshot);

    let manifest_path = output.join("catalog-manifest.json");
    let mut manifest: Value =
        serde_json::from_slice(&fs::read(&manifest_path).expect("manifest should be readable"))
            .expect("manifest should parse");
    manifest["packages"][0]["digest"] = Value::String(format!("sha256:{}", "0".repeat(64)));
    let mut corrupt_manifest =
        serde_json::to_vec_pretty(&manifest).expect("corrupt manifest should serialize");
    corrupt_manifest.push(b'\n');
    fs::write(&manifest_path, corrupt_manifest).expect("manifest corruption should be injected");
    let corrupt_snapshot = snapshot_files(&output);
    let error = build_team_catalog(&source, &output)
        .expect_err("unverified existing output must not be replaced");
    assert_eq!(error.category(), ErrorCategory::PolicyViolation);
    assert_eq!(snapshot_files(&output), corrupt_snapshot);
}

#[cfg(unix)]
#[test]
fn symbolic_links_and_executable_data_files_are_rejected() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let temporary = TestDirectory::new("unix-policy");
    let linked_source = temporary.path("linked-source");
    copy_fixture(&linked_source);
    let rules = linked_source.join("packages/common/rules.toml");
    let external = temporary.path("external.toml");
    fs::write(
        &external,
        fs::read(&rules).expect("rules should be readable"),
    )
    .expect("external rules should be written");
    fs::remove_file(&rules).expect("fixture rules should be removed");
    symlink(&external, &rules).expect("fixture symlink should be created");
    let linked_error = validate_team_catalog(&linked_source).expect_err("symlink should fail");
    assert_eq!(linked_error.category(), ErrorCategory::PolicyViolation);

    let executable_source = temporary.path("executable-source");
    copy_fixture(&executable_source);
    let manifest = executable_source.join("packages/common/manifest.toml");
    let mut permissions = fs::metadata(&manifest)
        .expect("manifest should inspect")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&manifest, permissions).expect("manifest mode should update");
    let executable_error =
        validate_team_catalog(&executable_source).expect_err("executable data should fail");
    assert_eq!(executable_error.category(), ErrorCategory::PolicyViolation);
}

#[cfg(windows)]
#[test]
fn symbolic_link_data_is_rejected_when_the_host_can_create_it() {
    use std::os::windows::fs::symlink_file;

    let temporary = TestDirectory::new("windows-policy");
    let source = temporary.path("source");
    copy_fixture(&source);
    let rules = source.join("packages/common/rules.toml");
    let external = temporary.path("external.toml");
    fs::write(
        &external,
        fs::read(&rules).expect("rules should be readable"),
    )
    .expect("external rules should be written");
    fs::remove_file(&rules).expect("fixture rules should be removed");
    match symlink_file(&external, &rules) {
        Ok(()) => {
            let error = validate_team_catalog(&source).expect_err("symlink should fail");
            assert_eq!(error.category(), ErrorCategory::PolicyViolation);
        }
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {}
        Err(error) => panic!("unexpected symlink setup failure: {error}"),
    }
}

#[test]
fn manifest_file_digests_cover_every_sync_payload_byte() {
    let temporary = TestDirectory::new("manifest");
    let source = temporary.path("source");
    let output = temporary.path("output");
    copy_fixture(&source);
    let report = build_team_catalog(&source, &output).expect("catalog should build");
    let manifest_bytes = fs::read(output.join("catalog-manifest.json"))
        .expect("catalog manifest should be readable");
    assert_eq!(sha256_digest(&manifest_bytes), report.manifest_digest);
    let manifest: Value =
        serde_json::from_slice(&manifest_bytes).expect("catalog manifest should parse");
    let files = manifest["files"]
        .as_array()
        .expect("manifest files should be an array");
    assert_eq!(files.len(), 7);
    for file in files {
        let path = file["path"].as_str().expect("file path should be text");
        let bytes = fs::read(output.join(path.replace('/', std::path::MAIN_SEPARATOR_STR)))
            .expect("manifest file should be readable");
        assert_eq!(file["bytes"], bytes.len());
        assert_eq!(file["digest"], sha256_digest(&bytes));
    }
}
