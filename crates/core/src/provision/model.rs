//! The scan and projection layer: turning a domain's declared artifact
//! folders into a hashed artifact set, and projecting that set per harness
//! into the desired keys a reconcile engine (M5) will diff against a
//! harness's live directory. Everything here is a read-only filesystem
//! walk plus pure data shaping; nothing is written.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use crate::config::{self, DomainEntry};
use crate::harness::HarnessKind;
use crate::manifest::{self, ArtifactType, Manifest};
use crate::parse::parse_engram;
use crate::provision::receipt::sha256_hex;
use crate::provision::translate::{self, OptionalMark, Translated};

/// Whether `component` is safe to use as a single path component when an
/// artifact's location is joined onto a harness config directory, i.e.
/// whether `dir.join(component)` can only ever land inside `dir`.
///
/// A component reaches this check from two directions. Scanning a domain
/// feeds it names that are repo-controlled today (a skill folder, a command
/// file, an MCP config's own `name` field) but were read off a filesystem or
/// out of JSON someone else wrote, so a hostile value is only ever a bad
/// actor away. Reading the provisioning receipt back (`plain_rel_key` in
/// [`crate::provision::receipt`]) feeds it names out of a plain JSON file
/// under the state directory, editable by anything that can write there -
/// fully attacker-controlled. Neither caller may let a rejected value reach
/// a join; the artifact or receipt row is skipped outright instead.
///
/// A component is plain when it is non-empty, contains none of `/`, `\` or
/// `:`, and does not start with `.` (which also rejects the bare `.` and
/// `..` components). This mirrors `is_plain_skill_name` in the cli crate's
/// `install.rs` - same threat model, same rejection shape - but is not
/// unified with it: that guard stays where it is, this one covers the
/// broader provisioning path-component case.
pub fn is_plain_component(component: &str) -> bool {
    !component.is_empty()
        && !component.starts_with('.')
        && !component.contains('/')
        && !component.contains('\\')
        && !component.contains(':')
}

/// One scanned file artifact: which kind it is, its key-shaped relative path
/// within that kind, where it lives on disk and its content hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactFile {
    /// The artifact category this file belongs to.
    pub kind: ArtifactType,
    /// The path within its kind, `/`-separated regardless of platform, for
    /// example `tide-tables/scripts/chart.sh`.
    pub rel: String,
    /// Where the file lives on disk.
    pub source: PathBuf,
    /// Lowercase hex sha256 of the file's bytes.
    pub sha256: String,
    /// Whether the artifact says `metadata: { provision: optional }`. Every
    /// file of a skill carries the value read from its `SKILL.md`; a command
    /// or a markdown agent carries its own. A `.toml` agent carries no mark
    /// of its own (Codex refuses an agent file with a `metadata` table): it
    /// takes the mark of a markdown agent with the same stem, else `false`.
    pub optional: bool,
}

/// One scanned MCP server config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpArtifact {
    /// The server's name: the JSON file's own `name` member when present,
    /// the file stem otherwise.
    pub name: String,
    /// The `server` member, serialized compactly with sorted keys - a stable
    /// form for hashing regardless of the source file's own key order.
    pub server_json: String,
    /// Lowercase hex sha256 of `server_json`'s bytes.
    pub sha256: String,
    /// Whether the JSON object says `"metadata": { "provision": "optional" }`.
    pub optional: bool,
}

/// One domain's scanned artifacts: every file and MCP config its declared
/// provisioning folders contain, in deterministic order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomainArtifacts {
    /// The domain name.
    pub domain: String,
    /// File artifacts, sorted by [`ArtifactType`] declaration order then by
    /// `rel`.
    pub files: Vec<ArtifactFile>,
    /// MCP artifacts, sorted by name.
    pub mcps: Vec<McpArtifact>,
}

// --- artifact keys -----------------------------------------------------------

/// The key a provisioning decision names this file's artifact by:
/// `<kind>/<name>`, where the name is the source name with the dialect
/// extension dropped. A skill is its folder (`skills/tide-tables` for every
/// file of the skill), a command is its `rel` without `.md` with the
/// namespace slash kept (`commands/charts/plot-route`), an agent is its stem
/// whether it is a `.md` or a `.toml` file (`agents/quartermaster`). The key
/// is always the source name, never a per-harness name such as Codex's flat
/// command file.
pub fn artifact_key(file: &ArtifactFile) -> String {
    let artifact = match file.kind {
        ArtifactType::Skills => file
            .rel
            .split_once('/')
            .map_or(file.rel.as_str(), |(dir, _)| dir)
            .to_string(),
        ArtifactType::Commands => file
            .rel
            .strip_suffix(".md")
            .unwrap_or(&file.rel)
            .to_string(),
        ArtifactType::Agents => translate::agent_stem(&file.rel),
        // A scan never puts an MCP into the files list; keep the rel as is.
        ArtifactType::Mcps => file.rel.clone(),
    };
    format!("{}/{}", file.kind.id(), artifact)
}

/// The key a provisioning decision names this MCP server by:
/// `mcps/<name>`, the name as the scan reads it.
pub fn mcp_key(mcp: &McpArtifact) -> String {
    format!("{}/{}", ArtifactType::Mcps.id(), mcp.name)
}

