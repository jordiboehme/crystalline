//! The CLI's side of the connected servers: `connect <url>`,
//! `disconnect <name|url>`, a local rename of a mounted domain, and the rows
//! `status` and `doctor` print about every source.

use std::time::{Duration, Instant};

use crystalline_core::config::GlobalConfig;
use crystalline_remote::{
    Connection, HiddenReason, LocalDomain, MountTable, ONE_DOMAIN_LIMIT, OriginIdentity,
    ROUTING_FILE, RemoteFailure, Revocation, SourceRecord, SourceSet, read_cached, remote_dir,
};
use serde::Serialize;
use serde_json::{Value, json};

/// How long `status` and `doctor` wait for one source, everything included:
/// the sign-in read on this machine and the server's answer.
pub const SOURCE_STATUS_LIMIT: Duration = Duration::from_secs(5);

/// This machine's domains as the mount table reads them, from the loaded
/// config (the CLI has no engine at hand for `connect`).
pub fn local_domains_of(cfg: &GlobalConfig) -> Vec<LocalDomain> {
    let api_url = cfg.github.as_ref().and_then(|g| g.api_url.clone());
    cfg.domains
        .iter()
        .map(|(name, entry)| LocalDomain {
            name: name.clone(),
            aliases: entry.aliases.clone(),
            origin: entry
                .origin
                .as_ref()
                .map(|origin| OriginIdentity::of(origin, api_url.as_deref())),
        })
        .collect()
}

/// The warning `connect` prints when the local copy a server replaces holds
/// work the team has not seen (decision D19).
pub fn unshared_warning(local: &str, source: &str, changes: usize) -> String {
    format!(
        "warning: your local copy of '{local}' holds {changes} change(s) you have not shared; share them first: they stay hidden while you are connected to {source}"
    )
}

/// The prefixes of the tokens a Crystalline server issues: a personal MCP
/// token, and an OAuth access and refresh token.
const TOKEN_PREFIXES: [&str; 3] = ["cmt_", "coa_", "cor_"];

/// Whether a command-line word looks like a token a server issued.
pub fn looks_like_token(word: &str) -> bool {
    let word = word.trim();
    TOKEN_PREFIXES.iter().any(|p| word.starts_with(p))
}

/// The refusal of a token written on the command line (ruling F19). It
/// repeats `url` only when it is a server address, and never the token.
pub fn token_refusal(url: Option<&str>, name: Option<&str>) -> String {
    let url = url
        .filter(|u| crystalline_remote::normalize_server_url(u).is_ok())
        .unwrap_or("<url>");
    let name = name.map(|n| format!(" --name {n}")).unwrap_or_default();
    format!(
        "refusing a token on the command line, because it lands in the shell history and the process list; run crystalline connect {url}{name} --token and paste it when asked, or pipe it on stdin"
    )
}

/// What `connect <url>` says about a word after the URL that is not a token.
pub const EXTRA_WORD: &str = "crystalline connect takes one server address and nothing after it; to give the source a name on this machine, use --name <name>";

/// The flags of `connect` and the global ones that take a value as the next
/// word.
const VALUE_FLAGS: [&str; 4] = ["--name", "--db", "--config", "--format"];

/// What `connect` says when `github` came after a flag: clap reads a word
/// after any flag as the server address, so the subcommand must come first.
pub const GITHUB_FIRST: &str = "put github right after connect, before any flag, for example crystalline connect github --json";

/// The first word of `words` that is no flag and no flag's value.
fn first_word(words: &[String]) -> Option<usize> {
    let mut at = 0;
    while at < words.len() {
        let word = &words[at];
        if VALUE_FLAGS.contains(&word.as_str()) {
            at += 2;
        } else if word.starts_with('-') {
            at += 1;
        } else {
            return Some(at);
        }
    }
    None
}

/// The refusal of a token anywhere after `connect`, before clap reads the
/// command line (clap's own errors, and the checks after it, would repeat
/// the value): a word with a token's prefix, as an argument or as a flag's
/// value, or `--token=<value>`. `None` for any other command line, and for
/// `connect github`, whose own `--token` takes a GitHub token.
pub fn inline_token_refusal(args: &[String]) -> Option<String> {
    // Only when `connect` is the command itself, after the global flags.
    let at = first_word(args)?;
    if args[at] != "connect" {
        return None;
    }
    let rest = &args[at + 1..];
    // clap takes `github` as the subcommand only as the very first word;
    // after any flag it is read as the server address.
    if rest.first().map(String::as_str) == Some("github") {
        return None;
    }
    let token = |word: &String| {
        let value = word.split_once('=').map_or(word.as_str(), |(_, v)| v);
        word.starts_with("--token=") || looks_like_token(word) || looks_like_token(value)
    };
    if !rest.iter().any(token) {
        return None;
    }
    if first_word(rest).is_some_and(|i| rest[i] == "github") {
        return Some(GITHUB_FIRST.to_string());
    }
    let url = rest
        .iter()
        .find(|w| crystalline_remote::normalize_server_url(w).is_ok())
        .map(String::as_str);
    Some(token_refusal(url, None))
}

