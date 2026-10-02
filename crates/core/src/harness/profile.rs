//! The harness profile table: one static row per [`HarnessKind`] that says
//! where the harness keeps its MCP servers, its session hook, its always-on
//! pointer and its skills, and how a hook command must be spelled for it.
//!
//! The three legacy harnesses (Claude Code, Codex, GitHub Copilot) carry
//! `Legacy` styles: their install code predates the table and keeps its own
//! paths in [`super::harness_paths`]. Their rows still hold the config root
//! and the skills folder, so every harness answers those two questions from
//! one place. The four profile harnesses (Cursor, Kiro, Gemini CLI, Qwen
//! Code) are described completely here; the facts come from each harness's
//! own documentation and from the files a real install leaves on disk.
//!
//! Pure data and pure path joins: nothing here reads a file or the PATH.

use std::path::{Path, PathBuf};

use super::HarnessKind;
use crate::config;

/// The folder a [`PathSpec`] is relative to.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Base {
    /// The user's home folder.
    Home,
    /// Copilot's home folder: `$COPILOT_HOME` when it is set and non-empty,
    /// `~/.copilot` otherwise.
    CopilotHome,
}

/// A user-scope path: a base folder plus a relative path below it. An empty
/// `rel` names the base folder itself.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct PathSpec {
    /// The folder `rel` is joined onto.
    pub base: Base,
    /// The path below `base`, with `/` separators; empty for the base itself.
    pub rel: &'static str,
}

impl PathSpec {
    /// A path below the home folder.
    pub const fn home(rel: &'static str) -> PathSpec {
        PathSpec {
            base: Base::Home,
            rel,
        }
    }

    /// A path below Copilot's home folder.
    pub const fn copilot_home(rel: &'static str) -> PathSpec {
        PathSpec {
            base: Base::CopilotHome,
            rel,
        }
    }

    /// The path below the two given roots. Pure, so a test can resolve a
    /// spec against a temporary folder without touching the environment.
    pub fn resolve_in(&self, home: &Path, copilot_home: &Path) -> PathBuf {
        let base = match self.base {
            Base::Home => home,
            Base::CopilotHome => copilot_home,
        };
        if self.rel.is_empty() {
            base.to_path_buf()
        } else {
            base.join(self.rel)
        }
    }

    /// The path below this user's real home folder and Copilot home.
    pub fn resolve(&self) -> PathBuf {
        self.resolve_in(&config::expand_tilde("~"), &super::copilot_home())
    }
}

/// The shape of the MCP entry a JSON `mcpServers` map takes.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum EntryShape {
    /// Cursor's stdio entry, which carries `"type": "stdio"` beside the
    /// command and its arguments.
    CursorStdio,
    /// The plain `command` plus `args` entry.
    Plain,
}

/// How a harness registers an MCP server.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum McpStyle {
    /// Registered by the legacy install code through the harness's own CLI.
    Legacy,
    /// One entry under `map_key` in a JSON (or JSONC) file the harness reads.
    JsonEntry {
        /// The file that holds the map.
        file: PathSpec,
        /// The top-level key of the server map.
        map_key: &'static str,
        /// The shape of the entry written under the map.
        shape: EntryShape,
    },
    /// Registered by running the harness CLI; kept for later harnesses
    /// whose only documented surface is a CLI.
    Cli {
        /// The argument template that adds the server.
        add: &'static [&'static str],
        /// The argument template that removes it.
        remove: &'static [&'static str],
    },
}

/// The unit a hook timeout is written in.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TimeoutUnit {
    /// Whole seconds.
    Seconds,
    /// Milliseconds.
    Millis,
}

/// How a harness stores its session-start hook.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum HookDialect {
    /// Written by the legacy install code.
    Legacy,
    /// Claude Code's group shape under `hooks.SessionStart` in a shared
    /// settings file: a group with a `matcher` and a `hooks` array.
    ClaudeGroups {
        /// The shared settings file.
        file: PathSpec,
        /// The timeout written beside the command, in `unit`.
        timeout: u64,
        /// The unit the harness reads `timeout` in.
        unit: TimeoutUnit,
        /// The session-start matcher.
        matcher: &'static str,
    },
    /// A flat map from event name to a list of `{ "command": ... }` entries.
    FlatEventMap {
        /// The hooks file.
        file: PathSpec,
        /// The event name the session-start entry goes under.
        event: &'static str,
    },
    /// A hook file Crystalline owns completely, in Kiro's format.
    KiroOwned {
        /// The owned hook file.
        file: PathSpec,
    },
    /// The harness has no session hook.
    None,
}

