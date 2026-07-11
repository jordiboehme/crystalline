//! The harness model: which coding harnesses Crystalline can integrate
//! with, where each one's config lives on disk and how its own CLI is
//! invoked. Kept in core so both the cli crate (`install`/`uninstall`,
//! `doctor`) and the service crate (shelling out to a harness's CLI, and in
//! a later milestone the daemon's own artifact provisioning) share one
//! definition without either crate depending on the other.
//!
//! Everything specific to installing Crystalline itself into a harness -
//! the hooks storage style, the `mcp add` argument shape, the hook command
//! constants - stays with `install` in the cli crate; this module holds
//! only the pure, reusable facts about a harness.

use std::path::PathBuf;

use crate::config;
use crate::manifest::ArtifactType;

/// Which coding harness is being targeted. [`HarnessKind::id`] produces the
/// stable spellings `claude-code`, `codex` and `copilot`, mirrored by the
/// cli crate's `clap::ValueEnum` wrapper for identical CLI spellings and
/// help text.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum HarnessKind {
    /// Anthropic's Claude Code CLI: hooks in `settings.json`, skills in a
    /// `skills` folder under `.claude`.
    ClaudeCode,
    /// The Codex CLI: hooks in a dedicated `hooks.json`, skills under
    /// `.agents/skills`.
    Codex,
    /// The GitHub Copilot CLI: hooks in a wholly Crystalline-owned
    /// `crystalline.json` under Copilot's hooks folder, skills under
    /// `.copilot/skills` (user) or `.github/skills` (project).
    Copilot,
}

impl HarnessKind {
    /// The stable identifier used in machine-readable output, and reused by
    /// `doctor` to label its harnesses section with the same spelling
    /// `crystalline install <name>` takes.
    pub fn id(self) -> &'static str {
        match self {
            HarnessKind::ClaudeCode => "claude-code",
            HarnessKind::Codex => "codex",
            HarnessKind::Copilot => "copilot",
        }
    }

    /// The harness for a stable id (the same spelling [`HarnessKind::id`]
    /// produces); `None` for an id this binary does not know, such as one a
    /// future binary wrote to the install receipt.
    pub fn from_id(id: &str) -> Option<HarnessKind> {
        match id {
            "claude-code" => Some(HarnessKind::ClaudeCode),
            "codex" => Some(HarnessKind::Codex),
            "copilot" => Some(HarnessKind::Copilot),
            _ => None,
        }
    }

    /// The human-facing product name.
    pub fn display_name(self) -> &'static str {
        match self {
            HarnessKind::ClaudeCode => "Claude Code",
            HarnessKind::Codex => "Codex",
            HarnessKind::Copilot => "GitHub Copilot CLI",
        }
    }

    /// The CLI binary that owns this harness's MCP registration.
    pub fn cli(self) -> &'static str {
        match self {
            HarnessKind::ClaudeCode => "claude",
            HarnessKind::Codex => "codex",
            HarnessKind::Copilot => "copilot",
        }
    }

    /// Candidate argv forms for this harness's CLI, tried in order: each is
    /// the program name followed by prefix args placed before the verb args.
    /// The Copilot CLI is also reachable as `gh copilot` (the GitHub CLI
    /// forwards args it does not recognize), so a machine with only `gh`
    /// still registers; a `gh` whose first run wants to install Copilot
    /// fails fast against the null stdin, which degrades to the printed
    /// manual command like any other failure.
    pub fn cli_invocations(self) -> &'static [&'static [&'static str]] {
        match self {
            HarnessKind::ClaudeCode => &[&["claude"]],
            HarnessKind::Codex => &[&["codex"]],
            HarnessKind::Copilot => &[&["copilot"], &["gh", "copilot", "--"]],
        }
    }
}

/// The settings file and skills folder for a harness at a given scope. Reused
/// by `doctor`, which reads these same two paths to report each harness's
/// onboarding trace without ever writing to them.
pub struct HarnessPaths {
    /// The JSON file the hooks live in (`settings.json` for Claude Code, a
    /// dedicated `hooks.json` for Codex).
    pub settings: PathBuf,
    /// The folder each skill's `<name>/SKILL.md` is copied under.
    pub skills_dir: PathBuf,
}

