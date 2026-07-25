//! Embedded inert standards package parsing, validation, and digesting.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::project::Language;
use crate::store::sha256_digest;
use crate::{ErrorCategory, MinoError};

/// Requirement level assigned to an inert standards rule.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuleLevel {
    /// The rule must be followed when its package applies.
    Required,
    /// The rule is preferred but may be superseded by project facts.
    Recommended,
}

/// One inert standards rule.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StandardRule {
    /// Globally stable rule identifier.
    pub id: String,
    /// Requirement level.
    pub level: RuleLevel,
    /// Human-readable rule text.
    pub text: String,
}

/// One inert verification command template.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckTemplate {
    /// Globally stable check identifier.
    pub id: String,
    /// Canonical default argument vector.
    pub argv: Vec<String>,
    /// Executable or capability required by the template.
    pub tool: String,
    /// Whether plan completion requires this check.
    pub required: bool,
    /// Optional project script that takes precedence over the default command.
    #[serde(default)]
    pub project_script: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PackageManifest {
    package_id: String,
    display_name: String,
    version: String,
    languages: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RulesDocument {
    rules: Vec<StandardRule>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChecksDocument {
    checks: Vec<CheckTemplate>,
}

/// One parsed, versioned, digest-bound first-party standards package.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct StandardsPackage {
    package_id: String,
    display_name: String,
    version: String,
    digest: String,
    languages: Vec<Language>,
    rules: Vec<StandardRule>,
    checks: Vec<CheckTemplate>,
}

impl StandardsPackage {
    /// Returns the stable package identifier.
    #[must_use]
    pub fn package_id(&self) -> &str {
        &self.package_id
    }

    /// Returns the package display name.
    #[must_use]
    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    /// Returns the exact embedded package version.
    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Returns the SHA-256 digest over the three inert package documents.
    #[must_use]
    pub fn digest(&self) -> &str {
        &self.digest
    }

    /// Returns languages governed by this package.
    #[must_use]
    pub fn languages(&self) -> &[Language] {
        &self.languages
    }

    /// Returns package rules in stable identifier order.
    #[must_use]
    pub fn rules(&self) -> &[StandardRule] {
        &self.rules
    }

    /// Returns check templates in stable identifier order.
    #[must_use]
    pub fn checks(&self) -> &[CheckTemplate] {
        &self.checks
    }
}

/// Validated first-party standards packages embedded into the binary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct EmbeddedCatalog {
    packages: Vec<StandardsPackage>,
}

impl EmbeddedCatalog {
    /// Parses and validates all embedded first-party packages.
    ///
    /// # Errors
    ///
    /// Returns a validation error for malformed TOML, unknown languages,
    /// duplicate identifiers, empty commands, or unexpected package IDs.
    pub fn load() -> Result<Self, MinoError> {
        let mut packages = vec![
            parse_package(
                "common",
                include_str!("../../assets/standards/common/manifest.toml"),
                include_str!("../../assets/standards/common/rules.toml"),
                include_str!("../../assets/standards/common/checks.toml"),
            )?,
            parse_package(
                "rust",
                include_str!("../../assets/standards/rust/manifest.toml"),
                include_str!("../../assets/standards/rust/rules.toml"),
                include_str!("../../assets/standards/rust/checks.toml"),
            )?,
            parse_package(
                "typescript-javascript",
                include_str!("../../assets/standards/typescript-javascript/manifest.toml"),
                include_str!("../../assets/standards/typescript-javascript/rules.toml"),
                include_str!("../../assets/standards/typescript-javascript/checks.toml"),
            )?,
            parse_package(
                "python",
                include_str!("../../assets/standards/python/manifest.toml"),
                include_str!("../../assets/standards/python/rules.toml"),
                include_str!("../../assets/standards/python/checks.toml"),
            )?,
        ];
        packages.sort_by(|left, right| left.package_id.cmp(&right.package_id));
        validate_global_ids(&packages)?;
        Ok(Self { packages })
    }

    /// Returns packages in stable package-ID order.
    #[must_use]
    pub fn packages(&self) -> &[StandardsPackage] {
        &self.packages
    }

    /// Returns one package by stable identifier.
    #[must_use]
    pub fn package(&self, package_id: &str) -> Option<&StandardsPackage> {
        self.packages
            .iter()
            .find(|package| package.package_id == package_id)
    }

