//! The `claude -p` command of a headless run and the private MCP config it
//! reads.

use super::*;

/// The agent's MCP config in a fresh directory only this user can read. The
/// file and the directory go away when this is dropped: at the end of a
/// run, on an error and after a stop signal alike.
pub(super) struct PrivateConfig {
    /// Removed with everything in it on drop.
    _dir: tempfile::TempDir,
    pub(super) path: PathBuf,
}

impl PrivateConfig {
    pub(super) fn write(config: &Value) -> anyhow::Result<PrivateConfig> {
        let dir = tempfile::Builder::new()
            .prefix("crystalline-run-")
            .tempdir()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700))?;
        }
        let path = dir.path().join("mcp.json");
        private_file(&path, serde_json::to_string(config)?.as_bytes())?;
        Ok(PrivateConfig { _dir: dir, path })
    }

    /// Write `text` as the skills file beside the config, readable only by
    /// this user and removed with it. Its path is what the agent is started
    /// with (`--append-system-prompt-file`).
    pub(super) fn write_skills(&self, text: &str) -> anyhow::Result<PathBuf> {
        let path = self._dir.path().join("skills.md");
        private_file(&path, text.as_bytes())?;
        Ok(path)
    }
}

/// A new file at `path` with `bytes`, mode 0600 on unix.
fn private_file(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

/// What one headless agent session is started with.
pub struct ClaudeRun {
    /// The `claude` binary.
    pub program: PathBuf,
    /// The MCP config file.
    pub mcp_config: PathBuf,
    /// The filled-in shipped prompt.
    pub prompt: String,
    /// The bare MCP tool names the agent may never call, as the server
    /// answered them (`automation_prompt`).
    pub disallowed_tools: Vec<String>,
    /// The file of skill text appended to the agent's system prompt
    /// (`--append-system-prompt-file`), when the prompt says the skills are
    /// loaded: the runner then hands them over itself.
    pub skills_file: Option<PathBuf>,
    /// `--max-turns`.
    pub max_turns: u32,
    /// `--model`, when one is named.
    pub model: Option<String>,
}

/// The tools `names` in Claude Code's names, joined for `--disallowedTools`.
pub(super) fn disallowed_tools<S: AsRef<str>>(names: &[S]) -> String {
    names
        .iter()
        .map(|name| format!("{MCP_TOOL_PREFIX}__{}", name.as_ref()))
        .collect::<Vec<_>>()
        .join(",")
}

/// The argv after the program, in order. The prompt comes last, after `-p`,
/// so none of the list-taking flags (`--tools`, `--allowedTools`,
/// `--disallowedTools`, `--mcp-config`) can take it for one of its values.
pub(super) fn claude_args(cfg: &ClaudeRun) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "--mcp-config".into(),
        cfg.mcp_config.to_string_lossy().into_owned(),
        "--strict-mcp-config".into(),
        "--setting-sources".into(),
        String::new(),
        "--tools".into(),
        String::new(),
        "--allowedTools".into(),
        MCP_TOOL_PREFIX.into(),
        "--disallowedTools".into(),
        disallowed_tools(&cfg.disallowed_tools),
        "--output-format".into(),
        "stream-json".into(),
        "--verbose".into(),
        "--max-turns".into(),
        cfg.max_turns.to_string(),
    ];
    if let Some(model) = &cfg.model {
        args.push("--model".into());
        args.push(model.clone());
    }
    if let Some(skills) = &cfg.skills_file {
        args.push("--append-system-prompt-file".into());
        args.push(skills.to_string_lossy().into_owned());
    }
    args.push("-p".into());
    args.push(cfg.prompt.clone());
    args
}

/// The `claude -p` command of one headless run: only the Crystalline MCP
/// server, no built-in tools, no settings files, the forbidden tools denied
/// and the stream as JSON lines.
pub fn claude_command(cfg: &ClaudeRun) -> std::process::Command {
    let mut command = std::process::Command::new(&cfg.program);
    // The agent is not Crystalline: it gets no embedding thread variables.
    crystalline_core::thread_env::strip_from(&mut command);
    command.args(claude_args(cfg));
    command
}

/// `claude` on `PATH`, or the error that names the install docs.
pub(super) fn find_claude() -> anyhow::Result<PathBuf> {
    let names: &[&str] = if cfg!(windows) {
        &["claude.exe", "claude.cmd", "claude"]
    } else {
        &["claude"]
    };
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            for name in names {
                let candidate = dir.join(name);
                if is_executable(&candidate) {
                    return Ok(candidate);
                }
            }
        }
    }
    anyhow::bail!(
        "claude is not on PATH. A headless run needs Claude Code. Install it: {CLAUDE_INSTALL_DOCS}"
    )
}

