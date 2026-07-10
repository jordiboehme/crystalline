//! Shelling out to a coding harness's own CLI (`claude`, `codex` or
//! `copilot`) to register or deregister the Crystalline MCP server, and to
//! register or deregister a domain-provisioned MCP server through the
//! [`SystemMcpRunner`]. Used by the cli crate's `install`/`uninstall`
//! commands and by the daemon's own artifact provisioning.

use std::process::{Command, Stdio};

use crystalline_core::{HarnessKind, McpOutcome, McpRunner};

/// The outcome of running a harness CLI once.
pub enum CliRun {
    /// The command exited zero.
    Ok,
    /// The command ran but exited non-zero.
    Failed,
    /// The command could not be spawned because the binary is not on PATH.
    NotFound,
}

/// Run a harness CLI, discarding its stdout and stderr so nothing leaks into
/// this command's own output, and never surfacing an error: a spawn failure
/// for a missing binary is reported as [`CliRun::NotFound`], any other spawn
/// or wait failure as [`CliRun::Failed`].
fn run_cli(program: &str, args: &[&str]) -> CliRun {
    match Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
    {
        Ok(status) if status.success() => CliRun::Ok,
        Ok(_) => CliRun::Failed,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => CliRun::NotFound,
        Err(_) => CliRun::Failed,
    }
}

/// Run a harness CLI trying each of its candidate invocations in order: a
/// missing binary moves on to the next candidate, any other outcome is
/// final. Only when every candidate is missing does the whole run read as
/// [`CliRun::NotFound`].
pub fn run_harness_cli(harness: HarnessKind, args: &[&str]) -> CliRun {
    for candidate in harness.cli_invocations() {
        let (program, prefix) = candidate
            .split_first()
            .expect("a candidate invocation always names a program");
        let full: Vec<&str> = prefix.iter().chain(args.iter()).copied().collect();
        match run_cli(program, &full) {
            CliRun::NotFound => continue,
            outcome => return outcome,
        }
    }
    CliRun::NotFound
}

/// The outcome of running a harness CLI once while capturing its stderr, so a
/// caller can tell a harness's own refusal (an "already exists" say) apart
/// from a plain failure. The capturing sibling of [`CliRun`], used by the
/// MCP runner where the difference decides whether a name belongs to someone
/// else.
enum CliOutput {
    /// The command exited zero.
    Ok,
    /// The command ran but exited non-zero; `stderr` is its captured stderr.
    Failed { stderr: String },
    /// The binary is not on PATH.
    NotFound,
}

