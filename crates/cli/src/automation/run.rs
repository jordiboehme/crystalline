//! `run` and `watch`: take the lease, run the agent, share, record.
//!
//! The lease is renewed by a future of its own, polled beside the agent's
//! exit, the stop signal and later the share and the record. A renew that
//! hangs therefore never holds up a stop, and the lease stays fresh until
//! the record answers.

use super::*;

/// How long one renew may take. Well under the five minutes after which a
/// lease is stale; a renew that takes longer is retried on the next tick.
const RENEW_LIMIT: Duration = Duration::from_secs(15);

/// How long one record call may take before it is tried again.
const RECORD_LIMIT: Duration = Duration::from_secs(60);

/// Once a stop was seen, the record gets this long in all, so the whole stop
/// (up to 5 s for the agent, 5 s for the rest of its stream, then this) ends
/// inside a 30 s grace period such as Kubernetes gives a pod.
const STOPPED_RECORD_BUDGET: Duration = Duration::from_secs(15);

/// The first try of a record after a stop; a second try gets what is left of
/// [`STOPPED_RECORD_BUDGET`].
const STOPPED_RECORD_TRY: Duration = Duration::from_secs(10);

/// The waits before each new try of a failed record.
const RECORD_WAITS: [Duration; 3] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(4),
];

/// How many run logs are kept per loop and domain, like the run records.
const LOGS_KEPT: usize = crystalline_core::automation::RUNS_KEPT;

/// What `run` and `watch` start every run with.
pub(super) struct RunOptions {
    pub(super) args: RunArgs,
    pub(super) dry_run: bool,
    pub(super) json: bool,
    /// Skip the wait after failed runs (`run --force`).
    pub(super) force: bool,
}

/// The exit code of a recorded run: 2 when it failed, else 0. The agent's
/// own code does not decide it, since `claude` exits 0 on an API error too.
pub(super) fn exit_code(outcome: RunOutcome) -> u8 {
    if outcome == RunOutcome::Failed {
        EXIT_ERROR
    } else {
        0
    }
}

/// Work `domain`, or every due domain in scope: 1 when nothing is due or
/// every due domain is already running, 2 when a run failed, else 0.
pub(super) async fn run_due(
    lp: Loop,
    client: &AutomationClient,
    domain: Option<&str>,
    opts: &RunOptions,
    stop: &watch::Receiver<bool>,
) -> anyhow::Result<u8> {
    let named: Vec<String> = domain.map(str::to_string).into_iter().collect();
    let answer = client.status(lp, &named, opts.force).await?;
    let due: Vec<String> = domains_of(&answer)
        .iter()
        .filter(|d| d["due"] == true)
        .filter_map(|d| d["domain"].as_str().map(str::to_string))
        .collect();
    if due.is_empty() {
        for d in domains_of(&answer) {
            let name = d["domain"].as_str().unwrap_or("?");
            if d["running"].is_object() {
                println!("{name}: already running");
            } else if let Some(home) = d["runs_on"].as_str() {
                println!("{name}: runs on {home}");
            } else if let Some(reason) = d["reason"].as_str() {
                println!("{name}: {reason}");
            }
        }
        println!("Nothing is due.");
        return Ok(EXIT_NOTHING);
    }
    let claude = find_claude()?;
    let mut code = None;
    for name in &due {
        if *stop.borrow() {
            break;
        }
        match run_one(lp, client, name, &claude, opts, stop).await? {
            Ran::Busy => {}
            Ran::Exited(c) => {
                if c != 0 || code.is_none() {
                    code = Some(c);
                }
            }
        }
    }
    Ok(code.unwrap_or(EXIT_NOTHING))
}

/// What one domain's run came to.
pub(super) enum Ran {
    /// The lease was held elsewhere; nothing ran.
    Busy,
    /// The run was recorded; this is its exit code.
    Exited(u8),
}

/// How the agent session ended.
pub(super) enum Ended {
    /// It exited by itself, with this code (`None` when a signal killed it).
    Exited(Option<i32>),
    /// This process got a stop signal and stopped it, or never started it.
    Stopped,
    /// A renew was refused: another run holds the lease now.
    LeaseLost(String),
    /// It could not be started or watched.
    Broken(String),
}

