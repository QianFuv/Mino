//! Bundled repository-level Mino Skill inspection and safe installation.

use std::fs;
use std::path::{Path, PathBuf};

use crate::MinoError;

use super::{
    IntegrationArtifactKind, IntegrationFindingSeverity, IntegrationReport, IntegrationStatus,
    IntegrationWriter, artifact, ensure_no_symlink, finding, guarded_write,
};

const SKILL_MARKER: &str = "<!-- mino-managed-skill:v1 -->";
const SKILL_RELATIVE_ROOT: &str = ".agents/skills/mino";

struct BundledFile {
    relative_path: &'static str,
    bytes: &'static [u8],
}

type InspectedFile = (PathBuf, Option<Vec<u8>>, &'static [u8]);

const BUNDLED_FILES: &[BundledFile] = &[
    BundledFile {
        relative_path: "SKILL.md",
        bytes: include_bytes!("../../assets/skill/mino/SKILL.md"),
    },
    BundledFile {
        relative_path: "agents/openai.yaml",
        bytes: include_bytes!("../../assets/skill/mino/agents/openai.yaml"),
    },
    BundledFile {
        relative_path: "references/approval-boundaries.md",
        bytes: include_bytes!("../../assets/skill/mino/references/approval-boundaries.md"),
    },
    BundledFile {
        relative_path: "references/command-contract.md",
        bytes: include_bytes!("../../assets/skill/mino/references/command-contract.md"),
    },
    BundledFile {
        relative_path: "references/examples/draft-plan.yaml",
        bytes: include_bytes!("../../assets/skill/mino/references/examples/draft-plan.yaml"),
    },
    BundledFile {
        relative_path: "references/examples/git-cleanup-proposal.yaml",
        bytes: include_bytes!(
            "../../assets/skill/mino/references/examples/git-cleanup-proposal.yaml"
        ),
    },
    BundledFile {
        relative_path: "references/examples/amendment-patch.yaml",
        bytes: include_bytes!("../../assets/skill/mino/references/examples/amendment-patch.yaml"),
    },
    BundledFile {
        relative_path: "references/examples/review-rework-task.yaml",
        bytes: include_bytes!(
            "../../assets/skill/mino/references/examples/review-rework-task.yaml"
        ),
    },
];

pub(super) fn reconcile(
    root: &Path,
    should_apply: bool,
    writer: Option<&IntegrationWriter>,
    report: &mut IntegrationReport,
) -> Result<(), MinoError> {
    let skill_root = root.join(SKILL_RELATIVE_ROOT);
    if let Err(error) = ensure_no_symlink(root, &skill_root) {
        report.artifacts.push(artifact(
            IntegrationArtifactKind::Skill,
            skill_root.clone(),
            IntegrationStatus::Conflict,
            None,
        ));
        report.findings.push(finding(
            "mino_skill_conflict",
            IntegrationFindingSeverity::Error,
            error.message(),
            skill_root,
        ));
        return Ok(());
    }
    if !skill_root.exists() {
        return reconcile_missing(&skill_root, should_apply, writer, report);
    }
    if !skill_root.is_dir() {
        conflict(
            skill_root,
            "The repository-level Mino Skill path exists but is not a directory",
            report,
        );
        return Ok(());
    }
    let entry_path = skill_root.join("SKILL.md");
    let Some(entry_bytes) = read_optional(&entry_path)? else {
        conflict(
            skill_root,
            "The existing Skill directory has no ownership-bearing SKILL.md",
            report,
        );
        return Ok(());
    };
    let is_owned =
        std::str::from_utf8(&entry_bytes).is_ok_and(|contents| contents.contains(SKILL_MARKER));
    let current = match inspect_files(root, &skill_root) {
        Ok(current) => current,
        Err(error) => {
            conflict(skill_root, error.message(), report);
            return Ok(());
        }
    };
    if current
        .iter()
        .all(|(_, actual, expected)| actual.as_deref() == Some(*expected))
    {
        report.artifacts.push(artifact(
            IntegrationArtifactKind::Skill,
            skill_root,
            IntegrationStatus::Current,
            None,
        ));
        return Ok(());
    }
    if !is_owned {
        conflict(
            skill_root,
            "The existing Skill differs and has no Mino ownership marker; every byte was preserved",
            report,
        );
        return Ok(());
    }
    if !should_apply {
        report.artifacts.push(artifact(
            IntegrationArtifactKind::Skill,
            skill_root.clone(),
            IntegrationStatus::Drift,
            Some("Refresh only the Mino-owned bundled Skill files.".to_owned()),
        ));
        report.findings.push(finding(
            "mino_skill_drift",
            IntegrationFindingSeverity::Warning,
            "The Mino-owned Skill differs from the bundled version",
            skill_root,
        ));
        return Ok(());
    }
    for (path, actual, expected) in current {
        guarded_write(writer, &path, actual.as_deref(), expected)?;
    }
    report.artifacts.push(artifact(
        IntegrationArtifactKind::Skill,
        skill_root,
        IntegrationStatus::Updated,
        None,
    ));
    Ok(())
}

fn reconcile_missing(
    skill_root: &Path,
    should_apply: bool,
    writer: Option<&IntegrationWriter>,
    report: &mut IntegrationReport,
) -> Result<(), MinoError> {
    if should_apply {
        for bundled in BUNDLED_FILES {
            guarded_write(
                writer,
                &skill_root.join(bundled.relative_path),
                None,
                bundled.bytes,
            )?;
        }
        report.artifacts.push(artifact(
            IntegrationArtifactKind::Skill,
            skill_root.to_path_buf(),
            IntegrationStatus::Created,
            None,
        ));
    } else {
        report.artifacts.push(artifact(
            IntegrationArtifactKind::Skill,
            skill_root.to_path_buf(),
            IntegrationStatus::Missing,
            Some("Install the exact bundled repository-level Mino Skill.".to_owned()),
        ));
        report.findings.push(finding(
            "mino_skill_missing",
            IntegrationFindingSeverity::Warning,
            "The repository-level Mino Skill is not installed",
            skill_root.to_path_buf(),
        ));
    }
    Ok(())
}

fn inspect_files(root: &Path, skill_root: &Path) -> Result<Vec<InspectedFile>, MinoError> {
    let mut files = Vec::with_capacity(BUNDLED_FILES.len());
    for bundled in BUNDLED_FILES {
        let path = skill_root.join(bundled.relative_path);
        ensure_no_symlink(root, &path)?;
        files.push((path.clone(), read_optional(&path)?, bundled.bytes));
    }
    Ok(files)
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, MinoError> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(MinoError::new(
            crate::ErrorCategory::EnvironmentUnavailable,
            format!("Failed to read {}: {error}", path.display()),
        )),
    }
}

fn conflict(skill_root: PathBuf, message: &str, report: &mut IntegrationReport) {
    report.artifacts.push(artifact(
        IntegrationArtifactKind::Skill,
        skill_root.clone(),
        IntegrationStatus::Conflict,
        None,
    ));
    report.findings.push(finding(
        "mino_skill_conflict",
        IntegrationFindingSeverity::Error,
        message,
        skill_root,
    ));
}