/// Split a provisioning key into its kind and its name, or `None` when the
/// key is malformed.
///
/// The key splits on its first `/` only. The kind must be one of the four
/// ids (`skills`, `commands`, `agents`, `mcps`). The name must not be empty;
/// for skills, agents and mcps it is one plain path component
/// ([`is_plain_component`]), for commands every `/`-separated segment must be
/// plain, so a namespaced command (`commands/charts/plot-route`) is accepted.
///
/// Keys compare exactly: the kind and the name are case-sensitive, so
/// `Skills/x` is malformed and `skills/Tide` names a different artifact than
/// `skills/tide`. The same holds wherever a decision is matched against a key
/// (`Provision::selects`).
pub fn parse_artifact_key(key: &str) -> Option<(ArtifactType, &str)> {
    let (kind, artifact) = key.split_once('/')?;
    let kind = match kind {
        "skills" => ArtifactType::Skills,
        "commands" => ArtifactType::Commands,
        "agents" => ArtifactType::Agents,
        "mcps" => ArtifactType::Mcps,
        _ => return None,
    };
    let well_formed = match kind {
        ArtifactType::Commands => artifact.split('/').all(is_plain_component),
        _ => is_plain_component(artifact),
    };
    well_formed.then_some((kind, artifact))
}

/// Turn a read mark into the artifact's `optional` flag, raising one notice
/// for a value other than `optional`.
fn resolve_mark(key: &str, mark: OptionalMark, notices: &mut Vec<String>) -> bool {
    match mark {
        OptionalMark::Absent => false,
        OptionalMark::Optional => true,
        OptionalMark::Other(value) => {
            notices.push(format!(
                "`{key}` says `metadata.provision: {value}`; only `optional` has a meaning - treating it as not optional"
            ));
            false
        }
    }
}

/// The mark of a markdown source given as raw bytes. Bytes that are not
/// UTF-8 carry no frontmatter that parses, so they read as no mark.
fn markdown_mark(bytes: &[u8]) -> OptionalMark {
    match std::str::from_utf8(bytes) {
        Ok(text) => translate::frontmatter_optional_mark(text),
        Err(_) => OptionalMark::Absent,
    }
}

/// The mark of a skill, read from its `SKILL.md`. An unreadable file reads
/// as no mark.
fn skill_mark(skill_dir: &Path) -> OptionalMark {
    match std::fs::read(skill_dir.join("SKILL.md")) {
        Ok(bytes) => markdown_mark(&bytes),
        Err(_) => OptionalMark::Absent,
    }
}

// --- scan --------------------------------------------------------------------

/// Scan a domain's resolved source roots into a hashed artifact set. Each
/// `(kind, root)` pair in `source_roots` is scanned independently; a root
/// that does not exist on disk contributes nothing and produces no notice
/// (M105 already warns about a missing provisioning folder elsewhere - this
/// scan stays quiet about it). Every problem short of a missing root -
/// a skill folder with no `SKILL.md`, a hostile path component, an MCP
/// config with no `server` object - is reported as a notice and the single
/// offending artifact is skipped; scanning itself never fails.
///
/// Which artifacts are selected is shared with [`list_artifacts`] (the
/// `*_entries` enumerators below), so the two always agree on keys and marks.
pub fn scan_domain(
    domain: &str,
    source_roots: &[(ArtifactType, PathBuf)],
) -> (DomainArtifacts, Vec<String>) {
    let mut files = Vec::new();
    let mut mcps = Vec::new();
    let mut notices = Vec::new();

    for (kind, root) in source_roots {
        match kind {
            ArtifactType::Skills => scan_skills(*kind, root, &mut files, &mut notices),
            ArtifactType::Commands => scan_commands(*kind, root, &mut files, &mut notices),
            ArtifactType::Agents => scan_agents(*kind, root, &mut files, &mut notices),
            ArtifactType::Mcps => scan_mcps(root, &mut mcps, &mut notices),
        }
    }

    files.sort_by(|a, b| {
        kind_order(a.kind)
            .cmp(&kind_order(b.kind))
            .then_with(|| a.rel.cmp(&b.rel))
    });
    mcps.sort_by(|a, b| a.name.cmp(&b.name));
    share_markdown_agent_marks(&mut files);

    (
        DomainArtifacts {
            domain: domain.to_string(),
            files,
            mcps,
        },
        notices,
    )
}

/// Give every `.toml` agent the mark of the markdown agent with the same
/// stem. Both files are one artifact under one key, and a `.toml` agent
/// carries no mark of its own, so where an `.md` and a `.toml` agent share a
/// stem the `.md` file's mark wins; [`list_artifacts`] follows the same rule.
fn share_markdown_agent_marks(files: &mut [ArtifactFile]) {
    let mut markdown_marks: BTreeMap<String, bool> = BTreeMap::new();
    for file in files.iter() {
        if file.kind == ArtifactType::Agents && file.rel.ends_with(".md") {
            markdown_marks
                .entry(translate::agent_stem(&file.rel))
                .or_insert(file.optional);
        }
    }
    for file in files.iter_mut() {
        if file.kind == ArtifactType::Agents
            && file.rel.ends_with(".toml")
            && let Some(optional) = markdown_marks.get(&translate::agent_stem(&file.rel))
        {
            file.optional = *optional;
        }
    }
}

/// [`ArtifactType`]'s declaration order, used to sort a scan's files the
/// same way regardless of the order its source roots were given in.
fn kind_order(kind: ArtifactType) -> u8 {
    match kind {
        ArtifactType::Skills => 0,
        ArtifactType::Commands => 1,
        ArtifactType::Agents => 2,
        ArtifactType::Mcps => 3,
    }
}

/// The skill folders under `root`: each direct, visible, plainly named
/// subdirectory that carries a `SKILL.md`, as `(folder name, path)`. A
/// folder with an unsafe name or no `SKILL.md` is skipped with a notice.
fn skill_entries(root: &Path, notices: &mut Vec<String>) -> Vec<(String, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
    entries.sort_by_key(|e| e.file_name());

    let mut out = Vec::new();
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue; // hidden, skipped silently
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }
        if !is_plain_component(&name) {
            notices.push(format!(
                "domain's `skills` folder has a skill directory named `{name}`, which is not a safe path component - skipping it"
            ));
            continue;
        }
        let skill_dir = entry.path();
        if !skill_dir.join("SKILL.md").is_file() {
            notices.push(format!(
                "domain's `skills` folder has a `{name}` directory with no `SKILL.md` - skipping it"
            ));
            continue;
        }
        out.push((name, skill_dir));
    }
    out
}

