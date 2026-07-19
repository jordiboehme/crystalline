//! The Manifest model.
//!
//! A MANIFEST Engram sits at a domain root and drives routing. It must carry an
//! H2 `Scope` and an H2 `When to Use` section (matched case-insensitively after
//! trimming whitespace). Each section contributes its zero-indent bullets;
//! `When to Use` bullets are the routing input, falling back to `Scope`.

use std::path::{Path, PathBuf};

use indexmap::IndexMap;

use crate::engram::Engram;
use crate::parse::{body_lines, fence_marker, locate, parse_engram, parse_heading};
use crate::tags::is_lower_hyphen;

const SCOPE: &str = "scope";
const WHEN_TO_USE: &str = "when to use";
const PROVISIONING: &str = "provisioning";
const TAG_ALIASES: &str = "tag aliases";

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

    /// The `Tag Aliases` section, if present. `None` means the section is
    /// absent from the MANIFEST; `Some` means it is present, even when empty
    /// or made entirely of problem bullets. Parsing never fails: a bullet that
    /// cannot become a clean [`TagAliasDecl`] is recorded as a
    /// [`TagAliasProblem`] instead, so a caller can surface it without a
    /// MANIFEST-wide parse error.
    ///
    /// Each bullet is `old -> canonical`. The alias side is deliberately never
    /// linted, since recording a non-canonical old name like `multi_word` is
    /// the whole point. The canonical side is: a target that is not
    /// lowercase-with-hyphens is a [`TagAliasProblemKind::NonCanonicalTarget`]
    /// problem but the decl is still kept, because the derived index folds the
    /// target anyway. A mapping whose canonical is itself another mapping's
    /// alias is a [`TagAliasProblemKind::ChainedAlias`]; it too is kept, and
    /// resolution deliberately stays a single hop.
    pub fn tag_aliases(&self) -> Option<TagAliasSection> {
        let bullets = self.sections.get(TAG_ALIASES)?;
        let mut decls: Vec<TagAliasDecl> = Vec::new();
        let mut decl_bullets: Vec<String> = Vec::new();
        let mut problems: Vec<TagAliasProblem> = Vec::new();

        for bullet in bullets {
            match parse_tag_alias_bullet(bullet) {
                Ok(decl) => {
                    if let Some(existing) = decls
                        .iter()
                        .find(|d| d.alias.to_lowercase() == decl.alias.to_lowercase())
                    {
                        if existing.canonical.to_lowercase() == decl.canonical.to_lowercase() {
                            // Exact duplicate pair: silently dropped.
                        } else {
                            problems.push(TagAliasProblem {
                                kind: TagAliasProblemKind::DuplicateAlias,
                                bullet: bullet.clone(),
                                reason: format!(
                                    "`{}` is already aliased to `{}`, the first mapping wins",
                                    decl.alias, existing.canonical
                                ),
                            });
                        }
                        continue;
                    }
                    if !is_lower_hyphen(&decl.canonical) {
                        problems.push(TagAliasProblem {
                            kind: TagAliasProblemKind::NonCanonicalTarget,
                            bullet: bullet.clone(),
                            reason: format!(
                                "canonical `{}` is not a lowercase-with-hyphens tag",
                                decl.canonical
                            ),
                        });
                        // Decl kept: the derived index folds the target anyway.
                    }
                    decl_bullets.push(bullet.clone());
                    decls.push(decl);
                }
                Err((kind, reason)) => problems.push(TagAliasProblem {
                    kind,
                    bullet: bullet.clone(),
                    reason,
                }),
            }
        }

        // A mapping whose canonical is itself some mapping's alias is a chain.
        // The decl is kept but flagged, and resolution stays a single hop.
        for (decl, bullet) in decls.iter().zip(&decl_bullets) {
            let canonical_f = decl.canonical.to_lowercase();
            if decls.iter().any(|d| d.alias.to_lowercase() == canonical_f) {
                problems.push(TagAliasProblem {
                    kind: TagAliasProblemKind::ChainedAlias,
                    bullet: bullet.clone(),
                    reason: format!(
                        "canonical `{}` is itself an alias, resolution stays a single hop",
                        decl.canonical
                    ),
                });
            }
        }

        Some(TagAliasSection { decls, problems })
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

/// One `old -> canonical` mapping parsed from a `Tag Aliases` bullet. Both
/// sides are the trimmed verbatim text: the alias keeps its original casing
/// and any non-canonical spelling on purpose, and the canonical is kept as
/// written even when it is not lowercase-with-hyphens.
#[derive(Debug, Clone, PartialEq)]
pub struct TagAliasDecl {
    /// The old tag name, recorded verbatim. Never linted: preserving a
    /// non-canonical spelling like `multi_word` is the point of the map.
    pub alias: String,
    /// The canonical tag the alias folds into, recorded verbatim.
    pub canonical: String,
}

/// Why a `Tag Aliases` bullet was flagged. A decl is still kept for
/// [`TagAliasProblemKind::NonCanonicalTarget`] and
/// [`TagAliasProblemKind::ChainedAlias`]; the other kinds contribute no decl.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagAliasProblemKind {
    /// No `old -> canonical` shape: a missing `->` arrow or an empty side.
    Malformed,
    /// The alias and its canonical fold to the same tag, so there is nothing
    /// to merge.
    SelfAlias,
    /// A second mapping for an alias that was already mapped; the first wins.
    DuplicateAlias,
    /// The canonical target is not a lowercase-with-hyphens tag. The decl is
    /// kept, since the derived index folds the target anyway.
    NonCanonicalTarget,
    /// The canonical target is itself another mapping's alias. The decl is
    /// kept and resolution stays a single hop.
    ChainedAlias,
}