/// The local copies of the same domain `source` hides now: their names on
/// this machine, which can differ from the names of the mounts that hide
/// them.
fn hidden_copies(table: &MountTable, source: &str) -> Vec<String> {
    table
        .shadowed
        .iter()
        .filter(|h| h.source == source && h.reason == HiddenReason::Copy)
        .map(|h| h.local.clone())
        .collect()
}

/// Tell a running daemon that `sources.json` changed, and answer what it
/// said about the names. Best effort: a daemon that is not running reads the
/// file when it starts, and a running one also finds the change on its own
/// within seconds.
async fn reload_daemon() -> Vec<String> {
    let answer =
        crystalline_service::ctl_if_running_passive(json!({ "v": 1, "cmd": "sources_reload" }))
            .await
            .ok()
            .flatten();
    answer
        .and_then(|a| a["announcements"].as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|a| a.as_str().map(str::to_string))
        .collect()
}

/// The refusal of a `connect` address. A url whose path the rules refuse
/// gets the reason, which names the characters a path may use; anything
/// else gets the plain line, so a word that is no address (it may be a
/// token typed in the wrong place) is never repeated.
fn not_a_server_address(url: &str) -> String {
    const LINE: &str = "that is not a server address";
    const EXAMPLE: &str = "give one like https://crystalline.acme.com";
    use crystalline_core::base::{BaseProblem, PublicBase};
    let token_inside = url.split(['/', '?', '#', '=', '&']).any(looks_like_token);
    match PublicBase::parse(url) {
        Err(BaseProblem::Path(problem)) if !token_inside => {
            format!("{LINE}: {}; {EXAMPLE}", problem.sentence())
        }
        _ => format!("{LINE}; {EXAMPLE}"),
    }
}

/// `crystalline connect <url> [--name] [--token]`.
pub async fn connect_server(
    url: String,
    name: Option<String>,
    token: bool,
    choice: crystalline_remote::DomainChoice,
    json: bool,
) -> anyhow::Result<()> {
    use crystalline_remote::{
        connect_with_browser_choosing, connect_with_token_choosing, normalize_server_url,
    };
    // The address first, so a URL that can never work is refused before
    // anyone pastes a token for it; and a value that is no address is never
    // repeated, because it may be a token typed in the wrong place.
    if url == "github" {
        anyhow::bail!(GITHUB_FIRST);
    }
    if looks_like_token(&url) {
        anyhow::bail!(token_refusal(None, name.as_deref()));
    }
    match normalize_server_url(&url) {
        Ok(_) => {}
        Err(crystalline_remote::SignInError::BadUrl(_)) => {
            anyhow::bail!(not_a_server_address(&url))
        }
        Err(other) => return Err(other.into()),
    }
    let loaded = crate::cmd::load(None)?;
    let local = local_domains_of(&loaded.effective);
    let dir = remote_dir()?;
    let connected = if token {
        let pasted = read_token_from_stdin()?;
        connect_with_token_choosing(&url, name.as_deref(), choice, &pasted, &dir, &local).await?
    } else {
        connect_with_browser_choosing(
            &url,
            name.as_deref(),
            choice,
            &dir,
            &local,
            |authorize_url| {
                eprintln!(
                    "Opening your browser to sign in. If it does not open, visit:\n\n  {authorize_url}\n"
                );
                open_in_browser(authorize_url);
            },
            |note| {
                eprintln!("{note}");
                read_token_from_stdin().ok()
            },
        )
        .await?
    };
    let source = &connected.source;
    // The local copies this server hides, by their own names: the
    // announcement names the mount, which may be called differently.
    let table = SourceSet::load(
        dir.clone(),
        local.clone(),
        crystalline_core::secret_env::process_var,
    )
    .table();
    let warnings = match crystalline_core::config::origins_state_dir() {
        Ok(origins) => unshared_warnings(&table, &loaded.effective, &origins, &source.name),
        Err(_) => Vec::new(),
    };
    let _ = reload_daemon().await;
    if json {
        println!(
            "{}",
            json!({
                "name": source.name,
                "url": source.url,
                "account": source.account,
                "kind": source.kind.as_str(),
                "domains": source.domains,
                "taken": connected.taken,
                "not_chosen": connected.not_chosen,
                "not_offered": connected.not_offered,
                "came_back": connected.came_back,
                "announcements": connected.announcements.iter().map(ToString::to_string).collect::<Vec<_>>(),
                "warnings": warnings,
            })
        );
    } else {
        println!(
            "connected to {} as {} ({}); this machine calls it {}",
            source.url,
            source.account,
            source.kind.as_str(),
            source.name
        );
        match connected.taken.as_slice() {
            [] => println!("it takes no domains yet"),
            names => println!("it takes {}", names.join(", ")),
        }
        match connected.not_chosen.len() {
            0 => {}
            1 => println!("it leaves out 1 domain that is not on the list (--json names it)"),
            n => println!("it leaves out {n} domains that are not on the list (--json names them)"),
        }
        for name in &connected.not_offered {
            println!(
                "'{name}' is on the list, but the server does not offer it to {} (missing rights, or not there yet); it is taken once the server offers it",
                source.account
            );
        }
        for said in &connected.announcements {
            println!("{said}");
        }
        for local in &connected.came_back {
            println!("the local domain '{local}' is visible again");
        }
        for warning in &warnings {
            eprintln!("{warning}");
        }
        println!(
            "your agents now see its domains beside this machine's own; crystalline disconnect {} removes it again",
            source.name
        );
    }
    Ok(())
}