/// Take the lease on `domain`, run the agent, share, record. `Busy` when the
/// take was refused because a run is going or the loop runs elsewhere.
pub(super) async fn run_one(
    lp: Loop,
    client: &AutomationClient,
    domain: &str,
    claude: &Path,
    opts: &RunOptions,
    stop: &watch::Receiver<bool>,
) -> anyhow::Result<Ran> {
    let args = &opts.args;
    let prompt = headless_prompt(lp, domain, args.max_items);
    if opts.dry_run {
        let shown = ClaudeRun {
            program: claude.to_path_buf(),
            mcp_config: PathBuf::from("<mcp config>"),
            prompt: "<prompt>".to_string(),
            max_turns: args.max_turns,
            model: args.model.clone(),
        };
        let mut words = vec![claude.to_string_lossy().into_owned()];
        words.extend(
            claude_args(&shown).into_iter().map(
                |a| {
                    if a.is_empty() { "\"\"".to_string() } else { a }
                },
            ),
        );
        println!("Would run the {lp} loop on {domain}:");
        println!("  {}", words.join(" "));
        return Ok(Ran::Exited(0));
    }
    let take = client.verb(
        "automation_lease_take",
        lp,
        json!({ "domain": domain, "runner": args.runner }),
    );
    let mut stopping = stop.clone();
    let take = tokio::select! {
        take = take => take,
        _ = stopping.wait_for(|stopped| *stopped) => return Ok(Ran::Busy),
    };
    let take = match take {
        Ok(take) => take,
        Err(e) if is_refusal(&e) && is_busy(&e) => {
            println!("{domain}: {e}");
            return Ok(Ran::Busy);
        }
        Err(e) => return Err(e),
    };
    // What the take says about the domain's evolve home: that this
    // instance claimed it, or that every clone may still run evolve.
    if !opts.json
        && let Some(note) = take["note"].as_str()
    {
        println!("{domain}: {note}");
    }
    let token = take["token"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("the lease answer carried no token"))?
        .to_string();
    let period = Duration::from_secs(renew_secs(take["renew_secs"].as_u64()));
    let started = chrono::Utc::now();
    let lease = Lease {
        client,
        lp,
        domain,
        token: &token,
    };
    // A run that could not share its work does not start, unless it was not going
    // to share anyway.
    if !args.no_share
        && let Some(refused) = take["share_refused"].as_str()
    {
        return lease
            .fail_and_err(&started, anyhow::anyhow!("{refused}"), stop.clone())
            .await;
    }

    let (log_path, log) = match open_log(args.log_dir.as_deref(), lp, domain, &started) {
        Ok(opened) => opened,
        Err(e) => return lease.fail_and_err(&started, e, stop.clone()).await,
    };
    let config = match client
        .mcp_config(domain)
        .await
        .and_then(|c| PrivateConfig::write(&c))
    {
        Ok(config) => config,
        Err(e) => return lease.fail_and_err(&started, e, stop.clone()).await,
    };
    if !opts.json {
        println!(
            "Running the {lp} loop on {domain}. The stream goes to {}.",
            log_path.display()
        );
    }
    let run = ClaudeRun {
        program: claude.to_path_buf(),
        mcp_config: config.path.clone(),
        prompt,
        max_turns: args.max_turns,
        model: args.model.clone(),
    };
    let renewer = keep_renewed(&lease, period);
    tokio::pin!(renewer);
    let (ended, last) = run_agent(&run, log, renewer.as_mut(), stop.clone()).await;
    drop(config);
    let renewing = !matches!(ended, Ended::LeaseLost(_));

    let summary = last.as_deref().and_then(parse_run_summary);
    let (outcome, text) = judge(&ended, summary.as_ref(), args.max_items);
    let done = summary.as_ref().map_or(0, |s| s.done);
    let share_it = !args.no_share && matches!(ended, Ended::Exited(_));

    // The share and the record, with the lease kept fresh beside them until
    // the record answers.
    let finish = async {
        let (share, share_error) = if share_it {
            share_changes(&lease, &text, stop.clone()).await
        } else {
            (None, None)
        };
        // A refused share is in the recorded run, first so the summary's cap
        // never cuts it: on a server nobody reads this run's stderr. Anybody
        // who may run the loop reads this, so it is one plain sentence; the
        // reason and its fix are in `share_error` and on stderr for whoever
        // runs this command, and `crystalline doctor` names the fix for an admin.
        let recorded_summary = match &share_error {
            Some(_) => format!("{RUN_NOT_SHARED}\n\n{text}"),
            None => text.clone(),
        };
        let report = json!({
            "started_at": started.to_rfc3339(),
            "outcome": outcome,
            "done": done,
            "summary": recorded_summary,
        });
        (
            share,
            share_error,
            lease.record(&report, stop.clone()).await,
        )
    };
    tokio::pin!(finish);
    let mut lost_at_end = None;
    let (share, share_error, record) = if renewing {
        tokio::select! {
            finished = &mut finish => finished,
            why = renewer.as_mut() => {
                eprintln!("The lease on {domain} was lost while the run ended: {why}");
                lost_at_end = Some(why);
                (&mut finish).await
            }
        }
    } else {
        finish.await
    };
    let lost = match &ended {
        Ended::LeaseLost(why) => Some(why.clone()),
        _ => lost_at_end,
    };
    let record = match (lost, record) {
        (_, Ok(record)) => record,
        (Some(why), Err(_)) => anyhow::bail!(
            "another run holds the lease on {domain} now, so this run was not recorded: {why}"
        ),
        (None, Err(e)) => anyhow::bail!(
            "the run on {domain} could not be recorded: {e:#}. Its lease goes stale by itself within 5 minutes."
        ),
    };
    let exit = exit_code(outcome);
    if opts.json {
        println!(
            "{}",
            json!({
                "domain": domain,
                "loop": lp,
                "exit_code": exit,
                "log": log_path,
                "share": share,
                "share_error": share_error,
                "run": record["run"],
            })
        );
    } else {
        if let Some(url) = share.as_ref().and_then(|s| s["url"].as_str()) {
            println!("Shared the changes for review: {url}");
        }
        println!(
            "Recorded the run on {domain}: {}, {done} done.",
            outcome.as_str()
        );
    }
    Ok(Ran::Exited(exit))
}

