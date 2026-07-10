//! The Manifest model.
//!
//! A MANIFEST Engram sits at a domain root and drives routing. It must carry an
//! H2 `Scope` and an H2 `When to Use` section (matched case-insensitively after
//! trimming whitespace). Each section contributes its zero-indent bullets;
//! `When to Use` bullets are the routing input, falling back to `Scope`.

use std::path::{Path, PathBuf};

use indexmap::IndexMap;

use crate::engram::Engram;
use crate::parse::{body_lines, locate, parse_engram, parse_heading};

const SCOPE: &str = "scope";
const WHEN_TO_USE: &str = "when to use";
const PROVISIONING: &str = "provisioning";

/// The starter MANIFEST engram for a new domain: valid frontmatter and the two
/// required routing sections (`Scope`, `When to Use`) plus a `Notes for Agents`
/// section, all as prompts to fill in. `today` is a pre-formatted `%Y-%m-%d`
/// date so this stays free of a time dependency. Shared by `domain init`,
/// `domain add --virtual` and the MCP `add_domain` tool so every scaffold looks
/// the same.
pub fn manifest_template(name: &str, today: &str) -> String {
    format!(
        "---\n\
type: manifest\n\
title: {name}\n\
permalink: manifest\n\
tags:\n  - manifest\n\
status: current\n\
recorded_at: {today}\n\
---\n\n\
# {name}\n\n\
## Scope\n\n\
- Describe the knowledge this domain covers\n\n\
## When to Use\n\n\
- Describe when an agent should route here\n\n\
## Notes for Agents\n\n\
- Add guidance for agents working in this domain\n"
    )
}

/// A parsed Manifest: the H2 sections and their top-level bullets.
#[derive(Debug, Clone, PartialEq)]
pub struct Manifest {
    /// Canonical H2 section key (lowercased, whitespace-collapsed) to its
    /// zero-indent bullets, in document order. The first of any duplicate H2
    /// wins.
    pub sections: IndexMap<String, Vec<String>>,
}

impl Manifest {
    /// Build a Manifest from an Engram and its source text.
    pub fn from_engram(engram: &Engram, source: &str) -> Manifest {
        let (_, _, body_start) = locate(source);
        let body = &source[body_start..];
        let body_line_start = source[..body_start].bytes().filter(|b| *b == b'\n').count() + 1;
        let _ = &engram.frontmatter; // frontmatter validation belongs to verify.

        let mut sections: IndexMap<String, Vec<String>> = IndexMap::new();
        let mut current: Option<String> = None;

        for bl in body_lines(body, body_line_start) {
            if bl.in_fence {
                continue;
            }
            if let Some((level, text)) = parse_heading(bl.text) {
                if level == 2 {
                    let key = section_key(&text);
                    if sections.contains_key(&key) {
                        // First duplicate H2 wins: ignore the later section.
                        current = None;
                    } else {
                        sections.insert(key.clone(), Vec::new());
                        current = Some(key);
                    }
                } else if level < 2 {
                    // A new top-level section ends bullet collection.
                    current = None;
                }
                continue;
            }
            if let Some(key) = &current
                && let Some(bullet) = zero_indent_bullet(bl.text)
                && let Some(list) = sections.get_mut(key)
            {
                list.push(bullet.to_string());
            }
        }

        Manifest { sections }
    }

    /// The `Scope` bullets, if the section is present.
    pub fn scope(&self) -> &[String] {
        self.sections
            .get(SCOPE)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    /// The `When to Use` bullets, if the section is present.
    pub fn when_to_use(&self) -> &[String] {
        self.sections
            .get(WHEN_TO_USE)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    /// The routing bullets: `When to Use` bullets, falling back to `Scope` when
    /// `When to Use` is absent or empty.
    pub fn routing_bullets(&self) -> &[String] {
        let wtu = self.when_to_use();
        if wtu.is_empty() { self.scope() } else { wtu }
    }

    /// Whether the required `Scope` section is present.
    pub fn has_scope(&self) -> bool {
        self.sections.contains_key(SCOPE)
    }

    /// Whether the required `When to Use` section is present.
    pub fn has_when_to_use(&self) -> bool {
        self.sections.contains_key(WHEN_TO_USE)
    }

    /// The required sections that are missing, by canonical name.
    pub fn missing_required_sections(&self) -> Vec<&'static str> {
        let mut missing = Vec::new();
        if !self.has_scope() {
            missing.push("Scope");
        }
        if !self.has_when_to_use() {
            missing.push("When to Use");
        }
        missing
    }

    /// The `Provisioning` section, if present. `None` means the section is
    /// absent from the MANIFEST; `Some` means it is present, even when empty
    /// or made entirely of problem bullets. Parsing a bullet never fails: one
    /// that cannot become a [`ProvisioningDecl`] is kept as a
    /// [`ProvisioningProblem`] instead, so a caller can surface it without a
    /// MANIFEST-wide parse error.
    pub fn provisioning(&self) -> Option<ProvisioningSection> {
        let bullets = self.sections.get(PROVISIONING)?;
        let mut decls: Vec<ProvisioningDecl> = Vec::new();
        let mut problems: Vec<ProvisioningProblem> = Vec::new();

        for bullet in bullets {
            match parse_provisioning_bullet(bullet) {
                Ok(decl) => {
                    if decls.iter().any(|d| d.kind == decl.kind) {
                        problems.push(ProvisioningProblem {
                            kind: ProblemKind::DuplicateType,
                            bullet: bullet.clone(),
                            reason: format!(
                                "duplicate `{}` declaration, the first one wins",
                                decl.kind.id()
                            ),
                        });
                    } else {
                        decls.push(decl);
                    }
                }
                Err((kind, reason)) => problems.push(ProvisioningProblem {
                    kind,
                    bullet: bullet.clone(),
                    reason,
                }),
            }
        }

        Some(ProvisioningSection { decls, problems })
    }
}

/// A category of deployable artifact a domain can provision into an AI
/// harness config directory: skills, commands, agents or MCP configs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactType {
    /// Agent skills.
    Skills,
    /// Slash commands.
    Commands,
    /// Subagent definitions.
    Agents,
    /// MCP server configs.
    Mcps,
}

