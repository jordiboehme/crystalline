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
//! Determinism contract: `generate_prompt` and both renderers are pure
//! functions of their inputs (the config file and the MANIFESTs on disk).
//! No timestamps, process IDs, environment variables or other
//! environment-dependent values ever enter the output, and no unordered
//! iteration (a hash map, a hash set) drives ordering. Domain order comes
//! from the config's own registered order, filtered through a sorted
//! inclusion set and reordered preferred-first; `render_json` relies on
//! `serde_json`'s stable field order. Identical config plus identical
//! on-disk MANIFESTs must render byte-identical output every time, whether
//! rendered twice in one process or by two separate invocations of the
//! binary, so a harness can cache the rendered prompt across sessions.
//! This contract binds every prompt kind added after `system`, not only
//! the one implemented today.
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

use std::collections::BTreeSet;
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
}

/// Build the routing prompt for a workspace: apply `prompt.rules`
/// include/exclude glob filters, then a repo-local `preferred_domains`
/// reorder, then read each included domain's MANIFEST.
pub fn generate_prompt(global: &GlobalConfig, workspace: &Path) -> PromptOutput {
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
        let (bullets, warning) = load_routing_bullets(name, &entry.path);
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

    if output.domains.is_empty() {
        out.push_str("(no domains are registered for this workspace)\n\n");
    } else {
        for d in &output.domains {
            let label = if d.preferred {
                format!("{} (preferred)", d.name)
            } else {
                d.name.clone()
            };
            let _ = writeln!(out, "- {label}: {}", d.bullets.join("; "));
        }
        out.push('\n');
    }

    out.push_str("Behavior:\n");
    out.push_str(
        "- Narrow question, one domain clearly fits: search_engrams with domains=[that domain].\n",
    );
    out.push_str(
        "- Broad or unclear question: search_engrams without domains is an all-domain sweep.\n",
    );
    out.push_str(
        "- write_engram, edit_engram, move_engram and delete_engram always require an explicit domain; there is no default domain for writes.\n",
    );
    out.push_str(
        "- build_context on a crystalline:// anchor assembles related knowledge around a task.\n",
    );
    out.push_str(
        "- Read a domain's MANIFEST via read_engram only when its routing line above is not enough; list_domains with include_routing=true re-fetches this index mid-session.\n",
    );
    out
}

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
    workspace: String,
    domains: Vec<JsonPromptDomain<'a>>,
}

/// Render the prompt as JSON: `{version: 1, kind: "system", workspace,
/// domains: [{name, bullets, preferred}]}`. `kind` is additive; `version`
/// stays 1.
pub fn render_json(output: &PromptOutput) -> String {
    let wrapped = JsonPrompt {
        version: 1,
        kind: "system",
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
            "---\ntype: manifest\ntitle: MANIFEST\npermalink: test/manifest\ntags:\n- manifest\nstatus: current\nrecorded_at: 2026-01-01\ntimestamp: 2026-01-01T00:00:00+00:00\n---\n\n# Test Domain\n\n## Scope\n\n- test scope\n\n## When to Use\n\n{bullets}"
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
        domains.insert("alpha".to_string(), DomainEntry { path: alpha });
        domains.insert("beta".to_string(), DomainEntry { path: beta });

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
        let first = render_text(&generate_prompt(&global, tmp.path()));
        let second = render_text(&generate_prompt(&global, tmp.path()));
        assert_eq!(first.as_bytes(), second.as_bytes());
    }

    #[test]
    fn generate_and_render_json_is_byte_identical_across_repeated_runs() {
        let (tmp, global) = fixture();
        let first = render_json(&generate_prompt(&global, tmp.path()));
        let second = render_json(&generate_prompt(&global, tmp.path()));
        assert_eq!(first.as_bytes(), second.as_bytes());
    }

    #[test]
    fn render_text_names_the_mcp_server_and_exact_tool_names() {
        let (tmp, global) = fixture();
        let text = render_text(&generate_prompt(&global, tmp.path()));
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
        let json = render_json(&generate_prompt(&global, tmp.path()));
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["version"], 1);
        assert_eq!(value["kind"], "system");
    }
}