/// Each direct subdirectory of `root` that carries a `SKILL.md` is a skill;
/// every file inside it (recursively) becomes one [`ArtifactFile`] keyed
/// `<skill-dir-name>/<path-within-skill>`. The skill's `SKILL.md` is read once
/// for the optional mark, and every file of the skill carries it.
fn scan_skills(
    kind: ArtifactType,
    root: &Path,
    out: &mut Vec<ArtifactFile>,
    notices: &mut Vec<String>,
) {
    for (name, skill_dir) in skill_entries(root, notices) {
        let key = format!("{}/{}", kind.id(), name);
        let optional = resolve_mark(&key, skill_mark(&skill_dir), notices);
        let mut visible = Vec::new();
        walk_visible(&skill_dir, &mut Vec::new(), &mut visible);
        for (rel_in_skill, source) in visible {
            let components: Vec<&str> = rel_in_skill.split('/').collect();
            if let Some(offender) = components.iter().find(|c| !is_plain_component(c)) {
                notices.push(format!(
                    "skill `{name}` has a path component `{offender}` that is not safe - skipping `{rel_in_skill}`"
                ));
                continue;
            }
            let Some(sha256) = hash_file(&source) else {
                continue;
            };
            out.push(ArtifactFile {
                kind,
                rel: format!("{name}/{rel_in_skill}"),
                source,
                sha256,
                optional,
            });
        }
    }
}

/// The command files under `root`: every visible `*.md` file at any depth
/// whose path components are all plain, as `(rel, path)` with `rel`
/// `/`-separated from `root`. A file with an unsafe component is skipped with
/// a notice.
fn command_entries(root: &Path, notices: &mut Vec<String>) -> Vec<(String, PathBuf)> {
    let mut visible = Vec::new();
    walk_visible(root, &mut Vec::new(), &mut visible);
    let mut out = Vec::new();
    for (rel, source) in visible {
        if source.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let components: Vec<&str> = rel.split('/').collect();
        if let Some(offender) = components.iter().find(|c| !is_plain_component(c)) {
            notices.push(format!(
                "domain's `commands` folder has a path component `{offender}` that is not safe - skipping `{rel}`"
            ));
            continue;
        }
        out.push((rel, source));
    }
    out
}

/// Every `*.md` file under `root`, recursively; `rel` is the path from
/// `root`, `/`-separated, so subfolders act as command namespaces. The hash
/// and the optional mark come from one read of the file.
fn scan_commands(
    kind: ArtifactType,
    root: &Path,
    out: &mut Vec<ArtifactFile>,
    notices: &mut Vec<String>,
) {
    for (rel, source) in command_entries(root, notices) {
        let Ok(bytes) = std::fs::read(&source) else {
            continue;
        };
        let file = ArtifactFile {
            kind,
            rel,
            source,
            sha256: sha256_hex(&bytes),
            optional: false,
        };
        let optional = resolve_mark(&artifact_key(&file), markdown_mark(&bytes), notices);
        out.push(ArtifactFile { optional, ..file });
    }
}

/// The agent files directly inside `root`, no recursion: every visible,
/// plainly named `*.md` or `*.toml` regular file, as `(file name, path)`. A
/// symlink is skipped silently rather than followed, the same no-follow
/// stance [`walk_visible`] documents - following one would let a hostile
/// entry stage the bytes of any file it can point at.
fn agent_entries(root: &Path, notices: &mut Vec<String>) -> Vec<(String, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
    entries.sort_by_key(|e| e.file_name());

    let mut out = Vec::new();
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue; // hidden, skipped silently
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        let ext = path.extension().and_then(|e| e.to_str());
        if !file_type.is_file() || !matches!(ext, Some("md") | Some("toml")) {
            continue;
        }
        if !is_plain_component(&name) {
            notices.push(format!(
                "domain's `agents` folder has a file named `{name}`, which is not a safe path component - skipping it"
            ));
            continue;
        }
        out.push((name, path));
    }
    out
}

/// Every `*.md` or `*.toml` file directly inside `root`, no recursion; `rel` is
/// the file name. The extension is the agent's source dialect: a `.md` file is
/// the markdown dialect Claude and GitHub Copilot read natively, a `.toml` file
/// is the Codex dialect; the projection in [`crate::provision::translate`]
/// decides which target gets a passthrough and which a conversion. A markdown
/// agent's optional mark comes from the same read as its hash; a `.toml` agent
/// is never read for a mark (see [`ArtifactFile::optional`]).
fn scan_agents(
    kind: ArtifactType,
    root: &Path,
    out: &mut Vec<ArtifactFile>,
    notices: &mut Vec<String>,
) {
    for (name, path) in agent_entries(root, notices) {
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let file = ArtifactFile {
            kind,
            rel: name,
            source: path,
            sha256: sha256_hex(&bytes),
            optional: false,
        };
        let optional = if file.rel.ends_with(".md") {
            resolve_mark(&artifact_key(&file), markdown_mark(&bytes), notices)
        } else {
            false
        };
        out.push(ArtifactFile { optional, ..file });
    }
}