impl ArtifactType {
    /// The canonical lowercase identifier used in a `Provisioning` bullet's
    /// type and echoed back in a duplicate-declaration problem.
    pub fn id(&self) -> &'static str {
        match self {
            ArtifactType::Skills => "skills",
            ArtifactType::Commands => "commands",
            ArtifactType::Agents => "agents",
            ArtifactType::Mcps => "mcps",
        }
    }

    /// Parse from an already-lowercased type string, `None` when it names
    /// none of the four known artifact types.
    fn parse(lowercased: &str) -> Option<ArtifactType> {
        match lowercased {
            "skills" => Some(ArtifactType::Skills),
            "commands" => Some(ArtifactType::Commands),
            "agents" => Some(ArtifactType::Agents),
            "mcps" => Some(ArtifactType::Mcps),
            _ => None,
        }
    }
}

/// One `type: path` declaration parsed from a `Provisioning` bullet. `path`
/// is relative to the MANIFEST and may still climb out of the domain root
/// with `..` components; that is a core feature, checked at resolution time
/// rather than here.
#[derive(Debug, Clone, PartialEq)]
pub struct ProvisioningDecl {
    /// The artifact type this folder provisions.
    pub kind: ArtifactType,
    /// The declared path, relative to the MANIFEST, trailing slash trimmed.
    pub path: String,
}

/// Why a `Provisioning` bullet did not become a clean [`ProvisioningDecl`].
/// Verify maps each kind to a distinct rule id, so it never has to match on
/// the free-form `reason` text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProblemKind {
    /// No `type: path` shape: a missing colon or an empty path.
    Malformed,
    /// The type names none of the four known artifact types.
    UnknownType,
    /// The path is absolute, home-relative, `.` or has a bad component.
    InvalidPath,
    /// A second declaration for a type that was already declared.
    DuplicateType,
}

/// A `Provisioning` bullet that did not parse into a [`ProvisioningDecl`],
/// kept verbatim alongside why it was rejected.
#[derive(Debug, Clone, PartialEq)]
pub struct ProvisioningProblem {
    /// The category of rejection, for routing to a verify rule.
    pub kind: ProblemKind,
    /// The original bullet text.
    pub bullet: String,
    /// Why the bullet was rejected.
    pub reason: String,
}

/// The parsed `Provisioning` section: clean declarations in document order,
/// plus any bullets that did not parse.
#[derive(Debug, Clone, PartialEq)]
pub struct ProvisioningSection {
    /// Declarations that parsed cleanly, in document order. At most one per
    /// [`ArtifactType`]: a later duplicate becomes a problem instead.
    pub decls: Vec<ProvisioningDecl>,
    /// Bullets that did not parse, or lost to an earlier duplicate.
    pub problems: Vec<ProvisioningProblem>,
}

/// The absolute artifact folders a domain declares INSIDE its own root, the
/// exclusion set shared by engram indexing and the verify markdown scan: those
/// folders hold deployable artifacts, not knowledge. Reads `root/MANIFEST.md`
/// and returns an empty vec whenever it is missing, unreadable or unparseable
/// (never an error), so a caller can compute exclusions unconditionally.
///
/// Each decl path is normalized LOGICALLY, processing `.` and `..` textually
/// without touching the filesystem, since the folders may not exist yet. A path
/// that climbs to or above the root is not an in-root folder and is dropped:
/// an out-of-root decl (`../skills`) is provisioned from elsewhere, and a
/// root-landing decl (`foo/..`) would exclude the whole domain.
pub fn in_root_artifact_dirs(root: &Path) -> Vec<PathBuf> {
    let manifest_path = root.join("MANIFEST.md");
    let Ok(source) = std::fs::read_to_string(&manifest_path) else {
        return Vec::new();
    };
    let Ok(engram) = parse_engram(&source) else {
        return Vec::new();
    };
    let manifest = Manifest::from_engram(&engram, &source);
    let Some(section) = manifest.provisioning() else {
        return Vec::new();
    };
    section
        .decls
        .iter()
        .filter_map(|decl| normalize_in_root(&decl.path).map(|rel| root.join(rel)))
        .collect()
}