/// Share the run's changes as a pull request: the share's answer, or `None`
/// when it failed or a stop signal came first, and for a failure what to record
/// about it.
async fn share_changes(
    lease: &Lease<'_>,
    summary: &str,
    mut stop: watch::Receiver<bool>,
) -> (Option<Value>, Option<String>) {
    let share = lease.client.verb(
        "automation_share",
        lease.lp,
        json!({ "domain": lease.domain, "token": lease.token, "summary": summary }),
    );
    tokio::select! {
        answer = share => match answer {
            Ok(answer) => (Some(answer), None),
            Err(e) => {
                eprintln!("Sharing the run's changes failed: {e:#}");
                let remote = matches!(lease.client, AutomationClient::Remote(_));
                (None, Some(share_failure(lease.domain, &format!("{e:#}"), remote)))
            }
        },
        _ = stop.wait_for(|stopped| *stopped) => {
            eprintln!("Stopped before the run's changes were shared.");
            (None, None)
        }
    }
}

/// What a recorded run says about a share that failed, for everybody who may read
/// it. No command line and no setting: those are for an admin.
const RUN_NOT_SHARED: &str =
    "The changes of this run could not be shared and stay on this instance. An admin can see why.";

/// What the operator of a run reads about a share that failed: that the changes
/// stayed here, why, and the fix. Later runs do not pick these files up,
/// so the last sentence is the way they still reach the team.
///
/// A remote runner's token need not be an admin's, so it reads only the reason,
/// which the server already put in plain words.
fn share_failure(domain: &str, error: &str, remote: bool) -> String {
    if remote {
        return format!(
            "The run's changes were not shared and stay on the server: {error} An admin can share them once that is fixed."
        );
    }
    let mut text = format!("The run's changes were not shared and stay on this machine: {error}");
    if error.contains("personal GitHub identities") {
        text.push_str(
            " An admin can also set github.share_identity to instance, so the instance credential opens the proposals.",
        );
    }
    text.push_str(&format!(
        " Once that is fixed, share them with: crystalline origin share {domain}"
    ));
    text
}

/// A take refusal that means "not now": a run is going, or the loop runs on
/// another instance.
pub(super) fn is_busy(e: &anyhow::Error) -> bool {
    let text = e.to_string();
    text == "already running" || text.starts_with("runs on ")
}

