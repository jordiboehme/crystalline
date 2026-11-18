//! `crystalline reflect` and `crystalline evolve` run their loop headless:
//! `status` says whether work is due, `run` works one due domain with a
//! Claude Code agent, `watch` polls and runs, and `prompt` prints the shipped
//! prompt for a runner outside the CLI.
//!
//! Every call goes through the automation verbs: to the local daemon over
//! its control socket, or to a server over the ctl route when
//! `CRYSTALLINE_REMOTE_URL` and `CRYSTALLINE_REMOTE_TOKEN` are set (ruling
//! R2). The agent reaches Crystalline the same way: the stdio `crystalline
//! mcp` entry locally, the server's HTTP MCP endpoint with the bearer
//! remotely.
//!
//! A run holds its lease from the take to the record. Every path after the
//! take ends in a record (which releases), so a failure never leaves a lease
//! behind that only the five-minute expiry could clear.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use clap::{Args, Subcommand};
use crystalline_core::automation::{
    DEFAULT_MAX_ITEMS, HEADLESS_DISALLOWED_TOOLS, LEASE_RENEW_SECS, Loop, RunOutcome, RunSummary,
    headless_prompt, parse_run_summary, runner_setup,
};
use crystalline_remote::{Connection, RemoteFailure};
use serde_json::{Value, json};
use tokio::io::AsyncBufReadExt as _;
use tokio::sync::watch;

use crate::ExitWith;

mod claude;
mod client;
mod run;

use claude::*;
use client::*;
use run::*;

/// Where a person installs Claude Code, named when `claude` is missing.
pub const CLAUDE_INSTALL_DOCS: &str = "https://code.claude.com/docs/en/setup";

/// Overrides the lease renew interval in seconds. A test seam: the server's
/// `renew_secs` (60) is the rule.
const RENEW_SECS_ENV: &str = "CRYSTALLINE_LEASE_RENEW_SECS";

/// The prefix Claude Code gives the tools of the MCP server registered as
/// `crystalline`.
const MCP_TOOL_PREFIX: &str = "mcp__crystalline";

/// How long a server may take for one automation verb. A share opens a pull
/// request and an evolve status sweeps the domain, so the ctl default is too
/// short.
const REMOTE_VERB_LIMIT: Duration = Duration::from_secs(300);

/// How long a stopped agent gets to end before it is killed.
const STOP_GRACE: Duration = Duration::from_secs(5);

/// Exit code of `status`, `run` and `watch` when something went wrong, so a
/// shell `if` never reads an error as "nothing due".
const EXIT_ERROR: u8 = 2;

/// Exit code of `status` and `run` when nothing is due or a run already
/// holds the lease.
const EXIT_NOTHING: u8 = 1;

/// The settings every run starts with, for `run` and for each run `watch`
/// starts.
#[derive(Args, Debug, Clone)]
pub struct RunArgs {
    /// The most items the agent works in one run.
    #[arg(long, default_value_t = DEFAULT_MAX_ITEMS)]
    pub max_items: u32,
    /// The most agent turns in one run.
    #[arg(long, default_value_t = 60)]
    pub max_turns: u32,
    /// The model the agent uses. Defaults to Claude Code's own default.
    #[arg(long)]
    pub model: Option<String>,
    /// Where the agent's stream is written, one file per run, readable by
    /// you only. The newest 50 per loop and domain are kept. Defaults to the
    /// runs folder in the state directory.
    #[arg(long)]
    pub log_dir: Option<PathBuf>,
    /// The runner label the lease and the run record show, for example
    /// k8s.
    #[arg(long, default_value = "cli")]
    pub runner: String,
    /// Keep the run's changes local instead of sharing them as a pull
    /// request.
    #[arg(long)]
    pub no_share: bool,
}

