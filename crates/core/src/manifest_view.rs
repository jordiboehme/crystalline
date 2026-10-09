//! The MANIFEST as the domain policies card, the MANIFEST editor and
//! `configure` with a domain read it: one reduction of the markdown, so the
//! three surfaces cannot drift apart. Pure: no IO and no clock.

use serde::Serialize;

use crate::manifest::{
    Manifest, PolicyKind, ProblemKind, TagAliasProblemKind, policy_registry, starter_stanzas,
};
use crate::parse::parse_engram;

/// One MANIFEST policy key: the registry row beside what this MANIFEST says.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PolicyRow {
    /// The frontmatter key.
    pub key: String,
    /// `choice` or `text`.
    pub kind: String,
    /// The value as the frontmatter writes it, `None` when the key is absent.
    pub declared: Option<String>,
    /// The value that holds.
    pub effective: String,
    /// The values a `choice` key takes, in display order.
    pub values: Vec<String>,
    /// What an absent or unrecognized declaration is read as.
    pub default: String,
    /// One line, present tense.
    pub meaning: String,
    /// `owner` or `admin`.
    pub changed_by: String,
}

/// Which routing section an agent reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutingSection {
    /// The `When to Use` bullets.
    WhenToUse,
    /// The `Scope` bullets, because `When to Use` is absent or empty.
    Scope,
    /// Nothing routes here.
    None,
}

/// A bullet the core crate flagged, kept verbatim beside why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ManifestProblemRow {
    /// The category, in snake case.
    pub kind: String,
    /// The bullet as written, without its dash.
    pub bullet: String,
    /// Why it was flagged.
    pub reason: String,
}

/// One `kind: path` provisioning declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProvisioningRow {
    /// `skills`, `commands`, `agents` or `mcps`.
    pub kind: String,
    /// The folder, relative to the MANIFEST.
    pub path: String,
}

/// The `Provisioning` section: what parsed, and what did not.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProvisioningFacts {
    /// The declarations that parsed.
    pub decls: Vec<ProvisioningRow>,
    /// The bullets that did not.
    pub problems: Vec<ManifestProblemRow>,
}

/// One `old -> canonical` mapping.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TagAliasRow {
    /// The alias, verbatim.
    pub alias: String,
    /// The canonical tag, verbatim.
    pub canonical: String,
}

/// The `Tag Aliases` section: the mappings kept, and the bullets flagged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TagAliasFacts {
    /// The mappings kept.
    pub decls: Vec<TagAliasRow>,
    /// The bullets flagged.
    pub problems: Vec<ManifestProblemRow>,
}

/// One MANIFEST body section that does something.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SectionRow {
    /// The H2 heading.
    pub name: String,
    /// Whether this MANIFEST has the section.
    pub present: bool,
    /// Whether verify requires it.
    pub required: bool,
    /// One sentence on what it does.
    pub meaning: String,
}

/// Everything the MANIFEST says, as rows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ManifestFacts {
    /// Whether the markdown parsed. A MANIFEST that did not parse declares
    /// nothing: its `policies` are empty and every section is absent.
    pub parsed: bool,
    /// The `Scope` bullets.
    pub scope: Vec<String>,
    /// The `When to Use` bullets.
    pub when_to_use: Vec<String>,
    /// Which of the two an agent reads.
    pub routing: RoutingSection,
    /// The required sections the MANIFEST lacks, by name.
    pub missing: Vec<String>,
    /// The `Provisioning` section, `None` when absent.
    pub provisioning: Option<ProvisioningFacts>,
    /// The `Tag Aliases` section, `None` when absent.
    pub tag_aliases: Option<TagAliasFacts>,
    /// One row per registry key.
    pub policies: Vec<PolicyRow>,
    /// One row per section that does something, in display order.
    pub sections: Vec<SectionRow>,
}

