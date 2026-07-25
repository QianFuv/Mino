//! Strict parsing for NUL-delimited Git status porcelain version 2.

use std::path::{Component, Path};

use serde::Serialize;

use super::{GitError, GitErrorKind};

/// Stable classification for one porcelain status entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GitStatusKind {
    /// A tracked path with an ordinary index or worktree change.
    Ordinary,
    /// A tracked path reported as renamed or copied.
    RenamedOrCopied,
    /// An unresolved merge entry.
    Unmerged,
    /// An untracked path.
    Untracked,
    /// An ignored path returned by an explicit ignored-files request.
    Ignored,
}

/// One normalized path and its machine-readable index/worktree state.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GitStatusEntry {
    /// Current project-relative path.
    pub path: String,
    /// Original path for a rename or copy.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_path: Option<String>,
    /// Porcelain index status column.
    pub index_status: char,
    /// Porcelain worktree status column.
    pub worktree_status: char,
    /// Four-character porcelain submodule state.
    pub submodule: String,
    /// Record classification.
    pub kind: GitStatusKind,
}

impl GitStatusEntry {
    /// Returns whether the index contains a change for this path.
    #[must_use]
    pub fn is_staged(&self) -> bool {
        !matches!(self.index_status, '.' | '?' | '!')
    }

    /// Returns whether the worktree or untracked set contains a change for this path.
    #[must_use]
    pub fn is_unstaged(&self) -> bool {
        self.worktree_status != '.' || self.kind == GitStatusKind::Untracked
    }

    /// Returns whether Git reports submodule-specific state for this entry.
    #[must_use]
    pub fn is_submodule(&self) -> bool {
        self.submodule.starts_with('S')
    }
}

/// Parsed branch headers and sorted changed-path records.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct PorcelainStatus {
    /// Full current commit object ID, absent for an unborn branch.
    pub branch_oid: Option<String>,
    /// Branch name or the literal detached marker.
    pub branch_head: Option<String>,
    /// Optional upstream branch name.
    pub branch_upstream: Option<String>,
    /// Optional ahead/behind counts from porcelain headers.
    pub ahead: Option<u64>,
    /// Optional ahead/behind counts from porcelain headers.
    pub behind: Option<u64>,
    /// Changed entries sorted by current path and original path.
    pub entries: Vec<GitStatusEntry>,
}

/// Parses `git status --porcelain=v2 --branch -z` bytes.
///
/// # Errors
///
/// Returns an invalid-output error for malformed headers, records, unsafe
/// paths, invalid UTF-8, duplicate paths, or incomplete rename pairs.
pub fn parse_porcelain_v2(bytes: &[u8]) -> Result<PorcelainStatus, GitError> {
    let records = bytes.split(|byte| *byte == 0).collect::<Vec<_>>();
    let mut status = PorcelainStatus::default();
    let mut index = 0;
    while index < records.len() {
        let record = records[index];
        index += 1;
        if record.is_empty() {
            continue;
        }
        match record[0] {
            b'#' => parse_header(record, &mut status)?,
            b'1' => status
                .entries
                .push(parse_tracked(record, 9, GitStatusKind::Ordinary, None)?),
            b'2' => {
                let original = records
                    .get(index)
                    .copied()
                    .ok_or_else(|| invalid("Porcelain rename/copy record has no original path"))?;
                index += 1;
                status.entries.push(parse_tracked(
                    record,
                    10,
                    GitStatusKind::RenamedOrCopied,
                    Some(original),
                )?);
            }
            b'u' => status
                .entries
                .push(parse_tracked(record, 11, GitStatusKind::Unmerged, None)?),
            b'?' => status
                .entries
                .push(parse_simple_path(record, GitStatusKind::Untracked, '?')?),
            b'!' => status
                .entries
                .push(parse_simple_path(record, GitStatusKind::Ignored, '!')?),
            _ => return Err(invalid("Porcelain v2 contains an unknown record type")),
        }
    }
    status.entries.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| left.original_path.cmp(&right.original_path))
    });
    if status
        .entries
        .windows(2)
        .any(|pair| pair[0].path == pair[1].path)
    {
        return Err(invalid("Porcelain v2 contains duplicate current paths"));
    }
    Ok(status)
}

