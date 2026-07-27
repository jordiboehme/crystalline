//! Routing prompt generation: `crystalline prompt system`.
//!
//! Static and dependency-free like [`crate::verify`]: given the global
//! config (registered domains plus `prompt.rules` path-glob filters) and a
//! workspace path, this reads each included domain's `MANIFEST.md` and
//! renders a compact routing block bound to the crystalline MCP server and
//! its exact tool names. Per the Purpose section of the project's house
//! rules, the block is framed as session onboarding: an agent is being
//! introduced to knowledge it already has, not handed a file listing.
//!
//! A domain whose `MANIFEST.md` is missing or unreadable never aborts the
//! run - it gets a placeholder routing line and a warning the caller can
//! print to stderr.
//!
//! Determinism contract: `generate_prompt`, `generate_prompt_unscoped` and
//! the three renderers (`render_text`, `render_json`, `render_instructions`)
//! are pure functions of their inputs (the config file and the MANIFESTs on
//! disk). No timestamps, process IDs, environment variables or other
//! environment-dependent values ever enter the output, and no unordered
//! iteration (a hash map, a hash set) drives ordering. Domain order comes
//! from the config's own registered order: `generate_prompt` filters it
//! through a sorted inclusion set and reorders preferred-first, while
//! `generate_prompt_unscoped` emits that registered order verbatim.
//! `render_json` relies on `serde_json`'s stable field order. Identical
//! config plus identical on-disk MANIFESTs must render byte-identical output
//! every time, whether rendered twice in one process or by two separate
//! invocations of the binary, so a harness can cache the rendered prompt
//! across sessions. This contract binds every prompt kind added after
//! `system`, not only the one implemented today.
//!
//! Latency contract: prompt commands run in session-start hooks for AI
//! agents and must stay fast - the target is under 50ms wall-clock for 30
//! registered domains in a release build. The budget is the invariant, not
//! the mechanism used to hit it. The `system` kind stays inside it by
//! reading only the global config and each domain's `MANIFEST.md`, nothing
//! else. A future prompt kind may need information that only lives in the
//! database; when it does, a cold database open in the hook path is what
//! to avoid. Prefer, in order: serving the needed state from a running
//! daemon's always-current in-memory state over one fast socket round
//! trip, or precomputing or caching what the new kind needs so generation
//! stays inside the budget. Design every new prompt kind against the
//! latency budget, not against a hard ban on any particular data source.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use globset::Glob;
use serde::Serialize;

use crate::config::{GlobalConfig, RepoConfig, expand_tilde};
use crate::manifest::Manifest;
use crate::parse::parse_engram;

/// One domain's routing entry.
#[derive(Debug, Clone, Serialize)]
pub struct PromptDomain {
    /// The domain name.
    pub name: String,
    /// Routing bullets: `## When to Use`, falling back to `## Scope`, or a
    /// placeholder when the MANIFEST could not be read.
    pub bullets: Vec<String>,
    /// Whether this domain was named in the workspace's repo-local
    /// `preferred_domains`.
    pub preferred: bool,
}

/// The result of building a routing prompt for a workspace.
#[derive(Debug, Clone)]
pub struct PromptOutput {
    /// The workspace path the prompt was built for.
    pub workspace: PathBuf,
    /// Domains in routing order: preferred domains first, then the rest in
    /// registry order.
    pub domains: Vec<PromptDomain>,
    /// Non-fatal problems (an unreadable or unparseable MANIFEST) meant for
    /// stderr, never a crash.
    pub warnings: Vec<String>,
    /// Whether the deployment serves the content API read-only. In the
    /// read-only variant the behavior block drops the write-tools line and
    /// states the knowledge is curated externally. Seeded from
    /// `service.read_only`; a `--read-only` flag can force it on.
    pub read_only: bool,
}