impl HookDialect {
    /// The file this dialect writes the hook to, or `None` for the legacy
    /// style and for a harness without hooks.
    pub fn file(&self) -> Option<PathSpec> {
        match *self {
            HookDialect::ClaudeGroups { file, .. }
            | HookDialect::FlatEventMap { file, .. }
            | HookDialect::KiroOwned { file } => Some(file),
            HookDialect::Legacy | HookDialect::None => None,
        }
    }
}

/// The output shape the session-start routing prompt is printed in.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SessionFormat {
    /// Plain text on stdout.
    Text,
    /// Copilot's `additionalContext` envelope.
    Copilot,
    /// Cursor's session-start JSON reply.
    Cursor,
    /// The `hookSpecificOutput` envelope Gemini CLI and Qwen Code read.
    HookSpecific,
}

impl SessionFormat {
    /// The value of `prompt system --format` for this shape, or `None` for
    /// plain text, which needs no flag.
    pub fn flag(self) -> Option<&'static str> {
        match self {
            SessionFormat::Text => None,
            SessionFormat::Copilot => Some("copilot"),
            SessionFormat::Cursor => Some("cursor"),
            SessionFormat::HookSpecific => Some("hook-specific"),
        }
    }
}

/// Where the always-on pointer to the routing prompt lives.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PointerStyle {
    /// No pointer.
    None,
    /// A file Crystalline owns completely.
    OwnedFile {
        /// The owned file.
        path: PathSpec,
        /// The frontmatter body written above the pointer.
        frontmatter: &'static str,
    },
}

/// How a hook can tell that it runs inside another harness that imported it.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum HostSignal {
    /// A field present in the hook's JSON payload.
    PayloadField(&'static str),
    /// An environment variable that is set.
    EnvVar(&'static str),
}

/// How the program in a hook or MCP command is spelled.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CommandSpelling {
    /// The bare `crystalline`, found on the PATH at run time.
    Bare,
    /// The absolute path of the binary found at install time, because the
    /// harness may run hooks with a PATH that lacks it (a GUI app).
    AbsolutePathEntry,
}

/// One harness's row in the profile table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HarnessProfile {
    /// The harness's own config folder.
    pub config_root: PathSpec,
    /// How the MCP server is registered.
    pub mcp: McpStyle,
    /// How the session-start hook is stored.
    pub hooks: HookDialect,
    /// The output shape of the session-start routing prompt.
    pub session_format: SessionFormat,
    /// Where the always-on pointer lives.
    pub pointer: PointerStyle,
    /// The user folder install copies the skills into.
    pub skills_write: PathSpec,
    /// Every user skills folder the harness reads, `skills_write` included.
    pub skills_reads: &'static [PathSpec],
    /// Harnesses whose hooks this harness also runs, each with the signal
    /// that shows such a hook it runs here.
    pub imports_hooks_from: &'static [(HarnessKind, HostSignal)],
    /// How the program in a written command is spelled.
    pub command_spelling: CommandSpelling,
    /// Whether the session hook has been seen to deliver the routing prompt
    /// in a live session of this harness.
    pub onboarding_verified: bool,
    /// Whether `crystalline install` accepts this harness in this build.
    pub install_ready: bool,
    /// Project-scope install; `None` while only user scope is supported.
    pub project: Option<()>,
}

impl HarnessProfile {
    /// Whether this row belongs to a harness the legacy install code serves.
    pub fn is_legacy(&self) -> bool {
        matches!(self.mcp, McpStyle::Legacy) && matches!(self.hooks, HookDialect::Legacy)
    }
}

/// The profile row for `kind`.
pub fn profile(kind: HarnessKind) -> &'static HarnessProfile {
    match kind {
        HarnessKind::ClaudeCode => &CLAUDE_CODE,
        HarnessKind::Codex => &CODEX,
        HarnessKind::Copilot => &COPILOT,
        HarnessKind::Cursor => &CURSOR,
        HarnessKind::Kiro => &KIRO,
        HarnessKind::Gemini => &GEMINI,
        HarnessKind::Qwen => &QWEN,
    }
}

static CLAUDE_CODE: HarnessProfile = HarnessProfile {
    config_root: PathSpec::home(".claude"),
    mcp: McpStyle::Legacy,
    hooks: HookDialect::Legacy,
    session_format: SessionFormat::Text,
    pointer: PointerStyle::None,
    skills_write: PathSpec::home(".claude/skills"),
    skills_reads: &[PathSpec::home(".claude/skills")],
    imports_hooks_from: &[],
    command_spelling: CommandSpelling::Bare,
    onboarding_verified: true,
    install_ready: true,
    project: None,
};