/// One [`unshared_warning`] per local copy `source` hides that holds work
/// the team has not seen, read from its origin state under `origins`.
pub fn unshared_warnings(
    table: &MountTable,
    cfg: &GlobalConfig,
    origins: &std::path::Path,
    source: &str,
) -> Vec<String> {
    hidden_copies(table, source)
        .into_iter()
        .filter_map(|hidden| {
            let root = cfg.domains.get(&hidden)?.file_path()?;
            let work = crystalline_service::unshared_work(&root, &origins.join(&hidden))?;
            (work.count() > 0).then(|| unshared_warning(&hidden, source, work.count()))
        })
        .collect()
}

/// The pasted token: one line from stdin, with a prompt when stdin is a
/// terminal. Never from the command line, where it would land in the shell
/// history and the process list.
pub fn read_token_from_stdin() -> anyhow::Result<String> {
    use std::io::{BufRead, IsTerminal};
    let mut line = String::new();
    if std::io::stdin().is_terminal() {
        // Echo off first, so the prompt says what really happens.
        let echo = EchoOff::new();
        eprint!(
            "Paste the personal MCP token (cmt_...) and press Enter{}: ",
            if echo.is_off() {
                " (it is not shown)"
            } else {
                " (visible while typing)"
            }
        );
        let read = std::io::stdin().lock().read_line(&mut line);
        drop(echo);
        read?;
    } else {
        std::io::stdin().lock().read_line(&mut line)?;
    }
    let token = line.trim().to_string();
    if token.is_empty() {
        anyhow::bail!("no token was given; issue one in Fluid under profile > Agent access");
    }
    Ok(token)
}

/// The terminal on stdin stops echoing what is typed while this lives, so a
/// pasted token stays off the screen and out of the scrollback. Unix only;
/// elsewhere it does nothing and the prompt says the paste is visible.
struct EchoOff {
    #[cfg(unix)]
    saved: Option<libc::termios>,
}

impl EchoOff {
    fn new() -> EchoOff {
        #[cfg(unix)]
        {
            // SAFETY: tcgetattr and tcsetattr on stdin with a termios this
            // function owns; a failure leaves the terminal as it was.
            unsafe {
                let mut term: libc::termios = std::mem::zeroed();
                if libc::tcgetattr(libc::STDIN_FILENO, &mut term) != 0 {
                    return EchoOff { saved: None };
                }
                let saved = term;
                // No echo, but the Enter still moves to the next line.
                term.c_lflag &= !libc::ECHO;
                term.c_lflag |= libc::ECHONL;
                if libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &term) != 0 {
                    return EchoOff { saved: None };
                }
                EchoOff { saved: Some(saved) }
            }
        }
        #[cfg(not(unix))]
        EchoOff {}
    }
}

impl EchoOff {
    /// Whether echo is off now.
    fn is_off(&self) -> bool {
        #[cfg(unix)]
        return self.saved.is_some();
        #[cfg(not(unix))]
        false
    }
}

impl Drop for EchoOff {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(saved) = &self.saved {
            // SAFETY: puts back the settings read in `new`.
            unsafe {
                libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, saved);
            }
        }
    }
}

/// Open `url` in the default browser, best effort: the URL is printed first
/// either way.
pub fn open_in_browser(url: &str) {
    #[cfg(target_os = "macos")]
    let opened = std::process::Command::new("open").arg(url).spawn();
    #[cfg(target_os = "windows")]
    let opened = std::process::Command::new("rundll32")
        .args(["url.dll,FileProtocolHandler", url])
        .spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let opened = std::process::Command::new("xdg-open").arg(url).spawn();
    if let Err(e) = opened {
        tracing::debug!("could not open a browser: {e}");
    }
}

