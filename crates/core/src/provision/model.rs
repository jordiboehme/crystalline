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

/// Scan a domain's resolved source roots into a hashed artifact set. Each
/// `(kind, root)` pair in `source_roots` is scanned independently; a root
/// that does not exist on disk contributes nothing and produces no notice
/// (M105 already warns about a missing provisioning folder elsewhere - this
/// scan stays quiet about it). Every problem short of a missing root -
/// a skill folder with no `SKILL.md`, a hostile path component, an MCP
/// config with no `server` object - is reported as a notice and the single
/// offending artifact is skipped; scanning itself never fails.
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

    (
        DomainArtifacts {
            domain: domain.to_string(),
            files,
            mcps,
        },
        notices,
    )
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

/// Each direct subdirectory of `root` that carries a `SKILL.md` is a skill;
/// every file inside it (recursively) becomes one [`ArtifactFile`] keyed
/// `<skill-dir-name>/<path-within-skill>`.
fn scan_skills(
    kind: ArtifactType,
    root: &Path,
    out: &mut Vec<ArtifactFile>,
    notices: &mut Vec<String>,
) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
    entries.sort_by_key(|e| e.file_name());

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
            });
        }
    }
}

/// Every `*.md` file under `root`, recursively; `rel` is the path from
/// `root`, `/`-separated, so subfolders act as command namespaces.
fn scan_commands(
    kind: ArtifactType,
    root: &Path,
    out: &mut Vec<ArtifactFile>,
    notices: &mut Vec<String>,
) {
    let mut visible = Vec::new();
    walk_visible(root, &mut Vec::new(), &mut visible);
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
        let Some(sha256) = hash_file(&source) else {
            continue;
        };
        out.push(ArtifactFile {
            kind,
            rel,
            source,
            sha256,
        });
    }
}

/// Every `*.md` file directly inside `root`, no recursion; `rel` is the file
/// name. A symlink is skipped silently rather than followed, the same
/// no-follow stance [`walk_visible`] documents - following one would let a
/// hostile entry stage the bytes of any file it can point at.
fn scan_agents(
    kind: ArtifactType,
    root: &Path,
    out: &mut Vec<ArtifactFile>,
    notices: &mut Vec<String>,
) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue; // hidden, skipped silently
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if !file_type.is_file() || path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        if !is_plain_component(&name) {
            notices.push(format!(
                "domain's `agents` folder has a file named `{name}`, which is not a safe path component - skipping it"
            ));
            continue;
        }
        let Some(sha256) = hash_file(&path) else {
            continue;
        };
        out.push(ArtifactFile {
            kind,
            rel: name,
            source: path,
            sha256,
        });
    }
}

/// Every `*.json` file directly inside `root`, no recursion. Each must parse
/// as a JSON object with an object `server` member; the MCP's name is its
/// own `name` string member, falling back to the file stem. A symlink is
/// skipped silently rather than followed, the same no-follow stance
/// [`walk_visible`] documents.
fn scan_mcps(root: &Path, out: &mut Vec<McpArtifact>, notices: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
    entries.sort_by_key(|e| e.file_name());

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
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();

        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
            notices.push(format!(
                "mcp config `{file_name}` is not valid JSON - skipping it"
            ));
            continue;
        };
        let Some(obj) = value.as_object() else {
            notices.push(format!(
                "mcp config `{file_name}` is not a JSON object - skipping it"
            ));
            continue;
        };
        let Some(server) = obj.get("server").and_then(|v| v.as_object()) else {
            notices.push(format!(
                "mcp config `{file_name}` has no `server` object member - skipping it"
            ));
            continue;
        };
        let name = match obj.get("name").and_then(|v| v.as_str()) {
            Some(n) => n.to_string(),
            None => stem,
        };
        if !is_plain_component(&name) {
            notices.push(format!(
                "mcp config `{file_name}` has a name `{name}` that is not a safe path component - skipping it"
            ));
            continue;
        }
        let server_value = serde_json::Value::Object(server.clone());
        let Ok(server_json) = serde_json::to_string(&server_value) else {
            continue;
        };
        let sha256 = sha256_hex(server_json.as_bytes());
        out.push(McpArtifact {
            name,
            server_json,
            sha256,
        });
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