static CODEX: HarnessProfile = HarnessProfile {
    config_root: PathSpec::home(".codex"),
    mcp: McpStyle::Legacy,
    hooks: HookDialect::Legacy,
    session_format: SessionFormat::Text,
    pointer: PointerStyle::None,
    skills_write: PathSpec::home(".agents/skills"),
    skills_reads: &[PathSpec::home(".agents/skills")],
    imports_hooks_from: &[],
    command_spelling: CommandSpelling::Bare,
    onboarding_verified: true,
    install_ready: true,
    project: None,
};

static COPILOT: HarnessProfile = HarnessProfile {
    config_root: PathSpec::copilot_home(""),
    mcp: McpStyle::Legacy,
    hooks: HookDialect::Legacy,
    session_format: SessionFormat::Copilot,
    pointer: PointerStyle::None,
    skills_write: PathSpec::copilot_home("skills"),
    skills_reads: &[PathSpec::copilot_home("skills")],
    imports_hooks_from: &[],
    command_spelling: CommandSpelling::Bare,
    onboarding_verified: true,
    install_ready: true,
    project: None,
};

static CURSOR: HarnessProfile = HarnessProfile {
    config_root: PathSpec::home(".cursor"),
    mcp: McpStyle::JsonEntry {
        file: PathSpec::home(".cursor/mcp.json"),
        map_key: "mcpServers",
        shape: EntryShape::CursorStdio,
    },
    hooks: HookDialect::FlatEventMap {
        file: PathSpec::home(".cursor/hooks.json"),
        event: "sessionStart",
    },
    session_format: SessionFormat::Cursor,
    pointer: PointerStyle::None,
    skills_write: PathSpec::home(".agents/skills"),
    skills_reads: &[
        PathSpec::home(".agents/skills"),
        PathSpec::home(".cursor/skills"),
        PathSpec::home(".claude/skills"),
        PathSpec::home(".codex/skills"),
    ],
    // Cursor runs the hooks in ~/.claude/settings.json too, and marks its
    // own payload with a `cursor_version` field.
    imports_hooks_from: &[(
        HarnessKind::ClaudeCode,
        HostSignal::PayloadField("cursor_version"),
    )],
    command_spelling: CommandSpelling::AbsolutePathEntry,
    onboarding_verified: false,
    install_ready: true,
    project: None,
};

static KIRO: HarnessProfile = HarnessProfile {
    config_root: PathSpec::home(".kiro"),
    mcp: McpStyle::JsonEntry {
        file: PathSpec::home(".kiro/settings/mcp.json"),
        map_key: "mcpServers",
        shape: EntryShape::Plain,
    },
    hooks: HookDialect::KiroOwned {
        file: PathSpec::home(".kiro/hooks/crystalline.json"),
    },
    session_format: SessionFormat::Text,
    pointer: PointerStyle::OwnedFile {
        path: PathSpec::home(".kiro/steering/crystalline.md"),
        frontmatter: "inclusion: always",
    },
    skills_write: PathSpec::home(".kiro/skills"),
    skills_reads: &[PathSpec::home(".kiro/skills")],
    imports_hooks_from: &[],
    command_spelling: CommandSpelling::AbsolutePathEntry,
    onboarding_verified: false,
    install_ready: true,
    project: None,
};

static GEMINI: HarnessProfile = HarnessProfile {
    config_root: PathSpec::home(".gemini"),
    mcp: McpStyle::JsonEntry {
        file: PathSpec::home(".gemini/settings.json"),
        map_key: "mcpServers",
        shape: EntryShape::Plain,
    },
    // Gemini CLI reads hook timeouts in milliseconds: 10 would be 10 ms.
    hooks: HookDialect::ClaudeGroups {
        file: PathSpec::home(".gemini/settings.json"),
        timeout: 10000,
        unit: TimeoutUnit::Millis,
        matcher: "startup",
    },
    session_format: SessionFormat::HookSpecific,
    pointer: PointerStyle::None,
    skills_write: PathSpec::home(".agents/skills"),
    skills_reads: &[
        PathSpec::home(".agents/skills"),
        PathSpec::home(".gemini/skills"),
    ],
    imports_hooks_from: &[],
    command_spelling: CommandSpelling::Bare,
    onboarding_verified: false,
    install_ready: true,
    project: None,
};

static QWEN: HarnessProfile = HarnessProfile {
    config_root: PathSpec::home(".qwen"),
    mcp: McpStyle::JsonEntry {
        file: PathSpec::home(".qwen/settings.json"),
        map_key: "mcpServers",
        shape: EntryShape::Plain,
    },
    // Qwen Code's timeout unit is unverified: its hooks are documented as
    // Claude compatible, where the unit is seconds. It writes 10000 as
    // milliseconds until a live check settles it, because a timeout that is
    // too long is harmless and one that is too short is not.
    hooks: HookDialect::ClaudeGroups {
        file: PathSpec::home(".qwen/settings.json"),
        timeout: 10000,
        unit: TimeoutUnit::Millis,
        matcher: "startup",
    },
    session_format: SessionFormat::HookSpecific,
    pointer: PointerStyle::None,
    skills_write: PathSpec::home(".qwen/skills"),
    skills_reads: &[PathSpec::home(".qwen/skills")],
    imports_hooks_from: &[],
    command_spelling: CommandSpelling::Bare,
    onboarding_verified: false,
    install_ready: false,
    project: None,
};