/// `crystalline disconnect <name|url>`.
pub async fn disconnect_server(target: String, json: bool) -> anyhow::Result<()> {
    // The overlay load first, like every other command: a refused environment
    // (both forms of a secret set) surfaces here instead of reading as unset
    // below.
    crate::cmd::load(None)?;
    let dir = remote_dir()?;
    let Some(gone) = crystalline_remote::disconnect(&dir, &target).await? else {
        let taken = crystalline_remote::load_sources(&dir)
            .map(|f| f.names())
            .unwrap_or_default();
        let env = crystalline_remote::env_source(crystalline_core::secret_env::process_var, &taken);
        if env.is_some_and(|e| e.url == target || e.name == target) {
            anyhow::bail!(
                "this source comes from CRYSTALLINE_REMOTE_URL and CRYSTALLINE_REMOTE_TOKEN; unset them to remove it"
            );
        }
        anyhow::bail!("no connected server is called {target}; crystalline status lists them");
    };
    if let Some(note) = gone.note() {
        eprintln!("note: {note}");
    }
    let said = reload_daemon().await;
    if json {
        println!(
            "{}",
            json!({ "disconnected": gone.name, "url": gone.url, "revoked": gone.revocation == Revocation::Revoked })
        );
    } else {
        println!(
            "disconnected from {} ({}); its domains are gone from this machine",
            gone.name, gone.url
        );
        for line in said {
            println!("{line}");
        }
    }
    Ok(())
}

/// `crystalline domain rename <domain> <new> [--local]` when `domain` is a
/// mounted domain. `None` when it is not one, and the ordinary rename runs.
pub async fn rename_mounted(
    domain: &str,
    new: &str,
    local: bool,
    json: bool,
) -> Option<anyhow::Result<()>> {
    let dir = remote_dir().ok()?;
    let loaded = match crate::cmd::load(None) {
        Ok(loaded) => loaded,
        Err(e) => {
            // A mounted name gets the configuration's own words, not the
            // engine's rename refusal further on; any other name goes on to
            // the ordinary rename, which a running daemon may still answer.
            let set = SourceSet::load(dir, Vec::new(), crystalline_core::secret_env::process_var);
            set.table().mount(domain)?;
            return Some(Err(e.context(format!(
                "'{domain}' comes from a connected server, and its name on this machine cannot change while this machine's configuration does not load"
            ))));
        }
    };
    let set = SourceSet::load(
        dir,
        local_domains_of(&loaded.effective),
        crystalline_core::secret_env::process_var,
    );
    let mount = set.table().mount(domain).cloned()?;
    if !local {
        return Some(Err(anyhow::anyhow!(
            "'{domain}' comes from {}; only its name on this machine can change here: add --local",
            mount.source
        )));
    }
    if let Err(why) = set.rename_local(domain, new) {
        return Some(Err(anyhow::anyhow!(why)));
    }
    let _ = reload_daemon().await;
    if json {
        println!(
            "{}",
            json!({ "renamed": domain, "to": new, "source": mount.source, "local": true })
        );
    } else {
        println!(
            "'{domain}' from {} is called '{new}' on this machine now",
            mount.source
        );
    }
    Some(Ok(()))
}

/// One mounted domain in a [`SourceRow`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct MountRow {
    /// Its name on this machine.
    pub local: String,
    /// Its name on the server.
    pub remote: String,
    /// Whether it is the same domain as a local copy, which is hidden.
    pub replaces_local: bool,
}

/// A domain a source offers that is left out: an earlier source offers the
/// same domain, or its name cannot name a domain on this machine.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SkippedRow {
    /// Its name on the later source, escaped when it cannot name a domain.
    pub remote: String,
    /// The source whose copy is used; empty when the name is the reason.
    pub kept_by: String,
    /// `same_domain` or `invalid_name`.
    pub reason: String,
    /// What `status` and `doctor` say about it, in a fixed sentence.
    pub note: String,
}

/// What `status` and `doctor` say about a domain a source offers and this
/// machine leaves out.
pub fn skipped_note(skipped: &crystalline_remote::Skipped) -> String {
    match skipped.reason {
        crystalline_remote::SkipReason::SameDomain => format!(
            "left out: '{}' ({} offers the same domain)",
            skipped.remote, skipped.kept_by
        ),
        crystalline_remote::SkipReason::InvalidName => format!(
            "left out: '{}' (its name cannot name a domain on this machine)",
            skipped.remote
        ),
        crystalline_remote::SkipReason::NotChosen => format!(
            "left out: '{}' (not on the list of domains this machine takes)",
            skipped.remote
        ),
    }
}