/// The JSON files directly inside `root`, no recursion: every visible
/// `*.json` regular file, as `(file name, path)`. A symlink is skipped
/// silently rather than followed, the same no-follow stance [`walk_visible`]
/// documents.
fn mcp_entries(root: &Path) -> Vec<(String, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
    entries.sort_by_key(|e| e.file_name());

    let mut out = Vec::new();
    for entry in entries {
        let file_name = entry.file_name().to_string_lossy().into_owned();
        if file_name.starts_with('.') {
            continue; // hidden, skipped silently
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if !file_type.is_file() || path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        out.push((file_name, path));
    }
    out
}

/// One MCP config read and checked: its name, its `server` object and its
/// optional flag, or `None` (with a notice where one is due) when the file
/// is not a usable MCP config.
fn read_mcp(
    file_name: &str,
    path: &Path,
    notices: &mut Vec<String>,
) -> Option<(String, serde_json::Map<String, serde_json::Value>, bool)> {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let text = std::fs::read_to_string(path).ok()?;
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        notices.push(format!(
            "mcp config `{file_name}` is not valid JSON - skipping it"
        ));
        return None;
    };
    let Some(obj) = value.as_object() else {
        notices.push(format!(
            "mcp config `{file_name}` is not a JSON object - skipping it"
        ));
        return None;
    };
    let Some(server) = obj.get("server").and_then(|v| v.as_object()) else {
        notices.push(format!(
            "mcp config `{file_name}` has no `server` object member - skipping it"
        ));
        return None;
    };
    let name = match obj.get("name").and_then(|v| v.as_str()) {
        Some(n) => n.to_string(),
        None => stem,
    };
    if !is_plain_component(&name) {
        notices.push(format!(
            "mcp config `{file_name}` has a name `{name}` that is not a safe path component - skipping it"
        ));
        return None;
    }
    let key = format!("{}/{}", ArtifactType::Mcps.id(), name);
    let optional = resolve_mark(&key, translate::json_optional_mark(obj), notices);
    Some((name, server.clone(), optional))
}

/// Every `*.json` file directly inside `root`, no recursion. Each must parse
/// as a JSON object with an object `server` member; the MCP's name is its
/// own `name` string member, falling back to the file stem.
fn scan_mcps(root: &Path, out: &mut Vec<McpArtifact>, notices: &mut Vec<String>) {
    for (file_name, path) in mcp_entries(root) {
        let Some((name, server, optional)) = read_mcp(&file_name, &path, notices) else {
            continue;
        };
        let server_value = serde_json::Value::Object(server);
        let Ok(server_json) = serde_json::to_string(&server_value) else {
            continue;
        };
        let sha256 = sha256_hex(server_json.as_bytes());
        out.push(McpArtifact {
            name,
            server_json,
            sha256,
            optional,
        });
    }
}

// --- the light listing -------------------------------------------------------

/// One artifact a domain ships, as [`list_artifacts`] reports it: one row per
/// artifact (a skill once, not once per file).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactListing {
    /// The artifact's kind.
    pub kind: ArtifactType,
    /// The provisioning key, `<kind>/<name>` (see [`artifact_key`]).
    pub key: String,
    /// The key without its `<kind>/` prefix.
    pub name: String,
    /// Whether the artifact is marked `metadata: { provision: optional }`.
    pub optional: bool,
}

/// What [`list_artifacts`] found in one domain.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArtifactListingReport {
    /// One row per artifact, sorted by kind ([`ArtifactType`] declaration
    /// order) and then by key.
    pub artifacts: Vec<ArtifactListing>,
    /// The kinds with at least one resolved source root that exists on disk,
    /// in kind order. A kind missing here may still ship artifacts that are
    /// not on this machine yet (a team domain's mirror not pulled, a tools
    /// repository not checked out), so an empty kind outside this list must
    /// not be read as "the domain ships nothing of this kind".
    pub present_kinds: Vec<ArtifactType>,
    /// The same scan notices [`scan_domain`] raises for these roots: skipped
    /// entries and optional marks with an unknown value.
    pub notices: Vec<String>,
}

/// List the artifacts a domain ships without hashing anything: the same
/// selection [`scan_domain`] makes (skill folders with a `SKILL.md`, `*.md`
/// commands at any depth, top-level `*.md` and `*.toml` agents, top-level
/// `*.json` MCP configs with a `server` object), reading per artifact only
/// the one file that can carry the optional mark.
///
/// An `.md` and a `.toml` agent with the same stem give one row, and the
/// `.md` file's mark wins (a `.toml` agent carries no mark). Two artifacts of
/// one kind with the same key (two MCP configs naming the same server) give
/// one row, the first in scan order.
pub fn list_artifacts(domain: &str, entry: &DomainEntry) -> ArtifactListingReport {
    list_artifacts_in(&resolve_source_roots(domain, entry))
}

