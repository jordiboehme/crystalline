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

pub mod pointer;
pub mod profile;

pub use profile::HarnessProfile;

/// Which coding harness is being targeted. [`HarnessKind::id`] produces the
/// stable spellings `claude-code`, `codex`, `copilot`, `cursor`, `kiro`,
/// `gemini` and `qwen`, mirrored by the cli crate's `clap::ValueEnum`
/// wrapper for identical CLI spellings.
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
    /// Cursor (editor and agent CLI): MCP in ~/.cursor/mcp.json, a
    /// sessionStart hook in ~/.cursor/hooks.json, skills under
    /// ~/.agents/skills.
    Cursor,
    /// Kiro (IDE and CLI): MCP in ~/.kiro/settings/mcp.json, a hook and a
    /// steering file under ~/.kiro, skills under ~/.kiro/skills.
    Kiro,
    /// Gemini CLI: MCP and a SessionStart hook in ~/.gemini/settings.json,
    /// skills under ~/.agents/skills.
    Gemini,
    /// Qwen Code: MCP and a SessionStart hook in ~/.qwen/settings.json,
    /// skills under ~/.qwen/skills.
    Qwen,
}

impl HarnessKind {
    /// Every harness, in declaration order. Lists that walk all harnesses
    /// (provisioning, doctor) iterate this, so a new variant cannot be
    /// forgotten in one of them; a test pins that it is complete.
    pub const ALL: [HarnessKind; 7] = [
        HarnessKind::ClaudeCode,
        HarnessKind::Codex,
        HarnessKind::Copilot,
        HarnessKind::Cursor,
        HarnessKind::Kiro,
        HarnessKind::Gemini,
        HarnessKind::Qwen,
    ];

    /// This harness's row in the profile table.
    pub fn profile(self) -> &'static HarnessProfile {
        profile::profile(self)
    }

    /// The stable identifier used in machine-readable output, and reused by
    /// `doctor` to label its harnesses section with the same spelling
    /// `crystalline install <name>` takes.
    pub fn id(self) -> &'static str {
        match self {
            HarnessKind::ClaudeCode => "claude-code",
            HarnessKind::Codex => "codex",
            HarnessKind::Copilot => "copilot",
            HarnessKind::Cursor => "cursor",
            HarnessKind::Kiro => "kiro",
            HarnessKind::Gemini => "gemini",
            HarnessKind::Qwen => "qwen",
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
            "cursor" => Some(HarnessKind::Cursor),
            "kiro" => Some(HarnessKind::Kiro),
            "gemini" => Some(HarnessKind::Gemini),
            "qwen" => Some(HarnessKind::Qwen),
            _ => None,
        }
    }

    /// The harness behind an MCP `initialize` handshake's `clientInfo.name`,
    /// or `None` for a name this binary does not recognize. Matching is
    /// case-insensitive on the name alone; the client's version is never part
    /// of it, so a harness upgrade cannot silently stop matching.
    ///
    /// **The MCP path no longer consults this.** Per-harness gating of the
    /// skill surface resolves once at startup, from the `--harness` argument
    /// the registration carries plus this machine's install receipt, because
    /// `clientInfo` is optional under the 2026-07-28 lifecycle and a server
    /// SHOULD NOT change behaviour on it. Do not wire this back into a
    /// handshake or a listing: a gate that varies per connection also breaks
    /// SEP-2567's list invariance. It stays as the name table for callers that
    /// want to identify a client for their own reasons, never to decide what
    /// to serve it.
    ///
    /// The table is deliberately small and conservative, because an unknown
    /// name must never match: a false match would cost a client the onboarding
    /// it depends on, while a missed match only costs some duplicated context.
    /// The three names, and how each was established:
    ///
    /// - `claude-code`: observed directly from Claude Code 2.1.220's own
    ///   `initialize` request against a probe stdio server.
    /// - `codex-mcp-client`: the constant the Codex CLI sends from
    ///   `codex-rs/core/src/mcp_connection_manager.rs`, quoted in
    ///   openai/codex#16485.
    /// - `github-copilot-developer`: the name every GitHub Copilot host sends,
    ///   quoted in github/copilot-cli#2107. That issue exists precisely
    ///   because the name does not distinguish the Copilot CLI from the other
    ///   Copilot hosts, so a non-CLI Copilot client connecting over stdio on a
    ///   machine where the Copilot CLI is installed matches too. Accepted:
    ///   both share Copilot's own skills folder, so the receipt's claim that
    ///   the skills are already on disk holds for either. Narrow this arm to a
    ///   CLI-specific signal once Copilot ships one.
    pub fn from_mcp_client_name(name: &str) -> Option<HarnessKind> {
        match name.trim().to_ascii_lowercase().as_str() {
            "claude-code" => Some(HarnessKind::ClaudeCode),
            "codex-mcp-client" => Some(HarnessKind::Codex),
            "github-copilot-developer" => Some(HarnessKind::Copilot),
            _ => None,
        }
    }

    /// The human-facing product name.
    pub fn display_name(self) -> &'static str {
        match self {
            HarnessKind::ClaudeCode => "Claude Code",
            HarnessKind::Codex => "Codex",
            HarnessKind::Copilot => "GitHub Copilot CLI",
            HarnessKind::Cursor => "Cursor",
            HarnessKind::Kiro => "Kiro",
            HarnessKind::Gemini => "Gemini CLI",
            HarnessKind::Qwen => "Qwen Code",
        }
    }

    /// The CLI binary that owns this harness's MCP registration. For the
    /// four profile harnesses, whose MCP entry install writes into a file,
    /// it is the harness's own CLI and appears only in manual hints.
    pub fn cli(self) -> &'static str {
        match self {
            HarnessKind::ClaudeCode => "claude",
            HarnessKind::Codex => "codex",
            HarnessKind::Copilot => "copilot",
            HarnessKind::Cursor => "cursor-agent",
            HarnessKind::Kiro => "kiro-cli",
            HarnessKind::Gemini => "gemini",
            HarnessKind::Qwen => "qwen",
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
            HarnessKind::Cursor => &[&["cursor-agent"]],
            HarnessKind::Kiro => &[&["kiro-cli"]],
            HarnessKind::Gemini => &[&["gemini"]],
            HarnessKind::Qwen => &[&["qwen"]],
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
        // The profile harnesses: the hook file and the skills folder from
        // their profile row, the same for both scopes. The project arm is
        // never reached, because install and uninstall refuse `--project`
        // for these four.
        (HarnessKind::Cursor | HarnessKind::Kiro | HarnessKind::Gemini | HarnessKind::Qwen, _) => {
            let profile = harness.profile();
            HarnessPaths {
                settings: profile
                    .hooks
                    .file()
                    .unwrap_or(profile.config_root)
                    .resolve(),
                skills_dir: user_skills_dir(harness),
            }
        }
    }
}