pub(super) fn renew_secs(server: Option<u64>) -> u64 {
    std::env::var(RENEW_SECS_ENV)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .or(server)
        .unwrap_or(LEASE_RENEW_SECS)
        .max(1)
}

/// `call`, tried again after each of `waits` while it fails without a
/// refusal (a daemon that restarts, a network gap). A refusal is the other
/// side's answer and is returned at once.
pub(super) async fn with_retries<T, F, Fut>(waits: &[Duration], mut call: F) -> anyhow::Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<T>>,
{
    let mut waits = waits.iter();
    loop {
        match call().await {
            Ok(answer) => return Ok(answer),
            Err(e) if is_refusal(&e) => return Err(e),
            Err(e) => match waits.next() {
                Some(wait) => {
                    eprintln!("Recording the run failed, trying again: {e:#}");
                    tokio::time::sleep(*wait).await;
                }
                None => return Err(e),
            },
        }
    }
}

/// The outcome a run is recorded with, and its summary text.
///
/// `stopped_at_cap` when the summary's `done` reached the cap, `done` when
/// the agent exited 0 with a summary below it, `failed` otherwise: no
/// summary, an exit code that is not 0, a stop signal or a lost lease. Never
/// `done` without a parsed summary.
pub(super) fn judge(
    ended: &Ended,
    summary: Option<&RunSummary>,
    max_items: u32,
) -> (RunOutcome, String) {
    let counts = summary.map(|s| {
        let mut text = format!(
            "Done: {}. Left for a person: {}. Errors: {}.",
            s.done, s.left, s.errors
        );
        if !s.text.is_empty() {
            text.push_str("\n\n");
            text.push_str(&s.text);
        }
        text
    });
    let why = match ended {
        Ended::Exited(Some(0)) => None,
        Ended::Exited(Some(c)) => Some(format!("The agent exited with code {c}.")),
        Ended::Exited(None) => Some("The agent was killed by a signal.".to_string()),
        Ended::Stopped => Some("The run was stopped by a signal.".to_string()),
        Ended::LeaseLost(_) => Some("Another run took the lease.".to_string()),
        Ended::Broken(e) => Some(format!("The agent could not run: {e}")),
    };
    match (why, summary, counts) {
        (None, Some(s), Some(text)) => {
            let outcome = if s.done >= u64::from(max_items) {
                RunOutcome::StoppedAtCap
            } else {
                RunOutcome::Done
            };
            (outcome, text)
        }
        (None, _, _) => (
            RunOutcome::Failed,
            "The agent ended without a summary block.".to_string(),
        ),
        (Some(why), _, Some(text)) => (RunOutcome::Failed, format!("{why}\n\n{text}")),
        (Some(why), _, None) => (RunOutcome::Failed, why),
    }
}

/// The lease a run holds.
pub(super) struct Lease<'a> {
    client: &'a AutomationClient,
    lp: Loop,
    domain: &'a str,
    token: &'a str,
}