/// Resolve the settings file and skills folder for a harness and scope. User
/// scope expands `~` through [`config::expand_tilde`]; `--project` scope is
/// relative to the current working directory, so it lands in the repository
/// the command is run from.
pub fn harness_paths(harness: HarnessKind, project: bool) -> HarnessPaths {
    match (harness, project) {
        (HarnessKind::ClaudeCode, false) => HarnessPaths {
            settings: config::expand_tilde("~/.claude/settings.json"),
            skills_dir: user_skills_dir(harness),
        },
        (HarnessKind::ClaudeCode, true) => HarnessPaths {
            settings: PathBuf::from(".claude/settings.json"),
            skills_dir: PathBuf::from(".claude/skills"),
        },
        (HarnessKind::Codex, false) => HarnessPaths {
            settings: config::expand_tilde("~/.codex/hooks.json"),
            skills_dir: user_skills_dir(harness),
        },
        (HarnessKind::Codex, true) => HarnessPaths {
            settings: PathBuf::from(".codex/hooks.json"),
            skills_dir: PathBuf::from(".agents/skills"),
        },
        (HarnessKind::Copilot, false) => HarnessPaths {
            settings: copilot_home().join("hooks").join("crystalline.json"),
            skills_dir: user_skills_dir(harness),
        },
        (HarnessKind::Copilot, true) => HarnessPaths {
            settings: PathBuf::from(".github/hooks/crystalline.json"),
            skills_dir: PathBuf::from(".github/skills"),
        },
    }
}

/// The user-scope skills folder for a harness. The single source of truth for
/// that path, shared by [`harness_paths`] (which surfaces it as
/// `skills_dir`) and [`artifact_base`] (which returns it for
/// [`ArtifactType::Skills`]) so the two can never drift.
fn user_skills_dir(harness: HarnessKind) -> PathBuf {
    match harness {
        HarnessKind::ClaudeCode => config::expand_tilde("~/.claude/skills"),
        HarnessKind::Codex => config::expand_tilde("~/.agents/skills"),
        HarnessKind::Copilot => copilot_home().join("skills"),
    }
}

/// The user-scope directory a harness stores artifacts of `kind` under, or
/// `None` when this harness keeps that kind nowhere on disk: an MCP config is
/// registered through the harness CLI rather than written as a file, and a
/// harness with no surface for a kind has no folder for it. A reconcile
/// engine maps a desired key `"<kind>/<rel>"` to a real path by joining `rel`
/// onto this base.
///
/// User scope only, which is where a domain provisions today. The full
/// matrix: every harness keeps skills under its own skills folder shared with
/// [`harness_paths`]; Claude Code stores commands and agents under
/// `~/.claude`; Codex stores its custom prompts (the flat directory a nested
/// command flattens into) under `~/.codex/prompts` and its TOML agents under
/// `~/.codex/agents`; GitHub Copilot reads markdown agents from its home's
/// `agents` folder but declined a command surface outright, so Copilot
/// commands stay `None` and are skipped with a notice. Every harness
/// registers MCP servers through its own CLI, never as a file, so `Mcps` is
/// always `None`.
pub fn artifact_base(harness: HarnessKind, kind: ArtifactType) -> anyhow::Result<Option<PathBuf>> {
    let base = match (harness, kind) {
        (_, ArtifactType::Skills) => Some(user_skills_dir(harness)),
        (HarnessKind::ClaudeCode, ArtifactType::Commands) => {
            Some(config::expand_tilde("~/.claude/commands"))
        }
        (HarnessKind::ClaudeCode, ArtifactType::Agents) => {
            Some(config::expand_tilde("~/.claude/agents"))
        }
        // Codex custom prompts are deprecated upstream but functional; this
        // base goes when the translate module's Codex command arm goes.
        (HarnessKind::Codex, ArtifactType::Commands) => {
            Some(config::expand_tilde("~/.codex/prompts"))
        }
        (HarnessKind::Codex, ArtifactType::Agents) => Some(config::expand_tilde("~/.codex/agents")),
        (HarnessKind::Copilot, ArtifactType::Agents) => Some(copilot_home().join("agents")),
        // The Copilot CLI declined a prompt-file surface (skills replace it),
        // and every harness registers MCP servers through its own CLI.
        (HarnessKind::Copilot, ArtifactType::Commands) | (_, ArtifactType::Mcps) => None,
    };
    Ok(base)
}