/// The four things a loop command does.
#[derive(Subcommand, Debug)]
pub enum LoopCommand {
    /// Show what is due, per domain. Exit 0 when something is due, 1 when
    /// nothing is due, 2 on an error.
    Status {
        /// Only these domains (repeatable). Omit for every domain you may run
        /// the loop on.
        #[arg(long)]
        domain: Vec<String>,
    },
    /// Work what is due with a headless Claude Code agent: take the lease,
    /// run the agent with the shipped prompt, share its changes as a pull
    /// request, record the run and release the lease. Exit 0 when the run is
    /// done or stopped at its item cap, 1 when nothing is due or a run is
    /// already going, 2 when the run failed or on an error.
    Run {
        /// The domain to work. Omit to work every due domain in turn.
        #[arg(long)]
        domain: Option<String>,
        #[command(flatten)]
        args: RunArgs,
        /// Show what would run and stop. Takes no lease and starts no agent.
        #[arg(long)]
        dry_run: bool,
        /// Run even while the loop backs off after failed runs. Nothing else
        /// is skipped: a domain with nothing due or a run going stays as it is.
        #[arg(long)]
        force: bool,
    },
    /// Poll the status and run when something is due, with the same settings
    /// as run. Runs until it gets SIGTERM or Ctrl-C; a run in progress is
    /// stopped and recorded first.
    Watch {
        /// Only these domains (repeatable). Omit for every domain you may run
        /// the loop on.
        #[arg(long)]
        domain: Vec<String>,
        /// Seconds between two status checks.
        #[arg(long, default_value_t = 60)]
        interval: u64,
        #[command(flatten)]
        args: RunArgs,
    },
    /// Print the shipped prompt a headless run starts its agent with, for a
    /// runner outside the CLI. With --json: the prompt, the tools the agent
    /// may not call, this version and their checksum, the answer the
    /// automation_prompt ctl verb gives.
    Prompt {
        /// The domain the prompt names.
        #[arg(long)]
        domain: String,
        /// The item cap the prompt names.
        #[arg(long, default_value_t = DEFAULT_MAX_ITEMS)]
        max_items: u32,
        /// Say the agent's skills are already loaded instead of calling the
        /// skills tool, for a runner that hands them to its agent itself.
        #[arg(long)]
        skills_loaded: bool,
    },
}

/// Run `command` for loop `lp`.
pub async fn run(lp: Loop, command: LoopCommand, json: bool) -> anyhow::Result<()> {
    if let LoopCommand::Prompt {
        domain,
        max_items,
        skills_loaded,
    } = &command
    {
        if json {
            let setup = runner_setup(lp, domain, *max_items, *skills_loaded);
            println!("{}", serde_json::to_string(&setup)?);
        } else {
            print!(
                "{}",
                headless_prompt(lp, domain, *max_items, *skills_loaded)
            );
        }
        return Ok(());
    }
    let code = match dispatch(lp, command, json).await {
        Ok(code) => code,
        Err(e) => {
            eprintln!("Error: {e:#}");
            EXIT_ERROR
        }
    };
    if code == 0 {
        Ok(())
    } else {
        Err(ExitWith(code).into())
    }
}

async fn dispatch(lp: Loop, command: LoopCommand, json: bool) -> anyhow::Result<u8> {
    let client = AutomationClient::from_env().await?;
    match command {
        LoopCommand::Status { domain } => {
            let answer = client.status(lp, &domain, false).await?;
            if json {
                println!("{answer}");
            } else {
                print_status(&answer);
            }
            Ok(if any_due(&answer) { 0 } else { EXIT_NOTHING })
        }
        LoopCommand::Run {
            domain,
            args,
            dry_run,
            force,
        } => {
            let stop = stop_signal();
            let opts = RunOptions {
                args,
                dry_run,
                json,
                force,
            };
            run_due(lp, &client, domain.as_deref(), &opts, &stop).await
        }
        LoopCommand::Watch {
            domain,
            interval,
            args,
        } => {
            let claude = find_claude()?;
            let stop = stop_signal();
            let opts = RunOptions {
                args,
                dry_run: false,
                json,
                force: false,
            };
            watch_loop(lp, &client, &domain, interval.max(1), &claude, &opts, stop).await;
            Ok(0)
        }
        LoopCommand::Prompt { .. } => unreachable!("prompt is answered before any client"),
    }
}

// --- status ----------------------------------------------------------------