/// The rows of `markdown`, read the way the engine reads every MANIFEST.
/// `domain` is this machine's local name for the domain, the fallback of a
/// `text` key whose declaration is absent or invalid.
pub fn manifest_facts(markdown: &str, domain: &str) -> ManifestFacts {
    let Ok(engram) = parse_engram(markdown) else {
        return ManifestFacts {
            parsed: false,
            scope: Vec::new(),
            when_to_use: Vec::new(),
            routing: RoutingSection::None,
            missing: vec!["Scope".to_string(), "When to Use".to_string()],
            provisioning: None,
            tag_aliases: None,
            policies: Vec::new(),
            sections: section_rows(None),
        };
    };
    let manifest = Manifest::from_engram(&engram, markdown);
    let routing = if !manifest.when_to_use().is_empty() {
        RoutingSection::WhenToUse
    } else if !manifest.scope().is_empty() {
        RoutingSection::Scope
    } else {
        RoutingSection::None
    };
    ManifestFacts {
        parsed: true,
        scope: manifest.scope().to_vec(),
        when_to_use: manifest.when_to_use().to_vec(),
        routing,
        missing: manifest
            .missing_required_sections()
            .iter()
            .map(|name| name.to_string())
            .collect(),
        provisioning: manifest.provisioning().map(|section| ProvisioningFacts {
            decls: section
                .decls
                .iter()
                .map(|decl| ProvisioningRow {
                    kind: decl.kind.id().to_string(),
                    path: decl.path.clone(),
                })
                .collect(),
            problems: section
                .problems
                .iter()
                .map(|problem| ManifestProblemRow {
                    kind: provisioning_problem_kind(problem.kind).to_string(),
                    bullet: problem.bullet.clone(),
                    reason: problem.reason.clone(),
                })
                .collect(),
        }),
        tag_aliases: manifest.tag_aliases().map(|section| TagAliasFacts {
            decls: section
                .decls
                .iter()
                .map(|decl| TagAliasRow {
                    alias: decl.alias.clone(),
                    canonical: decl.canonical.clone(),
                })
                .collect(),
            problems: section
                .problems
                .iter()
                .map(|problem| ManifestProblemRow {
                    kind: tag_alias_problem_kind(problem.kind).to_string(),
                    bullet: problem.bullet.clone(),
                    reason: problem.reason.clone(),
                })
                .collect(),
        }),
        policies: policy_rows(Some(&manifest), domain),
        sections: section_rows(Some(&manifest)),
    }
}

/// The registry rows joined with what `manifest` declares; `None` is a domain
/// with no readable MANIFEST, every key at its default. A `text` key whose
/// effective value is empty reads as `domain`.
pub fn policy_rows(manifest: Option<&Manifest>, domain: &str) -> Vec<PolicyRow> {
    policy_registry()
        .iter()
        .map(|spec| {
            let (declared, effective) = manifest
                .and_then(|m| m.policy(spec.key))
                .unwrap_or((None, spec.default));
            let effective = if spec.kind == PolicyKind::Text && effective.is_empty() {
                domain.to_string()
            } else {
                effective.to_string()
            };
            PolicyRow {
                key: spec.key.to_string(),
                kind: spec.kind.as_str().to_string(),
                declared: declared.map(str::to_string),
                effective,
                values: spec.values.iter().map(|v| v.to_string()).collect(),
                default: spec.default.to_string(),
                meaning: spec.meaning.to_string(),
                changed_by: spec.changed_by.as_str().to_string(),
            }
        })
        .collect()
}

/// One row per section a MANIFEST can declare, in the starter stanzas'
/// order, with whether `manifest` has it. `None` has none of them.
pub fn section_rows(manifest: Option<&Manifest>) -> Vec<SectionRow> {
    starter_stanzas()
        .iter()
        .map(|stanza| {
            let present = manifest.is_some_and(|m| match stanza.section {
                "When to Use" => m.has_when_to_use(),
                "Scope" => m.has_scope(),
                "Provisioning" => m.provisioning().is_some(),
                "Tag Aliases" => m.tag_aliases().is_some(),
                _ => false,
            });
            SectionRow {
                name: stanza.section.to_string(),
                present,
                required: stanza.required,
                meaning: stanza.meaning.to_string(),
            }
        })
        .collect()
}

/// The wire spelling of a provisioning problem's kind.
fn provisioning_problem_kind(kind: ProblemKind) -> &'static str {
    match kind {
        ProblemKind::Malformed => "malformed",
        ProblemKind::UnknownType => "unknown_type",
        ProblemKind::InvalidPath => "invalid_path",
        ProblemKind::DuplicateType => "duplicate_type",
    }
}

/// The wire spelling of a tag alias problem's kind.
fn tag_alias_problem_kind(kind: TagAliasProblemKind) -> &'static str {
    match kind {
        TagAliasProblemKind::Malformed => "malformed",
        TagAliasProblemKind::SelfAlias => "self_alias",
        TagAliasProblemKind::DuplicateAlias => "duplicate_alias",
        TagAliasProblemKind::NonCanonicalTarget => "non_canonical_target",
        TagAliasProblemKind::ChainedAlias => "chained_alias",
    }
}