/// Logically normalize a relative decl path, or `None` when it does not name a
/// folder strictly inside the root. `.` and empty components are dropped and
/// `..` pops the last kept component; a `..` with nothing to pop climbs above
/// the root, and an empty result lands on the root itself. Both cases return
/// `None`, since neither is an in-root folder to exclude.
fn normalize_in_root(path: &str) -> Option<PathBuf> {
    let (kept, climbs) = normalize_relative(path);
    if climbs > 0 || kept.is_empty() {
        return None;
    }
    Some(kept.iter().collect())
}

/// The textual `.`/`..` normalization shared by [`normalize_in_root`] and the
/// `provision` module's source-root resolution: split `path` on `/`, drop `.`
/// and empty components, and let `..` pop the last kept component. Returns the
/// components still kept once popping settles, plus how many `..` climbed past
/// an already-empty stack. Zero climbs means the result stays at or inside
/// whatever root the caller joins `kept` onto; more than zero means the path
/// climbs above that root, which `normalize_in_root` treats as leaving no
/// in-root folder to exclude, and `provision::resolve_source_roots` treats as
/// a decl that resolves from somewhere else entirely.
///
/// Public so the `crystalline-remote` crate can reuse the exact same `.`/`..`
/// semantics when it decides which team-domain decls point out of the fetched
/// subtree and where they land relative to the repository root, keeping the
/// mirror it populates in step with the source roots `resolve_source_roots`
/// resolves against it.
pub fn normalize_relative(path: &str) -> (Vec<&str>, usize) {
    let mut kept: Vec<&str> = Vec::new();
    let mut climbs: usize = 0;
    for component in path.split('/') {
        match component {
            "" | "." => continue,
            ".." => {
                if kept.pop().is_none() {
                    climbs += 1;
                }
            }
            other => kept.push(other),
        }
    }
    (kept, climbs)
}

fn section_key(title: &str) -> String {
    title
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn zero_indent_bullet(line: &str) -> Option<&str> {
    line.strip_prefix("- ").map(str::trim)
}

/// Parse one `Provisioning` bullet into a declaration, or the [`ProblemKind`]
/// and reason it was rejected. The bullet arrives already `- `-stripped and
/// trimmed.
fn parse_provisioning_bullet(bullet: &str) -> Result<ProvisioningDecl, (ProblemKind, String)> {
    let Some(colon) = bullet.find(':') else {
        return Err((
            ProblemKind::Malformed,
            format!("expected a `type: path` shape, no colon found in `{bullet}`"),
        ));
    };
    let kind_str = bullet[..colon].trim().to_lowercase();
    let Some(kind) = ArtifactType::parse(&kind_str) else {
        return Err((
            ProblemKind::UnknownType,
            format!(
                "unknown provisioning type `{kind_str}`, expected one of skills, commands, agents or mcps"
            ),
        ));
    };
    let mut path = bullet[colon + 1..].trim();
    if let Some(trimmed) = path.strip_suffix('/') {
        path = trimmed;
    }
    if path.is_empty() {
        return Err((ProblemKind::Malformed, "path is empty".to_string()));
    }
    if let Some(reason) = invalid_provisioning_path(path) {
        return Err((ProblemKind::InvalidPath, reason));
    }
    Ok(ProvisioningDecl {
        kind,
        path: path.to_string(),
    })
}

/// Why `path` cannot be provisioned from, or `None` when it is fine. The empty
/// path is caught earlier as [`ProblemKind::Malformed`]; everything rejected
/// here is a [`ProblemKind::InvalidPath`]. Absolute and home-relative paths are
/// rejected because provisioning paths are relative to the MANIFEST; `..`
/// components are allowed, since climbing out of the domain root is a core
/// feature checked later at resolution time. The `:` and `\` checks per
/// component guard the same Windows drive-relative and UNC forms as
/// `is_plain_skill_name` in the CLI's install path, applied to every path
/// component rather than a single name.
fn invalid_provisioning_path(path: &str) -> Option<String> {
    if path.starts_with('/') || path.starts_with('\\') || path.starts_with('~') {
        return Some(format!(
            "path `{path}` is absolute or home-relative, provisioning paths are relative to the MANIFEST"
        ));
    }
    if path == "." {
        return Some("path `.` does not name a folder".to_string());
    }
    for component in path.split('/') {
        if component.is_empty()
            || component == "."
            || component.contains(':')
            || component.contains('\\')
        {
            return Some(format!(
                "path `{path}` has an invalid component `{component}`"
            ));
        }
    }
    None
}