fn parse_header(record: &[u8], status: &mut PorcelainStatus) -> Result<(), GitError> {
    let value = utf8(record)?;
    if let Some(oid) = value.strip_prefix("# branch.oid ") {
        if oid == "(initial)" {
            status.branch_oid = None;
        } else if is_object_id(oid) {
            status.branch_oid = Some(oid.to_owned());
        } else {
            return Err(invalid("Porcelain branch object ID is invalid"));
        }
    } else if let Some(head) = value.strip_prefix("# branch.head ") {
        if head.is_empty() {
            return Err(invalid("Porcelain branch head is empty"));
        }
        status.branch_head = Some(head.to_owned());
    } else if let Some(upstream) = value.strip_prefix("# branch.upstream ") {
        if upstream.is_empty() {
            return Err(invalid("Porcelain branch upstream is empty"));
        }
        status.branch_upstream = Some(upstream.to_owned());
    } else if let Some(counts) = value.strip_prefix("# branch.ab ") {
        let mut fields = counts.split(' ');
        status.ahead = Some(parse_signed_count(fields.next(), '+')?);
        status.behind = Some(parse_signed_count(fields.next(), '-')?);
        if fields.next().is_some() {
            return Err(invalid("Porcelain branch counts have extra fields"));
        }
    }
    Ok(())
}

fn parse_signed_count(value: Option<&str>, prefix: char) -> Result<u64, GitError> {
    value
        .and_then(|value| value.strip_prefix(prefix))
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| invalid("Porcelain branch count is invalid"))
}

fn parse_tracked(
    record: &[u8],
    expected_fields: usize,
    kind: GitStatusKind,
    original_path: Option<&[u8]>,
) -> Result<GitStatusEntry, GitError> {
    let value = utf8(record)?;
    let fields = value.splitn(expected_fields, ' ').collect::<Vec<_>>();
    if fields.len() != expected_fields {
        return Err(invalid(
            "Porcelain tracked record has the wrong field count",
        ));
    }
    let xy = fields[1].as_bytes();
    if xy.len() != 2 || !xy.iter().all(u8::is_ascii) {
        return Err(invalid("Porcelain tracked record has an invalid XY field"));
    }
    let submodule = fields[2];
    if submodule.len() != 4 || !matches!(submodule.as_bytes().first(), Some(b'N' | b'S')) {
        return Err(invalid(
            "Porcelain tracked record has an invalid submodule field",
        ));
    }
    let path = normalize_path(fields[expected_fields - 1])?;
    let original_path = original_path
        .map(utf8)
        .transpose()?
        .map(normalize_path)
        .transpose()?;
    Ok(GitStatusEntry {
        path,
        original_path,
        index_status: char::from(xy[0]),
        worktree_status: char::from(xy[1]),
        submodule: submodule.to_owned(),
        kind,
    })
}

fn parse_simple_path(
    record: &[u8],
    kind: GitStatusKind,
    status: char,
) -> Result<GitStatusEntry, GitError> {
    if record.get(1) != Some(&b' ') {
        return Err(invalid("Porcelain path record has an invalid prefix"));
    }
    Ok(GitStatusEntry {
        path: normalize_path(utf8(&record[2..])?)?,
        original_path: None,
        index_status: status,
        worktree_status: status,
        submodule: "N...".to_owned(),
        kind,
    })
}

fn normalize_path(value: &str) -> Result<String, GitError> {
    let value = value.replace('\\', "/");
    let path = Path::new(&value);
    if value.is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        Err(invalid(format!("Git returned unsafe path {value}")))
    } else {
        Ok(value)
    }
}

fn utf8(bytes: &[u8]) -> Result<&str, GitError> {
    std::str::from_utf8(bytes).map_err(|_| invalid("Git porcelain output must be valid UTF-8"))
}

fn is_object_id(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn invalid(message: impl Into<String>) -> GitError {
    GitError::new(GitErrorKind::InvalidOutput, message)
}