/// Build the routing prompt for a workspace: apply `prompt.rules`
/// include/exclude glob filters, then a repo-local `preferred_domains`
/// reorder, then read each included domain's MANIFEST.
///
/// A file domain's routing bullets come from its `MANIFEST.md` on disk, kept
/// fast and dependency-free. A virtual domain has no disk root, so the caller
/// (which may reach a running daemon or the store) supplies its bullets in
/// `virtual_bullets`, keyed by domain name; this crate never touches a database.
/// The determinism contract holds: identical config plus identical MANIFEST
/// content (disk and supplied) render byte-identical output.
pub fn generate_prompt(
    global: &GlobalConfig,
    workspace: &Path,
    virtual_bullets: &BTreeMap<String, Vec<String>>,
) -> PromptOutput {
    let included = included_domain_names(global, workspace);
    let repo_cfg = load_repo_config(workspace);
    let preferred: Vec<String> = repo_cfg.map(|c| c.preferred_domains).unwrap_or_default();

    let mut ordered: Vec<&str> = Vec::new();
    for name in &preferred {
        if included.contains(name)
            && global.domains.contains_key(name)
            && !ordered.contains(&name.as_str())
        {
            ordered.push(name.as_str());
        }
    }
    for name in global.domains.keys() {
        if included.contains(name) && !ordered.contains(&name.as_str()) {
            ordered.push(name.as_str());
        }
    }

    let mut domains = Vec::with_capacity(ordered.len());
    let mut warnings = Vec::new();
    for name in ordered {
        let entry = &global.domains[name];
        let is_preferred = preferred.iter().any(|p| p == name);
        let (bullets, warning) = if entry.is_virtual() {
            virtual_routing_bullets(name, virtual_bullets.get(name))
        } else {
            match &entry.path {
                Some(path) => load_routing_bullets(name, path),
                None => virtual_routing_bullets(name, virtual_bullets.get(name)),
            }
        };
        if let Some(w) = warning {
            warnings.push(w);
        }
        domains.push(PromptDomain {
            name: name.to_string(),
            bullets,
            preferred: is_preferred,
        });
    }

    PromptOutput {
        workspace: workspace.to_path_buf(),
        domains,
        warnings,
        read_only: global.read_only(),
    }
}

/// Build the routing prompt with no workspace context: every registered
/// domain in config order, unfiltered. This is the shape the MCP
/// `instructions` channel wants, where a server serves one index to every
/// connecting agent and there is no workspace path to scope by. So
/// `prompt.rules` path-glob filters never apply (there is nothing to match a
/// glob against) and no domain is ever marked `preferred` (a repo-local
/// `preferred_domains` reorder needs a workspace too); `workspace` is left
/// empty and `read_only` is seeded from `service.read_only`.
///
/// Bullet sourcing is identical to [`generate_prompt`]: a file domain reads
/// its `MANIFEST.md` from disk, a virtual domain takes the caller-supplied
/// `virtual_bullets` keyed by domain name, and a missing or empty set gets the
/// same placeholder line and warning. The determinism contract holds.
pub fn generate_prompt_unscoped(
    global: &GlobalConfig,
    virtual_bullets: &BTreeMap<String, Vec<String>>,
) -> PromptOutput {
    let mut domains = Vec::with_capacity(global.domains.len());
    let mut warnings = Vec::new();
    for (name, entry) in &global.domains {
        let (bullets, warning) = if entry.is_virtual() {
            virtual_routing_bullets(name, virtual_bullets.get(name))
        } else {
            match &entry.path {
                Some(path) => load_routing_bullets(name, path),
                None => virtual_routing_bullets(name, virtual_bullets.get(name)),
            }
        };
        if let Some(w) = warning {
            warnings.push(w);
        }
        domains.push(PromptDomain {
            name: name.clone(),
            bullets,
            preferred: false,
        });
    }

    PromptOutput {
        workspace: PathBuf::new(),
        domains,
        warnings,
        read_only: global.read_only(),
    }
}

/// Routing bullets for a virtual domain, supplied by the caller from the
/// database. An absent or empty set gets a placeholder line and a warning,
/// mirroring the missing-MANIFEST path for file domains.
fn virtual_routing_bullets(
    name: &str,
    supplied: Option<&Vec<String>>,
) -> (Vec<String>, Option<String>) {
    match supplied {
        Some(bullets) if !bullets.is_empty() => (bullets.clone(), None),
        _ => (
            vec![placeholder(
                "add `## When to Use` or `## Scope` bullets to the virtual MANIFEST",
            )],
            Some(format!(
                "domain `{name}`: virtual MANIFEST has no Scope or When to Use bullets"
            )),
        ),
    }
}

fn included_domain_names(global: &GlobalConfig, workspace: &Path) -> BTreeSet<String> {
    let all: BTreeSet<String> = global.domains.keys().cloned().collect();
    let Some(prompt_cfg) = &global.prompt else {
        return all;
    };
    if prompt_cfg.rules.is_empty() {
        return all;
    }

    let mut included: BTreeSet<String> = BTreeSet::new();
    let mut excluded: BTreeSet<String> = BTreeSet::new();
    let mut any_matched = false;

    for (glob_key, rule) in &prompt_cfg.rules {
        let expanded = expand_tilde(glob_key);
        let Ok(glob) = Glob::new(&expanded.to_string_lossy()) else {
            continue;
        };
        let matcher = glob.compile_matcher();
        if !matcher.is_match(workspace) {
            continue;
        }
        any_matched = true;
        if let Some(inc) = &rule.include {
            included.extend(inc.iter().filter(|d| all.contains(*d)).cloned());
        }
        if let Some(exc) = &rule.exclude {
            excluded.extend(exc.iter().cloned());
        }
    }

    if !any_matched {
        return all;
    }
    let base = if included.is_empty() { all } else { included };
    base.difference(&excluded).cloned().collect()
}

fn load_repo_config(workspace: &Path) -> Option<RepoConfig> {
    let path = workspace.join(".crystalline.yaml");
    crate::config::load_yaml(&path).ok()
}

