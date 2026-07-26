//! Contract tests for explicit, verified, atomic, and offline-safe catalog synchronization.

use std::collections::BTreeMap;
use std::fmt::Write as FmtWrite;
use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

#[cfg(unix)]
use std::os::unix::fs::symlink;
#[cfg(windows)]
use std::os::windows::fs::symlink_dir;

use mino::project::{ProjectConfig, ProjectLayout, StandardsLock, initialize};
use mino::standards::{SourcePolicy, SyncLimits, SyncOptions, synchronize_all_with_options};
use mino::store::sha256_digest;
use mino::{ErrorCategory, MinoError};
use serde_json::Value;

static NEXT_TEMP_DIRECTORY: AtomicU64 = AtomicU64::new(1);

struct TestProject {
    path: PathBuf,
    layout: ProjectLayout,
}

impl TestProject {
    fn new(label: &str) -> Self {
        let sequence = NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "mino-standards-sync-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("temporary project should be created");
        initialize(&path).expect("temporary project should initialize");
        let path = path.canonicalize().expect("project root should resolve");
        let layout = ProjectLayout::new(&path);
        Self { path, layout }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn layout(&self) -> &ProjectLayout {
        &self.layout
    }

    fn configure_catalog(&self, url: Option<&str>) {
        let mut config: ProjectConfig = toml::from_str(
            &fs::read_to_string(self.layout.config()).expect("config should be readable"),
        )
        .expect("config should parse");
        config.catalog.url = url.map(str::to_owned);
        let mut rendered = toml::to_string_pretty(&config).expect("config should serialize");
        if !rendered.ends_with('\n') {
            rendered.push('\n');
        }
        fs::write(self.layout.config(), rendered).expect("config should be updated");
    }
}

impl Drop for TestProject {
    fn drop(&mut self) {
        let temporary_root = std::env::temp_dir();
        if self.path.starts_with(&temporary_root)
            && self
                .path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("mino-standards-sync-"))
        {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

#[derive(Clone)]
struct Route {
    status: u16,
    body: Vec<u8>,
    delay: Duration,
}

impl Route {
    fn ok(body: impl Into<Vec<u8>>) -> Self {
        Self {
            status: 200,
            body: body.into(),
            delay: Duration::ZERO,
        }
    }

    fn delayed(body: impl Into<Vec<u8>>, delay: Duration) -> Self {
        Self {
            status: 200,
            body: body.into(),
            delay,
        }
    }
}

struct TestServer {
    address: SocketAddr,
    stop: Arc<AtomicBool>,
    requests: Arc<Mutex<Vec<String>>>,
    thread: Option<JoinHandle<()>>,
}

impl TestServer {
    fn new(build_routes: impl FnOnce(&str) -> BTreeMap<String, Route>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("test server should bind");
        listener
            .set_nonblocking(true)
            .expect("test listener should be nonblocking");
        let address = listener
            .local_addr()
            .expect("test server address should resolve");
        let base_url = format!("http://{address}");
        let routes = Arc::new(build_routes(&base_url));
        let stop = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let server_stop = Arc::clone(&stop);
        let server_requests = Arc::clone(&requests);
        let thread = thread::spawn(move || {
            while !server_stop.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let routes = Arc::clone(&routes);
                        let requests = Arc::clone(&server_requests);
                        thread::spawn(move || handle_connection(stream, &routes, &requests));
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

    fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.address, path)
    }

    fn request_count(&self) -> usize {
        self.requests
            .lock()
            .expect("request log should be available")
            .len()
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = TcpStream::connect(self.address);
        if let Some(thread) = self.thread.take() {
            thread.join().expect("test server should stop");
        }
    }
}

fn handle_connection(
    mut stream: TcpStream,
    routes: &BTreeMap<String, Route>,
    requests: &Mutex<Vec<String>>,
) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
    let mut buffer = [0_u8; 1024];
    loop {
        let mut bytes = Vec::new();
        while bytes.len() < 16 * 1024 {
            match stream.read(&mut buffer) {
                Ok(0) | Err(_) => return,
                Ok(count) => {
                    bytes.extend_from_slice(&buffer[..count]);
                    if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
            }
        }
        let request = String::from_utf8_lossy(&bytes);
        let path = request
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .unwrap_or("/")
            .to_owned();
        requests
            .lock()
            .expect("request log should be available")
            .push(path.clone());
        let route = routes.get(&path).cloned().unwrap_or(Route {
            status: 404,
            body: b"not found".to_vec(),
            delay: Duration::ZERO,
        });
        thread::sleep(route.delay);
        let reason = if route.status == 200 {
            "OK"
        } else {
            "Not Found"
        };
        let header = format!(
            "HTTP/1.1 {} {reason}\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n",
            route.status,
            route.body.len()
        );
        if stream.write_all(header.as_bytes()).is_err()
            || stream.write_all(&route.body).is_err()
            || stream.flush().is_err()
        {
            return;
        }
    }
}

#[derive(Clone)]
struct PackageFixture {
    package_id: String,
    version: String,
    digest: String,
    manifest: String,
    rules: String,
    checks: String,
}

impl PackageFixture {
    fn new(package_id: &str, language: Option<&str>, sequence: usize) -> Self {
        let version = format!("1.{sequence}.0");
        let languages =
            language.map_or_else(|| "[]".to_owned(), |language| format!("[\"{language}\"]"));
        let manifest = format!(
            "package_id = \"{package_id}\"\ndisplay_name = \"{package_id}\"\nversion = \"{version}\"\nlanguages = {languages}\n"
        );
        let rules = format!(
            "[[rules]]\nid = \"sync.{package_id}.rule\"\nlevel = \"required\"\ntext = \"Downloaded inert rule {sequence}.\"\n"
        );
        let checks = format!(
            "[[checks]]\nid = \"sync.{package_id}.check\"\nargv = [\"should-never-execute\", \"{sequence}\"]\ntool = \"should-never-execute\"\nrequired = true\n"
        );
        let digest = package_digest(&manifest, &rules, &checks);
        Self {
            package_id: package_id.to_owned(),
            version,
            digest,
            manifest,
            rules,
            checks,
        }
    }
}

fn package_digest(manifest: &str, rules: &str, checks: &str) -> String {
    let mut input = Vec::new();
    for (name, source) in [
        ("manifest.toml", manifest),
        ("rules.toml", rules),
        ("checks.toml", checks),
    ] {
        input.extend_from_slice(name.as_bytes());
        input.push(0);
        input.extend_from_slice(source.as_bytes());
        input.push(0);
    }
    sha256_digest(&input)
}

fn catalog_source(base_url: &str, packages: &[PackageFixture], bad_digest: bool) -> String {
    let mut catalog = "catalog_version = 1\n".to_owned();
    for package in packages {
        let digest = if bad_digest {
            format!("sha256:{}", "0".repeat(64))
        } else {
            package.digest.clone()
        };
        write!(
            catalog,
            "\n[[packages]]\npackage_id = \"{}\"\nversion = \"{}\"\ndigest = \"{digest}\"\nmanifest_url = \"{base_url}/{}/manifest.toml\"\nrules_url = \"{base_url}/{}/rules.toml\"\nchecks_url = \"{base_url}/{}/checks.toml\"\n",
            package.package_id,
            package.version,
            package.package_id,
            package.package_id,
            package.package_id
        )
        .expect("catalog fixture should render");
    }
    catalog
}

fn fixture_routes(base_url: &str, packages: &[PackageFixture]) -> BTreeMap<String, Route> {
    let mut routes = BTreeMap::new();
    routes.insert(
        "/catalog.toml".to_owned(),
        Route::ok(catalog_source(base_url, packages, false)),
    );
    routes.insert(
        "/bad-digest.toml".to_owned(),
        Route::ok(catalog_source(base_url, packages, true)),
    );
    routes.insert("/invalid.toml".to_owned(), Route::ok("not = [valid"));
    routes.insert("/oversized.toml".to_owned(), Route::ok(vec![b'x'; 512]));
    routes.insert(
        "/slow.toml".to_owned(),
        Route::delayed(
            catalog_source(base_url, packages, false),
            Duration::from_millis(250),
        ),
    );
    for package in packages {
        let prefix = format!("/{}", package.package_id);
        routes.insert(
            format!("{prefix}/manifest.toml"),
            Route::ok(package.manifest.clone()),
        );
        routes.insert(
            format!("{prefix}/rules.toml"),
            Route::ok(package.rules.clone()),
        );
        routes.insert(
            format!("{prefix}/checks.toml"),
            Route::ok(package.checks.clone()),
        );
    }
    routes
}

fn local_options(limits: SyncLimits) -> SyncOptions {
    SyncOptions::new(limits, SourcePolicy::HttpsOrLoopbackHttp)
}

fn default_local_options() -> SyncOptions {
    local_options(SyncLimits::default())
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
        Err(error) => panic!("standards cache symlink should be created: {error}"),
    }
}

fn read_lock(layout: &ProjectLayout) -> StandardsLock {
    toml::from_str(
        &fs::read_to_string(layout.standards_lock()).expect("standards lock should be readable"),
    )
    .expect("standards lock should parse")
}

fn snapshot_files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, directory: &Path, snapshot: &mut BTreeMap<PathBuf, Vec<u8>>) {
        if !directory.exists() {
            return;
        }
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

fn assert_preserved(
    layout: &ProjectLayout,
    expected_lock: &[u8],
    expected_cache: &BTreeMap<PathBuf, Vec<u8>>,
) {
    assert_eq!(
        fs::read(layout.standards_lock()).expect("standards lock should remain"),
        expected_lock
    );
    assert_eq!(snapshot_files(&layout.standards_cache()), *expected_cache);
}

fn expect_environment_error(result: Result<mino::standards::StandardsSyncReport, MinoError>) {
    let error = result.expect_err("synchronization should fail");
    assert_eq!(error.category(), ErrorCategory::EnvironmentUnavailable);
}

fn run_mino(arguments: &[String]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mino"))
        .args(arguments)
        .output()
        .expect("Mino binary should run")
}

#[test]
fn valid_catalog_installs_every_package_and_reuses_exact_generation() {
    let project = TestProject::new("valid");
    let packages = vec![
        PackageFixture::new("common", None, 0),
        PackageFixture::new("rust", Some("rust"), 1),
    ];
    let server = TestServer::new(|base_url| fixture_routes(base_url, &packages));
    let catalog_url = server.url("/catalog.toml");
    project.configure_catalog(Some(&catalog_url));

    let first = synchronize_all_with_options(project.path(), default_local_options())
        .expect("valid catalog should synchronize");
    assert!(!first.reused_generation);
    assert_eq!(first.packages.len(), 2);
    assert_eq!(first.packages[0].package_id, "common");
    assert_eq!(first.packages[1].package_id, "rust");
    assert_eq!(first.catalog_url, catalog_url);
    assert_eq!(
        first.catalog_digest,
        sha256_digest(
            &fs::read(first.generation.join("catalog.toml")).expect("cached catalog should exist")
        )
    );
    assert!(first.generation.is_dir());
    assert!(
        first
            .generation
            .join("packages/rust/1.1.0/manifest.toml")
            .is_file()
    );
    let lock = read_lock(project.layout());
    assert!(lock.is_supported());
    assert_eq!(
        lock.catalog_digest.as_deref(),
        Some(first.catalog_digest.as_str())
    );
    assert_eq!(lock.packages.len(), 2);
    assert_eq!(server.request_count(), 7);

    let second = synchronize_all_with_options(project.path(), default_local_options())
        .expect("identical catalog should synchronize idempotently");
    assert!(second.reused_generation);
    assert_eq!(second.generation, first.generation);
    assert_eq!(second.catalog_digest, first.catalog_digest);
    assert_eq!(read_lock(project.layout()), lock);
    let generation_entries = fs::read_dir(project.layout().standards_cache().join("generations"))
        .expect("generations should be readable")
        .collect::<Result<Vec<_>, _>>()
        .expect("generation entries should be readable");
    assert_eq!(generation_entries.len(), 1);
}

#[cfg(any(unix, windows))]
#[test]
fn synchronization_rejects_symlinked_cache_ancestors() {
    for relative in [".mino/cache", ".mino/cache/standards/generations"] {
        let project = TestProject::new(&format!("symlink-{}", relative.replace('/', "-")));
        let external = TestProject::new("symlink-external");
        let packages = vec![PackageFixture::new("rust", Some("rust"), 1)];
        let server = TestServer::new(|base_url| fixture_routes(base_url, &packages));
        project.configure_catalog(Some(&server.url("/catalog.toml")));
        let lock_before =
            fs::read(project.layout.standards_lock()).expect("standards lock should be readable");
        let link = project.path.join(relative);
        if link.exists() {
            fs::remove_dir_all(&link).expect("fixture cache directory should be removed");
        }
        if let Some(parent) = link.parent() {
            fs::create_dir_all(parent).expect("cache parent should be created");
        }
        let sentinel = external.path.join("sentinel.txt");
        fs::write(&sentinel, b"outside\n").expect("outside sentinel should be written");
        if !create_directory_symlink(external.path(), &link) {
            continue;
        }

        let error = synchronize_all_with_options(project.path(), default_local_options())
            .expect_err("symlinked standards cache must be rejected");
        assert_eq!(error.category(), ErrorCategory::DriftDetected);
        assert_eq!(
            fs::read(project.layout.standards_lock())
                .expect("standards lock should remain readable"),
            lock_before
        );
        assert_eq!(
            fs::read(&sentinel).expect("outside sentinel should remain readable"),
            b"outside\n"
        );
        assert!(!external.path.join("standards").exists());
        assert!(!external.path.join("catalog.toml").exists());
    }
}

#[test]
fn every_prepublication_failure_preserves_active_cache_and_lock() {
    let project = TestProject::new("preserve");
    let packages = vec![PackageFixture::new("common", None, 0)];
    let server = TestServer::new(|base_url| fixture_routes(base_url, &packages));
    project.configure_catalog(Some(&server.url("/catalog.toml")));
    synchronize_all_with_options(project.path(), default_local_options())
        .expect("baseline catalog should synchronize");
    let expected_lock = fs::read(project.layout().standards_lock()).expect("lock should exist");
    let expected_cache = snapshot_files(&project.layout().standards_cache());

    project.configure_catalog(None);
    let missing = synchronize_all_with_options(project.path(), default_local_options())
        .expect_err("missing URL should fail");
    assert_eq!(missing.category(), ErrorCategory::EnvironmentUnavailable);
    assert_eq!(missing.missing(), ["catalog.url"]);
    assert_eq!(missing.next_actions().len(), 1);
    assert_preserved(project.layout(), &expected_lock, &expected_cache);

    project.configure_catalog(Some("file:///catalog.toml"));
    expect_environment_error(synchronize_all_with_options(
        project.path(),
        default_local_options(),
    ));
    assert_preserved(project.layout(), &expected_lock, &expected_cache);

    project.configure_catalog(Some(&server.url("/invalid.toml")));
    expect_environment_error(synchronize_all_with_options(
        project.path(),
        default_local_options(),
    ));
    assert_preserved(project.layout(), &expected_lock, &expected_cache);

    project.configure_catalog(Some(&server.url("/bad-digest.toml")));
    expect_environment_error(synchronize_all_with_options(
        project.path(),
        default_local_options(),
    ));
    assert_preserved(project.layout(), &expected_lock, &expected_cache);

    project.configure_catalog(Some(&server.url("/oversized.toml")));
    let small_catalog_limit = SyncLimits::new(Duration::from_secs(2), 64, 1024, 4096)
        .expect("small byte limits should be valid");
    expect_environment_error(synchronize_all_with_options(
        project.path(),
        local_options(small_catalog_limit),
    ));
    assert_preserved(project.layout(), &expected_lock, &expected_cache);

    project.configure_catalog(Some(&server.url("/slow.toml")));
    let short_timeout = SyncLimits::new(Duration::from_millis(40), 4096, 4096, 16 * 1024)
        .expect("short time limit should be valid");
    expect_environment_error(synchronize_all_with_options(
        project.path(),
        local_options(short_timeout),
    ));
    assert_preserved(project.layout(), &expected_lock, &expected_cache);
}

#[test]
fn production_policy_rejects_loopback_and_cli_requires_explicit_all_and_configuration() {
    let project = TestProject::new("cli");
    let packages = vec![PackageFixture::new("common", None, 0)];
    let server = TestServer::new(|base_url| fixture_routes(base_url, &packages));
    project.configure_catalog(Some(&server.url("/catalog.toml")));
    expect_environment_error(synchronize_all_with_options(
        project.path(),
        SyncOptions::default(),
    ));
    assert_eq!(server.request_count(), 0);

    project.configure_catalog(None);
    let base_arguments = vec![
        "--root".to_owned(),
        project.path().to_string_lossy().into_owned(),
        "--format".to_owned(),
        "json".to_owned(),
        "standards".to_owned(),
        "sync".to_owned(),
    ];
    let without_all = run_mino(&base_arguments);
    assert_eq!(without_all.status.code(), Some(2));
    assert!(without_all.stderr.is_empty());
    let without_all_json: Value =
        serde_json::from_slice(&without_all.stdout).expect("failure should be JSON");
    assert_eq!(
        without_all_json["error"]["code"],
        "incomplete_or_validation"
    );

    let mut with_all_arguments = base_arguments;
    with_all_arguments.push("--all".to_owned());
    let with_all = run_mino(&with_all_arguments);
    assert_eq!(with_all.status.code(), Some(7));
    assert!(with_all.stderr.is_empty());
    let with_all_json: Value =
        serde_json::from_slice(&with_all.stdout).expect("failure should be JSON");
    assert_eq!(with_all_json["error"]["code"], "environment_unavailable");
    assert_eq!(with_all_json["missing"][0], "catalog.url");
    assert_eq!(with_all_json["next_actions"][0]["id"], "project.show");
}