/// [`list_artifacts`] over already resolved source roots.
fn list_artifacts_in(source_roots: &[(ArtifactType, PathBuf)]) -> ArtifactListingReport {
    let mut notices = Vec::new();
    // Keyed by (kind order, key); the bool says whether a row came from a
    // markdown agent, so a later `.md` agent replaces an earlier `.toml` one.
    let mut rows: BTreeMap<(u8, String), (ArtifactListing, bool)> = BTreeMap::new();
    let mut present: Vec<ArtifactType> = Vec::new();

    let mut add = |kind: ArtifactType, artifact: String, optional: bool, markdown_agent: bool| {
        let key = format!("{}/{}", kind.id(), artifact);
        let row = ArtifactListing {
            kind,
            key: key.clone(),
            name: artifact,
            optional,
        };
        match rows.entry((kind_order(kind), key)) {
            std::collections::btree_map::Entry::Vacant(slot) => {
                slot.insert((row, markdown_agent));
            }
            std::collections::btree_map::Entry::Occupied(mut slot) => {
                if markdown_agent && !slot.get().1 {
                    slot.insert((row, true));
                }
            }
        }
    };

    for (kind, root) in source_roots {
        if root.is_dir() && !present.contains(kind) {
            present.push(*kind);
        }
        match kind {
            ArtifactType::Skills => {
                for (name, skill_dir) in skill_entries(root, &mut notices) {
                    let key = format!("{}/{}", kind.id(), name);
                    let optional = resolve_mark(&key, skill_mark(&skill_dir), &mut notices);
                    add(*kind, name, optional, false);
                }
            }
            ArtifactType::Commands => {
                for (rel, source) in command_entries(root, &mut notices) {
                    // An unreadable file is dropped, as the scan drops it.
                    let Ok(bytes) = std::fs::read(&source) else {
                        continue;
                    };
                    let artifact = rel.strip_suffix(".md").unwrap_or(&rel).to_string();
                    let key = format!("{}/{}", kind.id(), artifact);
                    let optional = resolve_mark(&key, markdown_mark(&bytes), &mut notices);
                    add(*kind, artifact, optional, false);
                }
            }
            ArtifactType::Agents => {
                for (file_name, path) in agent_entries(root, &mut notices) {
                    let artifact = translate::agent_stem(&file_name);
                    if file_name.ends_with(".md") {
                        let Ok(bytes) = std::fs::read(&path) else {
                            continue;
                        };
                        let key = format!("{}/{}", kind.id(), artifact);
                        let optional = resolve_mark(&key, markdown_mark(&bytes), &mut notices);
                        add(*kind, artifact, optional, true);
                    } else {
                        // A `.toml` agent is never read for a mark (Codex
                        // refuses a `metadata` table), only checked readable
                        // as the scan does.
                        if std::fs::File::open(&path).is_err() {
                            continue;
                        }
                        add(*kind, artifact, false, false);
                    }
                }
            }
            ArtifactType::Mcps => {
                for (file_name, path) in mcp_entries(root) {
                    if let Some((name, _, optional)) = read_mcp(&file_name, &path, &mut notices) {
                        add(*kind, name, optional, false);
                    }
                }
            }
        }
    }

    present.sort_by_key(|kind| kind_order(*kind));
    ArtifactListingReport {
        artifacts: rows.into_values().map(|(row, _)| row).collect(),
        present_kinds: present,
        notices,
    }
}

/// Recursively collect every visible (non-hidden) file under `dir`, as
/// `(rel, source)` pairs where `rel` is `/`-joined from the walk's starting
/// point. A path component starting with `.` - a file or a folder - is
/// skipped silently at every depth: this is where routine filesystem and
/// editor junk (`.git`, `.DS_Store`) drops out before it ever reaches an
/// `is_plain_component` check, which is reserved for names that are visible
/// but still unsafe. A symlink is neither a file nor a directory under
/// `FileType`'s own predicates, so it is skipped rather than followed.
fn walk_visible(dir: &Path, rel: &mut Vec<String>, out: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if file_type.is_dir() {
            rel.push(name);
            walk_visible(&path, rel, out);
            rel.pop();
        } else if file_type.is_file() {
            rel.push(name);
            out.push((rel.join("/"), path));
            rel.pop();
        }
    }
}

/// Lowercase hex sha256 of a file's bytes, or `None` when it cannot be read
/// (already gone, permissions) - the artifact is dropped rather than the
/// scan failing outright.
fn hash_file(path: &Path) -> Option<String> {
    std::fs::read(path).ok().map(|bytes| sha256_hex(&bytes))
}

/// Resolve a domain's declared provisioning folders into absolute source
/// roots ready for [`scan_domain`]. A virtual domain has no filesystem root
/// and resolves to nothing; a domain with an unreadable or unparseable
/// `MANIFEST.md` resolves to nothing too, the same "never an error" stance
/// [`crate::manifest::in_root_artifact_dirs`] takes.
///
/// Each decl's path is normalized logically (`.`/`..` processed textually,
/// no filesystem canonicalization, reusing the helper
/// [`crate::manifest::normalize_relative`] shares with `in_root_artifact_dirs`).
/// A domain with no origin resolves every decl - in-root or climbing above
/// the root with `..` - relative to its own root: escaping is a legitimate
/// way for a local domain to point at a folder living beside it. A domain
/// with an origin resolves an in-root decl the same way, but a decl that
/// climbs above the root resolves instead into that origin's state-directory
/// artifact mirror (`<origin_state_dir>/artifacts/<kind>`), the folder a
/// later milestone populates from the remote repository; resolving there
/// when it does not yet exist is fine; [`scan_domain`] treats a missing root
/// as empty. A decl that normalizes onto the root itself (`foo/..`) is
/// skipped for every domain, the same treatment `in_root_artifact_dirs`
/// gives it: the root is the domain's knowledge, never an artifact folder,
/// and scanning it would sweep the whole domain.
pub fn resolve_source_roots(domain: &str, entry: &DomainEntry) -> Vec<(ArtifactType, PathBuf)> {
    if entry.is_virtual() {
        return Vec::new();
    }
    let Some(root) = entry.file_path() else {
        return Vec::new();
    };
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

    let mut roots = Vec::new();
    for decl in &section.decls {
        let (kept, climbs) = manifest::normalize_relative(&decl.path);
        if climbs == 0 && kept.is_empty() {
            // Root-landing (`foo/..`): never a source root, see above.
            continue;
        }
        let resolved = if climbs > 0 && entry.origin.is_some() {
            match config::origin_state_dir(domain) {
                Ok(dir) => dir.join("artifacts").join(decl.kind.id()),
                Err(_) => continue,
            }
        } else {
            logical_join(&root, &decl.path)
        };
        roots.push((decl.kind, resolved));
    }
    roots
}