fn load_routing_bullets(name: &str, path: &Path) -> (Vec<String>, Option<String>) {
    let resolved = expand_tilde(&path.to_string_lossy());
    let manifest_path = resolved.join("MANIFEST.md");
    match std::fs::read_to_string(&manifest_path) {
        Ok(source) => match parse_engram(&source) {
            Ok(engram) => {
                let manifest = Manifest::from_engram(&engram, &source);
                let bullets = manifest.routing_bullets().to_vec();
                if bullets.is_empty() {
                    (
                        vec![placeholder(
                            "add `## When to Use` or `## Scope` bullets to MANIFEST.md",
                        )],
                        Some(format!(
                            "domain `{name}`: MANIFEST.md at {} has no Scope or When to Use bullets",
                            manifest_path.display()
                        )),
                    )
                } else {
                    (bullets, None)
                }
            }
            Err(e) => (
                vec![placeholder("MANIFEST.md could not be parsed")],
                Some(format!(
                    "domain `{name}`: MANIFEST.md at {} could not be parsed: {e}",
                    manifest_path.display()
                )),
            ),
        },
        Err(e) => (
            vec![placeholder("MANIFEST.md is missing or unreadable")],
            Some(format!(
                "domain `{name}`: MANIFEST.md is missing or unreadable at {}: {e}",
                manifest_path.display()
            )),
        ),
    }
}

fn placeholder(reason: &str) -> String {
    format!("(routing information unavailable: {reason})")
}

/// Render the "CRYSTALLINE KNOWLEDGE ROUTING" onboarding block: an intro
/// binding the block to the crystalline MCP server, one routing line per
/// domain joining its bullets with `; `, then the behavior rules naming the
/// exact tool an agent should reach for (`search_engrams`, `write_engram`
/// and the rest) so it never has to guess which server or tool a rule
/// refers to.
pub fn render_text(output: &PromptOutput) -> String {
    let mut out = String::new();
    out.push_str("CRYSTALLINE KNOWLEDGE ROUTING\n\n");
    out.push_str(
        "You are being onboarded to the knowledge accumulated in this workspace. It is served by the crystalline MCP server; these instructions govern its tools (your harness may prefix tool names, for example mcp__crystalline__search_engrams). The domains below are registered and ready to use; route searches and reads through them instead of starting from zero.\n\n",
    );
    render_routing_body(
        output,
        usize::MAX,
        "(no domains are registered for this workspace)",
        &mut out,
    );
    out
}

/// Render the shared body of an onboarding block: one routing line per domain
/// (bullets joined with `; `, capped at `bullet_cap`), then the fixed Behavior
/// rules naming the exact tools an agent reaches for. `empty_line` is the
/// message emitted in place of the domain lines when no domain is registered,
/// the one place the workspace-scoped `render_text` and the workspace-free
/// `render_instructions` word things differently.
///
/// This is the CLI `prompt system` order (domains, then Behavior); the MCP
/// `instructions` channel puts the Behavior block first, see
/// [`render_instructions`]. Both share the same two halves so the rules can
/// never drift between them.
fn render_routing_body(
    output: &PromptOutput,
    bullet_cap: usize,
    empty_line: &str,
    out: &mut String,
) {
    render_domain_lines(output, bullet_cap, empty_line, out);
    out.push('\n');
    render_behavior_block(output, out);
}

/// One routing line per domain, bullets joined with `; ` and capped at
/// `bullet_cap`; `empty_line` stands in when no domain is registered. A
/// `bullet_cap` of [`usize::MAX`] shows every bullet (the CLI `prompt system`
/// path), a small cap trims the routing lines for the token-lean MCP
/// `instructions` channel. The caller adds any separating blank line.
fn render_domain_lines(
    output: &PromptOutput,
    bullet_cap: usize,
    empty_line: &str,
    out: &mut String,
) {
    if output.domains.is_empty() {
        out.push_str(empty_line);
        out.push('\n');
        return;
    }
    for d in &output.domains {
        let label = if d.preferred {
            format!("{} (preferred)", d.name)
        } else {
            d.name.clone()
        };
        let bullets = d
            .bullets
            .iter()
            .take(bullet_cap)
            .cloned()
            .collect::<Vec<_>>()
            .join("; ");
        let _ = writeln!(out, "- {label}: {bullets}");
    }
}

/// The fixed `Behavior:` block, identical across both renderers: it drops the
/// write-tools line in read-only mode and always points at
/// `list_domains include_routing=true` as the mid-session re-fetch, so neither
/// caller has to restate either rule.
fn render_behavior_block(output: &PromptOutput, out: &mut String) {
    out.push_str("Behavior:\n");
    for bullet in behavior_bullets(output.read_only) {
        let _ = writeln!(out, "- {bullet}");
    }
}