/// One key in a [`DesiredSet`]: `"<kind.id()>/<rel>"`, harness-agnostic - a
/// reconcile engine maps it to a real path via [`crate::harness::HarnessPaths`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesiredFile {
    /// The domain that won this key.
    pub domain: String,
    /// Where the file lives on disk.
    pub source: PathBuf,
    /// Lowercase hex sha256 of the file's bytes.
    pub sha256: String,
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

/// Whether `harness` provisions artifacts of `kind` at all. This milestone's
/// support matrix: Claude Code takes all four kinds; Codex and Copilot take
/// skills only. M11 extends this as those harnesses gain command, agent and
/// MCP support of their own.
pub fn harness_supports(harness: HarnessKind, kind: ArtifactType) -> bool {
    matches!(
        (harness, kind),
        (HarnessKind::ClaudeCode, _)
            | (HarnessKind::Codex, ArtifactType::Skills)
            | (HarnessKind::Copilot, ArtifactType::Skills)
    )
}

/// Project every domain's scanned artifacts into one harness's desired set.
///
/// `all` must be in registry config order - the order domains are declared
/// in `config.yaml` - since that order is this function's collision-breaking
/// contract: the first domain to claim a key wins it outright, and every
/// later domain claiming the same key (or, for an MCP, the same name) only
/// produces a notice naming both domains and the key. A domain shipping an
/// artifact kind this harness does not support yet produces one notice per
/// `(domain, kind)` pair naming the harness, however many artifacts of that
/// kind the domain ships.
pub fn desired_set(harness: HarnessKind, all: &[DomainArtifacts]) -> (DesiredSet, Vec<String>) {
    let mut files: BTreeMap<String, DesiredFile> = BTreeMap::new();
    let mut mcps: BTreeMap<String, DesiredMcp> = BTreeMap::new();
    let mut notices = Vec::new();
    let mut unsupported_noted: HashSet<(String, &'static str)> = HashSet::new();

    for domain_artifacts in all {
        for file in &domain_artifacts.files {
            if !harness_supports(harness, file.kind) {
                if unsupported_noted.insert((domain_artifacts.domain.clone(), file.kind.id())) {
                    notices.push(format!(
                        "domain `{}` ships {}, but {} does not provision them yet - skipping",
                        domain_artifacts.domain,
                        file.kind.id(),
                        harness.display_name()
                    ));
                }
                continue;
            }
            let key = format!("{}/{}", file.kind.id(), file.rel);
            match files.get(&key) {
                Some(existing) => {
                    notices.push(format!(
                        "domain `{}` and domain `{}` both provision `{key}` - domain `{}` wins",
                        existing.domain, domain_artifacts.domain, existing.domain
                    ));
                }
                None => {
                    files.insert(
                        key,
                        DesiredFile {
                            domain: domain_artifacts.domain.clone(),
                            source: file.source.clone(),
                            sha256: file.sha256.clone(),
                        },
                    );
                }
            }
        }

        if domain_artifacts.mcps.is_empty() {
            continue;
        }
        if !harness_supports(harness, ArtifactType::Mcps) {
            if unsupported_noted.insert((domain_artifacts.domain.clone(), ArtifactType::Mcps.id()))
            {
                notices.push(format!(
                    "domain `{}` ships mcps, but {} does not provision them yet - skipping",
                    domain_artifacts.domain,
                    harness.display_name()
                ));
            }
            continue;
        }
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
        }
    }

    fn mcp(name: &str, domain: &str) -> McpArtifact {
        McpArtifact {
            name: name.to_string(),
            server_json: format!("{{\"domain\":\"{domain}\"}}"),
            sha256: sha256_hex(domain.as_bytes()),
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
    fn codex_and_copilot_support_only_skills() {
        for harness in [HarnessKind::Codex, HarnessKind::Copilot] {
            assert!(harness_supports(harness, ArtifactType::Skills));
            assert!(!harness_supports(harness, ArtifactType::Commands));
            assert!(!harness_supports(harness, ArtifactType::Agents));
            assert!(!harness_supports(harness, ArtifactType::Mcps));
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
    fn copilot_gets_skills_only_with_one_notice_per_unsupported_kind() {
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
        assert_eq!(desired.files.len(), 1);
        assert!(desired.files.contains_key("skills/tide-tables/SKILL.md"));
        assert!(desired.mcps.is_empty());
        // One notice for commands (despite two command files), one for
        // agents, one for mcps - not one per artifact.
        assert_eq!(notices.len(), 3);
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
