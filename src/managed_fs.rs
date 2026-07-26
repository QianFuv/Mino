//! Capability-rooted filesystem operations for project-managed state.

use std::error::Error;
use std::ffi::OsString;
use std::fmt::{self, Display, Formatter};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use cap_std::ambient_authority;
use cap_std::fs::{Dir, OpenOptions};

/// Stable categories for managed filesystem failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ManagedFsErrorKind {
    /// A caller supplied an invalid project-relative path.
    InvalidPath,
    /// An existing managed component is a symlink or has an unexpected type.
    UnsafeComponent,
    /// A capability-relative filesystem operation failed.
    Io,
}

/// A typed managed filesystem failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ManagedFsError {
    kind: ManagedFsErrorKind,
    message: String,
}

impl ManagedFsError {
    pub(crate) fn new(kind: ManagedFsErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub(crate) const fn kind(&self) -> ManagedFsErrorKind {
        self.kind
    }

    pub(crate) fn into_message(self) -> String {
        self.message
    }
}

impl Display for ManagedFsError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for ManagedFsError {}

/// A normalized non-empty path relative to one project root.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct ManagedPath(PathBuf);

impl ManagedPath {
    pub(crate) fn new(path: impl AsRef<Path>) -> Result<Self, ManagedFsError> {
        let path = path.as_ref();
        if path.as_os_str().is_empty()
            || path.is_absolute()
            || path
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(ManagedFsError::new(
                ManagedFsErrorKind::InvalidPath,
                format!(
                    "Managed path {} must be a non-empty normalized project-relative path",
                    path.display()
                ),
            ));
        }
        Ok(Self(path.to_path_buf()))
    }

    pub(crate) fn as_path(&self) -> &Path {
        &self.0
    }

    pub(crate) fn parent(&self) -> Option<Self> {
        let parent = self.0.parent()?;
        (!parent.as_os_str().is_empty()).then(|| Self(parent.to_path_buf()))
    }

    pub(crate) fn join(&self, path: impl AsRef<Path>) -> Result<Self, ManagedFsError> {
        Self::new(self.0.join(path))
    }
}

/// Simplified no-follow type information for one managed entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ManagedEntryKind {
    /// A regular file.
    File,
    /// A directory.
    Directory,
    /// A symbolic link or reparse link.
    Symlink,
    /// Another filesystem object.
    Other,
}

/// One direct child returned from a managed directory listing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ManagedDirEntry {
    pub(crate) name: OsString,
    pub(crate) kind: ManagedEntryKind,
}

/// An opened canonical project root used for managed filesystem effects.
#[derive(Clone, Debug)]
pub(crate) struct ProjectFs {
    root: PathBuf,
    directory: Arc<Dir>,
}