impl Lease<'_> {
    async fn renew(&self) -> anyhow::Result<Value> {
        self.client
            .verb(
                "automation_lease_renew",
                self.lp,
                json!({ "domain": self.domain, "token": self.token }),
            )
            .await
    }

    /// One record call, bounded by `limit`.
    async fn record_once(&self, report: &Value, limit: Duration) -> anyhow::Result<Value> {
        let call = self.client.verb(
            "automation_run_record",
            self.lp,
            json!({ "domain": self.domain, "token": self.token, "report": report }),
        );
        match tokio::time::timeout(limit, call).await {
            Ok(answer) => answer,
            Err(_) => Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "the record took too long",
            )
            .into()),
        }
    }

    /// Record the run (which releases the lease), each try bounded by
    /// [`RECORD_LIMIT`] and tried again after each of [`RECORD_WAITS`].
    /// Once a stop is seen, before or during that, the record gets
    /// [`STOPPED_RECORD_BUDGET`] in all instead.
    async fn record(
        &self,
        report: &Value,
        mut stop: watch::Receiver<bool>,
    ) -> anyhow::Result<Value> {
        if !*stop.borrow() {
            let tries = with_retries(&RECORD_WAITS, || self.record_once(report, RECORD_LIMIT));
            tokio::select! {
                answer = tries => return answer,
                _ = stop.wait_for(|stopped| *stopped) => {}
            }
        }
        self.record_stopped(report).await
    }

    /// The record after a stop: one try of [`STOPPED_RECORD_TRY`], and one
    /// more with what is left of [`STOPPED_RECORD_BUDGET`] when the first
    /// did not reach the other side.
    async fn record_stopped(&self, report: &Value) -> anyhow::Result<Value> {
        let deadline = tokio::time::Instant::now() + STOPPED_RECORD_BUDGET;
        match self.record_once(report, STOPPED_RECORD_TRY).await {
            Ok(answer) => Ok(answer),
            Err(e) if is_refusal(&e) => Err(e),
            Err(e) => {
                let left = deadline.saturating_duration_since(tokio::time::Instant::now());
                if left.is_zero() {
                    return Err(e);
                }
                eprintln!("Recording the run failed, trying once more before stopping: {e:#}");
                self.record_once(report, left).await
            }
        }
    }

    /// Record a failed run (which releases the lease), then return `e`.
    async fn fail_and_err<T>(
        &self,
        started: &chrono::DateTime<chrono::Utc>,
        e: anyhow::Error,
        stop: watch::Receiver<bool>,
    ) -> anyhow::Result<T> {
        let report = json!({
            "started_at": started.to_rfc3339(),
            "outcome": RunOutcome::Failed,
            "done": 0,
            "summary": format!("The run could not start: {e:#}"),
        });
        if let Err(record) = self.record(&report, stop).await {
            eprintln!("Recording the failed run failed too: {record:#}");
        }
        Err(e)
    }
}

/// Renew the lease every `period`, until a renew is refused: then it ends
/// with the refusal. A renew that fails to reach the other side, or takes
/// longer than [`RENEW_LIMIT`], is tried again on the next tick.
async fn keep_renewed(lease: &Lease<'_>, period: Duration) -> String {
    loop {
        tokio::time::sleep(period).await;
        match tokio::time::timeout(RENEW_LIMIT, lease.renew()).await {
            Ok(Ok(_)) => {}
            Ok(Err(e)) if is_refusal(&e) => return e.to_string(),
            Ok(Err(e)) => eprintln!("Renewing the lease failed, trying again: {e:#}"),
            Err(_) => eprintln!("Renewing the lease took too long, trying again."),
        }
    }
}

/// The domain name as it may stand in a file name: letters and digits of
/// any script, `-` and `_`; anything else, a `.` and every path separator
/// among them, becomes `-`.
fn file_safe(domain: &str) -> String {
    domain
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '-' | '_') {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// Create `<log_dir>/<loop>-<domain>-<started>.jsonl`, readable by this
/// user only, the folder made if needed. Only the newest [`LOGS_KEPT`] logs
/// of this loop and domain are kept.
pub(super) fn open_log(
    log_dir: Option<&Path>,
    lp: Loop,
    domain: &str,
    started: &chrono::DateTime<chrono::Utc>,
) -> anyhow::Result<(PathBuf, std::fs::File)> {
    let dir = match log_dir {
        Some(dir) => dir.to_path_buf(),
        None => crystalline_core::config::state_dir()?.join("runs"),
    };
    // Inside an app package a new log under AppData would land in the
    // package's private copy: refused like every other write there.
    crystalline_core::config::check_write(&dir)?;
    std::fs::create_dir_all(&dir)?;
    let prefix = format!("{lp}-{}-", file_safe(domain));
    let path = dir.join(format!(
        "{prefix}{}.jsonl",
        started.format("%Y%m%dT%H%M%S%.3fZ")
    ));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let file = options.open(&path)?;
    prune_logs(&dir, &prefix);
    Ok((path, file))
}

/// Remove all but the newest [`LOGS_KEPT`] logs named `<prefix><time>.jsonl`.
/// The time has no `-`, so another domain whose name starts the same way is
/// never counted.
fn prune_logs(dir: &Path, prefix: &str) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut ours: Vec<String> = entries
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|name| {
            name.strip_prefix(prefix)
                .and_then(|rest| rest.strip_suffix(".jsonl"))
                .is_some_and(|time| {
                    time.starts_with(|c: char| c.is_ascii_digit()) && !time.contains('-')
                })
        })
        .collect();
    ours.sort();
    let extra = ours.len().saturating_sub(LOGS_KEPT);
    for name in &ours[..extra] {
        let _ = std::fs::remove_file(dir.join(name));
    }
}