pub(super) fn is_executable(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        meta.is_file() && meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        meta.is_file()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pinned_run(model: Option<&str>) -> ClaudeRun {
        ClaudeRun {
            program: PathBuf::from("/usr/local/bin/claude"),
            mcp_config: PathBuf::from("/tmp/run/mcp.json"),
            prompt: "the prompt".to_string(),
            disallowed_tools: HEADLESS_DISALLOWED_TOOLS
                .iter()
                .map(|t| t.to_string())
                .collect(),
            skills_file: None,
            max_turns: 60,
            model: model.map(str::to_string),
        }
    }

    /// With skills to hand over, the file follows the other flags and stays
    /// before `-p`, so no list-taking flag can take it.
    #[test]
    fn the_skills_file_goes_before_the_prompt() {
        let mut run = pinned_run(None);
        run.skills_file = Some(PathBuf::from("/tmp/run/skills.md"));
        let args = claude_args(&run);
        assert_eq!(
            args[args.len() - 4..],
            [
                "--append-system-prompt-file",
                "/tmp/run/skills.md",
                "-p",
                "the prompt"
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_skills_file_is_private_and_goes_away_with_the_config() {
        use std::os::unix::fs::PermissionsExt as _;
        let config = PrivateConfig::write(&remote_mcp_config(
            "https://x",
            "cmt_t",
            "eng",
            Loop::Evolve,
        ))
        .unwrap();
        let skills = config.write_skills("skill text").unwrap();
        assert_eq!(skills.parent(), config.path.parent());
        assert_eq!(std::fs::read_to_string(&skills).unwrap(), "skill text");
        assert_eq!(
            std::fs::metadata(&skills).unwrap().permissions().mode() & 0o777,
            0o600
        );
        drop(config);
        assert!(!skills.exists());
    }

    #[test]
    fn the_claude_command_has_the_pinned_shape() {
        let command = claude_command(&pinned_run(Some("sonnet")));
        assert_eq!(command.get_program(), "/usr/local/bin/claude");
        let args: Vec<String> = command
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            [
                "--mcp-config",
                "/tmp/run/mcp.json",
                "--strict-mcp-config",
                "--setting-sources",
                "",
                "--tools",
                "",
                "--allowedTools",
                "mcp__crystalline",
                "--disallowedTools",
                "mcp__crystalline__delete_engram,mcp__crystalline__move_engram,\
                 mcp__crystalline__remove_domain,mcp__crystalline__add_domain,\
                 mcp__crystalline__configure,mcp__crystalline__provision,\
                 mcp__crystalline__share_changes,mcp__crystalline__update_domain,\
                 mcp__crystalline__resolve_conflict,mcp__crystalline__withdraw_proposal,\
                 mcp__crystalline__discard_changes",
                "--output-format",
                "stream-json",
                "--verbose",
                "--max-turns",
                "60",
                "--model",
                "sonnet",
                "-p",
                "the prompt",
            ]
        );
        let without: Vec<String> = claude_command(&pinned_run(None))
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(!without.iter().any(|a| a == "--model"), "{without:?}");
        assert_eq!(without[without.len() - 2..], ["-p", "the prompt"]);
    }

    #[test]
    fn the_disallowed_tools_are_every_headless_tool_by_its_mcp_name() {
        let list = disallowed_tools(HEADLESS_DISALLOWED_TOOLS);
        let names: Vec<&str> = list.split(',').collect();
        assert_eq!(names.len(), HEADLESS_DISALLOWED_TOOLS.len());
        for (full, bare) in names.iter().zip(HEADLESS_DISALLOWED_TOOLS) {
            assert_eq!(*full, format!("mcp__crystalline__{bare}"));
        }
    }

    #[cfg(unix)]
    #[test]
    fn the_mcp_config_is_private_and_goes_away() {
        use std::os::unix::fs::PermissionsExt as _;
        let config = PrivateConfig::write(&remote_mcp_config(
            "https://x",
            "cmt_t",
            "eng",
            Loop::Evolve,
        ))
        .unwrap();
        let path = config.path.clone();
        let dir = path.parent().unwrap().to_path_buf();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(&dir), 0o700);
        assert!(std::fs::read_to_string(&path).unwrap().contains("cmt_t"));
        drop(config);
        assert!(!path.exists() && !dir.exists());
    }
}