impl ProjectFs {
    pub(crate) fn open(root: &Path) -> Result<Self, ManagedFsError> {
        let canonical = root
            .canonicalize()
            .map_err(|error| io_error("resolve project root", root, &error))?;
        if !canonical.is_dir() {
            return Err(ManagedFsError::new(
                ManagedFsErrorKind::InvalidPath,
                format!("Project root {} is not a directory", canonical.display()),
            ));
        }
        let directory = Dir::open_ambient_dir(&canonical, ambient_authority())
            .map_err(|error| io_error("open project root", &canonical, &error))?;
        Ok(Self {
            root: canonical,
            directory: Arc::new(directory),
        })
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn display_path(&self, path: &ManagedPath) -> PathBuf {
        self.root.join(path.as_path())
    }

    pub(crate) fn ensure_directory(&self, path: &ManagedPath) -> Result<(), ManagedFsError> {
        let mut current = PathBuf::new();
        for component in path.as_path().components() {
            let Component::Normal(name) = component else {
                return Err(invalid_path(path.as_path()));
            };
            current.push(name);
            match self.directory.symlink_metadata(&current) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return Err(unsafe_component(&self.root.join(&current), "symbolic link"));
                }
                Ok(metadata) if !metadata.is_dir() => {
                    return Err(unsafe_component(&self.root.join(&current), "non-directory"));
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    self.directory.create_dir(&current).map_err(|error| {
                        io_error(
                            "create managed directory",
                            &self.root.join(&current),
                            &error,
                        )
                    })?;
                    let metadata = self.directory.symlink_metadata(&current).map_err(|error| {
                        io_error(
                            "reinspect managed directory",
                            &self.root.join(&current),
                            &error,
                        )
                    })?;
                    if metadata.file_type().is_symlink() || !metadata.is_dir() {
                        return Err(unsafe_component(
                            &self.root.join(&current),
                            "unsafe object created in place of a directory",
                        ));
                    }
                }
                Err(error) => {
                    return Err(io_error(
                        "inspect managed directory",
                        &self.root.join(&current),
                        &error,
                    ));
                }
            }
        }
        self.open_directory(path).map(|_| ())
    }

    pub(crate) fn create_directory(&self, path: &ManagedPath) -> Result<(), ManagedFsError> {
        self.ensure_parent(path)?;
        self.reject_final_symlink(path)?;
        self.directory.create_dir(path.as_path()).map_err(|error| {
            io_error("create managed directory", &self.display_path(path), &error)
        })?;
        self.require_directory(path)
    }

    pub(crate) fn entry_kind(
        &self,
        path: &ManagedPath,
    ) -> Result<Option<ManagedEntryKind>, ManagedFsError> {
        self.validate_ancestors(path, false)?;
        match self.directory.symlink_metadata(path.as_path()) {
            Ok(metadata) => Ok(Some(metadata_kind(&metadata))),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(io_error(
                "inspect managed entry",
                &self.display_path(path),
                &error,
            )),
        }
    }

    pub(crate) fn exists(&self, path: &ManagedPath) -> Result<bool, ManagedFsError> {
        self.entry_kind(path).map(|kind| kind.is_some())
    }

    pub(crate) fn is_file(&self, path: &ManagedPath) -> Result<bool, ManagedFsError> {
        self.entry_kind(path)
            .map(|kind| kind == Some(ManagedEntryKind::File))
    }

    pub(crate) fn is_directory(&self, path: &ManagedPath) -> Result<bool, ManagedFsError> {
        self.entry_kind(path)
            .map(|kind| kind == Some(ManagedEntryKind::Directory))
    }

    pub(crate) fn read(&self, path: &ManagedPath) -> Result<Vec<u8>, ManagedFsError> {
        self.require_file(path)?;
        let mut file = self
            .directory
            .open(path.as_path())
            .map(cap_std::fs::File::into_std)
            .map_err(|error| io_error("open managed file", &self.display_path(path), &error))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .map_err(|error| io_error("read managed file", &self.display_path(path), &error))?;
        Ok(bytes)
    }

    pub(crate) fn read_bounded(
        &self,
        path: &ManagedPath,
        maximum_bytes: u64,
    ) -> Result<Vec<u8>, ManagedFsError> {
        self.require_file(path)?;
        let file = self
            .directory
            .open(path.as_path())
            .map(cap_std::fs::File::into_std)
            .map_err(|error| io_error("open managed file", &self.display_path(path), &error))?;
        let mut bytes = Vec::new();
        file.take(maximum_bytes.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|error| io_error("read managed file", &self.display_path(path), &error))?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > maximum_bytes {
            return Err(ManagedFsError::new(
                ManagedFsErrorKind::UnsafeComponent,
                format!(
                    "Managed file {} exceeds the {maximum_bytes}-byte limit",
                    self.display_path(path).display()
                ),
            ));
        }
        Ok(bytes)
    }

    pub(crate) fn create_new_file(
        &self,
        path: &ManagedPath,
    ) -> Result<std::fs::File, ManagedFsError> {
        self.ensure_parent(path)?;
        self.reject_final_symlink(path)?;
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        self.open_with(path, &options, "create managed file")
    }

    pub(crate) fn open_lock_file(
        &self,
        path: &ManagedPath,
    ) -> Result<std::fs::File, ManagedFsError> {
        self.ensure_parent(path)?;
        self.reject_final_symlink(path)?;
        let mut options = OpenOptions::new();
        options.create(true).read(true).write(true);
        self.open_with(path, &options, "open managed lock")
    }

    pub(crate) fn open_append_file(
        &self,
        path: &ManagedPath,
    ) -> Result<std::fs::File, ManagedFsError> {
        self.ensure_parent(path)?;
        self.reject_final_symlink(path)?;
        let mut options = OpenOptions::new();
        options.create(true).append(true);
        self.open_with(path, &options, "open managed append file")
    }

    pub(crate) fn open_write_file(
        &self,
        path: &ManagedPath,
    ) -> Result<std::fs::File, ManagedFsError> {
        self.require_file(path)?;
        let mut options = OpenOptions::new();
        options.write(true);
        self.open_with(path, &options, "open managed file for writing")
    }

    pub(crate) fn open_read_write_file(
        &self,
        path: &ManagedPath,
    ) -> Result<std::fs::File, ManagedFsError> {
        self.require_file(path)?;
        let mut options = OpenOptions::new();
        options.read(true).write(true);
        self.open_with(path, &options, "open managed file for reading and writing")
    }

    pub(crate) fn rename(
        &self,
        source: &ManagedPath,
        destination: &ManagedPath,
    ) -> Result<(), ManagedFsError> {
        self.validate_ancestors(source, true)?;
        self.ensure_parent(destination)?;
        self.reject_final_symlink(destination)?;
        self.directory
            .rename(
                source.as_path(),
                self.directory.as_ref(),
                destination.as_path(),
            )
            .map_err(|error| {
                io_error(
                    "rename managed entry",
                    &self.display_path(destination),
                    &error,
                )
            })
    }

    pub(crate) fn hard_link(
        &self,
        source: &ManagedPath,
        destination: &ManagedPath,
    ) -> Result<(), ManagedFsError> {
        self.require_file(source)?;
        self.ensure_parent(destination)?;
        self.reject_final_symlink(destination)?;
        self.directory
            .hard_link(
                source.as_path(),
                self.directory.as_ref(),
                destination.as_path(),
            )
            .map_err(|error| io_error("link managed file", &self.display_path(destination), &error))
    }

    pub(crate) fn remove_file(&self, path: &ManagedPath) -> Result<(), ManagedFsError> {
        self.require_file(path)?;
        self.directory
            .remove_file(path.as_path())
            .map_err(|error| io_error("remove managed file", &self.display_path(path), &error))
    }

    pub(crate) fn remove_file_if_exists(&self, path: &ManagedPath) -> Result<(), ManagedFsError> {
        match self.entry_kind(path)? {
            None => Ok(()),
            Some(ManagedEntryKind::File) => self.remove_file(path),
            Some(kind) => Err(unexpected_kind(
                &self.display_path(path),
                kind,
                "regular file",
            )),
        }
    }

    pub(crate) fn remove_directory(&self, path: &ManagedPath) -> Result<(), ManagedFsError> {
        self.require_directory(path)?;
        self.directory
            .remove_dir(path.as_path())
            .map_err(|error| io_error("remove managed directory", &self.display_path(path), &error))
    }

    pub(crate) fn remove_directory_all(&self, path: &ManagedPath) -> Result<(), ManagedFsError> {
        self.require_directory(path)?;
        self.directory
            .remove_dir_all(path.as_path())
            .map_err(|error| {
                io_error(
                    "remove managed directory tree",
                    &self.display_path(path),
                    &error,
                )
            })
    }

    pub(crate) fn read_directory(
        &self,
        path: &ManagedPath,
    ) -> Result<Vec<ManagedDirEntry>, ManagedFsError> {
        let directory = self.open_directory(path)?;
        let mut entries = Vec::new();
        for entry in directory
            .entries()
            .map_err(|error| io_error("read managed directory", &self.display_path(path), &error))?
        {
            let entry = entry.map_err(|error| {
                io_error(
                    "read managed directory entry",
                    &self.display_path(path),
                    &error,
                )
            })?;
            let file_type = entry.file_type().map_err(|error| {
                io_error(
                    "inspect managed directory entry",
                    &self.display_path(path),
                    &error,
                )
            })?;
            let kind = if file_type.is_symlink() {
                ManagedEntryKind::Symlink
            } else if file_type.is_file() {
                ManagedEntryKind::File
            } else if file_type.is_dir() {
                ManagedEntryKind::Directory
            } else {
                ManagedEntryKind::Other
            };
            entries.push(ManagedDirEntry {
                name: entry.file_name(),
                kind,
            });
        }
        entries.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(entries)
    }

    pub(crate) fn sync_directory(&self, path: Option<&ManagedPath>) -> Result<(), ManagedFsError> {
        let display = path.map_or_else(|| self.root.clone(), |path| self.display_path(path));
        #[cfg(unix)]
        {
            if let Some(path) = path {
                self.require_directory(path)?;
            }
            let relative = path.map_or_else(|| Path::new("."), ManagedPath::as_path);
            let directory = self
                .directory
                .open(relative)
                .map(cap_std::fs::File::into_std)
                .map_err(|error| {
                    io_error(
                        "open managed directory for synchronization",
                        &display,
                        &error,
                    )
                })?;
            let metadata = directory.metadata().map_err(|error| {
                io_error(
                    "inspect managed directory for synchronization",
                    &display,
                    &error,
                )
            })?;
            if !metadata.is_dir() {
                return Err(unsafe_component(&display, "non-directory"));
            }
            directory
                .sync_all()
                .map_err(|error| io_error("synchronize managed directory", &display, &error))
        }
        #[cfg(not(unix))]
        {
            let directory = match path {
                Some(path) => self.open_directory(path)?,
                None => self
                    .directory
                    .try_clone()
                    .map_err(|error| io_error("clone project root", &self.root, &error))?,
            };
            sync_open_directory(&directory)
                .map_err(|error| io_error("synchronize managed directory", &display, &error))
        }
    }

    pub(crate) fn sync_parent(&self, path: &ManagedPath) -> Result<(), ManagedFsError> {
        self.sync_directory(path.parent().as_ref())
    }

    fn open_with(
        &self,
        path: &ManagedPath,
        options: &OpenOptions,
        action: &str,
    ) -> Result<std::fs::File, ManagedFsError> {
        self.directory
            .open_with(path.as_path(), options)
            .map(cap_std::fs::File::into_std)
            .map_err(|error| io_error(action, &self.display_path(path), &error))
    }

    fn ensure_parent(&self, path: &ManagedPath) -> Result<(), ManagedFsError> {
        match path.parent() {
            Some(parent) => self.ensure_directory(&parent),
            None => Ok(()),
        }
    }

    fn reject_final_symlink(&self, path: &ManagedPath) -> Result<(), ManagedFsError> {
        if self.entry_kind(path)? == Some(ManagedEntryKind::Symlink) {
            Err(unsafe_component(&self.display_path(path), "symbolic link"))
        } else {
            Ok(())
        }
    }

    fn require_file(&self, path: &ManagedPath) -> Result<(), ManagedFsError> {
        match self.entry_kind(path)? {
            Some(ManagedEntryKind::File) => Ok(()),
            Some(kind) => Err(unexpected_kind(
                &self.display_path(path),
                kind,
                "regular file",
            )),
            None => Err(ManagedFsError::new(
                ManagedFsErrorKind::Io,
                format!(
                    "Managed file {} does not exist",
                    self.display_path(path).display()
                ),
            )),
        }
    }

    fn require_directory(&self, path: &ManagedPath) -> Result<(), ManagedFsError> {
        match self.entry_kind(path)? {
            Some(ManagedEntryKind::Directory) => Ok(()),
            Some(kind) => Err(unexpected_kind(&self.display_path(path), kind, "directory")),
            None => Err(ManagedFsError::new(
                ManagedFsErrorKind::Io,
                format!(
                    "Managed directory {} does not exist",
                    self.display_path(path).display()
                ),
            )),
        }
    }

    fn open_directory(&self, path: &ManagedPath) -> Result<Dir, ManagedFsError> {
        self.require_directory(path)?;
        self.directory
            .open_dir(path.as_path())
            .map_err(|error| io_error("open managed directory", &self.display_path(path), &error))
    }

    fn validate_ancestors(
        &self,
        path: &ManagedPath,
        include_final: bool,
    ) -> Result<(), ManagedFsError> {
        let component_count = path.as_path().components().count();
        let mut current = PathBuf::new();
        for (index, component) in path.as_path().components().enumerate() {
            let Component::Normal(name) = component else {
                return Err(invalid_path(path.as_path()));
            };
            current.push(name);
            let is_final = index + 1 == component_count;
            if is_final && !include_final {
                break;
            }
            match self.directory.symlink_metadata(&current) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return Err(unsafe_component(&self.root.join(&current), "symbolic link"));
                }
                Ok(metadata) if !is_final && !metadata.is_dir() => {
                    return Err(unsafe_component(&self.root.join(&current), "non-directory"));
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
                Err(error) => {
                    return Err(io_error(
                        "inspect managed path",
                        &self.root.join(&current),
                        &error,
                    ));
                }
            }
        }
        Ok(())
    }
}