/// One local domain a source hides.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct HiddenRow {
    /// The local domain.
    pub local: String,
    /// `"copy"`: the same domain as a mount, which is used instead.
    /// `"collision"`: a different domain under a name the source gave out
    /// first.
    pub reason: String,
    /// The local name of the mount that hides it.
    pub by: String,
    /// What it means and the way out, in plain words.
    pub note: String,
}

/// One source, as `status` and `doctor` report it.
#[derive(Debug, Clone, Default, Serialize)]
pub struct SourceRow {
    pub name: String,
    pub url: String,
    pub account: String,
    pub kind: String,
    pub from_env: bool,
    /// `"keyring"`, `"file"` or `"environment"`.
    pub credential_store: Option<String>,
    /// When the access token in hand expires, for a browser sign-in.
    pub expires_at: Option<String>,
    /// Whether the server answered this run (also true when it answered
    /// with a refusal).
    pub reachable: bool,
    /// Why this run got no answer, in this machine's words: the source, its
    /// address, the likely cause, and that it recovers by itself.
    pub error: Option<String>,
    /// What the server itself wrote beside `error`, for a person reading
    /// `status` or `doctor` only.
    pub error_detail: Option<String>,
    /// When the server last confirmed the domain list this machine uses.
    pub fetched_at: Option<String>,
    /// Why the last try to refresh the domain list failed: the running
    /// daemon's record, or the cached one.
    pub failure: Option<String>,
    /// What the server wrote beside `failure`, from the running daemon.
    pub failure_detail: Option<String>,
    /// Every mounted domain.
    pub mounts: Vec<MountRow>,
    /// The local domains this source hides, by name.
    pub hidden_local: Vec<String>,
    /// The same, with why and the way out.
    pub shadowed: Vec<HiddenRow>,
    /// The domains left out because an earlier source offers them.
    pub skipped: Vec<SkippedRow>,
    /// The list of domains this source takes, by their names on the server;
    /// `None` takes all.
    pub domains: Option<Vec<String>>,
    /// Offered domains not on the list.
    pub not_chosen: Vec<String>,
    /// Listed names the server does not offer.
    pub not_offered: Vec<String>,
}

/// The table's part of one source's row: its names and what it hides.
fn fill_names(row: &mut SourceRow, table: &MountTable) {
    row.mounts = table
        .of_source(&row.name)
        .map(|m| MountRow {
            local: m.local.clone(),
            remote: m.remote.clone(),
            replaces_local: m.replaces_local,
        })
        .collect();
    row.shadowed = table
        .shadowed
        .iter()
        .filter(|h| h.source == row.name)
        .map(|h| HiddenRow {
            local: h.local.clone(),
            reason: match h.reason {
                HiddenReason::Copy => "copy",
                HiddenReason::Collision => "collision",
            }
            .to_string(),
            by: h.by.clone(),
            note: hidden_note(&h.local, &row.name, h.reason),
        })
        .collect();
    row.hidden_local = row.shadowed.iter().map(|h| h.local.clone()).collect();
    row.skipped = table
        .skipped
        .iter()
        .filter(|s| s.source == row.name && s.reason != crystalline_remote::SkipReason::NotChosen)
        .map(|s| SkippedRow {
            remote: s.remote.clone(),
            kept_by: s.kept_by.clone(),
            reason: match s.reason {
                crystalline_remote::SkipReason::SameDomain => "same_domain",
                crystalline_remote::SkipReason::InvalidName => "invalid_name",
                crystalline_remote::SkipReason::NotChosen => "not_chosen",
            }
            .to_string(),
            note: skipped_note(s),
        })
        .collect();
    row.not_chosen = table
        .skipped
        .iter()
        .filter(|s| s.source == row.name && s.reason == crystalline_remote::SkipReason::NotChosen)
        .map(|s| s.remote.clone())
        .collect();
    row.not_offered = table
        .not_offered
        .iter()
        .filter(|u| u.source == row.name)
        .map(|u| u.remote.clone())
        .collect();
}

/// What `status` and `doctor` say about a hidden local domain (ruling F8
/// REVISED).
pub fn hidden_note(local: &str, source: &str, reason: HiddenReason) -> String {
    let sentence = crystalline_remote::hidden_sentence(&crystalline_remote::Hidden {
        local: local.to_string(),
        source: source.to_string(),
        reason,
        by: local.to_string(),
    });
    match reason {
        HiddenReason::Copy => {
            format!("{sentence}; it cannot be removed or renamed while {source} is connected")
        }
        HiddenReason::Collision => sentence,
    }
}