    /// Returns the package governing a language.
    #[must_use]
    pub fn package_for_language(&self, language: Language) -> Option<&StandardsPackage> {
        self.packages
            .iter()
            .find(|package| package.languages.contains(&language))
    }
}

fn parse_package(
    expected_id: &str,
    manifest_source: &str,
    rules_source: &str,
    checks_source: &str,
) -> Result<StandardsPackage, MinoError> {
    let manifest: PackageManifest = parse_document(expected_id, "manifest", manifest_source)?;
    let mut rules: RulesDocument = parse_document(expected_id, "rules", rules_source)?;
    let mut checks: ChecksDocument = parse_document(expected_id, "checks", checks_source)?;
    if manifest.package_id != expected_id || manifest.version.trim().is_empty() {
        return Err(catalog_error(format!(
            "Embedded package {expected_id} has an invalid ID or version"
        )));
    }
    let languages = manifest
        .languages
        .iter()
        .map(|language| parse_language(language))
        .collect::<Result<Vec<_>, _>>()?;
    rules.rules.sort_by(|left, right| left.id.cmp(&right.id));
    checks.checks.sort_by(|left, right| left.id.cmp(&right.id));
    if rules
        .rules
        .iter()
        .any(|rule| rule.id.trim().is_empty() || rule.text.trim().is_empty())
        || checks.checks.iter().any(|check| {
            check.id.trim().is_empty()
                || check.argv.is_empty()
                || check.argv.iter().any(|argument| argument.trim().is_empty())
                || check.tool.trim().is_empty()
        })
    {
        return Err(catalog_error(format!(
            "Embedded package {expected_id} contains an incomplete rule or check"
        )));
    }
    let mut digest_input = Vec::new();
    for (name, source) in [
        ("manifest.toml", manifest_source),
        ("rules.toml", rules_source),
        ("checks.toml", checks_source),
    ] {
        digest_input.extend_from_slice(name.as_bytes());
        digest_input.push(0);
        digest_input.extend_from_slice(normalize_lf(source).as_bytes());
        digest_input.push(0);
    }
    Ok(StandardsPackage {
        package_id: manifest.package_id,
        display_name: manifest.display_name,
        version: manifest.version,
        digest: sha256_digest(&digest_input),
        languages,
        rules: rules.rules,
        checks: checks.checks,
    })
}

fn parse_document<T: for<'de> Deserialize<'de>>(
    package_id: &str,
    document: &str,
    source: &str,
) -> Result<T, MinoError> {
    toml::from_str(source).map_err(|error| {
        catalog_error(format!(
            "Failed to parse embedded {package_id}/{document}.toml: {error}"
        ))
    })
}

fn parse_language(language: &str) -> Result<Language, MinoError> {
    match language {
        "rust" => Ok(Language::Rust),
        "typescript-javascript" => Ok(Language::TypeScriptJavaScript),
        "python" => Ok(Language::Python),
        _ => Err(catalog_error(format!(
            "Embedded catalog contains unknown language {language}"
        ))),
    }
}

fn validate_global_ids(packages: &[StandardsPackage]) -> Result<(), MinoError> {
    let package_ids = packages
        .iter()
        .map(|package| package.package_id.as_str())
        .collect::<BTreeSet<_>>();
    let rule_ids = packages
        .iter()
        .flat_map(|package| package.rules.iter().map(|rule| rule.id.as_str()))
        .collect::<BTreeSet<_>>();
    let check_ids = packages
        .iter()
        .flat_map(|package| package.checks.iter().map(|check| check.id.as_str()))
        .collect::<BTreeSet<_>>();
    let rule_count = packages
        .iter()
        .map(|package| package.rules.len())
        .sum::<usize>();
    let check_count = packages
        .iter()
        .map(|package| package.checks.len())
        .sum::<usize>();
    if package_ids.len() != packages.len()
        || rule_ids.len() != rule_count
        || check_ids.len() != check_count
    {
        Err(catalog_error(
            "Embedded catalog contains duplicate package, rule, or check identifiers",
        ))
    } else {
        Ok(())
    }
}

fn normalize_lf(source: &str) -> String {
    source.replace("\r\n", "\n").replace('\r', "\n")
}

fn catalog_error(message: impl Into<String>) -> MinoError {
    MinoError::new(ErrorCategory::IncompleteOrValidation, message)
}
