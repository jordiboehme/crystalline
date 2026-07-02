//! Routing prompt generation: `crystalline prompt`.
//!
//! Static and dependency-free like [`crate::verify`]: given the global
//! config (registered domains plus `prompt.rules` path-glob filters) and a
//! workspace path, this reads each included domain's `MANIFEST.md` and
//! renders a compact routing block. Per the Purpose section of the project's
//! house rules, the block is framed as session onboarding: an agent is being
//! introduced to knowledge it already has, not handed a file listing.
//!
//! A domain whose `MANIFEST.md` is missing or unreadable never aborts the
//! run - it gets a placeholder routing line and a warning the caller can
//! print to stderr.

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

/// Render the "CRYSTALLINE KNOWLEDGE ROUTING" onboarding block: one line per
/// domain joining its bullets with `; `, followed by the behavior rules that
/// tell the agent when to narrow a search to a domain versus sweeping all of
/// them, and that writes always need an explicit domain.
pub fn render_text(output: &PromptOutput) -> String {
    let mut out = String::new();
    out.push_str("CRYSTALLINE KNOWLEDGE ROUTING\n\n");
    out.push_str(
        "You are being onboarded to the knowledge accumulated in this workspace. The domains below are registered and ready to use; route searches and reads through them instead of starting from zero.\n\n",
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
        "- Narrow question, one domain clearly fits: search with domains=[that domain].\n",
    );
    out.push_str("- Broad or unclear question: default to an all-domain sweep (omit domains).\n");
    out.push_str(
        "- Writes always require an explicit domain; there is no default domain for writes.\n",
    );
    out.push_str("- Read a domain's MANIFEST only when the routing line above is not enough.\n");
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
    workspace: String,
    domains: Vec<JsonPromptDomain<'a>>,
}

/// Render the prompt as JSON: `{version: 1, workspace, domains: [{name,
/// bullets, preferred}]}`.
pub fn render_json(output: &PromptOutput) -> String {
    let wrapped = JsonPrompt {
        version: 1,
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