/// Ask one source for its `status`, within [`SOURCE_STATUS_LIMIT`] for
/// everything: the sign-in read here and the answer there.
async fn ask_one(record: SourceRecord, dir: std::path::PathBuf) -> SourceRow {
    let mut row = SourceRow {
        name: record.name.clone(),
        url: record.url.clone(),
        account: record.account.clone(),
        kind: record.kind.as_str().to_string(),
        from_env: record.from_env,
        ..SourceRow::default()
    };
    if let Some(cached) = read_cached(&record.host_dir(&dir), ROUTING_FILE, &record.account) {
        row.fetched_at = Some(cached.fetched_at.to_rfc3339());
        row.failure = cached.last_failure;
    }
    let timed_out = RemoteFailure::TimedOut {
        source: record.name.clone(),
        url: record.url.clone(),
        after: SOURCE_STATUS_LIMIT,
    };
    let asked = async {
        let opened = tokio::task::spawn_blocking(move || Connection::open(record, &dir))
            .await
            .map_err(|e| RemoteFailure::Credential(e.to_string()))
            .and_then(|opened| opened);
        let connection = match opened {
            Ok(connection) => connection,
            Err(failure) => return (None, None, Err((failure, false))),
        };
        let store = Some(connection.store_kind().to_string());
        let expires_at = connection
            .credential()
            .await
            .and_then(|c| c.expires_at)
            .map(|at| at.to_rfc3339());
        let answer = connection
            .ctl_data(json!({ "v": 1, "cmd": "status" }))
            .await
            .map(|_| ())
            .map_err(|failure| {
                let reachable = !failure.is_unreachable();
                (failure, reachable)
            });
        (store, expires_at, answer)
    };
    match tokio::time::timeout(SOURCE_STATUS_LIMIT, asked).await {
        Ok((store, expires_at, answer)) => {
            row.credential_store = store;
            row.expires_at = expires_at;
            match answer {
                Ok(()) => row.reachable = true,
                Err((failure, reachable)) => {
                    row.reachable = reachable;
                    row.error_detail = failure.server_text().map(str::to_string);
                    row.error = Some(failure.to_string());
                }
            }
        }
        Err(_) => row.error = Some(timed_out.to_string()),
    }
    row
}

/// Every source, each asked for its `status` at once within
/// [`SOURCE_STATUS_LIMIT`], so a source that is down never holds the report
/// up: its row shows the cached names and why it did not answer. Before it
/// returns, a sign-in refresh one of the asks started is waited for with
/// what is left of [`ONE_DOMAIN_LIMIT`], so a token pair the server rotated
/// is saved: up to about 5 s more after the asks, 10 s in all at most.
pub async fn source_rows(cfg: &GlobalConfig) -> Vec<SourceRow> {
    let started = Instant::now();
    let Ok(dir) = remote_dir() else {
        return Vec::new();
    };
    let set = SourceSet::load(
        dir.clone(),
        local_domains_of(cfg),
        crystalline_core::secret_env::process_var,
    );
    let records = set.records();
    if records.is_empty() {
        return Vec::new();
    }
    let table = set.table();
    let mut tasks = tokio::task::JoinSet::new();
    for (index, record) in records.iter().cloned().enumerate() {
        let dir = dir.clone();
        tasks.spawn(async move { (index, ask_one(record, dir).await) });
    }
    let mut rows: Vec<Option<SourceRow>> = vec![None; records.len()];
    while let Some(done) = tasks.join_next().await {
        if let Ok((index, row)) = done {
            rows[index] = Some(row);
        }
    }
    let rows = rows
        .into_iter()
        .zip(records)
        .map(|(row, record)| {
            let mut row = row.unwrap_or_else(|| SourceRow {
                error: Some(format!(
                    "asking {} ({}) stopped before it answered, most likely a fault on this machine rather than the server; it recovers by itself, run crystalline status again",
                    record.name, record.url
                )),
                name: record.name.clone(),
                url: record.url.clone(),
                account: record.account.clone(),
                kind: record.kind.as_str().to_string(),
                from_env: record.from_env,
                ..SourceRow::default()
            });
            fill_names(&mut row, &table);
            row.domains = record.domains.clone();
            row
        })
        .collect();
    crystalline_remote::settle_refreshes(ONE_DOMAIN_LIMIT.saturating_sub(started.elapsed())).await;
    rows
}

/// Add what a running daemon knows about each source to `rows`: its last
/// failure to refresh, and what the server wrote beside it. `sources` is the
/// daemon's `status` answer's `sources` member.
pub fn with_daemon_failures(rows: &mut [SourceRow], sources: &Value) {
    let Some(sources) = sources.as_array() else {
        return;
    };
    for row in rows {
        let Some(daemon) = sources.iter().find(|s| s["name"] == row.name.as_str()) else {
            continue;
        };
        if let Some(failure) = daemon["failure"].as_str() {
            row.failure = Some(failure.to_string());
        }
        if let Some(detail) = daemon["failure_detail"].as_str() {
            row.failure_detail = Some(detail.to_string());
        }
    }
}