/// Copilot's home folder: `$COPILOT_HOME` when it is set and non-empty,
/// `~/.copilot` otherwise, matching how the Copilot CLI itself resolves its
/// hooks and skills locations.
fn copilot_home() -> PathBuf {
    match std::env::var_os("COPILOT_HOME") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => config::expand_tilde("~/.copilot"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn harness_paths_resolve_user_and_project_scopes() {
        for harness in [
            HarnessKind::ClaudeCode,
            HarnessKind::Codex,
            HarnessKind::Copilot,
        ] {
            let user = harness_paths(harness, false);
            let project = harness_paths(harness, true);
            match harness {
                HarnessKind::ClaudeCode => {
                    assert_eq!(
                        user.settings,
                        config::expand_tilde("~/.claude/settings.json")
                    );
                    assert_eq!(user.skills_dir, config::expand_tilde("~/.claude/skills"));
                    assert_eq!(project.settings, PathBuf::from(".claude/settings.json"));
                    assert_eq!(project.skills_dir, PathBuf::from(".claude/skills"));
                }
                HarnessKind::Codex => {
                    assert_eq!(user.settings, config::expand_tilde("~/.codex/hooks.json"));
                    assert_eq!(user.skills_dir, config::expand_tilde("~/.agents/skills"));
                    assert_eq!(project.settings, PathBuf::from(".codex/hooks.json"));
                    assert_eq!(project.skills_dir, PathBuf::from(".agents/skills"));
                }
                HarnessKind::Copilot => {
                    assert_eq!(
                        user.settings,
                        copilot_home().join("hooks").join("crystalline.json")
                    );
                    assert_eq!(user.skills_dir, copilot_home().join("skills"));
                    assert_eq!(
                        project.settings,
                        PathBuf::from(".github/hooks/crystalline.json")
                    );
                    assert_eq!(project.skills_dir, PathBuf::from(".github/skills"));
                }
            }
        }
    }

    #[test]
    fn artifact_base_maps_each_kind_to_its_user_scope_folder() {
        // Claude Code keeps skills, commands and agents as files, each under
        // its own `~/.claude` folder, but registers MCP servers through its
        // CLI - so those three resolve to a base and `Mcps` does not.
        assert_eq!(
            artifact_base(HarnessKind::ClaudeCode, ArtifactType::Skills).unwrap(),
            Some(config::expand_tilde("~/.claude/skills"))
        );
        assert_eq!(
            artifact_base(HarnessKind::ClaudeCode, ArtifactType::Commands).unwrap(),
            Some(config::expand_tilde("~/.claude/commands"))
        );
        assert_eq!(
            artifact_base(HarnessKind::ClaudeCode, ArtifactType::Agents).unwrap(),
            Some(config::expand_tilde("~/.claude/agents"))
        );
        assert_eq!(
            artifact_base(HarnessKind::ClaudeCode, ArtifactType::Mcps).unwrap(),
            None
        );
    }

    #[test]
    fn artifact_base_skills_folder_never_drifts_from_harness_paths() {
        // The skills base and `harness_paths`' `skills_dir` share one helper,
        // so every harness must agree on where skills land.
        for harness in [
            HarnessKind::ClaudeCode,
            HarnessKind::Codex,
            HarnessKind::Copilot,
        ] {
            assert_eq!(
                artifact_base(harness, ArtifactType::Skills).unwrap(),
                Some(harness_paths(harness, false).skills_dir)
            );
        }
    }

    #[test]
    fn artifact_base_codex_maps_prompts_and_agents_under_dot_codex() {
        // A nested command flattens into Codex's flat prompts directory; a
        // markdown agent renders into a TOML file under its agents directory.
        assert_eq!(
            artifact_base(HarnessKind::Codex, ArtifactType::Commands).unwrap(),
            Some(config::expand_tilde("~/.codex/prompts"))
        );
        assert_eq!(
            artifact_base(HarnessKind::Codex, ArtifactType::Agents).unwrap(),
            Some(config::expand_tilde("~/.codex/agents"))
        );
        assert_eq!(
            artifact_base(HarnessKind::Codex, ArtifactType::Mcps).unwrap(),
            None
        );
    }

    #[test]
    fn artifact_base_copilot_maps_agents_under_its_home_and_commands_nowhere() {
        // Copilot agents ride the same `COPILOT_HOME` helper as its skills and
        // hooks; commands have no Copilot surface at all, so their base stays
        // `None` and the desired-set projection skips them with a notice.
        assert_eq!(
            artifact_base(HarnessKind::Copilot, ArtifactType::Agents).unwrap(),
            Some(copilot_home().join("agents"))
        );
        assert_eq!(
            artifact_base(HarnessKind::Copilot, ArtifactType::Commands).unwrap(),
            None
        );
        assert_eq!(
            artifact_base(HarnessKind::Copilot, ArtifactType::Mcps).unwrap(),
            None
        );
    }

    #[test]
    fn artifact_base_copilot_skills_honor_copilot_home() {
        // Copilot's skills base rides on the same `COPILOT_HOME` helper the
        // rest of Copilot's paths use.
        let copilot = artifact_base(HarnessKind::Copilot, ArtifactType::Skills)
            .unwrap()
            .unwrap();
        assert_eq!(copilot, copilot_home().join("skills"));
    }
}