fn metadata_kind(metadata: &cap_std::fs::Metadata) -> ManagedEntryKind {
    if metadata.file_type().is_symlink() {
        ManagedEntryKind::Symlink
    } else if metadata.is_file() {
        ManagedEntryKind::File
    } else if metadata.is_dir() {
        ManagedEntryKind::Directory
    } else {
        ManagedEntryKind::Other
    }
}

fn invalid_path(path: &Path) -> ManagedFsError {
    ManagedFsError::new(
        ManagedFsErrorKind::InvalidPath,
        format!("Managed path {} is not normalized", path.display()),
    )
}

fn unsafe_component(path: &Path, observed: &str) -> ManagedFsError {
    ManagedFsError::new(
        ManagedFsErrorKind::UnsafeComponent,
        format!(
            "Managed path {} contains an unsafe {observed} component",
            path.display()
        ),
    )
}

fn unexpected_kind(path: &Path, actual: ManagedEntryKind, expected: &str) -> ManagedFsError {
    ManagedFsError::new(
        ManagedFsErrorKind::UnsafeComponent,
        format!(
            "Managed path {} is {actual:?}, expected {expected}",
            path.display()
        ),
    )
}

fn io_error(action: &str, path: &Path, error: &std::io::Error) -> ManagedFsError {
    ManagedFsError::new(
        ManagedFsErrorKind::Io,
        format!("Failed to {action} {}: {error}", path.display()),
    )
}