/// A `Tag Aliases` bullet that was flagged, kept verbatim alongside why. Verify
/// maps every kind to a single rule, so it never matches on the `reason` text.
#[derive(Debug, Clone, PartialEq)]
pub struct TagAliasProblem {
    /// The category of the problem.
    pub kind: TagAliasProblemKind,
    /// The original bullet text.
    pub bullet: String,
    /// Why the bullet was flagged.
    pub reason: String,
}

/// The parsed `Tag Aliases` section: kept mappings in document order, plus any
/// flagged bullets. A [`TagAliasProblemKind::NonCanonicalTarget`] or
/// [`TagAliasProblemKind::ChainedAlias`] bullet appears in both lists: it is a
/// kept decl and a problem at once.
#[derive(Debug, Clone, PartialEq)]
pub struct TagAliasSection {
    /// Mappings that were kept, in document order. Aliases are unique after
    /// case-folding: a later duplicate is dropped or flagged instead.
    pub decls: Vec<TagAliasDecl>,
    /// Bullets that were flagged, in document order with chain problems last.
    pub problems: Vec<TagAliasProblem>,
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

/// Parse one `Tag Aliases` bullet into a mapping, or the
/// [`TagAliasProblemKind`] and reason it was rejected. The bullet arrives
/// already `- `-stripped and trimmed. Only the context-free checks live here:
/// a missing arrow or empty side is [`TagAliasProblemKind::Malformed`] and a
/// fold-equal pair is [`TagAliasProblemKind::SelfAlias`]. Duplicate, chained
/// and non-canonical checks need the whole section, so they stay in
/// [`Manifest::tag_aliases`].
fn parse_tag_alias_bullet(bullet: &str) -> Result<TagAliasDecl, (TagAliasProblemKind, String)> {
    let Some((left, right)) = bullet.split_once("->") else {
        return Err((
            TagAliasProblemKind::Malformed,
            format!("expected an `old -> canonical` shape, no `->` arrow found in `{bullet}`"),
        ));
    };
    let alias = left.trim();
    let canonical = right.trim();
    if alias.is_empty() || canonical.is_empty() {
        return Err((
            TagAliasProblemKind::Malformed,
            format!("expected an `old -> canonical` shape, an empty side in `{bullet}`"),
        ));
    }
    if alias.to_lowercase() == canonical.to_lowercase() {
        return Err((
            TagAliasProblemKind::SelfAlias,
            format!("`{alias}` is aliased to itself, nothing to merge"),
        ));
    }
    Ok(TagAliasDecl {
        alias: alias.to_string(),
        canonical: canonical.to_string(),
    })
}

/// The folded `(alias, canonical)` pairs a MANIFEST source declares, for the
/// derived tag-alias index only. Both sides are lowercased and aliases are
/// deduplicated first-wins. Empty when the source does not parse or carries no
/// `Tag Aliases` section. Kept mappings are included even when their target is
/// non-canonical or chained, since the index folds the target regardless; the
/// bullets that became problems without a decl are skipped.
pub fn tag_alias_pairs(source: &str) -> Vec<(String, String)> {
    let Ok(engram) = parse_engram(source) else {
        return Vec::new();
    };
    let manifest = Manifest::from_engram(&engram, source);
    let Some(section) = manifest.tag_aliases() else {
        return Vec::new();
    };
    let mut pairs: Vec<(String, String)> = Vec::new();
    for decl in &section.decls {
        let alias = decl.alias.to_lowercase();
        if pairs.iter().any(|(a, _)| *a == alias) {
            continue;
        }
        pairs.push((alias, decl.canonical.to_lowercase()));
    }
    pairs
}

/// Append an `old -> canonical` mapping to a MANIFEST's `Tag Aliases` section,
/// byte-preserving: every byte outside the spliced bullet line is kept
/// verbatim, so an unusually formatted MANIFEST survives untouched. Returns the
/// rewritten source, or `None` when the folded pair is already present (nothing
/// to add).
///
/// When the section exists the bullet lands after its last zero-indent bullet
/// (or after the heading and its blank line when the section is empty). When it
/// is absent a fresh `## Tag Aliases` section is appended at end of file. A
/// fenced fake `## Tag Aliases` heading inside a code block is never matched.
pub fn append_tag_alias(source: &str, alias: &str, canonical: &str) -> Option<String> {
    let want = (alias.to_lowercase(), canonical.to_lowercase());
    if tag_alias_pairs(source).into_iter().any(|pair| pair == want) {
        return None;
    }

    let bullet = format!("- {alias} -> {canonical}\n");

    if let Some(offset) = tag_aliases_insert_offset(source) {
        let mut out = String::with_capacity(source.len() + bullet.len() + 1);
        out.push_str(&source[..offset]);
        // The offset is always a line boundary, except at a final line with no
        // trailing newline; guard so the bullet never joins the previous line.
        if offset > 0 && !source[..offset].ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&bullet);
        out.push_str(&source[offset..]);
        Some(out)
    } else {
        let mut out = String::with_capacity(source.len() + bullet.len() + 20);
        out.push_str(source);
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str("\n## Tag Aliases\n\n");
        out.push_str(&bullet);
        Some(out)
    }
}

