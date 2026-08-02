//! Platform-aware executable resolution for bounded child processes.

use std::ffi::{OsStr, OsString};
use std::io;

#[cfg(windows)]
use std::path::Path;

#[cfg(windows)]
const SAFE_WINDOWS_EXTENSIONS: &[&str] = &[".COM", ".EXE", ".BAT", ".CMD"];

/// Resolves one executable using only the environment admitted to the child.
///
/// Explicit paths are preserved. On Windows, bare names are resolved from
/// absolute PATH entries with a restricted PATHEXT set so command wrappers and
/// native executables use the same deterministic lookup for probes and checks.
///
/// # Errors
///
/// Returns an invalid-input error for an empty or NUL-containing program, or a
/// not-found error when Windows lookup finds no eligible regular file.
pub(crate) fn resolve_program(
    program: &str,
    path: Option<&OsStr>,
    path_extensions: Option<&OsStr>,
) -> io::Result<OsString> {
    if program.is_empty() || program.contains('\0') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Program must be non-empty and contain no NUL character",
        ));
    }

    #[cfg(windows)]
    {
        resolve_windows_program(program, path, path_extensions)
    }
    #[cfg(not(windows))]
    {
        let _ = (path, path_extensions);
        Ok(OsString::from(program))
    }
}

#[cfg(windows)]
fn resolve_windows_program(
    program: &str,
    path: Option<&OsStr>,
    path_extensions: Option<&OsStr>,
) -> io::Result<OsString> {
    if Path::new(program).is_absolute() || program.contains('/') || program.contains('\\') {
        return Ok(OsString::from(program));
    }

    let has_extension = Path::new(program).extension().is_some();
    let extensions = windows_extensions(path_extensions);
    if let Some(path) = path {
        for directory in std::env::split_paths(path).filter(|entry| entry.is_absolute()) {
            if has_extension {
                let candidate = directory.join(program);
                if candidate.is_file() {
                    return Ok(candidate.into_os_string());
                }
                continue;
            }
            for extension in &extensions {
                let candidate = directory.join(format!("{program}{extension}"));
                if candidate.is_file() {
                    return Ok(candidate.into_os_string());
                }
            }
        }
    }

    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!("Program {program} was not found in the allowed PATH"),
    ))
}

#[cfg(windows)]
fn windows_extensions(path_extensions: Option<&OsStr>) -> Vec<&'static str> {
    let mut extensions = Vec::new();
    if let Some(path_extensions) = path_extensions {
        for raw_extension in path_extensions.to_string_lossy().split(';') {
            let normalized = raw_extension.trim().to_ascii_uppercase();
            let normalized = if normalized.starts_with('.') {
                normalized
            } else {
                format!(".{normalized}")
            };
            let Some(extension) = SAFE_WINDOWS_EXTENSIONS
                .iter()
                .copied()
                .find(|extension| *extension == normalized)
            else {
                continue;
            };
            if !extensions.contains(&extension) {
                extensions.push(extension);
            }
        }
    }
    if extensions.is_empty() {
        extensions.extend_from_slice(SAFE_WINDOWS_EXTENSIONS);
    }
    extensions
}