/// The user-scope skills folder for a harness. The single source of truth for
/// that path, shared by [`harness_paths`] (which surfaces it as
/// `skills_dir`) and [`artifact_base`] (which returns it for
/// [`ArtifactType::Skills`]) so the two can never drift. It reads the
/// profile row's `skills_write`, so the table and the path helpers are one
/// statement.
fn user_skills_dir(harness: HarnessKind) -> PathBuf {
    harness.profile().skills_write.resolve()
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
        // The profile harnesses provision no commands or agents yet
        // (`harness_supports` is false for them), so they have no base.
        (
            HarnessKind::Cursor | HarnessKind::Kiro | HarnessKind::Gemini | HarnessKind::Qwen,
            ArtifactType::Commands | ArtifactType::Agents,
        ) => None,
    };
    Ok(base)
}

/// Copilot's home folder: `$COPILOT_HOME` when it is set and non-empty,
/// `~/.copilot` otherwise, matching how the Copilot CLI itself resolves its
/// hooks and skills locations.
pub(crate) fn copilot_home() -> PathBuf {
    match std::env::var_os("COPILOT_HOME") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => config::expand_tilde("~/.copilot"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `initialize` client names the three harnesses actually send, and
    /// the rule that everything else is a miss. Each name's provenance is on
    /// [`HarnessKind::from_mcp_client_name`].
    #[test]
    fn mcp_client_names_map_only_the_three_known_harnesses() {
        assert_eq!(
            HarnessKind::from_mcp_client_name("claude-code"),
            Some(HarnessKind::ClaudeCode)
        );
        assert_eq!(
            HarnessKind::from_mcp_client_name("codex-mcp-client"),
            Some(HarnessKind::Codex)
        );
        assert_eq!(
            HarnessKind::from_mcp_client_name("github-copilot-developer"),
            Some(HarnessKind::Copilot)
        );

        // Case-insensitive, and surrounding whitespace is not a miss.
        assert_eq!(
            HarnessKind::from_mcp_client_name("  Claude-Code "),
            Some(HarnessKind::ClaudeCode)
        );

        // A harness id is not a client name: the two vocabularies are
        // deliberately separate, so `from_id` spellings do not match here.
        for name in [
            "",
            "codex",
            "copilot",
            "claude",
            "claude-desktop",
            "cursor-vscode",
            "mcp-inspector",
            "crystalline",
        ] {
            assert_eq!(
                HarnessKind::from_mcp_client_name(name),
                None,
                "'{name}' must never match a harness"
            );
        }
    }

    /// `ALL` is complete. The successor ring is an exhaustive match, so a new
    /// variant does not compile until it gets an arm here; put it into the
    /// ring, and the walk below then demands it in `ALL` as well.
    #[test]
    fn all_lists_every_variant_once_in_declaration_order() {
        fn next(k: HarnessKind) -> HarnessKind {
            match k {
                HarnessKind::ClaudeCode => HarnessKind::Codex,
                HarnessKind::Codex => HarnessKind::Copilot,
                HarnessKind::Copilot => HarnessKind::Cursor,
                HarnessKind::Cursor => HarnessKind::Kiro,
                HarnessKind::Kiro => HarnessKind::Gemini,
                HarnessKind::Gemini => HarnessKind::Qwen,
                HarnessKind::Qwen => HarnessKind::ClaudeCode,
            }
        }
        let mut walked = vec![HarnessKind::ClaudeCode];
        let mut k = next(HarnessKind::ClaudeCode);
        while k != HarnessKind::ClaudeCode {
            walked.push(k);
            k = next(k);
        }
        assert_eq!(walked, HarnessKind::ALL);
        let ids: Vec<&str> = HarnessKind::ALL.iter().map(|k| k.id()).collect();
        assert_eq!(
            ids,
            [
                "claude-code",
                "codex",
                "copilot",
                "cursor",
                "kiro",
                "gemini",
                "qwen"
            ]
        );
        for k in HarnessKind::ALL {
            assert_eq!(HarnessKind::from_id(k.id()), Some(k), "{k:?} round-trips");
        }
    }

    /// The four new rows carry the landscape's facts; the gate and the
    /// install flag start closed.
    #[test]
    fn the_new_profiles_hold_the_landscape_facts() {
        use profile::*;
        let c = HarnessKind::Cursor.profile();
        assert!(
            matches!(c.mcp, McpStyle::JsonEntry { file, map_key: "mcpServers", shape: EntryShape::CursorStdio } if file == PathSpec::home(".cursor/mcp.json"))
        );
        assert!(
            matches!(c.hooks, HookDialect::FlatEventMap { file, event: "sessionStart" } if file == PathSpec::home(".cursor/hooks.json"))
        );
        assert_eq!(c.session_format.flag(), Some("cursor"));
        assert_eq!(c.command_spelling, CommandSpelling::AbsolutePathEntry);
        assert_eq!(
            c.imports_hooks_from,
            &[(
                HarnessKind::ClaudeCode,
                HostSignal::PayloadField("cursor_version")
            )]
        );
        assert!(c.skills_reads.contains(&PathSpec::home(".claude/skills")));

        let g = HarnessKind::Gemini.profile();
        assert!(
            matches!(
                g.hooks,
                HookDialect::ClaudeGroups {
                    timeout: 10000,
                    unit: TimeoutUnit::Millis,
                    matcher: "startup",
                    ..
                }
            ),
            "Gemini reads milliseconds: 10 would be a 10 ms timeout"
        );
        assert_eq!(g.session_format.flag(), Some("hook-specific"));
        assert_eq!(g.pointer_when, PointerWhen::OnlyWithoutHook);
        assert_eq!(g.skills_write, PathSpec::home(".agents/skills"));

        let k = HarnessKind::Kiro.profile();
        assert!(
            matches!(k.pointer, PointerStyle::OwnedFile { path, .. } if path == PathSpec::home(".kiro/steering/crystalline.md"))
        );
        assert_eq!(k.pointer_when, PointerWhen::Always);
        assert_eq!(
            HarnessKind::Qwen.profile().skills_write,
            PathSpec::home(".qwen/skills")
        );

        for k in [
            HarnessKind::Cursor,
            HarnessKind::Kiro,
            HarnessKind::Gemini,
            HarnessKind::Qwen,
        ] {
            assert!(
                !k.profile().onboarding_verified,
                "{k:?} is unverified in 0.22.1"
            );
            assert!(!k.profile().is_legacy());
            assert!(k.profile().project.is_none());
        }
        for k in [
            HarnessKind::ClaudeCode,
            HarnessKind::Codex,
            HarnessKind::Copilot,
        ] {
            assert!(k.profile().is_legacy() && k.profile().onboarding_verified);
        }
    }

    /// The profile's skills folder and the existing path helpers agree for
    /// every harness, so the table cannot drift from `artifact_base`.
    #[test]
    fn the_profile_skills_folder_never_drifts_from_harness_paths() {
        for k in HarnessKind::ALL {
            assert_eq!(
                k.profile().skills_write.resolve(),
                harness_paths(k, false).skills_dir,
                "{k:?}"
            );
            assert_eq!(
                k.profile().skills_write.resolve(),
                user_skills_dir(k),
                "{k:?}"
            );
        }
    }

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
                _ => unreachable!("only the three legacy harnesses are walked here"),
            }
        }
    }

    /// The profile harnesses take their hook file and skills folder from the
    /// profile row, at both scopes (the project arm is never reached).
    #[test]
    fn harness_paths_of_the_profile_harnesses_come_from_the_profile() {
        let cases = [
            (
                HarnessKind::Cursor,
                "~/.cursor/hooks.json",
                "~/.agents/skills",
            ),
            (
                HarnessKind::Kiro,
                "~/.kiro/hooks/crystalline.json",
                "~/.kiro/skills",
            ),
            (
                HarnessKind::Gemini,
                "~/.gemini/settings.json",
                "~/.agents/skills",
            ),
            (HarnessKind::Qwen, "~/.qwen/settings.json", "~/.qwen/skills"),
        ];
        for (harness, settings, skills) in cases {
            for project in [false, true] {
                let paths = harness_paths(harness, project);
                assert_eq!(
                    paths.settings,
                    config::expand_tilde(settings),
                    "{harness:?}"
                );
                assert_eq!(
                    paths.skills_dir,
                    config::expand_tilde(skills),
                    "{harness:?}"
                );
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
        for harness in HarnessKind::ALL {
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