/// The text of the session's final answer: the `result` event's `result`,
/// else the last assistant message's text.
#[derive(Default)]
pub(super) struct Scan {
    result: Option<String>,
    assistant: Option<String>,
}

impl Scan {
    pub(super) fn read(&mut self, line: &str) {
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            return;
        };
        match event["type"].as_str() {
            Some("result") => {
                if let Some(text) = event["result"].as_str() {
                    self.result = Some(text.to_string());
                }
            }
            Some("assistant") => {
                let text: Vec<&str> = event["message"]["content"]
                    .as_array()
                    .map(|blocks| {
                        blocks
                            .iter()
                            .filter(|b| b["type"] == "text")
                            .filter_map(|b| b["text"].as_str())
                            .collect()
                    })
                    .unwrap_or_default();
                if !text.is_empty() {
                    self.assistant = Some(text.join("\n"));
                }
            }
            _ => {}
        }
    }

    pub(super) fn last(self) -> Option<String> {
        self.result.or(self.assistant)
    }
}

/// A Crystalline credential the agent never needs: the bearer travels in
/// the private MCP config, and the agent's own `crystalline mcp` reaches the
/// daemon over its socket.
pub(super) fn is_credential_var(name: &str) -> bool {
    name.starts_with("CRYSTALLINE_")
        && ["TOKEN", "SECRET", "PASSWORD"]
            .iter()
            .any(|word| name.contains(word))
}

/// Run the agent to its end, writing its stream to `log`, while `renewer`
/// keeps the lease. Returns how it ended and the text of its final answer.
async fn run_agent<R>(
    run: &ClaudeRun,
    log: std::fs::File,
    mut renewer: std::pin::Pin<&mut R>,
    mut stop: watch::Receiver<bool>,
) -> (Ended, Option<String>)
where
    R: std::future::Future<Output = String>,
{
    if *stop.borrow() {
        return (Ended::Stopped, None);
    }
    let mut command = tokio::process::Command::from(claude_command(run));
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .kill_on_drop(true);
    for (name, _) in std::env::vars_os() {
        if name.to_str().is_some_and(is_credential_var) {
            command.env_remove(name);
        }
    }
    #[cfg(unix)]
    command.process_group(0);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => return (Ended::Broken(e.to_string()), None),
    };
    let scan = Arc::new(Mutex::new(Scan::default()));
    let reader = child.stdout.take().map(|stdout| {
        let scan = scan.clone();
        tokio::spawn(async move {
            let mut log = std::io::BufWriter::new(log);
            let mut lines = tokio::io::BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let _ = writeln!(log, "{line}");
                scan.lock().unwrap().read(&line);
            }
            let _ = log.flush();
        })
    });

    let ended = tokio::select! {
        status = child.wait() => match status {
            Ok(status) => Ended::Exited(status.code()),
            Err(e) => Ended::Broken(e.to_string()),
        },
        _ = stop.wait_for(|stopped| *stopped) => {
            stop_child(&mut child).await;
            Ended::Stopped
        }
        why = renewer.as_mut() => {
            eprintln!("The lease was lost: {why}. Stopping the agent.");
            stop_child(&mut child).await;
            Ended::LeaseLost(why)
        }
    };
    // What is left of the stream after the agent ended. A process it left
    // behind may hold the pipe open, so the wait is bounded.
    if let Some(reader) = reader {
        let abort = reader.abort_handle();
        if tokio::time::timeout(STOP_GRACE, reader).await.is_err() {
            abort.abort();
        }
    }
    let last = std::mem::take(&mut *scan.lock().unwrap()).last();
    (ended, last)
}

/// Stop the agent and everything it started: SIGTERM to its process group,
/// then SIGKILL after [`STOP_GRACE`].
async fn stop_child(child: &mut tokio::process::Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id().and_then(|pid| i32::try_from(pid).ok()) {
        // SAFETY: kill(2) with a negative pid signals that process group;
        // the group is the child's own (`process_group(0)`).
        unsafe {
            libc::kill(-pid, libc::SIGTERM);
        }
        if tokio::time::timeout(STOP_GRACE, child.wait()).await.is_ok() {
            // SAFETY: as above; whatever the agent left in its group goes too.
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
            return;
        }
        // SAFETY: as above.
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
        }
    }
    let _ = child.start_kill();
    let _ = child.wait().await;
}

