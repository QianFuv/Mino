//! Read-only Git observations used by protocol policy gates.

mod changes;

pub use changes::{
    ChangedFile, GitChangeError, GitChangeErrorKind, GitChangeSet, inspect_changes,
    matches_file_map_path,
};