/// The behavior rules an agent follows when working with this server's tools,
/// one string per rule, without the leading `- ` and without a trailing
/// newline. The read-only set drops the four content-mutating tools and the
/// capture language; the read-write set names them.
///
/// This is the single source of the rule set: [`render_routing_body`] renders
/// it as the Behavior block of both onboarding renderers, and the MCP
/// `list_domains` response returns it verbatim for remote clients that never
/// see the initialize instructions. Both paths therefore teach exactly the
/// same rules and the set can never drift between them.
pub fn behavior_bullets(read_only: bool) -> Vec<String> {
    let mut bullets = vec![
        "Narrow question, one domain fits: search_engrams with domains=[that domain].".to_string(),
        "Broad or unclear question: search_engrams with no domains sweeps them all.".to_string(),
    ];
    if read_only {
        // Read-only variant: no write-tools line and no capture language; the
        // knowledge is maintained outside this deployment.
        bullets.push(
            "This deployment's knowledge is read-only and curated externally; search and read to learn, never try to capture or change anything."
                .to_string(),
        );
    } else {
        bullets.push(
            "write_engram, edit_engram, move_engram and delete_engram always need an explicit domain."
                .to_string(),
        );
        bullets.push(
            "Pass write_engram a folder matching the domain's layout: a topic prefix build_context can glob, a subfolder once a topic clusters, the root for singletons."
                .to_string(),
        );
        bullets.push(
            "Check vocabulary for tags and categories in use before inventing one.".to_string(),
        );
        bullets.push(
            "Salience (0-10) lifts an engram in search: set it at write time on exceptionally valuable knowledge and raise it when one proves key to a task."
                .to_string(),
        );
    }
    bullets.push(
        "build_context on a crystalline:// anchor gathers knowledge around a task.".to_string(),
    );
    bullets.push(
        "Read a MANIFEST with read_engram only when the domain's routing line is not enough; list_domains with include_routing=true re-fetches the index mid-session."
            .to_string(),
    );
    bullets
}

/// The size a rendered `instructions` block must stay inside, in bytes.
///
/// Claude Code's tool-search mode (on by default once a session's MCP tool
/// descriptions grow past roughly a tenth of the context window) loads only
/// tool names plus each server's initialize `instructions` at session start
/// and truncates those instructions at about 2KB. Anything past the cut never
/// reaches the model. 1900 leaves headroom under that 2048-byte cut for a
/// client that measures slightly differently or appends a note of its own (the
/// service does exactly that for the TOON response format), so the header, the
/// intro and the whole Behavior block always survive.
pub const INSTRUCTIONS_BUDGET: usize = 1900;

/// Render the onboarding block for the MCP `instructions` channel: the same
/// "CRYSTALLINE KNOWLEDGE ROUTING" header and shared Behavior rules as
/// [`render_text`], but with a workspace-free intro, the Behavior block hoisted
/// above the domain lines and each domain's routing line capped at three
/// bullets to stay token-lean on every connect.
///
/// This is the block a server hands the model at initialize, so the intro
/// frames the tools as this server's own (the harness may prefix their names)
/// rather than assuming a workspace context: there is no workspace over MCP, so
/// `prompt.rules` and `preferred_domains` never shaped this output. Both intros
/// are compressed so that the first 512 bytes of the rendered block stand on
/// their own: a client that truncates the instructions still reads what
/// Crystalline is, that searching these domains comes before answering from
/// memory and that `list_domains` with `include_routing=true` re-fetches the
/// whole index and its behavior rules at any time. The read-write intro adds
/// the write and refine language and the search-before-you-write rule; the
/// read-only intro states the knowledge is curated and drops both. The Behavior
/// block then handles read-only tool dropping and repeats the re-fetch rule.
///
/// Order and degradation both serve [`INSTRUCTIONS_BUDGET`]. Header, intro and
/// Behavior block are fixed-size and universally applicable, so they come
/// first and are never degraded; the domain lines are the variable part and
/// every one of them is re-fetchable with a single `list_domains
/// include_routing=true` call, so a client that truncates the tail only ever
/// loses re-fetchable content. When the full render would still exceed the
/// budget the domain lines degrade in two deterministic steps: first every
/// routing line drops to one bullet, then the whole list collapses to a single
/// count line pointing at the same re-fetch call. Degradation is a pure
/// function of the rendered sizes, so the determinism contract holds and the
/// cost is at most three renders of the same small strings.
pub fn render_instructions(output: &PromptOutput) -> String {
    let mut head = String::new();
    head.push_str("CRYSTALLINE KNOWLEDGE ROUTING\n\n");
    if output.read_only {
        head.push_str(
            "Crystalline is your durable memory across sessions: the domains below hold curated knowledge as engrams you search and read (your harness may prefix tool names, for example mcp__crystalline__search_engrams). Search them before answering from memory; list_domains with include_routing=true returns this index and its behavior rules at any time.\n\n",
        );
    } else {
        head.push_str(
            "Crystalline is your durable memory across sessions: the domains below hold knowledge as engrams you read, write and refine (your harness may prefix tool names, for example mcp__crystalline__search_engrams). Search them before answering from memory and before writing; list_domains with include_routing=true returns this index and its behavior rules at any time.\n\n",
        );
    }
    render_behavior_block(output, &mut head);
    head.push('\n');
    head.push_str("Domains:\n");

    // Tier 1: every routing line, up to three bullets each.
    let mut full = head.clone();
    render_domain_lines(output, 3, "(no domains are registered yet)", &mut full);
    if full.len() <= INSTRUCTIONS_BUDGET {
        return full;
    }

    // Tier 2: one bullet per domain, still one line per domain.
    let mut lean = head.clone();
    render_domain_lines(output, 1, "(no domains are registered yet)", &mut lean);
    if lean.len() <= INSTRUCTIONS_BUDGET {
        return lean;
    }

    // Tier 3: a single count line; the index itself is one tool call away.
    let mut counted = head;
    let n = output.domains.len();
    let noun = if n == 1 { "domain" } else { "domains" };
    let _ = writeln!(
        counted,
        "{n} {noun} registered; list_domains with include_routing=true returns the full index and its behavior rules."
    );
    counted
}