/// Run a CLI once capturing its output, so its stderr can be inspected. The
/// capturing sibling of [`run_cli`]: stdin is still null, but stdout and
/// stderr are collected rather than discarded.
fn run_cli_output(program: &str, args: &[&str]) -> CliOutput {
    match Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .output()
    {
        Ok(out) if out.status.success() => CliOutput::Ok,
        Ok(out) => CliOutput::Failed {
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => CliOutput::NotFound,
        Err(_) => CliOutput::Failed {
            stderr: String::new(),
        },
    }
}

/// Run a harness CLI capturing its stderr, trying each candidate invocation in
/// order the same way [`run_harness_cli`] does: a missing binary moves on, any
/// other outcome is final, and only an all-missing run reads as
/// [`CliOutput::NotFound`].
fn run_harness_cli_output(harness: HarnessKind, args: &[&str]) -> CliOutput {
    for candidate in harness.cli_invocations() {
        let (program, prefix) = candidate
            .split_first()
            .expect("a candidate invocation always names a program");
        let full: Vec<&str> = prefix.iter().chain(args.iter()).copied().collect();
        match run_cli_output(program, &full) {
            CliOutput::NotFound => continue,
            outcome => return outcome,
        }
    }
    CliOutput::NotFound
}

/// The argument vector (after the CLI program name) that registers `name` with
/// `server_json` for `harness`, or `None` when this harness has no MCP runner
/// arm yet. Claude Code takes a user-scope `mcp add-json`; Codex and Copilot
/// gain their own arms in a later milestone. Kept pure so a test can assert the
/// exact argv without spawning anything.
fn mcp_add_argv(harness: HarnessKind, name: &str, server_json: &str) -> Option<Vec<String>> {
    match harness {
        HarnessKind::ClaudeCode => Some(
            ["mcp", "add-json", name, server_json, "--scope", "user"]
                .into_iter()
                .map(String::from)
                .collect(),
        ),
        HarnessKind::Codex | HarnessKind::Copilot => None,
    }
}

/// The argument vector (after the CLI program name) that deregisters `name` for
/// `harness`, or `None` when this harness has no MCP runner arm yet. The remove
/// counterpart of [`mcp_add_argv`].
fn mcp_remove_argv(harness: HarnessKind, name: &str) -> Option<Vec<String>> {
    match harness {
        HarnessKind::ClaudeCode => Some(
            ["mcp", "remove", name, "--scope", "user"]
                .into_iter()
                .map(String::from)
                .collect(),
        ),
        HarnessKind::Codex | HarnessKind::Copilot => None,
    }
}

/// The exact command a user would run by hand for `argv`: the harness's own
/// plain CLI program followed by the argument vector, mirroring how the cli
/// crate's `install` builds its manual command strings.
fn manual_command(harness: HarnessKind, argv: &[String]) -> String {
    format!("{} {}", harness.cli(), argv.join(" "))
}

/// The real [`McpRunner`]: it registers and deregisters domain-provisioned MCP
/// servers by shelling out to a harness's own CLI. A harness with no runner arm
/// yet answers [`McpOutcome::Unsupported`]; a Claude Code add refused because
/// the name already belongs to someone else is read off the CLI's stderr as
/// [`McpOutcome::AlreadyExists`], so a reconcile leaves that foreign
/// registration untouched; a missing binary or any other non-zero exit becomes
/// [`McpOutcome::Failed`] carrying the exact command to run by hand.
pub struct SystemMcpRunner;

impl McpRunner for SystemMcpRunner {
    fn add(&mut self, harness: HarnessKind, name: &str, server_json: &str) -> McpOutcome {
        let Some(argv) = mcp_add_argv(harness, name, server_json) else {
            return McpOutcome::Unsupported;
        };
        let manual = manual_command(harness, &argv);
        let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
        match run_harness_cli_output(harness, &refs) {
            CliOutput::Ok => McpOutcome::Applied,
            CliOutput::NotFound => McpOutcome::Failed { manual },
            CliOutput::Failed { stderr } => {
                if stderr.to_lowercase().contains("already exists") {
                    McpOutcome::AlreadyExists
                } else {
                    McpOutcome::Failed { manual }
                }
            }
        }
    }

    fn remove(&mut self, harness: HarnessKind, name: &str) -> McpOutcome {
        let Some(argv) = mcp_remove_argv(harness, name) else {
            return McpOutcome::Unsupported;
        };
        let manual = manual_command(harness, &argv);
        let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
        match run_harness_cli_output(harness, &refs) {
            CliOutput::Ok => McpOutcome::Applied,
            CliOutput::NotFound | CliOutput::Failed { .. } => McpOutcome::Failed { manual },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_code_add_argv_is_user_scope_add_json() {
        let argv =
            mcp_add_argv(HarnessKind::ClaudeCode, "lighthouse", "{\"type\":\"http\"}").unwrap();
        assert_eq!(
            argv,
            vec![
                "mcp",
                "add-json",
                "lighthouse",
                "{\"type\":\"http\"}",
                "--scope",
                "user",
            ]
        );
        assert_eq!(
            manual_command(HarnessKind::ClaudeCode, &argv),
            "claude mcp add-json lighthouse {\"type\":\"http\"} --scope user"
        );
    }

    #[test]
    fn claude_code_remove_argv_is_user_scope_remove() {
        let argv = mcp_remove_argv(HarnessKind::ClaudeCode, "lighthouse").unwrap();
        assert_eq!(argv, vec!["mcp", "remove", "lighthouse", "--scope", "user"]);
        assert_eq!(
            manual_command(HarnessKind::ClaudeCode, &argv),
            "claude mcp remove lighthouse --scope user"
        );
    }

    #[test]
    fn codex_and_copilot_have_no_mcp_runner_arm_yet() {
        for harness in [HarnessKind::Codex, HarnessKind::Copilot] {
            assert!(mcp_add_argv(harness, "lighthouse", "{}").is_none());
            assert!(mcp_remove_argv(harness, "lighthouse").is_none());
        }
    }
}