/// Poll every `interval` seconds and run each due domain, until a stop
/// signal. An error is printed and the next poll tries again, so a daemon
/// restart or a network gap does not end a sidecar.
pub(super) async fn watch_loop(
    lp: Loop,
    client: &AutomationClient,
    domains: &[String],
    interval: u64,
    claude: &Path,
    opts: &RunOptions,
    mut stop: watch::Receiver<bool>,
) {
    loop {
        if *stop.borrow() {
            return;
        }
        let status = tokio::select! {
            status = client.status(lp, domains, false) => status,
            _ = stop.wait_for(|stopped| *stopped) => return,
        };
        match status {
            Ok(answer) => {
                let due: Vec<String> = domains_of(&answer)
                    .iter()
                    .filter(|d| d["due"] == true)
                    .filter_map(|d| d["domain"].as_str().map(str::to_string))
                    .collect();
                for domain in due {
                    if *stop.borrow() {
                        return;
                    }
                    if let Err(e) = run_one(lp, client, &domain, claude, opts, &stop).await {
                        eprintln!("Error: {e:#}");
                    }
                }
            }
            Err(e) => eprintln!("Error: {e:#}"),
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(interval)) => {}
            _ = stop.wait_for(|stopped| *stopped) => return,
        }
    }
}

/// A flag that turns true on SIGTERM or Ctrl-C. Only `run` and `watch` set
/// it up: once installed, the signals no longer end the process by
/// themselves.
pub(super) fn stop_signal() -> watch::Receiver<bool> {
    let (tx, rx) = watch::channel(false);
    tokio::spawn(async move {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{SignalKind, signal};
            let Ok(mut term) = signal(SignalKind::terminate()) else {
                return;
            };
            tokio::select! {
                _ = term.recv() => {}
                _ = tokio::signal::ctrl_c() => {}
            }
        }
        #[cfg(not(unix))]
        {
            let _ = tokio::signal::ctrl_c().await;
        }
        let _ = tx.send(true);
    });
    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refused_share_names_why_and_the_way_out() {
        let text = share_failure(
            "kb",
            "This instance shares with personal GitHub identities through its GitHub App.",
            false,
        );
        assert!(text.contains("not shared"), "{text}");
        assert!(text.contains("through its GitHub App"), "{text}");
        assert!(text.contains("github.share_identity to instance"), "{text}");
        assert!(text.ends_with("crystalline origin share kb"), "{text}");
        let other = share_failure("kb", "GitHub could not be reached", false);
        assert!(!other.contains("github.share_identity"), "{other}");
        let remote = share_failure(
            "kb",
            "This instance shares with personal GitHub identities through its GitHub App.",
            true,
        );
        assert!(!remote.contains("github.share_identity"), "{remote}");
        assert!(!remote.contains("crystalline "), "{remote}");
    }

    fn summary(done: u64) -> RunSummary {
        RunSummary {
            done,
            left: 1,
            errors: 0,
            text: "One thread waits.".to_string(),
        }
    }

    #[test]
    fn the_outcome_follows_the_exit_the_summary_and_the_cap() {
        let ok = Ended::Exited(Some(0));
        let (o, text) = judge(&ok, Some(&summary(3)), 20);
        assert_eq!(o, RunOutcome::Done);
        assert_eq!(
            text,
            "Done: 3. Left for a person: 1. Errors: 0.\n\nOne thread waits."
        );
        assert_eq!(
            judge(&ok, Some(&summary(20)), 20).0,
            RunOutcome::StoppedAtCap
        );
        assert_eq!(judge(&ok, None, 20).0, RunOutcome::Failed);
        let (o, text) = judge(&Ended::Exited(Some(3)), Some(&summary(1)), 20);
        assert_eq!(o, RunOutcome::Failed);
        assert!(text.starts_with("The agent exited with code 3."), "{text}");
        assert_eq!(
            judge(&Ended::Stopped, Some(&summary(1)), 20).0,
            RunOutcome::Failed
        );
        assert_eq!(judge(&Ended::Exited(None), None, 20).0, RunOutcome::Failed);
    }

    #[test]
    fn the_final_answer_is_the_result_else_the_last_assistant_text() {
        let mut scan = Scan::default();
        scan.read(r#"{"type":"system","subtype":"init"}"#);
        scan.read(r#"{"type":"assistant","message":{"content":[{"type":"text","text":"first"}]}}"#);
        scan.read(r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"x"}]}}"#);
        scan.read("not json");
        assert_eq!(std::mem::take(&mut scan).last().as_deref(), Some("first"));
        scan.read(r#"{"type":"assistant","message":{"content":[{"type":"text","text":"a"}]}}"#);
        scan.read(r#"{"type":"result","result":"the end","is_error":false}"#);
        assert_eq!(scan.last().as_deref(), Some("the end"));
    }

    #[test]
    fn a_busy_refusal_is_already_running_or_runs_on() {
        assert!(is_busy(&anyhow::anyhow!("already running")));
        assert!(is_busy(&anyhow::anyhow!("runs on team-server")));
        assert!(!is_busy(&anyhow::anyhow!("the lease is gone")));
        assert!(!is_busy(&anyhow::anyhow!("already running, said nobody")));
    }

    fn io_error() -> anyhow::Error {
        std::io::Error::other("the daemon went away").into()
    }

    #[tokio::test]
    async fn a_failed_record_is_tried_again_after_each_wait() {
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let waits = [Duration::ZERO; 3];
        let answer = with_retries(&waits, || async {
            let n = calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if n < 3 { Err(io_error()) } else { Ok(n) }
        })
        .await;
        assert_eq!(answer.unwrap(), 3);
        assert_eq!(calls.into_inner(), 4);
    }

    #[tokio::test]
    async fn a_record_gives_up_after_the_last_wait() {
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let waits = [Duration::ZERO; 3];
        let answer: anyhow::Result<()> = with_retries(&waits, || async {
            calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err(io_error())
        })
        .await;
        assert!(answer.is_err());
        assert_eq!(calls.into_inner(), 4);
    }

    #[tokio::test]
    async fn a_refused_record_is_not_tried_again() {
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let waits = [Duration::ZERO; 3];
        let answer: anyhow::Result<()> = with_retries(&waits, || async {
            calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err(anyhow::anyhow!("the lease is gone"))
        })
        .await;
        assert!(answer.is_err());
        assert_eq!(calls.into_inner(), 1);
    }

    #[test]
    fn the_outcome_failed_always_exits_2() {
        for (ended, summary) in [
            (Ended::Exited(Some(0)), None),
            (Ended::Exited(Some(3)), Some(summary(1))),
            (Ended::Stopped, Some(summary(1))),
        ] {
            let (outcome, _) = judge(&ended, summary.as_ref(), 20);
            assert_eq!(exit_code(outcome), 2);
        }
        assert_eq!(exit_code(RunOutcome::Done), 0);
        assert_eq!(exit_code(RunOutcome::StoppedAtCap), 0);
    }

    #[test]
    fn only_crystalline_credentials_are_kept_from_the_agent() {
        for name in [
            "CRYSTALLINE_REMOTE_TOKEN",
            "CRYSTALLINE_REMOTE_TOKEN_FILE",
            "CRYSTALLINE_GITHUB_TOKEN",
            "CRYSTALLINE_AUTH_OIDC_CLIENT_SECRET",
            "CRYSTALLINE_ADMIN_PASSWORD",
            "CRYSTALLINE_AUTOMATION_WEBHOOK_SECRET_FILE",
        ] {
            assert!(is_credential_var(name), "{name}");
        }
        for name in [
            "ANTHROPIC_API_KEY",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "CRYSTALLINE_REMOTE_URL",
            "CRYSTALLINE_TEST_NO_KEYCHAIN",
            "PATH",
        ] {
            assert!(!is_credential_var(name), "{name}");
        }
    }

    #[test]
    fn a_log_name_keeps_letters_of_any_script_and_no_path_parts() {
        assert_eq!(file_safe("café"), "café");
        assert_ne!(file_safe("café"), file_safe("cafè"));
        assert_eq!(file_safe("team_2-docs"), "team_2-docs");
        for name in ["../up", "a/b", "a\\b", "a..b", ".hidden"] {
            let safe = file_safe(name);
            assert!(
                !safe.contains('/') && !safe.contains('\\') && !safe.contains(".."),
                "{name} became {safe}"
            );
        }
    }
}
