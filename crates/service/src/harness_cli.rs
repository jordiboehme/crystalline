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

/// The transport shape a harness CLI needs, distilled from the artifact json's
/// `server` object. Claude Code takes the raw json; Codex and Copilot take
/// positional command line arguments, so the object is reduced to one of these.
enum ServerShape {
    /// A stdio server: a command, its args and its environment.
    Stdio {
        /// The executable to launch.
        command: String,
        /// The arguments passed to it.
        args: Vec<String>,
        /// `KEY`/`VALUE` environment pairs, in sorted key order.
        env: Vec<(String, String)>,
    },
    /// An HTTP server: its URL and any request headers.
    Http {
        /// The server URL.
        url: String,
        /// `NAME`/`VALUE` header pairs, in sorted key order.
        headers: Vec<(String, String)>,
    },
}

/// Reduce a `server` json object to the [`ServerShape`] a positional CLI needs,
/// or `None` when it names neither a `command` nor a `url`. A `type` of `http`,
/// `streamable-http` or `sse`, or a bare `url` with no `command`, reads as HTTP;
/// anything with a `command` reads as stdio (an entry with no `type` is stdio,
/// the same default the harness CLIs take).
fn parse_server_shape(server_json: &str) -> Option<ServerShape> {
    let value: serde_json::Value = serde_json::from_str(server_json).ok()?;
    let obj = value.as_object()?;
    let ty = obj.get("type").and_then(|t| t.as_str());
    let http = matches!(ty, Some("http") | Some("streamable-http") | Some("sse"))
        || (obj.contains_key("url") && !obj.contains_key("command"));
    if http {
        let url = obj.get("url")?.as_str()?.to_string();
        Some(ServerShape::Http {
            url,
            headers: string_pairs(obj.get("headers")),
        })
    } else {
        let command = obj.get("command")?.as_str()?.to_string();
        let args = obj
            .get("args")
            .and_then(|a| a.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        Some(ServerShape::Stdio {
            command,
            args,
            env: string_pairs(obj.get("env")),
        })
    }
}

/// Read a json object of string values into `(key, value)` pairs. serde_json
/// backs its map with a `BTreeMap`, so the pairs come out in sorted key order -
/// the same stable order the scan's canonical `server` json already uses, so a
/// built argv is deterministic.
fn string_pairs(value: Option<&serde_json::Value>) -> Vec<(String, String)> {
    value
        .and_then(|v| v.as_object())
        .map(|obj| {
            obj.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

/// The argument vector (after the CLI program name) that registers `name` with
/// `server_json` for `harness`, plus any notices for `server` fields this
/// harness's CLI has no flag for (dropped with their names, never silently),
/// or `None` when the server shape cannot be built into a command line for
/// this harness at all. Claude Code takes a user-scope `mcp add-json` with the
/// raw json; Codex and Copilot take positional `mcp add` forms translated from
/// the server object. Kept pure so a test can assert the exact argv and
/// notices without spawning anything.
fn mcp_add_argv(
    harness: HarnessKind,
    name: &str,
    server_json: &str,
) -> Option<(Vec<String>, Vec<String>)> {
    match harness {
        HarnessKind::ClaudeCode => Some((
            ["mcp", "add-json", name, server_json, "--scope", "user"]
                .into_iter()
                .map(String::from)
                .collect(),
            Vec::new(),
        )),
        HarnessKind::Codex => codex_add_argv(name, parse_server_shape(server_json)?),
        HarnessKind::Copilot => copilot_add_argv(name, parse_server_shape(server_json)?),
    }
}

/// Codex `mcp add`: stdio as `mcp add <name> [--env K=V ...] -- <cmd> [args]`,
/// HTTP as `mcp add <name> --url <url>`. Codex exposes no header flag on an
/// HTTP add, so headers cannot be carried: the server still registers, and a
/// notice names the dropped `headers` field and its keys so the drop is never
/// silent.
fn codex_add_argv(name: &str, shape: ServerShape) -> Option<(Vec<String>, Vec<String>)> {
    let mut argv = vec!["mcp".to_string(), "add".to_string(), name.to_string()];
    let mut notices = Vec::new();
    match shape {
        ServerShape::Stdio { command, args, env } => {
            for (key, value) in env {
                argv.push("--env".to_string());
                argv.push(format!("{key}={value}"));
            }
            argv.push("--".to_string());
            argv.push(command);
            argv.extend(args);
        }
        ServerShape::Http { url, headers } => {
            argv.push("--url".to_string());
            argv.push(url);
            if !headers.is_empty() {
                let keys: Vec<&str> = headers.iter().map(|(k, _)| k.as_str()).collect();
                notices.push(format!(
                    "the `headers` field of the MCP server `{name}` ({}) has no Codex CLI flag - registering it without those headers.",
                    keys.join(", ")
                ));
            }
        }
    }
    Some((argv, notices))
}

/// Copilot `mcp add`: stdio as `mcp add <name> [--env K=V ...] -- <cmd> [args]`,
/// HTTP as `mcp add --transport http <name> <url> [--header "K: V" ...]`. Unlike
/// Codex, Copilot does take a `--header` flag, so HTTP headers are carried and
/// nothing is dropped.
fn copilot_add_argv(name: &str, shape: ServerShape) -> Option<(Vec<String>, Vec<String>)> {
    let argv = match shape {
        ServerShape::Stdio { command, args, env } => {
            let mut argv = vec!["mcp".to_string(), "add".to_string(), name.to_string()];
            for (key, value) in env {
                argv.push("--env".to_string());
                argv.push(format!("{key}={value}"));
            }
            argv.push("--".to_string());
            argv.push(command);
            argv.extend(args);
            argv
        }
        ServerShape::Http { url, headers } => {
            let mut argv = vec![
                "mcp".to_string(),
                "add".to_string(),
                "--transport".to_string(),
                "http".to_string(),
                name.to_string(),
                url,
            ];
            for (key, value) in headers {
                argv.push("--header".to_string());
                argv.push(format!("{key}: {value}"));
            }
            argv
        }
    };
    Some((argv, Vec::new()))
}

/// The argument vector (after the CLI program name) that deregisters `name` for
/// `harness`. The remove counterpart of [`mcp_add_argv`]: Claude Code takes a
/// user-scope `mcp remove`; Codex and Copilot take the symmetric `mcp remove
/// <name>` (Codex documents the verb; Copilot's is the symmetric form of its
/// documented `mcp add`).
fn mcp_remove_argv(harness: HarnessKind, name: &str) -> Option<Vec<String>> {
    match harness {
        HarnessKind::ClaudeCode => Some(
            ["mcp", "remove", name, "--scope", "user"]
                .into_iter()
                .map(String::from)
                .collect(),
        ),
        HarnessKind::Codex | HarnessKind::Copilot => Some(
            ["mcp", "remove", name]
                .into_iter()
                .map(String::from)
                .collect(),
        ),
    }
}

/// The exact command a user would run by hand for `argv`: the harness's own
/// plain CLI program followed by the argument vector, mirroring how the cli
/// crate's `install` builds its manual command strings.
fn manual_command(harness: HarnessKind, argv: &[String]) -> String {
    format!("{} {}", harness.cli(), argv.join(" "))
}

/// The real [`McpRunner`]: it registers and deregisters domain-provisioned MCP
/// servers by shelling out to a harness's own CLI. All three harnesses have an
/// arm; a `server` object that names neither a command nor a url cannot be
/// built into a command line and answers [`McpOutcome::Unsupported`]. An add
/// refused because the name already belongs to someone else is read off the
/// CLI's stderr as [`McpOutcome::AlreadyExists`] (matched on the "already
/// exists" phrasing Claude Code prints), so a reconcile leaves that foreign
/// registration untouched; a missing binary or any other non-zero exit becomes
/// [`McpOutcome::Failed`] carrying the exact command to run by hand.
pub struct SystemMcpRunner;

impl McpRunner for SystemMcpRunner {
    fn add(&mut self, harness: HarnessKind, name: &str, server_json: &str) -> McpOutcome {
        let Some((argv, notices)) = mcp_add_argv(harness, name, server_json) else {
            return McpOutcome::Unsupported;
        };
        let manual = manual_command(harness, &argv);
        let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
        match run_harness_cli_output(harness, &refs) {
            CliOutput::Ok => McpOutcome::Applied { notices },
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
            CliOutput::Ok => McpOutcome::applied(),
            CliOutput::NotFound | CliOutput::Failed { .. } => McpOutcome::Failed { manual },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_code_add_argv_is_user_scope_add_json() {
        let (argv, notices) =
            mcp_add_argv(HarnessKind::ClaudeCode, "lighthouse", "{\"type\":\"http\"}").unwrap();
        assert!(notices.is_empty(), "{notices:?}");
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
    fn codex_stdio_add_argv_passes_env_then_command_after_the_separator() {
        let server =
            r#"{"args":["--port","7"],"command":"buoy","env":{"TOKEN":"abc"},"type":"stdio"}"#;
        let (argv, notices) = mcp_add_argv(HarnessKind::Codex, "buoy", server).unwrap();
        assert!(notices.is_empty(), "stdio drops nothing: {notices:?}");
        assert_eq!(
            argv,
            vec![
                "mcp",
                "add",
                "buoy",
                "--env",
                "TOKEN=abc",
                "--",
                "buoy",
                "--port",
                "7"
            ]
        );
        assert_eq!(
            manual_command(HarnessKind::Codex, &argv),
            "codex mcp add buoy --env TOKEN=abc -- buoy --port 7"
        );
    }

    #[test]
    fn codex_http_add_argv_uses_url_and_notices_dropped_headers() {
        // Codex has no header flag on an http add, so the headers object is
        // dropped rather than forged into an unsupported flag - but never
        // silently: the add still goes ahead and a notice names the field,
        // the header keys and the server.
        let server = r#"{"headers":{"Authorization":"Bearer x"},"type":"http","url":"https://example.test/mcp"}"#;
        let (argv, notices) = mcp_add_argv(HarnessKind::Codex, "lighthouse", server).unwrap();
        assert_eq!(
            argv,
            vec![
                "mcp",
                "add",
                "lighthouse",
                "--url",
                "https://example.test/mcp"
            ]
        );
        assert!(
            !argv.iter().any(|a| a.contains("Authorization")),
            "headers never reach the argv"
        );
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(
            notices[0].contains("`headers`")
                && notices[0].contains("Authorization")
                && notices[0].contains("lighthouse"),
            "the notice names the field, the keys and the server: {notices:?}"
        );
    }

    #[test]
    fn copilot_stdio_add_argv_matches_the_codex_stdio_shape() {
        let server = r#"{"command":"buoy","env":{"TOKEN":"abc"},"type":"stdio"}"#;
        let (argv, notices) = mcp_add_argv(HarnessKind::Copilot, "buoy", server).unwrap();
        assert!(notices.is_empty(), "{notices:?}");
        assert_eq!(
            argv,
            vec!["mcp", "add", "buoy", "--env", "TOKEN=abc", "--", "buoy"]
        );
    }

    #[test]
    fn copilot_http_add_argv_uses_transport_flag_and_carries_headers() {
        let server = r#"{"headers":{"Authorization":"Bearer x"},"type":"http","url":"https://example.test/mcp"}"#;
        let (argv, notices) = mcp_add_argv(HarnessKind::Copilot, "lighthouse", server).unwrap();
        assert!(
            notices.is_empty(),
            "headers are carried, nothing drops: {notices:?}"
        );
        assert_eq!(
            argv,
            vec![
                "mcp",
                "add",
                "--transport",
                "http",
                "lighthouse",
                "https://example.test/mcp",
                "--header",
                "Authorization: Bearer x",
            ]
        );
    }

    #[test]
    fn codex_and_copilot_remove_argv_is_the_symmetric_form() {
        for harness in [HarnessKind::Codex, HarnessKind::Copilot] {
            let argv = mcp_remove_argv(harness, "lighthouse").unwrap();
            assert_eq!(argv, vec!["mcp", "remove", "lighthouse"]);
        }
    }

    #[test]
    fn an_unbuildable_server_shape_has_no_argv() {
        // Neither a command nor a url: nothing to register, so no argv.
        assert!(mcp_add_argv(HarnessKind::Codex, "x", r#"{"note":"empty"}"#).is_none());
        assert!(mcp_add_argv(HarnessKind::Copilot, "x", r#"{"note":"empty"}"#).is_none());
    }
}