/// The human `Sources:` block.
pub fn render_sources(rows: &[SourceRow]) {
    print!("{}", sources_block(rows));
}

/// The human `Sources:` block as text; empty when there are no sources.
pub fn sources_block(rows: &[SourceRow]) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    if rows.is_empty() {
        return out;
    }
    let _ = writeln!(out, "Sources:");
    for row in rows {
        let kind = match row.kind.as_str() {
            "oauth" => "signed in through the browser; renews on its own",
            _ if row.from_env => "a token from the environment",
            _ => "a pasted token",
        };
        let state = match &row.error {
            Some(error) => error.clone(),
            None => "answers".to_string(),
        };
        let _ = writeln!(
            out,
            "  {} {} as {} ({kind}): {state}",
            row.name, row.url, row.account
        );
        match &row.domains {
            None => {
                let _ = writeln!(out, "    takes every domain it offers");
            }
            Some(list) => {
                let _ = writeln!(out, "    takes only {}", list.join(", "));
            }
        }
        if let Some(detail) = &row.error_detail {
            let _ = writeln!(out, "    the server wrote: {detail}");
        }
        if row.error.is_some() {
            match &row.fetched_at {
                Some(at) => {
                    let _ = writeln!(out, "    domains as the server last listed them at {at}");
                }
                None => {
                    let _ = writeln!(
                        out,
                        "    no domain list is cached yet; its domains are unavailable until it answers"
                    );
                }
            }
        }
        if row.error.is_none()
            && let Some(failure) = &row.failure
        {
            let _ = writeln!(out, "    last failure: {failure}");
        }
        if let Some(detail) = &row.failure_detail {
            let _ = writeln!(out, "    the server wrote then: {detail}");
        }
        let names: Vec<String> = row
            .mounts
            .iter()
            .map(|m| {
                if m.local == m.remote {
                    m.local.clone()
                } else {
                    format!("{} ('{}' on {})", m.local, m.remote, row.name)
                }
            })
            .collect();
        if !names.is_empty() {
            let _ = writeln!(out, "    domains: {}", names.join(", "));
        }
        for hidden in &row.shadowed {
            let _ = writeln!(out, "    {}", hidden.note);
        }
        for skipped in &row.skipped {
            let _ = writeln!(out, "    {}", skipped.note);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_backslash_is_named_as_the_reason() {
        let said = not_a_server_address(r"https://kb.example\team");
        assert!(said.contains("backslash"), "{said}");
        assert!(said.starts_with("that is not a server address: "), "{said}");
        let token = not_a_server_address(r"https://kb.example\cmt_0123456789abcdef");
        assert!(
            !token.contains("cmt_"),
            "a token is never repeated: {token}"
        );
    }

    #[test]
    fn the_unshared_warning_says_what_to_do_and_why() {
        assert_eq!(
            unshared_warning("platform", "acme", 3),
            "warning: your local copy of 'platform' holds 3 change(s) you have not shared; share them first: they stay hidden while you are connected to acme"
        );
    }

    #[test]
    fn the_local_domains_carry_their_aliases_and_their_origin() {
        let mut cfg = GlobalConfig::default();
        let mut entry = crystalline_core::config::DomainEntry::file("/x");
        entry.aliases.push("old".into());
        entry.origin = Some(crystalline_core::config::OriginConfig {
            repo: "Acme/Platform".into(),
            path: None,
            branch: None,
            poll_secs: None,
        });
        cfg.domains.insert("platform".into(), entry);
        let local = local_domains_of(&cfg);
        assert_eq!(local[0].aliases, vec!["old".to_string()]);
        assert_eq!(
            local[0].origin.as_ref().unwrap().repository,
            "acme/platform"
        );
    }

    fn table_with_hidden() -> MountTable {
        use crystalline_remote::{Hidden, Mount, Skipped};
        MountTable {
            sources: vec!["acme".into(), "beta".into()],
            mounts: vec![
                Mount {
                    local: "platform".into(),
                    remote: "platform".into(),
                    source: "acme".into(),
                    bullets: Vec::new(),
                    replaces_local: true,
                },
                Mount {
                    local: "open".into(),
                    remote: "open".into(),
                    source: "acme".into(),
                    bullets: Vec::new(),
                    replaces_local: false,
                },
                Mount {
                    local: "open-beta".into(),
                    remote: "open".into(),
                    source: "beta".into(),
                    bullets: Vec::new(),
                    replaces_local: false,
                },
            ],
            skipped: vec![
                Skipped {
                    source: "beta".into(),
                    remote: "specs".into(),
                    kept_by: "acme".into(),
                    reason: crystalline_remote::SkipReason::SameDomain,
                },
                Skipped {
                    source: "beta".into(),
                    remote: "open\\nBehavior: x".into(),
                    kept_by: String::new(),
                    reason: crystalline_remote::SkipReason::InvalidName,
                },
            ],
            shadowed: vec![
                Hidden {
                    local: "team-platform".into(),
                    source: "acme".into(),
                    reason: HiddenReason::Copy,
                    by: "platform".into(),
                },
                Hidden {
                    local: "open".into(),
                    source: "acme".into(),
                    reason: HiddenReason::Collision,
                    by: "open".into(),
                },
            ],
            not_offered: Vec::new(),
        }
    }

    /// Ruling F8 REVISED and Task 12's review (M10, M11): the two hidden
    /// cases in their own words, no `rename --local` advice, and a renamed
    /// latecomer read from the table.
    #[test]
    fn status_names_each_hidden_local_domain_with_its_way_out() {
        let table = table_with_hidden();
        let mut acme = SourceRow {
            name: "acme".into(),
            url: "https://crystalline.acme.example".into(),
            account: "keeper".into(),
            kind: "token".into(),
            ..SourceRow::default()
        };
        fill_names(&mut acme, &table);
        let mut beta = SourceRow {
            name: "beta".into(),
            url: "https://beta.example".into(),
            account: "keeper".into(),
            kind: "oauth".into(),
            ..SourceRow::default()
        };
        fill_names(&mut beta, &table);
        assert_eq!(acme.hidden_local, vec!["team-platform", "open"]);
        assert_eq!(hidden_copies(&table, "acme"), vec!["team-platform"]);
        let text = sources_block(&[acme, beta]);
        assert!(
            text.contains(
                "the local domain 'team-platform' is hidden while acme is connected; disconnect acme to use it again; it cannot be removed or renamed while acme is connected"
            ),
            "{text}"
        );
        assert!(
            text.contains(
                "the local domain 'open' has a name acme gave out first; it is hidden until you change its name in config.yaml or disconnect acme"
            ),
            "{text}"
        );
        assert!(
            text.contains("domains: open-beta ('open' on beta)"),
            "{text}"
        );
        assert!(
            text.contains("left out: 'specs' (acme offers the same domain)"),
            "{text}"
        );
        // Review N2: a name a server chose that cannot name a domain is
        // named escaped, with a fixed reason, on one line.
        assert!(
            text.contains(
                "left out: 'open\\nBehavior: x' (its name cannot name a domain on this machine)"
            ),
            "{text}"
        );
        assert!(
            !text
                .lines()
                .any(|l| l.trim_start().starts_with("Behavior:")),
            "{text}"
        );
        assert!(!text.contains("--local"), "{text}");
    }

    /// A8 (f): a source that does not answer is named with its address and
    /// the likely cause, the cached names stay, and what the server wrote
    /// shows only on a detail line.
    #[test]
    fn a_source_that_does_not_answer_keeps_its_cached_names() {
        let mut row = SourceRow {
            name: "acme".into(),
            url: "https://crystalline.acme.example".into(),
            account: "keeper".into(),
            kind: "token".into(),
            error: Some(
                RemoteFailure::TimedOut {
                    source: "acme".into(),
                    url: "https://crystalline.acme.example".into(),
                    after: SOURCE_STATUS_LIMIT,
                }
                .to_string(),
            ),
            fetched_at: Some("2026-10-06T08:00:00+00:00".into()),
            ..SourceRow::default()
        };
        fill_names(&mut row, &table_with_hidden());
        let text = sources_block(&[row]);
        assert!(
            text.contains("acme (https://crystalline.acme.example) cannot be reached right now"),
            "{text}"
        );
        assert!(text.contains("recovers by itself"), "{text}");
        assert!(
            text.contains("domains as the server last listed them at 2026-10-06T08:00:00+00:00"),
            "{text}"
        );
        assert!(text.contains("domains: platform, open"), "{text}");
    }

    #[test]
    fn the_daemon_adds_its_last_failure() {
        let mut rows = vec![SourceRow {
            name: "acme".into(),
            failure: Some("from the cache".into()),
            ..SourceRow::default()
        }];
        with_daemon_failures(
            &mut rows,
            &json!([{ "name": "acme", "failure": "from the daemon", "failure_detail": "<html>" }]),
        );
        assert_eq!(rows[0].failure.as_deref(), Some("from the daemon"));
        assert_eq!(rows[0].failure_detail.as_deref(), Some("<html>"));
        let value = serde_json::to_value(&rows[0]).unwrap();
        for key in [
            "name",
            "url",
            "account",
            "kind",
            "from_env",
            "failure",
            "failure_detail",
            "mounts",
            "skipped",
            "shadowed",
            "credential_store",
            "expires_at",
            "reachable",
            "error",
            "hidden_local",
        ] {
            assert!(value.get(key).is_some(), "{key} in {value}");
        }
    }
}