/// The byte offset into `source` where a new `Tag Aliases` bullet should be
/// spliced, or `None` when the MANIFEST has no such H2. Walks the body with the
/// parser's fence state machine, so a fenced `## Tag Aliases` never counts, and
/// locks onto the first real heading (first-duplicate-wins, like
/// [`Manifest::from_engram`]). The section ends at the next H1 or H2 heading or
/// at end of file; the offset is just past the section's last zero-indent
/// bullet, or just past the heading and one following blank line when empty.
fn tag_aliases_insert_offset(source: &str) -> Option<usize> {
    /// The running insertion point inside the located section, plus whether the
    /// heading's immediately following line is still unseen.
    struct InSection {
        insert_at: usize,
        first_line: bool,
    }

    let (_has_fm, _fm_span, body_start) = locate(source);
    let body = &source[body_start..];

    let mut fence: Option<(char, usize)> = None;
    let mut state: Option<InSection> = None;
    let mut abs = body_start;

    for line in body.split_inclusive('\n') {
        abs += line.len();
        let line_end = abs;
        let content = line.strip_suffix('\n').unwrap_or(line);
        let text = content.trim_end_matches('\r');

        match fence {
            None => {
                if let Some((c, n, _)) = fence_marker(text) {
                    // Opening fence line: never a heading or bullet.
                    fence = Some((c, n));
                    continue;
                }
            }
            Some((fc, fcount)) => {
                // Inside a fence: skip, testing the parser's close rule.
                if let Some((c, n, _)) = fence_marker(text)
                    && c == fc
                    && n >= fcount
                    && text.trim_start()[n..].trim().is_empty()
                {
                    fence = None;
                }
                continue;
            }
        }

        match &mut state {
            None => {
                if let Some((level, htext)) = parse_heading(text)
                    && level == 2
                    && section_key(&htext) == TAG_ALIASES
                {
                    state = Some(InSection {
                        insert_at: line_end,
                        first_line: true,
                    });
                }
            }
            Some(sec) => {
                if let Some((level, _)) = parse_heading(text) {
                    if level <= 2 {
                        // A new top-level section ends this one.
                        return Some(sec.insert_at);
                    }
                    // A deeper heading stays inside the section.
                    sec.first_line = false;
                } else if zero_indent_bullet(text).is_some() {
                    sec.insert_at = line_end;
                    sec.first_line = false;
                } else if sec.first_line && text.trim().is_empty() {
                    // The one blank line right under an empty heading.
                    sec.insert_at = line_end;
                    sec.first_line = false;
                } else {
                    sec.first_line = false;
                }
            }
        }
    }

    state.map(|sec| sec.insert_at)
}