/// What a hook can see of the process that runs it: the field names
/// present in its JSON payload and a test for a set environment variable.
pub struct HostEvidence<'a> {
    /// The top-level field names the payload carries.
    pub payload_fields: &'a [&'a str],
    /// Whether the named environment variable is set.
    pub env: &'a dyn Fn(&str) -> bool,
}

impl HostEvidence<'_> {
    /// Whether `signal` shows in this evidence.
    pub fn shows(&self, signal: HostSignal) -> bool {
        match signal {
            HostSignal::PayloadField(name) => self.payload_fields.contains(&name),
            HostSignal::EnvVar(name) => (self.env)(name),
        }
    }
}

/// The harness that runs a hook written for `written_for` as an import and
/// has a session hook of its own, so the imported hook must stay silent:
/// the first kind in [`HarnessKind::ALL`] whose `imports_hooks_from` holds
/// `written_for` with a signal `evidence` shows, and which is in `hooked`
/// (the harnesses whose install has the hook part). `None` means the hook
/// prints. A harness's own hook is never silenced by this, because no row
/// imports its own hooks.
pub fn silenced_by_importer(
    written_for: HarnessKind,
    evidence: &HostEvidence<'_>,
    hooked: &[HarnessKind],
) -> Option<HarnessKind> {
    HarnessKind::ALL.into_iter().find(|importer| {
        hooked.contains(importer)
            && profile(*importer)
                .imports_hooks_from
                .iter()
                .any(|&(from, signal)| from == written_for && evidence.shows(signal))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_imported_claude_hook_is_silenced_only_inside_cursor_with_cursors_own_hook() {
        let no_env = |_: &str| false;
        let in_cursor = HostEvidence {
            payload_fields: &["cursor_version"],
            env: &no_env,
        };
        let elsewhere = HostEvidence {
            payload_fields: &[],
            env: &no_env,
        };
        let both = [HarnessKind::ClaudeCode, HarnessKind::Cursor];
        assert_eq!(
            silenced_by_importer(HarnessKind::ClaudeCode, &in_cursor, &both),
            Some(HarnessKind::Cursor)
        );
        assert_eq!(
            silenced_by_importer(
                HarnessKind::ClaudeCode,
                &in_cursor,
                &[HarnessKind::ClaudeCode]
            ),
            None,
            "no Cursor hook: the import speaks"
        );
        assert_eq!(
            silenced_by_importer(HarnessKind::ClaudeCode, &elsewhere, &both),
            None,
            "plain Claude Code"
        );
        assert_eq!(
            silenced_by_importer(HarnessKind::Cursor, &in_cursor, &both),
            None,
            "Cursor's own hook always prints"
        );
        assert_eq!(
            silenced_by_importer(HarnessKind::Codex, &in_cursor, &both),
            None
        );
    }

    #[test]
    fn a_path_spec_resolves_against_the_given_roots() {
        let home = Path::new("/h");
        let copilot = Path::new("/c");
        assert_eq!(
            PathSpec::home(".cursor/mcp.json").resolve_in(home, copilot),
            PathBuf::from("/h/.cursor/mcp.json")
        );
        assert_eq!(
            PathSpec::copilot_home("skills").resolve_in(home, copilot),
            PathBuf::from("/c/skills")
        );
        assert_eq!(
            PathSpec::copilot_home("").resolve_in(home, copilot),
            PathBuf::from("/c"),
            "an empty rel is the base itself, without a trailing separator"
        );
    }

    #[test]
    fn every_row_reads_the_folder_it_writes() {
        for k in HarnessKind::ALL {
            let p = profile(k);
            assert!(p.skills_reads.contains(&p.skills_write), "{k:?}");
        }
    }

    #[test]
    fn no_harness_writes_an_instruction_file() {
        const FORBIDDEN: [&str; 4] = ["claude.md", "agents.md", "gemini.md", "qwen.md"];
        for k in HarnessKind::ALL {
            if let PointerStyle::OwnedFile { path, .. } = profile(k).pointer {
                let name = path.rel.rsplit('/').next().unwrap().to_ascii_lowercase();
                assert!(!FORBIDDEN.contains(&name.as_str()), "{k:?} writes {name}");
            }
        }
    }
}