/// Join `path` onto `root`, processing its `/`-separated `.` and `..`
/// components against `root` itself rather than against the filesystem: `.`
/// and empty components are dropped, a plain component is pushed, and `..`
/// pops the previously pushed component - or, once there is nothing left of
/// `path` to pop, one more level of `root` itself, exactly the local-domain
/// "may escape the root" case this function exists for.
fn logical_join(root: &Path, path: &str) -> PathBuf {
    let mut result = root.to_path_buf();
    for component in path.split('/') {
        match component {
            "" | "." => continue,
            ".." => {
                result.pop();
            }
            other => result.push(other),
        }
    }
    result
}

/// The bytes a desired file resolves to when a reconcile finally writes it.
///
/// A [`DesiredPayload::File`] is a byte-identical passthrough: the target
/// harness speaks the same dialect the artifact was authored in, so the source
/// file is copied verbatim and the desired hash is the source hash. A
/// [`DesiredPayload::Rendered`] carries the exact bytes a cross-dialect
/// translation produced (a markdown agent emitted as Codex TOML, a Codex prompt
/// nested for Claude), hashed over those bytes rather than the source's - so
/// hand-edit detection and `.bak` semantics stay relative to what was actually
/// installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DesiredPayload {
    /// Copy this source file's bytes unchanged (same-dialect passthrough).
    File(PathBuf),
    /// Write these already-translated bytes (cross-dialect target).
    Rendered(Vec<u8>),
}

/// One key in a [`DesiredSet`]: `"<kind.id()>/<rel>"`, harness-agnostic - a
/// reconcile engine maps it to a real path via [`crate::harness::HarnessPaths`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesiredFile {
    /// The domain that won this key.
    pub domain: String,
    /// The bytes to write: a passthrough source path or rendered bytes.
    pub payload: DesiredPayload,
    /// Lowercase hex sha256 of the bytes actually written.
    pub sha256: String,
}

impl DesiredFile {
    /// The passthrough source path when this file is a byte-identical copy of a
    /// source file, or `None` for a cross-dialect rendered target that has no
    /// single source path on disk.
    pub fn source_path(&self) -> Option<&Path> {
        match &self.payload {
            DesiredPayload::File(path) => Some(path),
            DesiredPayload::Rendered(_) => None,
        }
    }
}

/// One desired MCP server, keyed by its own name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesiredMcp {
    /// The domain that won this name.
    pub domain: String,
    /// The `server` member, serialized compactly with sorted keys.
    pub server_json: String,
    /// Lowercase hex sha256 of `server_json`'s bytes.
    pub sha256: String,
}

/// The artifacts one harness should end up with, once every domain's scan is
/// projected through the harness's support matrix and cross-domain
/// collisions are resolved.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DesiredSet {
    /// File key to its winning artifact.
    pub files: BTreeMap<String, DesiredFile>,
    /// MCP name to its winning artifact.
    pub mcps: BTreeMap<String, DesiredMcp>,
}

/// Whether `harness` provisions artifacts of `kind` at all. The full matrix:
/// artifacts are authored in a source dialect and translated to every harness
/// whose format can carry them, so Claude Code and Codex take all four kinds
/// (a command flattens for Codex, an agent converts to or from TOML, an MCP
/// server registers through the harness CLI) and GitHub Copilot takes every
/// kind but commands - the Copilot CLI declined a prompt-file surface, so a
/// command has nowhere to land there and is skipped with a notice. How each
/// supported pair actually maps (passthrough versus a cross-dialect render)
/// lives in `crate::provision::translate`; this gate answers only the coarse
/// "at all" question the MCP path and the skip notices need.
///
/// The four profile harnesses (Cursor, Kiro, Gemini CLI, Qwen Code) take
/// skills only: they have no command or agent surface this crate speaks and
/// register MCP servers by hand, so every other pair is `false`.
pub fn harness_supports(harness: HarnessKind, kind: ArtifactType) -> bool {
    match harness {
        HarnessKind::ClaudeCode | HarnessKind::Codex => true,
        HarnessKind::Copilot => kind != ArtifactType::Commands,
        HarnessKind::Cursor | HarnessKind::Kiro | HarnessKind::Gemini | HarnessKind::Qwen => {
            kind == ArtifactType::Skills
        }
    }
}