/// The paste-able custom-instructions snippet for remote clients whose harness
/// runs no session hooks and never shows the model a server's initialize
/// instructions: a standing instruction that points the agent at the one tool
/// call which onboards it.
///
/// It is deliberately static and content-free - it names no domain, no count
/// and no setting - so it can never go stale in a place Crystalline cannot
/// update. All dynamic routing comes from the `list_domains` call it points at.
pub const CONNECTOR_SNIPPET: &str = "This environment includes the Crystalline knowledge server over MCP. At the start of every session call its list_domains tool with include_routing set to true; the result is your onboarding: one routing line per domain plus the behavior rules for this server's tools. Follow it, search those domains before answering from memory and re-fetch it mid-session with the same call whenever you need it again.";

#[derive(Serialize)]
struct JsonPromptDomain<'a> {
    name: &'a str,
    bullets: &'a [String],
    preferred: bool,
}

#[derive(Serialize)]
struct JsonPrompt<'a> {
    version: u32,
    kind: &'static str,
    read_only: bool,
    workspace: String,
    domains: Vec<JsonPromptDomain<'a>>,
}

/// Render the prompt as JSON: `{version: 1, kind: "system", read_only,
/// workspace, domains: [{name, bullets, preferred}]}`. `read_only` is always
/// present; `kind` is additive and `version` stays 1.
pub fn render_json(output: &PromptOutput) -> String {
    let wrapped = JsonPrompt {
        version: 1,
        kind: "system",
        read_only: output.read_only,
        workspace: output.workspace.display().to_string(),
        domains: output
            .domains
            .iter()
            .map(|d| JsonPromptDomain {
                name: &d.name,
                bullets: &d.bullets,
                preferred: d.preferred,
            })
            .collect(),
    };
    serde_json::to_string_pretty(&wrapped).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{DomainEntry, GlobalConfig};

    fn manifest_source(when_to_use: &[&str]) -> String {
        let bullets: String = when_to_use.iter().map(|b| format!("- {b}\n")).collect();
        format!(
            "---\ntype: manifest\ntitle: MANIFEST\npermalink: manifest\ntags:\n- manifest\nstatus: current\nrecorded_at: 2026-01-01\ntimestamp: 2026-01-01T00:00:00+00:00\n---\n\n# Test Domain\n\n## Scope\n\n- test scope\n\n## When to Use\n\n{bullets}"
        )
    }

    /// Two domains on disk: `alpha` is preferred and has a `MANIFEST.md`,
    /// `beta` has none at all, exercising the placeholder-routing-line and
    /// warning path in the same fixture used to prove determinism.
    fn fixture() -> (tempfile::TempDir, GlobalConfig) {
        let tmp = tempfile::tempdir().unwrap();
        let alpha = tmp.path().join("alpha");
        let beta = tmp.path().join("beta");
        std::fs::create_dir_all(&alpha).unwrap();
        std::fs::create_dir_all(&beta).unwrap();
        std::fs::write(
            alpha.join("MANIFEST.md"),
            manifest_source(&[
                "When asked about alpha things",
                "When comparing alpha to beta",
            ]),
        )
        .unwrap();
        std::fs::write(
            tmp.path().join(".crystalline.yaml"),
            "preferred_domains:\n- alpha\n",
        )
        .unwrap();

        let mut domains = indexmap::IndexMap::new();
        domains.insert("alpha".to_string(), DomainEntry::file(alpha));
        domains.insert("beta".to_string(), DomainEntry::file(beta));

        let global = GlobalConfig {
            domains,
            ..GlobalConfig::default()
        };
        (tmp, global)
    }

    // Determinism contract, part (a): generate + render twice from the same
    // on-disk inputs must produce byte-identical output, for both formats.

    #[test]
    fn generate_and_render_text_is_byte_identical_across_repeated_runs() {
        let (tmp, global) = fixture();
        let first = render_text(&generate_prompt(&global, tmp.path(), &BTreeMap::new()));
        let second = render_text(&generate_prompt(&global, tmp.path(), &BTreeMap::new()));
        assert_eq!(first.as_bytes(), second.as_bytes());
    }

    #[test]
    fn generate_and_render_json_is_byte_identical_across_repeated_runs() {
        let (tmp, global) = fixture();
        let first = render_json(&generate_prompt(&global, tmp.path(), &BTreeMap::new()));
        let second = render_json(&generate_prompt(&global, tmp.path(), &BTreeMap::new()));
        assert_eq!(first.as_bytes(), second.as_bytes());
    }

    #[test]
    fn render_text_names_the_mcp_server_and_exact_tool_names() {
        let (tmp, global) = fixture();
        let text = render_text(&generate_prompt(&global, tmp.path(), &BTreeMap::new()));
        assert!(text.contains("crystalline MCP server"));
        for tool in [
            "search_engrams",
            "write_engram",
            "edit_engram",
            "move_engram",
            "delete_engram",
            "build_context",
            "read_engram",
            "list_domains",
        ] {
            assert!(
                text.contains(tool),
                "expected the rendered text to mention {tool}:\n{text}"
            );
        }
    }

    #[test]
    fn render_json_reports_kind_system() {
        let (tmp, global) = fixture();
        let json = render_json(&generate_prompt(&global, tmp.path(), &BTreeMap::new()));
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["version"], 1);
        assert_eq!(value["kind"], "system");
    }

    /// Generate a prompt and force the read-only variant on, the way the
    /// `--read-only` flag does at the CLI boundary.
    fn read_only_output(tmp: &tempfile::TempDir, global: &GlobalConfig) -> PromptOutput {
        let mut output = generate_prompt(global, tmp.path(), &BTreeMap::new());
        output.read_only = true;
        output
    }

    #[test]
    fn read_write_default_names_the_mutating_tools() {
        let (tmp, global) = fixture();
        let output = generate_prompt(&global, tmp.path(), &BTreeMap::new());
        assert!(!output.read_only, "default mode is read-write");
        let text = render_text(&output);
        assert!(text.contains("write_engram, edit_engram, move_engram and delete_engram"));
    }

    #[test]
    fn read_only_text_drops_the_write_line_and_names_no_mutating_tools() {
        let (tmp, global) = fixture();
        let text = render_text(&read_only_output(&tmp, &global));
        // The read-only line is present.
        assert!(
            text.contains("read-only and curated externally"),
            "read-only line expected:\n{text}"
        );
        // None of the four content-mutating tool names appear anywhere.
        for tool in [
            "write_engram",
            "edit_engram",
            "move_engram",
            "delete_engram",
        ] {
            assert!(
                !text.contains(tool),
                "{tool} must not appear in the read-only prompt:\n{text}"
            );
        }
        // The read tools an agent still needs are named.
        assert!(text.contains("search_engrams"));
        assert!(text.contains("build_context"));
    }

    #[test]
    fn render_json_carries_the_read_only_flag_in_both_modes() {
        let (tmp, global) = fixture();

        let rw = render_json(&generate_prompt(&global, tmp.path(), &BTreeMap::new()));
        let rw_value: serde_json::Value = serde_json::from_str(&rw).unwrap();
        assert_eq!(rw_value["read_only"], serde_json::json!(false));

        let ro = render_json(&read_only_output(&tmp, &global));
        let ro_value: serde_json::Value = serde_json::from_str(&ro).unwrap();
        assert_eq!(ro_value["read_only"], serde_json::json!(true));
    }

    #[test]
    fn determinism_holds_in_read_only_mode() {
        let (tmp, global) = fixture();
        let first_text = render_text(&read_only_output(&tmp, &global));
        let second_text = render_text(&read_only_output(&tmp, &global));
        assert_eq!(first_text.as_bytes(), second_text.as_bytes());

        let first_json = render_json(&read_only_output(&tmp, &global));
        let second_json = render_json(&read_only_output(&tmp, &global));
        assert_eq!(first_json.as_bytes(), second_json.as_bytes());
    }

    // --- unscoped prompt + instructions renderer -----------------------------

    /// A config whose `prompt.rules` would exclude `beta` for any workspace,
    /// proving the unscoped generator ignores those filters entirely.
    fn fixture_with_prompt_rules() -> (tempfile::TempDir, GlobalConfig) {
        let (tmp, mut global) = fixture();
        let mut rules = indexmap::IndexMap::new();
        rules.insert(
            "**".to_string(),
            crate::config::PromptRule {
                include: Some(vec!["alpha".to_string()]),
                exclude: None,
            },
        );
        global.prompt = Some(crate::config::PromptConfig { rules });
        (tmp, global)
    }

    #[test]
    fn unscoped_ignores_prompt_rules_and_marks_nothing_preferred() {
        let (_tmp, global) = fixture_with_prompt_rules();
        let output = generate_prompt_unscoped(&global, &BTreeMap::new());
        // Every registered domain is present in config order, `beta` included
        // despite the include-only rule that would drop it in the scoped path.
        let names: Vec<&str> = output.domains.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "beta"]);
        // No workspace means no preferred reorder: nothing is ever preferred.
        assert!(output.domains.iter().all(|d| !d.preferred));
    }

    #[test]
    fn render_instructions_caps_bullets_at_three() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("many");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("MANIFEST.md"),
            manifest_source(&["one", "two", "three", "four", "five"]),
        )
        .unwrap();
        let mut domains = indexmap::IndexMap::new();
        domains.insert("many".to_string(), DomainEntry::file(root));
        let global = GlobalConfig {
            domains,
            ..GlobalConfig::default()
        };

        let output = generate_prompt_unscoped(&global, &BTreeMap::new());
        // The domain really has five bullets; the CLI text shows them all.
        assert_eq!(output.domains[0].bullets.len(), 5);
        let text = render_instructions(&output);
        let line = text
            .lines()
            .find(|l| l.starts_with("- many:"))
            .expect("a routing line for the many domain");
        assert!(line.contains("one; two; three"), "first three kept: {line}");
        assert!(
            !line.contains("four") && !line.contains("five"),
            "bullets past three dropped: {line}"
        );
    }

    #[test]
    fn render_instructions_read_only_names_no_mutating_tool() {
        let (_tmp, global) = fixture();
        let mut output = generate_prompt_unscoped(&global, &BTreeMap::new());
        output.read_only = true;
        let text = render_instructions(&output);
        assert!(
            text.contains("read-only and curated externally"),
            "read-only behavior line expected:\n{text}"
        );
        for tool in [
            "write_engram",
            "edit_engram",
            "move_engram",
            "delete_engram",
        ] {
            assert!(
                !text.contains(tool),
                "{tool} must not appear read-only:\n{text}"
            );
        }
        // The read tools an agent still needs are named.
        assert!(text.contains("search_engrams"));
        assert!(text.contains("build_context"));
    }

    /// A remote client may truncate the instructions it shows the model, so
    /// the head of the block has to stand on its own: the fallback that
    /// re-fetches the whole index must be inside the first 512 bytes in both
    /// modes, even with no domain registered at all.
    #[test]
    fn render_instructions_first_512_bytes_carry_the_fallback_call() {
        let global = GlobalConfig::default();
        let mut output = generate_prompt_unscoped(&global, &BTreeMap::new());
        assert!(output.domains.is_empty());
        for read_only in [false, true] {
            output.read_only = read_only;
            let text = render_instructions(&output);
            let head = std::str::from_utf8(&text.as_bytes()[..512.min(text.len())]).unwrap();
            assert!(
                head.contains("list_domains"),
                "read_only={read_only}: list_domains expected in the first 512 bytes:\n{head}"
            );
            assert!(
                head.contains("include_routing=true"),
                "read_only={read_only}: include_routing=true expected in the first 512 bytes:\n{head}"
            );
        }
    }

    /// The instructions order is header, intro, Behavior block, domain lines,
    /// so a client that truncates the tail only ever loses the re-fetchable
    /// domain list.
    #[test]
    fn render_instructions_puts_the_behavior_block_above_the_domain_lines() {
        let (_tmp, global) = fixture();
        let text = render_instructions(&generate_prompt_unscoped(&global, &BTreeMap::new()));
        let behavior = text.find("Behavior:").expect("a behavior block");
        let domains = text.find("Domains:").expect("a domains section");
        let alpha = text.find("- alpha:").expect("a routing line for alpha");
        assert!(
            behavior < domains && domains < alpha,
            "expected Behavior before the domain lines:\n{text}"
        );
        // The CLI renderer keeps its own domains-first order.
        let cli = render_text(&generate_prompt(&global, _tmp.path(), &BTreeMap::new()));
        assert!(
            cli.find("- alpha").unwrap() < cli.find("Behavior:").unwrap(),
            "render_text keeps domains first:\n{cli}"
        );
    }

    /// Scaffold `count` file domains, each with `bullets_per_domain` routing
    /// bullets, the way the CLI latency test scaffolds a large registry.
    fn many_domains(count: usize, bullets_per_domain: usize) -> (tempfile::TempDir, GlobalConfig) {
        let tmp = tempfile::tempdir().unwrap();
        let mut domains = indexmap::IndexMap::new();
        for i in 0..count {
            let name = format!("domain{i:02}");
            let dir = tmp.path().join(&name);
            std::fs::create_dir_all(&dir).unwrap();
            let bullets: Vec<String> = (0..bullets_per_domain)
                .map(|b| format!("When a task touches {name} aspect {b}"))
                .collect();
            let refs: Vec<&str> = bullets.iter().map(|s| s.as_str()).collect();
            std::fs::write(dir.join("MANIFEST.md"), manifest_source(&refs)).unwrap();
            domains.insert(name, DomainEntry::file(dir));
        }
        let global = GlobalConfig {
            domains,
            ..GlobalConfig::default()
        };
        (tmp, global)
    }

    /// The budget contract: Claude Code's tool-search mode truncates a server's
    /// initialize instructions at roughly 2KB, so the whole rendered block has
    /// to fit under 2048 bytes however many domains are registered, in both
    /// modes.
    #[test]
    fn render_instructions_stays_under_2kb_for_a_large_registry() {
        for count in [0usize, 1, 2, 5, 30, 200] {
            let (_tmp, global) = many_domains(count, 5);
            let mut output = generate_prompt_unscoped(&global, &BTreeMap::new());
            for read_only in [false, true] {
                output.read_only = read_only;
                let text = render_instructions(&output);
                assert!(
                    text.len() <= 2048,
                    "count={count} read_only={read_only}: {} bytes rendered:\n{text}",
                    text.len()
                );
                assert!(
                    text.len() <= INSTRUCTIONS_BUDGET,
                    "count={count} read_only={read_only}: {} bytes exceeds the budget",
                    text.len()
                );
                // The fixed head survives at every size.
                assert!(text.starts_with("CRYSTALLINE KNOWLEDGE ROUTING"));
                assert!(text.contains("Behavior:"));
                assert!(text.contains("include_routing=true"));
            }
        }
    }

    /// Degradation happens in two deterministic steps: full routing lines, then
    /// one bullet per line, then a single count line. Each tier is reached by
    /// growing the registry and every tier renders identically twice.
    #[test]
    fn render_instructions_degrades_deterministically_in_three_tiers() {
        // Tier 1: a small registry keeps three bullets per routing line.
        let (_t1, small) = many_domains(1, 5);
        let tier1 = render_instructions(&generate_prompt_unscoped(&small, &BTreeMap::new()));
        assert!(
            tier1.contains("aspect 0; When a task touches domain00 aspect 1; When a task touches domain00 aspect 2"),
            "three bullets kept:\n{tier1}"
        );

        // Tier 2: enough domains to blow the budget at three bullets each, but
        // not at one - one routing line per domain survives, trimmed.
        let (_t2, mid) = many_domains(12, 5);
        let tier2 = render_instructions(&generate_prompt_unscoped(&mid, &BTreeMap::new()));
        assert!(
            tier2.contains("- domain11: When a task touches domain11 aspect 0\n"),
            "one bullet per domain:\n{tier2}"
        );
        assert!(
            !tier2.contains("aspect 1"),
            "second bullet dropped:\n{tier2}"
        );
        assert!(
            !tier2.contains("domains registered;"),
            "not yet the count line:\n{tier2}"
        );

        // Tier 3: a registry too large for even one bullet per line collapses to
        // the count line, which points back at the same re-fetch call.
        let (_t3, big) = many_domains(200, 5);
        let tier3 = render_instructions(&generate_prompt_unscoped(&big, &BTreeMap::new()));
        assert!(
            tier3.contains(
                "200 domains registered; list_domains with include_routing=true returns the full index and its behavior rules."
            ),
            "count line:\n{tier3}"
        );
        assert!(!tier3.contains("- domain00:"), "no routing lines:\n{tier3}");

        // Every tier is a pure function of its input.
        for (global, rendered) in [(&small, &tier1), (&mid, &tier2), (&big, &tier3)] {
            let again = render_instructions(&generate_prompt_unscoped(global, &BTreeMap::new()));
            assert_eq!(again.as_bytes(), rendered.as_bytes());
        }
    }

    /// The count line reads as a sentence for a single domain too.
    #[test]
    fn the_count_line_is_singular_for_one_domain() {
        let (_tmp, global) = many_domains(1, 5);
        let mut output = generate_prompt_unscoped(&global, &BTreeMap::new());
        // Force the tier-3 shape by giving the one domain a bullet no budget
        // could hold.
        output.domains[0].bullets = vec!["x".repeat(INSTRUCTIONS_BUDGET)];
        let text = render_instructions(&output);
        assert!(
            text.contains("1 domain registered; list_domains with include_routing=true"),
            "singular count line:\n{text}"
        );
    }

    #[test]
    fn render_instructions_is_byte_identical_across_repeated_runs() {
        let (_tmp, global) = fixture();
        let first = render_instructions(&generate_prompt_unscoped(&global, &BTreeMap::new()));
        let second = render_instructions(&generate_prompt_unscoped(&global, &BTreeMap::new()));
        assert_eq!(first.as_bytes(), second.as_bytes());
    }
}
