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
            skills_dir: config::expand_tilde("~/.claude/skills"),
        },
        (HarnessKind::ClaudeCode, true) => HarnessPaths {
            settings: PathBuf::from(".claude/settings.json"),
            skills_dir: PathBuf::from(".claude/skills"),
        },
        (HarnessKind::Codex, false) => HarnessPaths {
            settings: config::expand_tilde("~/.codex/hooks.json"),
            skills_dir: config::expand_tilde("~/.agents/skills"),
        },
        (HarnessKind::Codex, true) => HarnessPaths {
            settings: PathBuf::from(".codex/hooks.json"),
            skills_dir: PathBuf::from(".agents/skills"),
        },
        (HarnessKind::Copilot, false) => HarnessPaths {
            settings: copilot_home().join("hooks").join("crystalline.json"),
            skills_dir: copilot_home().join("skills"),
        },
        (HarnessKind::Copilot, true) => HarnessPaths {
            settings: PathBuf::from(".github/hooks/crystalline.json"),
            skills_dir: PathBuf::from(".github/skills"),
        },
    }
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
}