/// Project every domain's scanned artifacts into one harness's desired set,
/// translating each artifact into the harness's own dialect along the way.
///
/// `all` must be in registry config order - the order domains are declared
/// in `config.yaml` - since that order is this function's collision-breaking
/// contract: the first domain to claim a key wins it outright, and every
/// later domain claiming the same key (or, for an MCP, the same name) only
/// produces a notice naming both domains and the key. Two of one domain's own
/// artifacts can now collide too - a nested command and a flat command that
/// flatten to the same Codex key - and the same first-wins rule settles it,
/// with a notice naming both source files. A harness that provisions a kind at
/// all but cannot carry one artifact (a malformed cross-dialect source, an
/// agent too thin to render) skips that one artifact with its own notice; a
/// kind a harness has no surface for at all (a command for GitHub Copilot)
/// produces one notice per `(domain, kind)` pair, however many it ships.
pub fn desired_set(harness: HarnessKind, all: &[DomainArtifacts]) -> (DesiredSet, Vec<String>) {
    let mut files: BTreeMap<String, DesiredFile> = BTreeMap::new();
    let mut mcps: BTreeMap<String, DesiredMcp> = BTreeMap::new();
    let mut notices = Vec::new();
    let mut unsupported_noted: HashSet<(String, &'static str)> = HashSet::new();
    // Which domain and source `rel` won each target key, so a collision can name
    // both the winning and the losing source, whatever dialect renamed them.
    let mut winners: BTreeMap<String, (String, String)> = BTreeMap::new();

    for domain_artifacts in all {
        for file in &domain_artifacts.files {
            match translate::translate_file(harness, file) {
                Translated::Unsupported => {
                    if unsupported_noted.insert((domain_artifacts.domain.clone(), file.kind.id())) {
                        notices.push(unsupported_kind_notice(
                            harness,
                            &domain_artifacts.domain,
                            file.kind,
                        ));
                    }
                }
                Translated::Skip { notice } => notices.push(notice),
                Translated::Emit {
                    rel,
                    payload,
                    sha256,
                    notices: field_notices,
                } => {
                    let key = format!("{}/{}", file.kind.id(), rel);
                    match winners.get(&key) {
                        Some((win_domain, win_rel)) => {
                            notices.push(if *win_domain == domain_artifacts.domain {
                                same_domain_collision_notice(
                                    &domain_artifacts.domain,
                                    win_rel,
                                    &file.rel,
                                    &key,
                                )
                            } else {
                                cross_domain_collision_notice(
                                    win_domain,
                                    &domain_artifacts.domain,
                                    &key,
                                )
                            });
                        }
                        None => {
                            files.insert(
                                key.clone(),
                                DesiredFile {
                                    domain: domain_artifacts.domain.clone(),
                                    payload,
                                    sha256,
                                },
                            );
                            winners
                                .insert(key, (domain_artifacts.domain.clone(), file.rel.clone()));
                            notices.extend(field_notices);
                        }
                    }
                }
            }
        }

        // The servers project unconditionally, whatever [`harness_supports`]
        // says: it is false for mcps on the four profile harnesses (skills
        // only), and the engine reports the registration as unsupported there
        // instead of dropping the server from the desired set.
        for mcp in &domain_artifacts.mcps {
            match mcps.get(&mcp.name) {
                Some(existing) => {
                    notices.push(format!(
                        "domain `{}` and domain `{}` both provision the mcp `{}` - domain `{}` wins",
                        existing.domain, domain_artifacts.domain, mcp.name, existing.domain
                    ));
                }
                None => {
                    mcps.insert(
                        mcp.name.clone(),
                        DesiredMcp {
                            domain: domain_artifacts.domain.clone(),
                            server_json: mcp.server_json.clone(),
                            sha256: mcp.sha256.clone(),
                        },
                    );
                }
            }
        }
    }

    (DesiredSet { files, mcps }, notices)
}

/// The notice for a kind a harness has no surface for at all - a command for
/// GitHub Copilot, whose CLI declined a prompt-file surface in favor of skills.
fn unsupported_kind_notice(harness: HarnessKind, domain: &str, kind: ArtifactType) -> String {
    format!(
        "domain `{domain}` ships {k}, but {h} provides no {k} surface - skipping them. Ship them as skills to reach {h} users.",
        k = kind.id(),
        h = harness.display_name()
    )
}

/// The notice for two different domains claiming the same target key: the
/// earlier domain in config order keeps it.
fn cross_domain_collision_notice(winner: &str, loser: &str, key: &str) -> String {
    format!(
        "domain `{winner}` and domain `{loser}` both provision `{key}` - domain `{winner}` wins"
    )
}

/// The notice for one domain's own two artifacts colliding on a target key - a
/// nested command and a flat command that flatten to the same Codex key. The
/// first source in scan order keeps the key; both source paths are named.
fn same_domain_collision_notice(
    domain: &str,
    winner_rel: &str,
    loser_rel: &str,
    key: &str,
) -> String {
    format!(
        "domain `{domain}` maps both `{winner_rel}` and `{loser_rel}` to `{key}` - keeping `{winner_rel}`"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- is_plain_component --------------------------------------------------

    #[test]
    fn plain_component_accepts_ordinary_names() {
        assert!(is_plain_component("tide-tables"));
        assert!(is_plain_component("SKILL.md"));
        assert!(is_plain_component("chart.sh"));
    }

    #[test]
    fn plain_component_rejects_empty_dot_and_dotdot() {
        assert!(!is_plain_component(""));
        assert!(!is_plain_component("."));
        assert!(!is_plain_component(".."));
    }

    #[test]
    fn plain_component_rejects_separators_and_colon() {
        assert!(!is_plain_component("a/b"));
        assert!(!is_plain_component("a\\b"));
        assert!(!is_plain_component("a:b"));
    }

    #[test]
    fn plain_component_rejects_leading_dot() {
        assert!(!is_plain_component(".hidden"));
        assert!(!is_plain_component(".DS_Store"));
    }

    // --- harness_supports / desired_set --------------------------------------

    fn file(kind: ArtifactType, rel: &str, domain: &str) -> ArtifactFile {
        ArtifactFile {
            kind,
            rel: rel.to_string(),
            source: PathBuf::from(format!("/{domain}/{rel}")),
            sha256: sha256_hex(rel.as_bytes()),
            optional: false,
        }
    }

    fn mcp(name: &str, domain: &str) -> McpArtifact {
        McpArtifact {
            name: name.to_string(),
            server_json: format!("{{\"domain\":\"{domain}\"}}"),
            sha256: sha256_hex(domain.as_bytes()),
            optional: false,
        }
    }

    #[test]
    fn claude_code_supports_all_four_kinds() {
        for kind in [
            ArtifactType::Skills,
            ArtifactType::Commands,
            ArtifactType::Agents,
            ArtifactType::Mcps,
        ] {
            assert!(harness_supports(HarnessKind::ClaudeCode, kind));
        }
    }

    #[test]
    fn codex_supports_all_kinds_and_copilot_all_but_commands() {
        for kind in [
            ArtifactType::Skills,
            ArtifactType::Commands,
            ArtifactType::Agents,
            ArtifactType::Mcps,
        ] {
            assert!(harness_supports(HarnessKind::Codex, kind));
        }
        // GitHub Copilot has no command surface, but takes every other kind.
        assert!(harness_supports(HarnessKind::Copilot, ArtifactType::Skills));
        assert!(harness_supports(HarnessKind::Copilot, ArtifactType::Agents));
        assert!(harness_supports(HarnessKind::Copilot, ArtifactType::Mcps));
        assert!(!harness_supports(
            HarnessKind::Copilot,
            ArtifactType::Commands
        ));
    }

    /// The four profile harnesses provision skills and nothing else.
    #[test]
    fn the_profile_harnesses_support_skills_only() {
        for harness in [
            HarnessKind::Cursor,
            HarnessKind::Kiro,
            HarnessKind::Gemini,
            HarnessKind::Qwen,
        ] {
            for kind in [
                ArtifactType::Skills,
                ArtifactType::Commands,
                ArtifactType::Agents,
                ArtifactType::Mcps,
            ] {
                assert_eq!(
                    harness_supports(harness, kind),
                    kind == ArtifactType::Skills,
                    "{harness:?} {kind:?}"
                );
            }
        }
    }

    /// Only skills reach a profile harness's desired set, so a stray command
    /// or agent row in the receipt cannot make provisioning write anything.
    #[test]
    fn a_profile_harness_desired_set_holds_skills_only() {
        let harbor = DomainArtifacts {
            domain: "harbor".to_string(),
            files: vec![
                file(ArtifactType::Skills, "tide-tables/SKILL.md", "harbor"),
                file(ArtifactType::Commands, "charts/plot-route.md", "harbor"),
                file(ArtifactType::Agents, "quartermaster.md", "harbor"),
            ],
            mcps: Vec::new(),
        };
        for harness in [
            HarnessKind::Cursor,
            HarnessKind::Kiro,
            HarnessKind::Gemini,
            HarnessKind::Qwen,
        ] {
            let (desired, _) = desired_set(harness, std::slice::from_ref(&harbor));
            let keys: Vec<&str> = desired.files.keys().map(String::as_str).collect();
            assert_eq!(keys, ["skills/tide-tables/SKILL.md"], "{harness:?}");
        }
    }

    #[test]
    fn claude_code_desired_set_gets_all_four_kinds_and_mcps() {
        let harbor = DomainArtifacts {
            domain: "harbor".to_string(),
            files: vec![
                file(ArtifactType::Skills, "tide-tables/SKILL.md", "harbor"),
                file(ArtifactType::Commands, "charts/plot-route.md", "harbor"),
                file(ArtifactType::Agents, "quartermaster.md", "harbor"),
            ],
            mcps: vec![mcp("lighthouse", "harbor")],
        };
        let (desired, notices) =
            desired_set(HarnessKind::ClaudeCode, std::slice::from_ref(&harbor));
        assert!(notices.is_empty());
        assert_eq!(desired.files.len(), 3);
        assert!(desired.files.contains_key("skills/tide-tables/SKILL.md"));
        assert!(desired.files.contains_key("commands/charts/plot-route.md"));
        assert!(desired.files.contains_key("agents/quartermaster.md"));
        assert_eq!(desired.mcps.len(), 1);
        assert!(desired.mcps.contains_key("lighthouse"));
    }

    #[test]
    fn copilot_takes_skills_agents_and_mcps_and_skips_commands_with_one_notice() {
        let harbor = DomainArtifacts {
            domain: "harbor".to_string(),
            files: vec![
                file(ArtifactType::Skills, "tide-tables/SKILL.md", "harbor"),
                file(ArtifactType::Commands, "charts/plot-route.md", "harbor"),
                file(ArtifactType::Commands, "charts/plot-return.md", "harbor"),
                file(ArtifactType::Agents, "quartermaster.md", "harbor"),
            ],
            mcps: vec![mcp("lighthouse", "harbor")],
        };
        let (desired, notices) = desired_set(HarnessKind::Copilot, std::slice::from_ref(&harbor));
        // Skills pass through, the markdown agent passes through natively, and
        // the MCP registers - only the two commands have no Copilot surface.
        assert_eq!(desired.files.len(), 2);
        assert!(desired.files.contains_key("skills/tide-tables/SKILL.md"));
        assert!(desired.files.contains_key("agents/quartermaster.md"));
        assert_eq!(desired.mcps.len(), 1);
        // One notice for commands despite two command files - not one per file.
        assert_eq!(notices.len(), 1);
        assert!(
            notices
                .iter()
                .any(|n| n.contains("commands") && n.contains("GitHub Copilot CLI"))
        );
    }

    #[test]
    fn cross_domain_collision_first_domain_wins_with_a_notice() {
        let first = DomainArtifacts {
            domain: "harbor".to_string(),
            files: vec![file(ArtifactType::Skills, "tide-tables/SKILL.md", "harbor")],
            mcps: vec![mcp("lighthouse", "harbor")],
        };
        let second = DomainArtifacts {
            domain: "cove".to_string(),
            files: vec![file(ArtifactType::Skills, "tide-tables/SKILL.md", "cove")],
            mcps: vec![mcp("lighthouse", "cove")],
        };
        let (desired, notices) = desired_set(HarnessKind::ClaudeCode, &[first, second]);
        assert_eq!(desired.files.len(), 1);
        assert_eq!(
            desired.files["skills/tide-tables/SKILL.md"].domain,
            "harbor"
        );
        assert_eq!(desired.mcps.len(), 1);
        assert_eq!(desired.mcps["lighthouse"].domain, "harbor");
        assert_eq!(notices.len(), 2);
        assert!(notices.iter().any(|n| n.contains("harbor")
            && n.contains("cove")
            && n.contains("skills/tide-tables/SKILL.md")));
        assert!(
            notices
                .iter()
                .any(|n| n.contains("harbor") && n.contains("cove") && n.contains("lighthouse"))
        );
    }
}