#[cfg(not(unix))]
fn sync_open_directory(directory: &Dir) -> std::io::Result<()> {
    directory.dir_metadata().map(|_| ())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::{ManagedPath, ProjectFs};

    static NEXT_TEMP_DIRECTORY: AtomicU64 = AtomicU64::new(1);

    struct TestRoot {
        path: PathBuf,
    }

    impl TestRoot {
        fn new() -> Self {
            let sequence = NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "mino-managed-fs-sync-{}-{sequence}",
                std::process::id()
            ));
            fs::create_dir(&path).expect("temporary project root should be created");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let temporary_root = std::env::temp_dir();
            if self.path.starts_with(&temporary_root)
                && self
                    .path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with("mino-managed-fs-sync-"))
            {
                let _ = fs::remove_dir_all(&self.path);
            }
        }
    }

    #[test]
    #[cfg_attr(miri, ignore = "requires host filesystem support")]
    fn project_and_managed_directories_can_be_synchronized() {
        let root = TestRoot::new();
        let filesystem = ProjectFs::open(root.path()).expect("project filesystem should open");
        let managed = ManagedPath::new(".mino").expect("managed path should be valid");
        filesystem
            .ensure_directory(&managed)
            .expect("managed directory should be created");

        filesystem
            .sync_directory(None)
            .expect("project root should synchronize");
        filesystem
            .sync_directory(Some(&managed))
            .expect("managed directory should synchronize");
    }
}