fn domains_of(answer: &Value) -> &[Value] {
    answer["domains"].as_array().map_or(&[], Vec::as_slice)
}

fn any_due(answer: &Value) -> bool {
    domains_of(answer).iter().any(|d| d["due"] == true)
}

/// The trigger webhook's state, when the MANIFEST turns it on.
fn delivery_line(delivery: &Value) -> Option<String> {
    let delivery = delivery.as_object()?;
    let configured = delivery.get("configured") == Some(&Value::Bool(true));
    // The recorded error first: with a URL but no secret it says so.
    if let Some(error) = delivery.get("last_error").and_then(Value::as_str) {
        return Some(if configured {
            format!("trigger: last delivery failed: {error}")
        } else {
            format!("trigger: {error}")
        });
    }
    if !configured {
        return Some("trigger: needs a webhook URL and a secret on this instance".to_string());
    }
    Some(match delivery.get("last_sent").and_then(Value::as_str) {
        Some(at) => format!("trigger: last sent {at}"),
        None => "trigger: nothing sent yet".to_string(),
    })
}

fn count_line(counts: &Value) -> String {
    let Some(map) = counts.as_object() else {
        return String::new();
    };
    map.iter()
        .map(|(key, n)| format!("{} {n}", key.replace('_', " ")))
        .collect::<Vec<_>>()
        .join(", ")
}

/// One line per domain, plus the last run when there was one.
fn print_status(answer: &Value) {
    let domains = domains_of(answer);
    if domains.is_empty() {
        println!("No domain to run this loop on.");
        return;
    }
    for d in domains {
        let name = d["domain"].as_str().unwrap_or("?");
        let state = if let Some(home) = d["runs_on"].as_str() {
            format!("runs on {home}")
        } else if let Some(reason) = d["reason"].as_str() {
            reason.to_string()
        } else if d["running"].is_object() {
            format!(
                "running ({} since {})",
                d["running"]["runner"].as_str().unwrap_or("?"),
                d["running"]["taken_at"].as_str().unwrap_or("?")
            )
        } else if d["due"] == true {
            "due".to_string()
        } else {
            "nothing due".to_string()
        };
        let counts = count_line(&d["counts"]);
        if counts.is_empty() {
            println!("{name}: {state}");
        } else {
            println!("{name}: {state} ({counts})");
        }
        let left = d["left_for_person"].as_u64().unwrap_or(0);
        if left > 0 {
            println!("  left for a person: {left}");
        }
        if let Some(line) = delivery_line(&d["delivery"]) {
            println!("  {line}");
        }
        if let Some(run) = d["last_run"].as_object() {
            println!(
                "  last run: {} at {}, {} done",
                run.get("outcome").and_then(Value::as_str).unwrap_or("?"),
                run.get("ended_at").and_then(Value::as_str).unwrap_or("?"),
                run.get("done").and_then(Value::as_u64).unwrap_or(0),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::delivery_line;
    use serde_json::json;

    #[test]
    fn the_delivery_line_says_what_the_trigger_did() {
        assert_eq!(delivery_line(&json!(null)), None);
        assert_eq!(
            delivery_line(&json!({"configured": false, "last_sent": null, "last_error": null}))
                .as_deref(),
            Some("trigger: needs a webhook URL and a secret on this instance")
        );
        assert_eq!(
            delivery_line(&json!({"configured": false, "last_sent": null, "last_error": "no webhook secret is set on this instance, so nothing is sent"}))
                .as_deref(),
            Some("trigger: no webhook secret is set on this instance, so nothing is sent")
        );
        assert_eq!(
            delivery_line(&json!({"configured": true, "last_sent": "2026-10-22T08:00:00Z", "last_error": "the receiver answered 500"}))
                .as_deref(),
            Some("trigger: last delivery failed: the receiver answered 500")
        );
        assert_eq!(
            delivery_line(&json!({"configured": true, "last_sent": "2026-10-22T08:00:00Z", "last_error": null}))
                .as_deref(),
            Some("trigger: last sent 2026-10-22T08:00:00Z")
        );
    }
}
