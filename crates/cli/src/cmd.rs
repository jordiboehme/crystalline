//! Implementations of the data and domain-management subcommands.
//!
//! Every one of these that touches the derived index reaches it through
//! [`reach_index`] and nowhere else: a running daemon owns the index file, so a
//! verb that opens the database on its own answers a person with a lock error
//! on exactly the machines the daemon is there to serve. `crates/cli/tests/index_access.rs`
//! guards the rule.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, anyhow, bail};
use crystalline_core::config::{
    self, DatabaseBackend, DomainEntry, EmbeddingsConfig, GlobalConfig,
};
use crystalline_index::{
    ChunkParams, DomainKind, NoReindexHooks, RebuildKind, Store, apply_scan, configured_model_id,
    download_local_model, provider_from_config, reindex_domains, resolve_forward_refs,
    run_embedding_pass, scan_domain,
};
use tokio::sync::Mutex as TokioMutex;

/// The embeddings config to use: the configured one, or the local default.
fn embeddings_config(cfg: &GlobalConfig) -> EmbeddingsConfig {
    cfg.embeddings.clone().unwrap_or_else(|| EmbeddingsConfig {
        provider: "local".to_string(),
        model: crystalline_index::embed::DEFAULT_MODEL_ID.to_string(),
        endpoint: None,
        api_key_env: None,
    })
}

/// Chunk parameters fingerprinted for the active model, so chunks written at
/// sync time match the provider that later embeds them.
fn chunk_params(cfg: &GlobalConfig) -> ChunkParams {
    ChunkParams::for_model(configured_model_id(cfg.embeddings.as_ref()))
}

/// Build the provider and embed every chunk that needs it, printing one progress
/// line per batch to stderr.
async fn embed_pass(store: &dyn Store, cfg: &GlobalConfig) -> Result<()> {
    let ecfg = embeddings_config(cfg);
    let provider = provider_from_config(&ecfg).await.map_err(|e| {
        anyhow!(
            "could not initialize the '{}' embedding provider: {e}",
            ecfg.provider
        )
    })?;
    let report = run_embedding_pass(store, provider.as_ref(), |done, total| {
        eprintln!("  embedding {done}/{total} chunks");
    })
    .await
    .map_err(|e| anyhow!("embedding failed: {e}"))?;
    if report.chunks == 0 {
        eprintln!("  embeddings already up to date");
    } else {
        eprintln!(
            "  embedded {} chunks in {} batches with model '{}'",
            report.chunks,
            report.batches,
            provider.model_id()
        );
    }
    Ok(())
}

/// Load the effective config through the service's single load chokepoint: the
/// config file resolved from the `--config` override, then `CRYSTALLINE_CONFIG`,
/// then the default global path, with the environment overlay parsed and
/// applied. Readers use `loaded.effective`; the file mutators (`domain add`,
/// `domain remove`) mutate `loaded.file` and save it back to `loaded.path`, so
/// no environment value ever bakes into `config.yaml`.
pub(crate) fn load(config_override: Option<&Path>) -> Result<crystalline_service::LoadedConfig> {
    crystalline_service::overlay::load(config_override)
}

/// Resolve the index database path from an optional override.
pub(crate) fn db_path(override_path: Option<&Path>) -> Result<PathBuf> {
    match override_path {
        Some(p) => Ok(p.to_path_buf()),
        None => config::index_db_path()
            .map_err(|e| anyhow!("could not resolve the default database path: {e}")),
    }
}

// --- one way to reach the index -----------------------------------------------

/// How a verb wants the index opened when it opens one directly.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum OpenAs {
    /// A read. On the Turso backend an absent index file means nothing has
    /// been synced on this machine yet, and the verb answers that rather than
    /// creating an empty database to read no rows from.
    Read,
    /// A write. The index is created when it is not there yet.
    Write,
    /// `reindex --wipe`'s corruption-recovery open, which rebuilds a Turso
    /// database that will not open at all. Creates like [`OpenAs::Write`],
    /// and a no-op distinction on Postgres, which has no local file.
    Rebuild,
}

/// Where a CLI verb reached the index, and how.
pub(crate) enum IndexRoute {
    /// A running daemon owns the index and answered the verb's control
    /// request; this is its reply, in the daemon's own JSON.
    Daemon(serde_json::Value),
    /// No daemon was asked, or none answered, and the index opened here.
    Direct(Arc<TokioMutex<dyn Store>>),
    /// There is no index on this machine yet, so there is nothing to read.
    /// Only an [`OpenAs::Read`] ever lands here; a write creates the file.
    Absent(PathBuf),
    /// There is an index and this command could not reach it: a daemon owns
    /// the file and answered nothing, or the open failed outright. The string
    /// says which, naming the daemon and a remedy, and carries the raw error
    /// at its end rather than on its own.
    Unreachable(String),
}

/// The one way a CLI verb reaches the index.
///
/// Ask a running daemon first, since on any machine with one the daemon owns
/// the index file and a second opener gets a lock error rather than an answer;
/// open the index directly when there is no daemon to ask; and where neither
/// is possible say so in words that name the daemon and the remedy, so that a
/// verb which can answer part of its question from configuration alone
/// degrades (see [`domain_list`]) and one which cannot refuses readably (see
/// [`index_unreachable`]).
///
/// `request` is the verb's ctl request, or `None` for a verb whose caller has
/// already asked the daemon under a different verb (`domain add`, which asks
/// for a sync of the one domain it just registered). An explicit `--db` or
/// `--config` override names an exact index the running daemon may not serve,
/// so it bypasses the daemon entirely; that is
/// [`crystalline_service::use_daemon`]'s rule and this is the only place the
/// CLI applies it to the index.
pub(crate) async fn reach_index(
    request: Option<serde_json::Value>,
    cfg: &GlobalConfig,
    config_override: Option<&Path>,
    db_override: Option<&Path>,
    open_as: OpenAs,
) -> Result<IndexRoute> {
    let bypassed = !crystalline_service::use_daemon(db_override, config_override);
    if !bypassed
        && let Some(request) = request
        && let Some(data) = crystalline_service::ctl_if_running(request).await?
    {
        return Ok(IndexRoute::Daemon(data));
    }
    let db = db_path(db_override)?;
    let turso = backend_is_turso(cfg);
    if open_as == OpenAs::Read && turso && !db.exists() {
        return Ok(IndexRoute::Absent(db));
    }
    // Postgres has no local file, so naming one in a failure would point at a
    // path nothing lives at.
    let location = if turso {
        db.display().to_string()
    } else {
        "the configured database".to_string()
    };
    match crystalline_index::open_store(&cfg.database(), db_override, open_as == OpenAs::Rebuild)
        .await
    {
        Ok(store) => Ok(IndexRoute::Direct(store)),
        Err(e) => Ok(IndexRoute::Unreachable(
            crystalline_service::instance::index_unreachable_words(
                &location,
                &e.to_string(),
                bypassed,
            ),
        )),
    }
}

/// The words a listing uses when the daemon it asked answered badly: an error
/// envelope, or a reply that did not arrive whole. Its own sentence rather
/// than the shared lock wording in
/// [`crystalline_service::instance::words_for_holder`], because nothing here
/// is about a lock: the daemon is reachable and the answer is not usable.
fn daemon_answered_badly(what: &str, error: &str) -> String {
    format!(
        "the running Crystalline daemon answered {what} with an error instead of the counts. Look at it with: crystalline doctor --fix, or stop it with: crystalline ctl shutdown and run this again. The daemon reported: {error}"
    )
}

/// The words a listing uses when it never reached a daemon at all: the only
/// other way [`reach_index`] fails is [`db_path`], which resolves the default
/// database location and fails on a home directory it cannot read. Its own
/// sentence rather than [`daemon_answered_badly`]'s, because that one asserts a
/// daemon answered, and sending somebody to `crystalline ctl shutdown` over a
/// failed path resolution points at the wrong machine entirely.
fn listing_not_reached(what: &str, error: &str) -> String {
    format!(
        "{what} could not reach the index, so the per-domain counts are missing. Look at this machine's configuration with: crystalline doctor. The failure was: {error}"
    )
}

/// The opened store a verb needs, or the error it fails with. Total over
/// every route, so no caller has to write a panicking arm for a variant its
/// own call cannot produce: a verb that sent no ctl request never sees
/// `Daemon`, and one whose dispatch already rendered the daemon's reply never
/// reaches here with it, but a future caller that does gets a sentence rather
/// than a crash.
pub(crate) fn local_store(route: IndexRoute, verb: &str) -> Result<Arc<TokioMutex<dyn Store>>> {
    match route {
        IndexRoute::Direct(store) => Ok(store),
        IndexRoute::Absent(db) => Err(index_absent(verb, &db)),
        IndexRoute::Unreachable(why) => Err(index_unreachable(verb, &why)),
        IndexRoute::Daemon(_) => Err(index_unreachable(
            verb,
            "a running Crystalline daemon answered a request this command does not know how to read. Look at it with: crystalline doctor --fix",
        )),
    }
}

/// The error a verb fails with when it needs the index and
/// [`reach_index`] could not reach it. One wording for every verb, so a
/// person meets the same sentence wherever they hit the same state.
pub(crate) fn index_unreachable(verb: &str, reason: &str) -> anyhow::Error {
    anyhow!("`crystalline {verb}` needs the index and could not reach it: {reason}")
}

/// The error a verb fails with when there is no index on this machine at all
/// and it has nothing to answer from.
pub(crate) fn index_absent(verb: &str, db: &Path) -> anyhow::Error {
    anyhow!(
        "`crystalline {verb}` needs the index and there is none at {} yet: nothing has been synced on this machine. Run: crystalline sync",
        db.display()
    )
}

/// Whether the effective backend is the local Turso file (so an absent file
/// means "no index yet"). Postgres has no local file and is always opened.
pub(crate) fn backend_is_turso(cfg: &GlobalConfig) -> bool {
    cfg.database().backend == DatabaseBackend::Turso
}

/// The absolute, tilde-expanded filesystem root a file domain points at, or
/// `None` for a virtual domain (which has no path).
pub(crate) fn resolve_domain_path(entry: &DomainEntry) -> Option<PathBuf> {
    entry.file_path().filter(|_| !entry.is_virtual())
}

// --- relative time -------------------------------------------------------------

/// Bucket a duration in seconds into a compact human string: `42s`, `12m`,
/// `3h` or `4d`. The single-unit sibling of `main.rs`'s `format_uptime`
/// (which keeps its own `3h07m` minute precision for a live daemon's "up"
/// line); this is the coarser granularity a "how long ago" reading wants.
fn humanize_duration(secs: u64) -> String {
    if secs >= 86_400 {
        format!("{}d", secs / 86_400)
    } else if secs >= 3_600 {
        format!("{}h", secs / 3_600)
    } else if secs >= 60 {
        format!("{}m", secs / 60)
    } else {
        format!("{secs}s")
    }
}

/// Render an ISO-8601 timestamp as `<duration> ago` for human output
/// (`5m ago`, `2h ago`, `3d ago`). Used for `status`'s last-sync column, the
/// domain list's host heartbeat and `origin status`'s last-checked line - every
/// place a raw timestamp reads worse to a person than how long ago it was.
/// Falls back to the value unchanged when it does not parse as a timestamp,
/// or names a future instant (clock skew, a deliberately future value, where
/// "ago" would mislead), so a caller can pass any string straight through
/// without an `Option` dance. `--json` output always keeps the raw ISO
/// string; this is for human rendering only.
pub(crate) fn relative_time(value: &str) -> String {
    let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(value) else {
        return value.to_string();
    };
    let secs = (chrono::Utc::now() - parsed.with_timezone(&chrono::Utc)).num_seconds();
    if secs < 0 {
        return value.to_string();
    }
    format!("{} ago", humanize_duration(secs as u64))
}

// --- domain init -------------------------------------------------------------

/// Scaffold a MANIFEST.md at a domain root if one is absent. Never touches the
/// global config.
pub fn domain_init(path: &Path, name: Option<&str>, json: bool) -> Result<()> {
    std::fs::create_dir_all(path)
        .with_context(|| format!("creating domain directory {}", path.display()))?;
    let manifest = path.join("MANIFEST.md");
    let domain_name = name
        .map(str::to_string)
        .or_else(|| path.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "domain".to_string());

    let created = if manifest.exists() {
        false
    } else {
        let today = chrono::Utc::now()
            .date_naive()
            .format("%Y-%m-%d")
            .to_string();
        std::fs::write(
            &manifest,
            crystalline_core::manifest_template(&domain_name, &today),
        )
        .with_context(|| format!("writing {}", manifest.display()))?;
        true
    };

    if json {
        println!(
            "{}",
            serde_json::json!({
                "manifest": manifest.display().to_string(),
                "created": created,
                "name": domain_name,
            })
        );
    } else if created {
        println!("Scaffolded {}", manifest.display());
        println!(
            "Edit the Scope and When to Use sections, then run: crystalline domain add {domain_name} {}",
            path.display()
        );
    } else {
        println!("MANIFEST.md already exists at {}", manifest.display());
    }
    Ok(())
}

// --- domain add --------------------------------------------------------------

/// Register a domain in the global config. Refuses without a MANIFEST.md.
/// Returns the canonicalized domain root; indexing is a separate step (see
/// [`sync_domain_direct`]) so the daemon-dispatch decision stays in `main.rs`,
/// alongside `sync` and `reindex`'s own dispatch.
///
/// An absent `path` defaults to `<domains_root>/<name>` through
/// [`crystalline_service::default_domain_folder`], the same placement rule
/// the MCP `add_domain` tool's local-domain path uses (`Engine::domain_add_local`),
/// so both entry points agree on where an unrooted domain ends up. The
/// default path still needs a pre-scaffolded `MANIFEST.md`, exactly like an
/// explicit path does - `domain add` never auto-creates one, `domain init`
/// does.
pub(crate) fn domain_add_register(
    name: &str,
    path: Option<&Path>,
    config_override: Option<&Path>,
) -> Result<PathBuf> {
    // Mutate the file truth and save it back to the resolved path; the
    // environment overlay is never written. An env-defined domain of the same
    // name is refused: it is managed by its variable, not the config file.
    let loaded = load(config_override)?;
    if let Some(env) = loaded.overlay.env_domain(name) {
        bail!(
            "domain '{name}' is defined by the environment variable {}; unset it to manage this domain in the config file",
            env.var
        );
    }

    let root = match path {
        Some(p) => p.to_path_buf(),
        None => crystalline_service::default_domain_folder(&loaded.effective.domains_root(), name),
    };

    let manifest = root.join("MANIFEST.md");
    if !manifest.exists() {
        bail!(
            "no MANIFEST.md at {}. Run: crystalline domain init {}",
            root.display(),
            root.display()
        );
    }
    let abs = std::fs::canonicalize(&root).unwrap_or_else(|_| root.clone());

    let mut cfg = loaded.file;
    cfg.domains
        .insert(name.to_string(), DomainEntry::file(abs.clone()));
    config::save_yaml(&loaded.path, &cfg)
        .map_err(|e| anyhow!("failed to save config {}: {e}", loaded.path.display()))?;
    Ok(abs)
}

/// Register a virtual domain in the global config (database-backed, no path).
/// Returns the MANIFEST markdown to scaffold into the database.
pub(crate) fn domain_add_register_virtual(
    name: &str,
    config_override: Option<&Path>,
) -> Result<String> {
    let loaded = load(config_override)?;
    if let Some(env) = loaded.overlay.env_domain(name) {
        bail!(
            "domain '{name}' is defined by the environment variable {}; unset it to manage this domain in the config file",
            env.var
        );
    }
    let mut cfg = loaded.file;
    cfg.domains
        .insert(name.to_string(), DomainEntry::virtual_domain());
    config::save_yaml(&loaded.path, &cfg)
        .map_err(|e| anyhow!("failed to save config {}: {e}", loaded.path.display()))?;
    let today = chrono::Utc::now()
        .date_naive()
        .format("%Y-%m-%d")
        .to_string();
    Ok(crystalline_core::manifest_template(name, &today))
}

/// Print the `domain add --virtual` result.
pub(crate) fn print_domain_add_virtual(name: &str, scaffold: &serde_json::Value, json: bool) {
    if json {
        println!(
            "{}",
            serde_json::json!({
                "registered": name,
                "kind": "virtual",
                "manifest": scaffold,
            })
        );
    } else {
        println!("Registered virtual domain '{name}' (database-backed, no files)");
        let created = scaffold
            .get("created")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        if created {
            println!("Scaffolded MANIFEST.md into the database");
        } else {
            println!("MANIFEST.md already present in the database");
        }
        println!("Capture engrams with: crystalline write {name} \"<title>\" --content \"...\"");
    }
}

/// Sync a single, just-registered domain directly and return its report.
/// Parse failures in individual files land in the report's `failed` list
/// rather than aborting; only a harder error (the index cannot be reached,
/// the transaction fails) is propagated.
///
/// Reaches the index with no ctl request of its own: `domain add`'s dispatch
/// has already asked the daemon to sync this one domain and only falls through
/// to here when none answered, so asking a second time would ask the same
/// question twice.
pub(crate) async fn sync_domain_direct(
    name: &str,
    root: &Path,
    config_override: Option<&Path>,
    db_override: Option<&Path>,
) -> Result<crystalline_index::SyncReport> {
    let cfg = load(config_override)?.effective;
    let route = reach_index(None, &cfg, config_override, db_override, OpenAs::Write).await?;
    let store = local_store(route, "domain add")?;
    let params = chunk_params(&cfg);
    // First lock window: resolve the domain id and snapshot its stamps. The scan
    // then runs with no lock held; the second window applies transactionally.
    let (domain, snapshot) = {
        let store = store.lock().await;
        let domain = store
            .upsert_domain(name, Some(&root.to_string_lossy()), DomainKind::File)
            .await?;
        let snapshot = store.file_stamps(domain).await?;
        (domain, snapshot)
    };
    let scan = scan_domain(name, root, snapshot, &params, false).await?;
    let store = store.lock().await;
    apply_scan(&*store, domain, scan)
        .await
        .map_err(|e| anyhow!("sync of '{name}' failed: {e}"))
}

/// Print `domain add`'s combined registration-and-index output.
pub(crate) fn print_domain_add(
    name: &str,
    path: &Path,
    report: &crystalline_index::SyncReport,
    json: bool,
) {
    if json {
        println!(
            "{}",
            serde_json::json!({
                "registered": name,
                "path": path.display().to_string(),
                "synced": true,
                "sync": report,
            })
        );
    } else {
        println!("Registered domain '{name}' at {}", path.display());
        print_report(report);
    }
}

/// Print `domain add --no-sync`'s registration-only output.
pub(crate) fn print_domain_add_no_sync(name: &str, path: &Path, json: bool) {
    if json {
        println!(
            "{}",
            serde_json::json!({
                "registered": name,
                "path": path.display().to_string(),
                "synced": false,
            })
        );
    } else {
        println!("Registered domain '{name}' at {}", path.display());
        println!("Not synced (--no-sync); run: crystalline sync --domain {name}");
    }
}

// --- domain add --origin ------------------------------------------------------

/// Parses a `domain add --origin owner/repo[/subpath...]` value into
/// `(owner/repo, subpath)`. A thin `anyhow` wrapper over the shared
/// [`crystalline_service::parse_origin_spec`] the environment overlay also
/// parses origins through, so the CLI flag and `CRYSTALLINE_DOMAIN_*_ORIGIN`
/// agree on the grammar; the `--origin` framing is re-attached here.
pub(crate) fn parse_origin_spec(spec: &str) -> Result<(String, Option<String>)> {
    crystalline_service::parse_origin_spec(spec).map_err(|e| anyhow!("--origin {e}"))
}

/// Resolves `path` to an absolute path against the current directory,
/// without requiring it to exist (`std::fs::canonicalize` refuses a path
/// that is not there yet, which is exactly the common case for a team
/// domain's destination folder before it has been downloaded into).
pub(crate) fn absolute_path(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    let cwd = std::env::current_dir().context("resolving the current directory")?;
    Ok(cwd.join(path))
}

/// Print `domain add --origin`'s result: the connected team domain, its
/// root, how many engrams it holds and the base commit it is synced to.
/// For a target adopted in place, also how many local files were kept as
/// local changes against the origin.
pub(crate) fn print_origin_add(repo: &str, data: &serde_json::Value, json: bool) {
    if json {
        println!("{data}");
        return;
    }
    if data["already_connected"].as_bool().unwrap_or(false) {
        let name = data["domain"].as_str().unwrap_or("");
        println!("Domain '{name}' is already connected to {repo}");
        println!("  root: {}", data["root"].as_str().unwrap_or(""));
        println!(
            "  {} engrams at {}",
            data["engrams"].as_u64().unwrap_or(0),
            data["base_commit"].as_str().unwrap_or("")
        );
        return;
    }
    let name = data["domain"].as_str().unwrap_or("");
    println!("Connected team domain '{name}' to {repo}");
    println!("  root: {}", data["root"].as_str().unwrap_or(""));
    println!(
        "  {} engrams at {}",
        data["engrams"].as_u64().unwrap_or(0),
        data["base_commit"].as_str().unwrap_or("")
    );
    if data["adopted"].as_bool().unwrap_or(false) {
        let changes = data["local_changes"].as_u64().unwrap_or(0);
        println!(
            "  connected in place: existing files kept, {} added from the origin",
            data["files_added"].as_u64().unwrap_or(0)
        );
        if changes > 0 {
            println!("  {changes} local file(s) differ from the origin, ready to share or update");
        }
    }
    println!("Run: crystalline origin status --domain {name}");
}

// --- origin share, withdraw and resolve ---------------------------------------

/// Print `origin share`'s result: the proposal URL and change summary when
/// one was opened or an already-open one was updated in place, the friendly
/// "nothing to share" line when the team already has everything the domain
/// knows, the refusal when a reviewer amended the proposal's branch, or
/// (when conflicts are still pending) every conflicting path plus a pointer
/// at `origin resolve`. A proposal that stands in a chain of two or more open
/// layers also says where it sits, through [`stack_line`]; a share that also
/// carried refreshed folder listings says so in one line of its own, through
/// [`change_count_lines`]. A direct domain's commit is reported with its
/// branch and page (`committed`), and so are the three direct refusals: a
/// proposal still open, a branch whose rules refuse the commit, a branch that
/// moved twice.
pub(crate) fn print_origin_share(domain: &str, data: &serde_json::Value, json: bool) {
    if json {
        println!("{data}");
        return;
    }
    for line in origin_share_lines(domain, data) {
        println!("{line}");
    }
}

/// The lines [`print_origin_share`] prints, built rather than printed so the
/// wording of every outcome is a unit test rather than a subprocess.
pub(crate) fn origin_share_lines(domain: &str, data: &serde_json::Value) -> Vec<String> {
    let empty = Vec::new();
    let mut lines: Vec<String> = Vec::new();
    match data["outcome"].as_str().unwrap_or("") {
        "proposed" => {
            lines.push(format!(
                "Opened proposal: {}",
                data["url"].as_str().unwrap_or("")
            ));
            if let Some(summary) = data["summary"].as_str() {
                lines.push(format!("  {summary}"));
            }
            lines.extend(change_count_lines(data));
            if let Some(line) = stack_line(data) {
                lines.push(format!("  {line}"));
            }
            lines.extend(skipped_large_lines(&data["skipped_large"]));
        }
        "updated" => {
            let prop = &data["proposal"];
            lines.push(format!(
                "Updated proposal #{}: {}",
                prop["number"].as_u64().unwrap_or(0),
                prop["url"].as_str().unwrap_or("")
            ));
            if let Some(summary) = prop["summary"].as_str() {
                lines.push(format!("  {summary}"));
            }
            lines.extend(change_count_lines(prop));
            if let Some(line) = stack_line(prop) {
                lines.push(format!("  {line}"));
            }
            lines.extend(skipped_large_lines(&prop["skipped_large"]));
        }
        "committed" => {
            let sha = data["sha"].as_str().unwrap_or("");
            let short: String = sha.chars().take(7).collect();
            let at = data["url"].as_str().map(str::to_string).unwrap_or(short);
            lines.push(format!(
                "Committed to {}: {at}",
                data["branch"].as_str().unwrap_or("")
            ));
            if let Some(summary) = data["summary"].as_str() {
                lines.push(format!("  {summary}"));
            }
            lines.extend(change_count_lines(data));
            lines.extend(skipped_large_lines(&data["skipped_large"]));
        }
        "proposal_open" => {
            let prop = &data["proposal"];
            lines.push(format!(
                "Cannot share '{domain}': proposal #{} ({}) is still open; merge or withdraw it first.",
                prop["number"].as_u64().unwrap_or(0),
                prop["url"].as_str().unwrap_or("")
            ));
        }
        "branch_protected" => {
            lines.push(format!(
                "Cannot commit to {}: {} Set sharing: proposal in the MANIFEST, or ask a repository admin.",
                data["branch"].as_str().unwrap_or(""),
                data["message"].as_str().unwrap_or("")
            ));
        }
        "branch_moved" => {
            lines.push(format!(
                "The branch {} moved while sharing; run: crystalline origin update --domain {domain} and share again.",
                data["branch"].as_str().unwrap_or("")
            ));
        }
        "proposal_diverged" => {
            let prop = &data["proposal"];
            lines.push(format!(
                "Cannot update proposal #{} ({}): a reviewer amended its branch.",
                prop["number"].as_u64().unwrap_or(0),
                prop["url"].as_str().unwrap_or("")
            ));
            if let Some(guidance) = data["guidance"].as_str() {
                lines.push(format!("  {guidance}"));
            }
        }
        "nothing_to_share" => {
            lines.push(format!(
                "Nothing to share: '{domain}' already matches its origin."
            ));
            lines.extend(skipped_large_lines(&data["skipped_large"]));
        }
        "conflicts_pending" => {
            lines.push(format!(
                "Cannot share '{domain}': {} conflict(s) need to be resolved first.",
                data["count"].as_u64().unwrap_or(0)
            ));
            for c in data["conflicts"].as_array().unwrap_or(&empty) {
                lines.push(format!("  conflict: {}", c["path"].as_str().unwrap_or("")));
            }
            lines.push(format!(
                "Run: crystalline origin resolve {domain} <path> --keep mine|theirs"
            ));
        }
        other => lines.push(format!(
            "origin share '{domain}': unexpected outcome '{other}'"
        )),
    }
    lines
}

/// Where a shared proposal sits in its chain, or `None` when there is no
/// chain worth naming.
///
/// `stack_position` is what decides that, never `stack_number`: a chain whose
/// linking call failed carries real positions with a null number, and saying
/// "stack #null" would be worse than saying nothing about the number at all.
/// So the position is the gate and the number is named only when there is
/// one, with `(stack link pending)` standing in otherwise - the same debt
/// `origin status` reports until a share or a probing status settles it.
///
/// A chain of one open layer is not a chain a reader needs told about, so a
/// lone proposal renders exactly as it always did: no "layer 1 of 1" noise.
fn stack_line(proposal: &serde_json::Value) -> Option<String> {
    let number = proposal["number"].as_u64()?;
    let position = proposal["stack_position"].as_array()?;
    let layer = position.first()?.as_u64()?;
    let open = position.get(1)?.as_u64()?;
    if open < 2 {
        return None;
    }
    Some(match proposal["stack_number"].as_u64() {
        Some(stack) => format!("proposal #{number}, layer {layer} of {open} on stack #{stack}"),
        None => format!("proposal #{number}, layer {layer} of {open} (stack link pending)"),
    })
}

/// A shared proposal's or a direct commit's change mix: one line of counts for
/// the work somebody wrote, and one quiet line for the folder listings that
/// rode along with it.
///
/// The listings are `index.md` files, generated from the engrams beside them.
/// They travel with a share only in a domain that declares
/// `generated_indexes: shared`, and they say nothing on their own, so counting
/// them among the engrams would inflate every number a reader uses to
/// recognize their own work. The second line is skipped entirely when there
/// are none, which is most shares and all of them in a domain that keeps its
/// listings local.
fn change_count_lines(proposal: &serde_json::Value) -> Vec<String> {
    let (added, added_indexes) = split_indexes(&proposal["added"]);
    let (updated, updated_indexes) = split_indexes(&proposal["updated"]);
    let (deleted, deleted_indexes) = split_indexes(&proposal["deleted"]);
    let mut lines = vec![format!(
        "  {added} added, {updated} updated, {deleted} deleted"
    )];
    let indexes = added_indexes + updated_indexes + deleted_indexes;
    if indexes > 0 {
        let noun = if indexes == 1 { "index" } else { "indexes" };
        lines.push(format!("  also refreshes {indexes} folder {noun}"));
    }
    lines
}

/// How many of a path list are real work and how many are generated folder
/// listings, classified by filename since that is what a listing is.
fn split_indexes(paths: &serde_json::Value) -> (usize, usize) {
    let empty = Vec::new();
    let mut work = 0usize;
    let mut indexes = 0usize;
    for path in paths.as_array().unwrap_or(&empty) {
        match path.as_str() {
            Some(path) if crystalline_core::is_index_path(path) => indexes += 1,
            _ => work += 1,
        }
    }
    (work, indexes)
}

fn skipped_large_lines(skipped_large: &serde_json::Value) -> Vec<String> {
    let empty = Vec::new();
    skipped_large
        .as_array()
        .unwrap_or(&empty)
        .iter()
        .map(|s| {
            format!(
                "  skipped (too large): {} ({} bytes)",
                s[0].as_str().unwrap_or(""),
                s[1].as_u64().unwrap_or(0)
            )
        })
        .collect()
}

/// Print `origin withdraw`'s result: what was closed, what was restored,
/// what was left alone because it diverged since sharing, what no revert
/// could bring back, and what became of the chain the withdrawn layer stood
/// in - repaired under a new stack number, or dissolved when too few layers
/// survived to be a stack at all.
pub(crate) fn print_origin_withdraw(data: &serde_json::Value, json: bool) {
    if json {
        println!("{data}");
        return;
    }
    let number = data["number"].as_u64().unwrap_or(0);
    if data["closed"].as_bool().unwrap_or(false) {
        println!("Withdrew proposal #{number}: closed on GitHub.");
    } else {
        println!("Withdrew proposal #{number} (already closed).");
    }
    let empty = Vec::new();
    for p in data["restored"].as_array().unwrap_or(&empty) {
        println!("restored: {}", p.as_str().unwrap_or(""));
    }
    for p in data["deleted"].as_array().unwrap_or(&empty) {
        println!("deleted: {}", p.as_str().unwrap_or(""));
    }
    for p in data["skipped_diverged"].as_array().unwrap_or(&empty) {
        println!(
            "left alone (diverged since sharing): {}",
            p.as_str().unwrap_or("")
        );
    }
    for p in data["skipped_reverts"].as_array().unwrap_or(&empty) {
        println!(
            "could not restore (no reachable copy): {}",
            p.as_str().unwrap_or("")
        );
    }
    // A repair either recreated the stack under a new number (stack numbers
    // come off the same sequence as pull request numbers, so the old one
    // never comes back) or found too few survivors to be a stack at all.
    if data["repaired"].as_bool().unwrap_or(false) {
        match data["restacked"].as_u64() {
            Some(stack) => println!("stack repaired; now stack #{stack}"),
            None => println!("stack dissolved"),
        }
    }
}

/// A size a person reads: bytes up to a kilobyte, then one decimal until
/// ten, whole above, in KB and MB.
pub(crate) fn human_size(bytes: u64) -> String {
    let kb = bytes as f64 / 1024.0;
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let (value, unit) = if kb < 1024.0 {
        (kb, "KB")
    } else {
        (kb / 1024.0, "MB")
    };
    if value < 10.0 {
        format!("{value:.1} {unit}")
    } else {
        format!("{} {unit}", value.round() as u64)
    }
}

/// `origin diff`: one unified diff per changed path, in path order, with the
/// sides named; a binary change and a withheld side print one line each.
pub(crate) fn print_origin_diff(domain: &str, data: &serde_json::Value, json: bool) {
    if json {
        println!("{data}");
        return;
    }
    let empty = Vec::new();
    let changes = data["changes"].as_array().unwrap_or(&empty);
    if changes.is_empty() {
        println!("Nothing has changed in {domain}.");
        return;
    }
    for change in changes {
        let path = change["path"].as_str().unwrap_or("");
        let kind = change["kind"].as_str().unwrap_or("");
        let before = change["size_before"].as_u64();
        let after = change["size_after"].as_u64();
        if change["binary"].as_bool().unwrap_or(false) {
            let sizes = match (before, after) {
                (Some(b), Some(a)) => format!("{} to {}", human_size(b), human_size(a)),
                (None, Some(a)) => human_size(a),
                (Some(b), None) => human_size(b),
                (None, None) => String::new(),
            };
            println!("{path}: {kind}, {sizes}");
            continue;
        }
        let base = change["base"].as_str().unwrap_or("");
        let current = change["current"].as_str().unwrap_or("");
        let diff = similar::TextDiff::from_lines(base, current);
        print!(
            "{}",
            diff.unified_diff()
                .context_radius(3)
                .header(&format!("a/{path} (team)"), &format!("b/{path} (mine)"))
        );
    }
}

/// `origin discard`'s preview: one line per named path saying what the
/// discard would do, and the `(path, sha)` targets for the ones that are
/// changes. Unknown paths print as refused and are not among the targets.
pub(crate) fn print_discard_preview(
    listed: &serde_json::Value,
    paths: &[String],
    json: bool,
) -> Vec<(String, Option<String>)> {
    let empty = Vec::new();
    let changes = listed["changes"].as_array().unwrap_or(&empty);
    let mut targets = Vec::new();
    for path in paths {
        let Some(change) = changes
            .iter()
            .find(|c| c["path"].as_str() == Some(path.as_str()))
        else {
            if !json {
                println!("{path}  refused: not among this domain's unshared changes");
            }
            continue;
        };
        let sha = change["sha"].as_str().map(str::to_string);
        if !json {
            let before = change["size_before"].as_u64();
            let after = change["size_after"].as_u64();
            let line = match change["kind"].as_str().unwrap_or("") {
                "added" => format!(
                    "A {path}  {}  delete",
                    after.map(human_size).unwrap_or_default()
                ),
                "modified" => format!(
                    "M {path}  {} -> {}  restore the team's copy",
                    before.map(human_size).unwrap_or_default(),
                    after.map(human_size).unwrap_or_default()
                ),
                "deleted" => format!(
                    "D {path}  {}  restore the team's copy",
                    before.map(human_size).unwrap_or_default()
                ),
                other => format!("{other} {path}"),
            };
            println!("{line}");
        }
        targets.push((path.clone(), sha));
    }
    targets
}

/// Print `origin discard`'s report; true when at least one path was acted on.
pub(crate) fn print_origin_discard(data: &serde_json::Value, json: bool) -> bool {
    let empty = Vec::new();
    let acted = data["restored"].as_array().unwrap_or(&empty).len()
        + data["deleted"].as_array().unwrap_or(&empty).len()
        + data["cleared"].as_array().unwrap_or(&empty).len();
    if json {
        println!("{data}");
        return acted > 0;
    }
    for p in data["restored"].as_array().unwrap_or(&empty) {
        println!("restored: {}", p.as_str().unwrap_or(""));
    }
    for p in data["deleted"].as_array().unwrap_or(&empty) {
        println!("deleted: {}", p.as_str().unwrap_or(""));
    }
    for p in data["cleared"].as_array().unwrap_or(&empty) {
        println!("cleared: {}", p["path"].as_str().unwrap_or(""));
    }
    for p in data["refused"].as_array().unwrap_or(&empty) {
        println!(
            "refused: {} ({})",
            p["path"].as_str().unwrap_or(""),
            p["reason"].as_str().unwrap_or("")
        );
    }
    if let Some(n) = data["reindexed"].as_u64().filter(|n| *n > 0) {
        println!("re-indexed {n} file(s)");
    }
    acted > 0
}

/// Print `origin resolve`'s result: the resolved path and how many
/// conflicts remain open.
pub(crate) fn print_origin_resolve(data: &serde_json::Value, json: bool) {
    if json {
        println!("{data}");
        return;
    }
    println!("Resolved: {}", data["resolved"].as_str().unwrap_or(""));
    println!(
        "Remaining conflicts: {}",
        data["remaining"].as_u64().unwrap_or(0)
    );
}

/// Reads `--content-file`'s bytes for `origin resolve --content-file`, as raw
/// bytes rather than a UTF-8 string: a resolved file may be a binary asset,
/// and the merge must round-trip byte for byte.
pub(crate) fn read_resolve_content(path: &Path) -> Result<Vec<u8>> {
    std::fs::read(path).with_context(|| format!("reading {}", path.display()))
}

// --- provision -----------------------------------------------------------

/// Render a `provision` result: `status`'s report through
/// [`print_provision_status`], every other action (bare `provision`,
/// `allow`, `deny`) through [`print_provision_apply`].
pub(crate) fn print_provision(action: &str, data: &serde_json::Value, json: bool) {
    if action == "status" {
        print_provision_status(data, json);
    } else {
        print_provision_apply(data, json);
    }
}

/// Render an apply report (bare `provision`, `allow` or `deny`): one line per
/// harness with what it did, or "up to date" when nothing changed, then any
/// notices, then domains still awaiting a decision with a hint to opt them
/// in.
pub(crate) fn print_provision_apply(data: &serde_json::Value, json: bool) {
    if json {
        println!("{data}");
        return;
    }
    // An empty harness list is not announced here: the core apply already
    // raises its own no-harness notice (with the `crystalline install` hint)
    // whenever a domain is opted in, and that notice prints below.
    let empty = Vec::new();
    let harnesses = data["harnesses"].as_array().unwrap_or(&empty);
    for h in harnesses {
        let name = h["harness"].as_str().unwrap_or("");
        let actions = h["actions"].as_array().cloned().unwrap_or_default();
        if actions.is_empty() {
            println!("{name}: up to date");
            continue;
        }
        println!("{name}:");
        for a in &actions {
            println!(
                "  {} {}",
                provision_action_label(a["status"].as_str().unwrap_or("")),
                a["target"].as_str().unwrap_or("")
            );
        }
    }
    for notice in data["notices"].as_array().unwrap_or(&empty) {
        if let Some(n) = notice.as_str() {
            println!("note: {n}");
        }
    }
    print_provision_pending(data);
}

/// Render `provision status`: each domain's decision and declared counts,
/// each installed harness's installed, drifted, edited, orphaned and missing
/// counts, then domains still awaiting a decision. The harness line matches
/// `crystalline doctor`'s provisioning section wording exactly, so the two
/// surfaces never drift apart on what they report.
pub(crate) fn print_provision_status(data: &serde_json::Value, json: bool) {
    if json {
        println!("{data}");
        return;
    }
    let empty = Vec::new();
    for d in data["domains"].as_array().unwrap_or(&empty) {
        let name = d["domain"].as_str().unwrap_or("");
        if d["is_virtual"].as_bool().unwrap_or(false) {
            println!("{name}: virtual, never provisions artifacts");
            continue;
        }
        if !d["declares"].as_bool().unwrap_or(false) {
            println!("{name}: declares no provisioning");
            continue;
        }
        let decision = d["decision"].as_str().unwrap_or("undecided");
        println!("{name}: {decision}, {}", format_counts(&d["counts"]));
    }
    for h in data["harnesses"].as_array().unwrap_or(&empty) {
        println!(
            "{}: {} file(s) installed, {} mcp(s) installed, {} drifted, {} edited, {} orphaned, {} missing",
            h["harness"].as_str().unwrap_or(""),
            h["installed_files"].as_u64().unwrap_or(0),
            h["installed_mcps"].as_u64().unwrap_or(0),
            h["drift"].as_u64().unwrap_or(0),
            h["edited"].as_u64().unwrap_or(0),
            h["orphaned"].as_u64().unwrap_or(0),
            h["missing"].as_u64().unwrap_or(0),
        );
    }
    print_provision_pending(data);
}

/// The "domains awaiting a decision" tail shared by an apply report and a
/// status report.
fn print_provision_pending(data: &serde_json::Value) {
    let empty = Vec::new();
    let pending = data["pending"].as_array().unwrap_or(&empty);
    if pending.is_empty() {
        return;
    }
    println!("Domains awaiting a decision:");
    for p in pending {
        let name = p["domain"].as_str().unwrap_or("");
        println!(
            "  {name}: {} - run `crystalline provision allow {name}` to opt in.",
            format_counts(&p["counts"])
        );
    }
}

/// Render an [`ArtifactType`]-id-keyed counts object as `"2 skills, 1 mcps"`,
/// or a plain "no artifacts" when it is empty.
///
/// [`ArtifactType`]: crystalline_core::manifest::ArtifactType
fn format_counts(counts: &serde_json::Value) -> String {
    let Some(map) = counts.as_object() else {
        return "no artifacts".to_string();
    };
    if map.is_empty() {
        return "no artifacts".to_string();
    }
    map.iter()
        .map(|(kind, n)| format!("{} {kind}", n.as_u64().unwrap_or(0)))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The human phrase for one [`ActionStatus`] wire id, matching
/// `crystalline_service::engine::action_status_id`'s spellings.
///
/// [`ActionStatus`]: crystalline_core::ActionStatus
fn provision_action_label(status: &str) -> &str {
    match status {
        "installed" => "installed",
        "adopted" => "adopted",
        "foreign_kept" => "kept (foreign)",
        "updated" => "updated",
        "updated_backup" => "updated (edit kept as .bak)",
        "removed" => "removed",
        "retired_backup" => "retired (edit kept as .bak)",
        "mcp_added" => "mcp added",
        "mcp_updated" => "mcp updated",
        "mcp_removed" => "mcp removed",
        "mcp_skipped" => "mcp skipped",
        "mcp_failed" => "mcp failed",
        other => other,
    }
}

// --- domain remove -----------------------------------------------------------

/// Render the engine's own unregistration report.
///
/// The removal itself is `crystalline_service::domain_remove`, the entry point
/// every surface calls; this only says what happened. The two sentences it can
/// print are the two things that differ by kind, and the difference is the
/// whole reason a virtual domain needs `--purge`: a file or team domain's files
/// were left exactly where they are and registering the folder again re-adopts
/// them, while a virtual domain's engrams were in the database and are gone.
pub fn print_domain_remove(name: &str, report: &serde_json::Value, json: bool) {
    if json {
        println!("{report}");
        return;
    }
    let files_kept = report["files_kept"].as_bool().unwrap_or(true);
    println!("Unregistered domain '{name}' and cleared its rows from the index.");
    if files_kept {
        println!("Its files were left untouched: register the folder again to re-adopt them.");
    } else {
        println!("It was a virtual domain, so its engrams went with it.");
    }
    let rooms = report["rooms_closed"].as_u64().unwrap_or(0);
    if rooms > 0 {
        println!("{rooms} open co-editing session(s) were saved and closed.");
    }
}

/// The plan `domain review <domain> direct` prints before it sends an answer:
/// every actor holding drafts, what each of them holds, and the two kinds of
/// trouble a fold can run into.
pub fn print_review_plan(plan: &serde_json::Value) {
    for line in review_plan_lines(plan) {
        println!("{line}");
    }
}

/// The plan as lines, so what an operator reads is a value a test can hold.
///
/// Split out of [`print_review_plan`] rather than left inline because the plan
/// carries TWO kinds of trouble a fold runs into and they arrive in different
/// places - a path more than one person is drafting, and an address two of
/// their different paths both answer to - and a printer that renders one and
/// drops the other reads as a clean plan that is then refused at the confirm,
/// which is the one outcome this whole direction exists to prevent. That is a
/// thing to assert, not to eyeball.
fn review_plan_lines(plan: &serde_json::Value) -> Vec<String> {
    /// The string values of an array field, in order.
    fn names(value: &serde_json::Value) -> Vec<&str> {
        value
            .as_array()
            .map(|rows| rows.iter().filter_map(serde_json::Value::as_str).collect())
            .unwrap_or_default()
    }

    let actors = plan["actors"].as_array().cloned().unwrap_or_default();
    if actors.is_empty() {
        return vec![
            "Nobody is drafting in this domain, so leaving review mode ends nothing.".to_string(),
        ];
    }
    let mut out = vec!["Leaving review mode ends every private draft in this domain:".to_string()];
    for row in &actors {
        let actor = row["actor"].as_str().unwrap_or("?");
        let entries = row["entries"].as_u64().unwrap_or(0);
        let drafts = row["drafts"].as_array().cloned().unwrap_or_default();
        // How many of them are files, said on the actor's own line, because
        // what happens to a file when it folds is not what happens to a page:
        // the bytes become the team's file rather than a page they can read.
        let files = drafts
            .iter()
            .filter(|draft| draft["kind"] == serde_json::json!("file"))
            .count();
        out.push(match files {
            0 => format!("  {actor} ({entries} draft(s))"),
            1 => format!("  {actor} ({entries} draft changes, 1 of them a file)"),
            n => format!("  {actor} ({entries} draft changes, {n} of them files)"),
        });
        // Said before their rows, because it is about all of them: an actor
        // whose files could not be listed is in the plan so somebody knows they
        // are there, and leaving review mode refuses until the tree can be
        // read.
        if row["files_unreadable"].as_bool().unwrap_or(false) {
            out.push(format!(
                "    the files {actor} has drafted could not be read, so review mode cannot be \
                 taken off until they can"
            ));
        }
        for draft in drafts {
            let path = draft["path"].as_str().unwrap_or("?");
            let what = if draft["tombstone"].as_bool().unwrap_or(false) {
                "deleted"
            } else {
                "drafted"
            };
            let kind = if draft["kind"] == serde_json::json!("file") {
                "the file "
            } else {
                ""
            };
            out.push(format!("    {what} {kind}{path}"));
            if let Some(conflict) = draft["conflict"].as_str() {
                out.push(format!("      cannot be folded: {conflict}"));
            }
        }
    }
    for contested in plan["contested_paths"]
        .as_array()
        .cloned()
        .unwrap_or_default()
    {
        let path = contested["path"].as_str().unwrap_or("?");
        out.push(format!(
            "  {path} is drafted by {}, so at most one of them can be folded.",
            names(&contested["actors"]).join(" and ")
        ));
    }
    for contested in plan["contested_addresses"]
        .as_array()
        .cloned()
        .unwrap_or_default()
    {
        let permalink = contested["permalink"].as_str().unwrap_or("?");
        out.push(format!(
            "  {} answer to the address '{permalink}', drafted by {}, so at most one of them can \
             be folded: one engram answers to one address.",
            names(&contested["paths"]).join(" and "),
            names(&contested["actors"]).join(" and ")
        ));
    }
    out.push(
        "Answer with --fold <actor> (write their drafts into the folder) or --discard <actor> \
         (end them), one for each actor above."
            .to_string(),
    );
    out
}

/// What `domain review` says once the mode is what was asked for.
pub fn print_domain_review(report: &serde_json::Value, json: bool) {
    if json {
        println!("{report}");
        return;
    }
    let domain = report["domain"].as_str().unwrap_or("?");
    match report["review"].as_str() {
        Some(_) => println!(
            "Domain '{domain}' reviews changes before they land: every write now joins its \
             author's own draft, and the folder changes through a reviewed proposal."
        ),
        None => println!("Domain '{domain}' takes changes directly again."),
    }
    for row in report["folded"].as_array().cloned().unwrap_or_default() {
        println!(
            "  folded {}: {} file(s) written, {} deleted.",
            row["actor"].as_str().unwrap_or("?"),
            row["written"].as_u64().unwrap_or(0),
            row["deleted"].as_u64().unwrap_or(0)
        );
    }
    for row in report["discarded"].as_array().cloned().unwrap_or_default() {
        println!(
            "  discarded {}: {} draft(s) ended.",
            row["actor"].as_str().unwrap_or("?"),
            row["entries"].as_u64().unwrap_or(0)
        );
    }
    let rooms = report["rooms_closed"].as_u64().unwrap_or(0);
    if rooms > 0 {
        println!("{rooms} open co-editing session(s) were saved and closed.");
    }
}

// --- domain list -------------------------------------------------------------

/// The slice of a domain's index stats this listing prints: how many engrams
/// it holds, and which instance hosts it in a shared database. Both routes to
/// the index produce it, so the daemon's answer and a direct read render
/// identically. Read field by field rather than deserialized whole: the
/// daemon's rows carry an annotation of its own and [`crystalline_index::DomainStats`]
/// is a write-only shape.
struct ListedStats {
    name: String,
    engrams: i64,
    host_instance_id: Option<String>,
    host_heartbeat_at: Option<String>,
}

impl ListedStats {
    fn from_stats(d: &crystalline_index::DomainStats) -> ListedStats {
        ListedStats {
            name: d.name.clone(),
            engrams: d.engrams,
            host_instance_id: d.host_instance_id.clone(),
            host_heartbeat_at: d.host_heartbeat_at.clone(),
        }
    }

    fn from_json(v: &serde_json::Value) -> Option<ListedStats> {
        let text = |key: &str| {
            v.get(key)
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        };
        Some(ListedStats {
            name: text("name")?,
            engrams: v.get("engrams").and_then(serde_json::Value::as_i64)?,
            host_instance_id: text("host_instance_id"),
            host_heartbeat_at: text("host_heartbeat_at"),
        })
    }
}

/// List registered domains, with engram counts when the index can be read.
///
/// The registrations come from configuration, so this command always answers:
/// it is the counts, and only the counts, that need the index. When the index
/// cannot be reached the list still prints and each count says it was not
/// read, rather than the whole command failing or, worse, reporting a domain
/// as unindexed because a daemon happened to be holding the file. The daemon's
/// `status` reply carries the same per-domain stats a direct read would, so a
/// machine with a daemon gets real counts instead of a lock error.
pub async fn domain_list(
    config_override: Option<&Path>,
    db_override: Option<&Path>,
    json: bool,
) -> Result<()> {
    // Keep the whole `LoadedConfig`: reads use the effective config, and the
    // overlay marks which rows an environment variable defines.
    let loaded = load(config_override)?;
    let cfg = loaded.effective;
    // Why the counts are missing when they are, in the helper's words; `None`
    // once they were read, whichever route delivered them.
    let mut not_read: Option<String> = None;
    // The one verb that must answer whatever the index does, so the route is
    // matched rather than propagated with `?`. A daemon that replies with an
    // error envelope, or dies mid-exchange leaving a truncated line, makes
    // `ctl_if_running` fail, and letting that fail the command would put the
    // daemon's bare error where this listing's registrations belong - the raw
    // backend text this whole task exists to stop showing a person.
    let route = match reach_index(
        Some(serde_json::json!({ "v": 1, "cmd": "status" })),
        &cfg,
        config_override,
        db_override,
        OpenAs::Read,
    )
    .await
    {
        Ok(route) => Some(route),
        Err(e) => {
            // Which of the two failures this was. `db_path` is cheap and pure,
            // so asking it again is how the arm tells a daemon that answered
            // badly from a path that never resolved - the alternative is
            // classifying the error by its text, which is exactly what this
            // file stopped doing.
            not_read = Some(match db_path(db_override) {
                Ok(_) => daemon_answered_badly("this listing", &e.to_string()),
                Err(_) => listing_not_reached("this listing", &e.to_string()),
            });
            None
        }
    };
    let stats: Option<Vec<ListedStats>> = match route {
        None => None,
        Some(route) => match route {
            // The daemon's own `domain_stats`, annotated with a `hosted_here`
            // field this command has no use for. A reply that carries no counts,
            // or a row that does not read back, is a count nobody read: saying so
            // is the point, and rendering it as an empty set would put every
            // domain back on the "(not indexed)" line this routing exists to end.
            IndexRoute::Daemon(data) => {
                match data.get("domains").and_then(serde_json::Value::as_array) {
                    Some(rows) => {
                        let parsed: Vec<ListedStats> =
                            rows.iter().filter_map(ListedStats::from_json).collect();
                        if parsed.len() == rows.len() {
                            Some(parsed)
                        } else {
                            not_read = Some(
                            "the running Crystalline daemon answered, but its per-domain counts did not read back in the shape this listing expects. Check the daemon and the CLI are the same version with: crystalline status".to_string(),
                        );
                            None
                        }
                    }
                    None => {
                        not_read = Some(
                        "the running Crystalline daemon answered without the per-domain counts this listing reads. Check the daemon and the CLI are the same version with: crystalline status".to_string(),
                    );
                        None
                    }
                }
            }
            IndexRoute::Direct(store) => match store.lock().await.domain_stats().await {
                Ok(rows) => Some(rows.iter().map(ListedStats::from_stats).collect()),
                // Open, and still no counts: the index answered the open and not
                // the question, which is a different state from both "unreachable"
                // and "never synced" and must not be rendered as either.
                Err(e) => {
                    not_read = Some(format!(
                        "the index opened, but its per-domain counts could not be read. Look at it with: crystalline doctor --fix. The index reported: {e}"
                    ));
                    None
                }
            },
            // No index yet is not a failure to read one: a registered domain that
            // was never synced is exactly the "(not indexed)" case below.
            IndexRoute::Absent(_) => Some(Vec::new()),
            IndexRoute::Unreachable(why) => {
                not_read = Some(why);
                None
            }
        },
    };
    if let Some(why) = &not_read
        && !json
    {
        eprintln!("note: engram counts were not read; {why}");
    }
    let stat_for = |name: &str| {
        stats
            .as_ref()
            .and_then(|s| s.iter().find(|d| d.name == name))
    };
    let count_for = |name: &str| -> Option<i64> { stat_for(name).map(|d| d.engrams) };
    // The current host of a file domain in a shared database, `None` when
    // unhosted (every domain in a single-instance deployment, every virtual one).
    let host_for = |name: &str| -> Option<(String, Option<String>)> {
        stat_for(name).and_then(|d| {
            d.host_instance_id
                .clone()
                .map(|id| (id, d.host_heartbeat_at.clone()))
        })
    };

    if json {
        let domains: Vec<serde_json::Value> = cfg
            .domains
            .iter()
            .map(|(name, entry)| {
                // Env-defined domains carry `"source": "env"`; file entries
                // carry `"config"`, so a caller can tell which is which.
                let source = if loaded.overlay.env_domain(name).is_some() {
                    "env"
                } else {
                    "config"
                };
                serde_json::json!({
                    "name": name,
                    "kind": if entry.is_virtual() { "virtual" } else { "file" },
                    "path": entry.file_path().map(|p| p.display().to_string()),
                    "engrams": count_for(name),
                    "source": source,
                    "host": host_for(name).map(|(id, hb)| serde_json::json!({
                        "instance_id": id,
                        "heartbeat_at": hb,
                    })),
                })
            })
            .collect();
        // `engrams: null` alone cannot tell "not synced yet" from "nobody
        // read the index", and those want opposite reactions from a reader.
        // `counts` says which, and carries the helper's words when the
        // counts are missing.
        let counts = match &not_read {
            Some(why) => serde_json::json!({ "read": false, "reason": why }),
            None => serde_json::json!({ "read": true }),
        };
        println!(
            "{}",
            serde_json::json!({ "domains": domains, "counts": counts })
        );
        return Ok(());
    }

    if cfg.domains.is_empty() {
        println!("No domains registered. Add one with: crystalline domain add <name> <path>");
        return Ok(());
    }
    for (name, entry) in &cfg.domains {
        // A virtual domain reports "(virtual)" where a file domain shows its root.
        let mut location = entry
            .file_path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(virtual)".to_string());
        // An env-defined domain is marked so it reads as managed by its
        // variable, not the config file.
        if loaded.overlay.env_domain(name).is_some() {
            location.push_str(" (env)");
        }
        // In a shared database a file domain names the instance that hosts
        // it, along with how recently it last heartbeat.
        let host = host_for(name)
            .map(|(id, hb)| {
                let heartbeat = hb.as_deref().map(relative_time).unwrap_or_default();
                if heartbeat.is_empty() {
                    format!("\thosted by {id}")
                } else {
                    format!("\thosted by {id} (heartbeat {heartbeat})")
                }
            })
            .unwrap_or_default();
        match count_for(name) {
            Some(n) => println!("{name}\t{location}\t{n} engrams{host}"),
            None if not_read.is_some() => {
                println!("{name}\t{location}\t(counts not read){host}")
            }
            None => println!("{name}\t{location}\t(not indexed){host}"),
        }
    }
    Ok(())
}

// --- sync --------------------------------------------------------------------

/// Sync one or all registered domains, optionally embedding new chunks after,
/// into an index its dispatch already reached through [`reach_index`]. Taking
/// the opened store rather than opening one keeps the daemon-or-direct
/// decision in `main.rs`'s dispatch layer, where `reindex` and `status` make
/// the same one.
pub async fn sync(
    store: Arc<TokioMutex<dyn Store>>,
    cfg: &GlobalConfig,
    only: Option<&str>,
    embed: bool,
    json: bool,
) -> Result<()> {
    let targets = select_domains(cfg, only)?;
    let params = chunk_params(cfg);

    // Each domain this run applied, paired with the report its apply produced,
    // for the final cross-domain resolution pass below.
    let mut applied: Vec<(crystalline_index::DomainId, crystalline_index::SyncReport)> = Vec::new();
    for (name, entry) in targets {
        // Virtual domains have no files to sync.
        let Some(path) = resolve_domain_path(&entry) else {
            continue;
        };
        // A large domain's walk-and-hash pass can take a while with no
        // other output in between; say which domain is in flight before it
        // starts, the same way `embed_pass` announces each batch.
        if !json {
            eprintln!("syncing {name}...");
        }
        // First lock window: snapshot the stamps; scan with no lock held so the
        // walk-and-hash pass does not block concurrent readers; second window:
        // apply transactionally with the TOCTOU guards.
        let (domain, snapshot) = {
            let store = store.lock().await;
            let domain = store
                .upsert_domain(&name, Some(&path.to_string_lossy()), DomainKind::File)
                .await?;
            let snapshot = store.file_stamps(domain).await?;
            (domain, snapshot)
        };
        let scan = scan_domain(&name, &path, snapshot, &params, false).await?;
        let report = {
            let store = store.lock().await;
            apply_scan(&*store, domain, scan)
                .await
                .map_err(|e| anyhow!("sync of '{name}' failed: {e}"))?
        };
        applied.push((domain, report));
    }

    // Every domain of this run is in now, so a reference that pointed forward
    // into a domain later in the loop resolves here rather than waiting for the
    // next sync. A single-domain run is a no-op inside the pass.
    {
        let store = store.lock().await;
        resolve_forward_refs(&*store, &mut applied)
            .await
            .map_err(|e| anyhow!("resolving forward references failed: {e}"))?;
    }
    let reports: Vec<crystalline_index::SyncReport> =
        applied.into_iter().map(|(_, report)| report).collect();

    if json {
        println!("{}", serde_json::to_string(&reports)?);
    } else {
        for r in &reports {
            print_report(r);
        }
    }

    if embed {
        let store = store.lock().await;
        embed_pass(&*store, cfg).await?;
    }

    // Any sync, not just a full reindex, is a snapshot-preparation verb: a
    // downstream pipeline may ship index.db as a single file (sidecars
    // deleted), so the delta this sync just wrote must not sit stranded in
    // the WAL. Merge and shrink it now rather than waiting for a natural
    // checkpoint. A no-op on Postgres (no local WAL file).
    {
        let store = store.lock().await;
        store.checkpoint_wal().await?;
    }

    // A file that failed to read, parse or upsert is a real failure, not a
    // shrug: `doctor` exits 1 on a problem and `verify` exits 2, so a sync
    // that printed a `failed:` line and still exited 0 was the outlier, and a
    // CI step piping through it could not see the partial failure at all.
    // The full report (JSON included) has already printed above, so a
    // `--json` consumer still gets the complete document before this fails
    // the process. The direct path has no equivalent of a whole domain
    // skipped by a scan error: `scan_domain` above is called with `?`, so
    // that class already aborts the whole command immediately rather than
    // being collected here - `scan_failed` is always empty on this path, and
    // only the daemon-routed path in `sync_dispatch` (`main.rs`) passes one.
    if let Some(err) = sync_failure(&reports, &[]) {
        return Err(err);
    }
    Ok(())
}

/// The error `sync` fails with when either failure class is present, or
/// `None` when both are empty. Shared by both ways a sync can run - directly,
/// above, and daemon-routed through `sync_dispatch` in `main.rs`, which reads
/// both classes back out of the daemon's own JSON and calls this too - so the
/// wording and the trigger condition can never drift apart between the two
/// paths, and a user cannot tell which one handled their command from the
/// failure alone.
///
/// The two classes mean different things to a person, so a combined failure
/// names both rather than merging them into one count: a file in `reports[].failed`
/// is theirs to edit (bad frontmatter, most often), while a domain in
/// `scan_failed` could not be scanned at all, which is usually a path or
/// permission problem - not something a file edit fixes.
pub(crate) fn sync_failure(
    reports: &[crystalline_index::SyncReport],
    scan_failed: &[(String, String)],
) -> Option<anyhow::Error> {
    let failed_count: usize = reports.iter().map(|r| r.failed.len()).sum();
    if failed_count == 0 && scan_failed.is_empty() {
        return None;
    }
    let mut parts = Vec::new();
    if failed_count > 0 {
        let domains: Vec<&str> = reports
            .iter()
            .filter(|r| !r.failed.is_empty())
            .map(|r| r.domain.as_str())
            .collect();
        parts.push(format!(
            "{failed_count} file(s) failed to sync in domain(s): {}",
            domains.join(", ")
        ));
    }
    if !scan_failed.is_empty() {
        let domains: Vec<&str> = scan_failed.iter().map(|(name, _)| name.as_str()).collect();
        parts.push(format!(
            "{} domain(s) could not be scanned at all, usually a path or permission problem: {}",
            scan_failed.len(),
            domains.join(", ")
        ));
    }
    Some(anyhow!(parts.join("; ")))
}

// --- reindex -----------------------------------------------------------------

/// Reindex all domains: `full` re-reads every file rather than only the ones
/// whose stamp moved, `wipe` destroys the index first and rebuilds it from
/// scratch.
///
/// The two are different operations and the flags do not combine. A forced
/// reindex destroys nothing - every domain serves its previous complete rows
/// until its own rebuild commits, and an unchanged chunk keeps its embedding -
/// so an interruption costs a re-run rather than an hour of re-embedding. A
/// wipe is for the case nothing else can fix, a database file that will not
/// open, and its cost is the whole embedding corpus; its store was opened
/// resiliently by the dispatch above, which is what discards an unopenable
/// file.
///
/// The loop itself is [`crystalline_index::reindex_domains`], shared with the
/// daemon's `ctl reindex`, so the two paths cannot drift apart in what they
/// re-read, in what order they rebuild or in the passes they end with.
pub async fn reindex(
    store: Arc<TokioMutex<dyn Store>>,
    cfg: &GlobalConfig,
    full: bool,
    wipe: bool,
    embed: bool,
    json: bool,
) -> Result<()> {
    let targets = select_domains(cfg, None)?;
    let params = chunk_params(cfg);

    // Set by the resilient open when the database it found would not open at
    // all: those bytes were renamed aside rather than deleted, and the run says
    // where they went. Read before the wipe, since the wipe is what this is
    // reporting on.
    let set_aside = if wipe {
        store.lock().await.set_aside_database()
    } else {
        None
    };

    // A wipe destroys everything in the database and rebuilds from the files on
    // disk, so it is only ever safe when the files are the whole truth. A
    // virtual domain's engrams live nowhere else: wiping them is not a rebuild,
    // it is deleting knowledge, and no rebuild afterwards can bring them back.
    // Refuse rather than quietly doing something narrower than the verb's name,
    // and name the way out.
    //
    // The question here is asked of the DATABASE, not of this config. `Store::wipe`
    // is unscoped - thirteen bare deletes ending in `domain` - so what is at
    // risk is every virtual domain the index holds, and the two are routinely
    // not the same set: a domain dropped from the config keeps its rows until
    // something collects them (the whole orphaned-rows surface exists for that
    // state), and `--db`/`--config`, the flags that force this direct path in
    // the first place, are the documented way to point a narrower config at a
    // wider index. Asking only `cfg.domains` here would wave the wipe through in
    // exactly the cases those flags exist for.
    //
    // The config IS asked, one frame up in `reindex_dispatch`, and the two
    // guards are complements rather than a contradiction: this one is the
    // precise question and can only be asked once the index opens, so it cannot
    // fire in the state `--wipe` exists for, where the file does not open at all
    // and a fresh empty database has taken its place. The config is the only
    // signal left there. Neither is sufficient; both are cheap.
    if wipe {
        let store = store.lock().await;
        let stats = store
            .domain_stats()
            .await
            .map_err(|e| anyhow!("could not read the index before wiping it: {e}"))?;
        let virtual_domains: Vec<&str> = stats
            .iter()
            .filter(|d| d.kind == DomainKind::Virtual)
            .map(|d| d.name.as_str())
            .collect();
        if !virtual_domains.is_empty() {
            let exports: String = virtual_domains
                .iter()
                .map(|name| format!("\n  crystalline domain export <dir> --domain {name}"))
                .collect();
            bail!(
                "refusing to wipe: the index holds {} virtual domain(s) whose engrams live nowhere else, so a wipe would delete them for good: {}. The index opened, so copying them out works - do that first, then wipe:{}\n  crystalline reindex --wipe\n\nOr rebuild without destroying anything: crystalline reindex --full",
                virtual_domains.len(),
                virtual_domains.join(", "),
                exports
            );
        }
        store
            .wipe()
            .await
            .map_err(|e| anyhow!("wiping the index failed: {e}"))?;
    }

    // Only the file domains have files to (re)index. A virtual domain's rows
    // are its source of truth and are never rebuilt from anything.
    let file_targets: Vec<(String, PathBuf)> = targets
        .into_iter()
        .filter_map(|(name, entry)| resolve_domain_path(&entry).map(|p| (name, p)))
        .collect();

    // A wipe left nothing to compare against, so its rebuild is forced too:
    // every file is read, and the prefilter has no stamps to skip against
    // anyway.
    let rebuild = if wipe {
        Some(RebuildKind::Wipe)
    } else if full {
        Some(RebuildKind::Full)
    } else {
        None
    };
    let reports =
        reindex_domains(&*store, &file_targets, &params, rebuild, &NoReindexHooks).await?;

    // The rebuilt base rows are in; now the rows no file on disk describes. An
    // overlay draft is one actor's private version of a path and it lives
    // nowhere but the index, so a wipe takes it and no walk can bring it back -
    // the mirror under the state directory is what can, and this is the moment
    // to read it. Runs on every reindex, not only a wipe: it costs one
    // directory read per domain when there is nothing to restore, and a
    // daemonless installation has no other pass that would ever heal a draft.
    let drafts_restored = restore_overlay_journals(&store, &file_targets, &params).await;

    if json {
        println!(
            "{}",
            serde_json::json!({
                "full": full,
                "wipe": wipe,
                "reports": reports,
                "drafts_restored": drafts_restored,
                "set_aside": set_aside.as_ref().map(|p| p.display().to_string()),
            })
        );
    } else {
        println!(
            "Reindex ({}) complete",
            if wipe {
                "wiped and rebuilt"
            } else if full {
                "full"
            } else {
                "incremental"
            }
        );
        for r in &reports {
            print_report(r);
        }
        if drafts_restored > 0 {
            println!("  {drafts_restored} draft(s) restored from the overlay journal");
        }
        if let Some(aside) = &set_aside {
            println!(
                "  the database that would not open was set aside at {}, not deleted; remove it once you are satisfied with the rebuild",
                aside.display()
            );
        }
    }

    // The driver already checkpointed what the rebuild wrote, but the embed
    // pass runs after it, so its vectors need their own merge before a
    // downstream pipeline ships index.db as a single file with the sidecars
    // deleted. A no-op on Postgres (no local WAL file); on Turso this replaces
    // the downstream Docker image build's shell-out to `sqlite3` for the same
    // purpose.
    if embed {
        let store = store.lock().await;
        embed_pass(&*store, cfg).await?;
        store.checkpoint_wal().await?;
    }
    Ok(())
}

/// Put every mirrored draft back into the rebuilt index, answering with how
/// many rows were written across every domain.
///
/// Best effort, per domain: a journal that could not be read or a row that
/// could not be written is logged and the rebuild still reports what it
/// rebuilt. The scope is the domains this run just rebuilt, which is this
/// path's version of the engine's "never restore into a domain nobody
/// registers" - the targets came from the configuration.
async fn restore_overlay_journals(
    store: &Arc<TokioMutex<dyn Store>>,
    targets: &[(String, PathBuf)],
    chunk_params: &ChunkParams,
) -> u64 {
    let state_dir = match crystalline_core::config::state_dir() {
        Ok(dir) => dir,
        Err(e) => {
            tracing::warn!("the overlay journal could not be located: {e}");
            return 0;
        }
    };
    let mut restored = 0u64;
    for (name, root) in targets {
        // A domain with nothing mirrored is not touched at all. That keeps the
        // usual reindex a read of one directory per domain, and - since the
        // driver has already checkpointed the WAL by the time this runs - keeps
        // it from dirtying the WAL again with a write nobody needed.
        let counts = crystalline_service::overlay_journal::journal_counts(&state_dir, name);
        if counts.total == 0 && !counts.unreadable {
            continue;
        }
        let store = store.lock().await;
        let id = match store
            .upsert_domain(name, Some(&root.to_string_lossy()), DomainKind::File)
            .await
        {
            Ok(id) => id,
            Err(e) => {
                tracing::warn!("the overlay journal for '{name}' was not restored: {e}");
                continue;
            }
        };
        match crystalline_service::overlay_journal::restore_into(
            &*store,
            &state_dir,
            name,
            id,
            chunk_params,
        )
        .await
        {
            Ok(n) => restored += n,
            Err(e) => tracing::warn!("the overlay journal for '{name}' was not restored: {e}"),
        }
        // What the restore wrote must not sit stranded in the WAL either: a
        // reindex is a snapshot-preparation verb whichever rows it wrote last.
        if let Err(e) = store.checkpoint_wal().await {
            tracing::debug!("reindex: the WAL checkpoint after the restore did not run: {e}");
        }
    }
    restored
}

// --- status ------------------------------------------------------------------

/// Build the in-process status report in the same shape the daemon's ctl
/// `status` returns (minus its liveness fields and the exposure facts only a
/// serving process recorded - `started_by`, `http`, `allowed_hosts`), so both
/// paths render through [`render_status`] and `--json` yields one stable shape
/// either way. Reads whatever [`reach_index`] reached, and is the one verb
/// here that refuses rather than degrading: a status with no numbers in it
/// would be a report about nothing.
pub async fn status_value(route: IndexRoute, cfg: &GlobalConfig) -> Result<serde_json::Value> {
    let registered: Vec<String> = cfg.domains.keys().cloned().collect();
    let store = match route {
        IndexRoute::Direct(store) => store,
        // No index on this machine yet: the same "nothing synced" report the
        // direct path used to build for an absent database file.
        IndexRoute::Absent(db) => {
            return Ok(serde_json::json!({
                "indexed": false,
                "db_path": db.display().to_string(),
                "registered": registered,
            }));
        }
        IndexRoute::Unreachable(why) => return Err(index_unreachable("status", &why)),
        // The dispatch renders the daemon's own report and never sends one
        // here, but the daemon's report IS this function's return shape, so
        // handing it straight back is the honest total answer.
        IndexRoute::Daemon(data) => return Ok(data),
    };
    let store = store.lock().await;
    let info = store
        .store_info()
        .await
        .map_err(|e| anyhow!("could not read store info: {e}"))?;
    let stats = store
        .domain_stats()
        .await
        .map_err(|e| anyhow!("could not read domain stats: {e}"))?;
    let coverage = store
        .embedding_coverage()
        .await
        .map_err(|e| anyhow!("could not read embedding coverage: {e}"))?;

    // Coverage for the active model: how many chunks are embedded with it, and
    // whether hybrid search is therefore available.
    let active_model = configured_model_id(cfg.embeddings.as_ref());
    let active_embedded = coverage.embedded_for(&active_model);
    let hybrid_available = coverage.has_active_embeddings(&active_model);

    Ok(serde_json::json!({
        "indexed": true,
        "fts_mode": info.fts_mode,
        "schema_version": info.schema_version,
        "db_path": info.db_path,
        "db_size": info.db_size,
        "domains": stats,
        "registered": registered,
        "embeddings": {
            "active_model": active_model,
            "embedded_chunks": active_embedded,
            "total_chunks": coverage.total_chunks,
            "hybrid_available": hybrid_available,
            "models": coverage.models,
        },
    }))
}

/// Render a status report (the daemon's or the in-process one) as human
/// text. `daemon_note` says where the numbers come from - the one line that
/// keeps a fallback read from masquerading as the daemon's view.
pub fn render_status(data: &serde_json::Value, daemon_note: &str) {
    use serde_json::Value;

    println!("Daemon: {daemon_note}");
    // Only the daemon's own report carries these; a direct index read has no
    // daemon to describe, so the line is absent rather than guessed at. The
    // phrasing says "asked to bind" on purpose: these are the exposure facts
    // the daemon recorded on the way up, not a listener this command probed.
    if let Some(started_by) = data.get("started_by").and_then(Value::as_str) {
        let bound = data["http"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| "off".to_string());
        let hosts: Vec<&str> = data["allowed_hosts"]
            .as_array()
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let host_note = if hosts.is_empty() {
            "loopback only".to_string()
        } else {
            hosts.join(", ")
        };
        println!(
            "Exposure: asked to bind HTTP {bound}, Host allow-list {host_note} (started by {started_by})"
        );
        // A bounded life is the Claude Desktop extension's shape; a daemon
        // that outlives its clients has no line, not a "never".
        if let Some(secs) = data.get("idle_exit_secs").and_then(Value::as_u64) {
            println!("Lifetime: exits {secs}s after its last client disconnects");
        }
    }
    let registered: Vec<&str> = data["registered"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();

    if !data.get("indexed").and_then(Value::as_bool).unwrap_or(true) {
        if registered.is_empty() {
            // Nothing registered at all: pointing at `sync` here would send a
            // first-time user in a circle, since sync has nothing to sync
            // yet either.
            println!("No domains registered yet. Run: crystalline domain add <name> <path>");
        } else {
            println!(
                "No index at {} yet. Run: crystalline sync",
                data["db_path"].as_str().unwrap_or("(unknown)")
            );
            for name in registered {
                println!("{name}\t(not indexed yet)");
            }
        }
        return;
    }

    println!(
        "Index: {} ({} bytes, schema v{}, fts {})",
        data["db_path"].as_str().unwrap_or("(memory)"),
        data["db_size"].as_u64().unwrap_or(0),
        data["schema_version"].as_u64().unwrap_or(0),
        data["fts_mode"].as_str().unwrap_or("unknown")
    );
    let emb = &data["embeddings"];
    println!(
        "Embeddings: {}/{} chunks embedded with '{}', default search: {}",
        emb["embedded_chunks"].as_u64().unwrap_or(0),
        emb["total_chunks"].as_u64().unwrap_or(0),
        emb["active_model"].as_str().unwrap_or(""),
        if emb["hybrid_available"].as_bool().unwrap_or(false) {
            "hybrid"
        } else {
            "text"
        }
    );

    // The rebuild markers, printed directly under the coverage figure they
    // qualify: a number read as normal in the middle of a rebuild is the
    // incident this exists for, so the caveat travels with it rather than
    // sitting somewhere else in the report.
    //
    // The marker is durable and nothing clears it when a process is killed, so
    // it says history, not liveness. A live `reindex` in the daemon's activity
    // snapshot - which a direct read has none of - says a rebuild is running,
    // but not *which* domain's: the activity record carries no domain, so a
    // marker standing from a run that died last week and a rebuild running now
    // on another domain cannot be told apart from here. So the live line says
    // only what is known - a rebuild is running, and this domain is stamped -
    // and never claims the two are the same one. Either way the domain's rows
    // are complete, because a rebuild clears nothing and coverage can only go
    // up across one.
    let rebuild_is_live = data["activity"]["now"]
        .as_array()
        .is_some_and(|now| now.iter().any(|a| a["kind"].as_str() == Some("reindex")));
    for d in data["domains"].as_array().into_iter().flatten() {
        let Some(started) = d["rebuild_started"].as_str() else {
            continue;
        };
        let name = d["name"].as_str().unwrap_or("");
        let kind = d["rebuild_kind"].as_str();
        if rebuild_is_live {
            match kind {
                // A wipe empties the index before it rebuilds, so nothing about
                // "the rows from before it" is true while one runs.
                Some("wipe") => println!(
                    "  a rebuild is running now; '{name}' was stamped {started} by a wipe, which destroyed its rows and every embedding it had before it began"
                ),
                _ => println!(
                    "  a rebuild is running now; '{name}' was stamped {started} and its rows are the ones from before that rebuild lands"
                ),
            }
        } else {
            match kind {
                Some("wipe") => println!(
                    "  a wipe of '{name}' started {started} and did not finish; that domain's rows and every embedding it had were destroyed before it began, so what is there is only what the rebuild managed. Run: crystalline reindex --full"
                ),
                Some("full") => println!(
                    "  a full rebuild of '{name}' started {started} and did not finish; that domain's rows are the ones from before it. Run: crystalline reindex --full"
                ),
                // A marker a binary older than the kind column stamped: say what
                // is known and claim nothing about the rows either way.
                _ => println!(
                    "  a rebuild of '{name}' started {started} and did not finish. Run: crystalline reindex --full"
                ),
            }
        }
    }

    // What the daemon is doing right now; only its report carries this.
    if let Some(activity) = data.get("activity") {
        let running = activity["now"].as_array().cloned().unwrap_or_default();
        if running.is_empty() {
            println!("Activity: idle");
        }
        for entry in &running {
            let domain = entry["domain"]
                .as_str()
                .map(|d| format!(" '{d}'"))
                .unwrap_or_default();
            println!(
                "Activity: {}{} ({}s)",
                entry["kind"].as_str().unwrap_or("working"),
                domain,
                entry["for_secs"].as_u64().unwrap_or(0)
            );
        }
        let backlog = activity["embedding_backlog"].as_u64().unwrap_or(0);
        if backlog > 0 {
            println!("  embedding backlog: {backlog} chunks");
        }
    }

    let domains = data["domains"].as_array().cloned().unwrap_or_default();
    if domains.is_empty() && registered.is_empty() {
        println!("No domains indexed yet.");
    }
    let mut indexed_names = std::collections::BTreeSet::new();
    for d in &domains {
        let name = d["name"].as_str().unwrap_or("");
        indexed_names.insert(name.to_string());
        let last_sync = d["last_sync"].as_str().map(relative_time);
        println!(
            "{}\t{} engrams, {} observations, {} relations ({} unresolved), {} links ({} unresolved)\tlast sync {}",
            name,
            d["engrams"].as_u64().unwrap_or(0),
            d["observations"].as_u64().unwrap_or(0),
            d["relations"].as_u64().unwrap_or(0),
            d["unresolved_relations"].as_u64().unwrap_or(0),
            d["links"].as_u64().unwrap_or(0),
            d["unresolved_links"].as_u64().unwrap_or(0),
            last_sync.as_deref().unwrap_or("never")
        );
    }
    // Registered domains the index holds no row for yet.
    for name in registered {
        if !indexed_names.contains(name) {
            println!("{name}\t(not indexed yet)");
        }
    }

    // Team-origin schedule: what the background poller has planned and how
    // its last pass went, present only when collaboration is enabled.
    for d in data["origins"]["domains"]
        .as_array()
        .iter()
        .flat_map(|a| a.iter())
    {
        let next = d["next_due"]
            .as_str()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|t| {
                let secs = (t.with_timezone(&chrono::Utc) - chrono::Utc::now()).num_seconds();
                if secs > 0 {
                    format!("in {secs}s")
                } else {
                    "due now".to_string()
                }
            })
            .unwrap_or_else(|| "not scheduled".to_string());
        let last = d["last_result"]["outcome"].as_str().unwrap_or("never");
        println!(
            "Origin '{}' ({}): next poll {}, last {}",
            d["domain"].as_str().unwrap_or(""),
            d["repo"].as_str().unwrap_or(""),
            next,
            last
        );
    }
}

/// Show per-domain counts and index diagnostics from the index the dispatch
/// reached. `daemon_note` says which view this is, so a direct read never
/// masquerades as a running daemon's.
pub async fn status(
    route: IndexRoute,
    cfg: &GlobalConfig,
    json: bool,
    daemon_note: &str,
) -> Result<()> {
    let value = status_value(route, cfg).await?;
    if json {
        println!("{value}");
    } else {
        render_status(&value, daemon_note);
    }
    Ok(())
}

// --- model download ----------------------------------------------------------

/// Pre-fetch the local embedding model, printing the cache path and size. Exits
/// non-zero (via the returned error) when the fetch fails or the build has no
/// local embedding support.
pub async fn model_download(config_override: Option<&Path>, json: bool) -> Result<()> {
    let cfg = load(config_override)?.effective;
    let ecfg = embeddings_config(&cfg);
    let download = download_local_model(&ecfg)
        .await
        .map_err(|e| anyhow!("model download failed: {e}"))?;

    let mb = download.bytes as f64 / (1024.0 * 1024.0);
    if json {
        println!(
            "{}",
            serde_json::json!({
                "path": download.path.display().to_string(),
                "bytes": download.bytes,
            })
        );
    } else {
        println!("Model ready at {} ({mb:.1} MB)", download.path.display());
    }
    Ok(())
}

// --- import --------------------------------------------------------------

/// Import a markdown knowledge base with YAML frontmatter into a registered
/// domain: normalize legacy `type` values, backfill temporal metadata, drop
/// sentinel open-ended dates, strip a source permalink prefix and record write
/// provenance where a file carries none. Pure file transformation: never
/// touches the index,
/// the socket or the network.
#[allow(clippy::too_many_arguments)]
pub fn import(
    src: &Path,
    domain: &str,
    map: Option<&Path>,
    strip_prefix: Option<&str>,
    dry_run: bool,
    config_override: Option<&Path>,
    json: bool,
) -> Result<()> {
    let cfg = load(config_override)?.effective;
    let entry = cfg.domains.get(domain).ok_or_else(|| {
        anyhow!(
            "no domain named '{domain}' is registered. Register it first: crystalline domain add {domain} <path>"
        )
    })?;
    // The legacy converter targets a file domain's directory. A virtual domain
    // has no directory, so point the user at `crystalline domain import`.
    let domain_dir = resolve_domain_path(entry).ok_or_else(|| {
        anyhow!(
            "domain '{domain}' is virtual and has no directory; load engrams into it with `crystalline domain import <path> --domain {domain}` instead"
        )
    })?;

    let type_map = match map {
        Some(p) => {
            let file: crystalline_core::import::TypeMapFile = config::load_yaml(p)
                .map_err(|e| anyhow!("failed to load --map {}: {e}", p.display()))?;
            crystalline_core::import::merge_type_map(&file.mappings)
        }
        None => crystalline_core::import::default_type_map(),
    };

    let options = crystalline_core::import::ImportOptions {
        src_dir: src.to_path_buf(),
        domain_dir,
        type_map,
        strip_prefix: strip_prefix.map(str::to_string),
        dry_run,
    };
    let report = crystalline_core::import::import_tree(&options)
        .map_err(|e| anyhow!("import failed: {e}"))?;

    if json {
        println!("{}", serde_json::to_string(&report)?);
    } else {
        print_import_report(&report, dry_run);
    }

    // Printed to stderr, never stdout, so `--json` output stays a single
    // parseable value. `import` never auto-syncs; this is only a hint.
    if !dry_run {
        eprintln!("Run: crystalline sync --domain {domain}");
    }
    Ok(())
}

fn print_import_report(r: &crystalline_core::import::ImportReport, dry_run: bool) {
    use std::io::Write as _;

    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::new(stdout.lock());
    if dry_run {
        writeln!(out, "Dry run: no files were written.").unwrap();
    }
    writeln!(
        out,
        "{} converted, {} copied, {} skipped",
        r.files_converted, r.files_copied, r.files_skipped
    )
    .unwrap();
    writeln!(
        out,
        "type mapped: {}, temporal backfilled: {}, sentinels dropped: {}, prefixes stripped: {}, collisions: {}",
        r.type_mapped,
        r.temporal_backfilled,
        r.sentinels_dropped,
        r.prefixes_stripped,
        r.collisions
    )
    .unwrap();
    for w in &r.warnings {
        writeln!(out, "  warning: {w}").unwrap();
    }
    for f in &r.files {
        if !f.changes.is_empty() {
            writeln!(out, "  {}", f.path).unwrap();
            for c in &f.changes {
                writeln!(out, "    {c}").unwrap();
            }
        }
    }
    out.flush().unwrap();
}

// --- connect github ------------------------------------------------------------

/// The credential `connect github` addresses: this machine's own by default,
/// the machine owner's personal identity under `--personal`, or the account
/// `--as` names (admin use, for a bot identity remote agents share as).
///
/// `--as` is normalized here - trimmed and lowercased - because that is the
/// shape every other layer mints an account name in: the auth store folds a
/// login name exactly this way before storing it, so `--as Bot` and `--as bot`
/// have to address the one credential that account's own Fluid connect would.
/// The token store's allowlist is the belt to that braces, and a name that
/// still fails it is taught HERE, naming the class a name may be drawn from,
/// rather than surfacing the store's generic refusal at the end of a sign-in.
///
/// The rejected name is quoted through `escape_debug`: it failed the allowlist,
/// so unlike everywhere else an identity name is interpolated it may carry a
/// control byte or a terminal escape, and this message is printed to a
/// terminal.
pub(crate) fn connect_identity(
    personal: bool,
    account: Option<&str>,
) -> Result<crystalline_remote::TokenIdentity> {
    use crystalline_remote::TokenIdentity;

    if !personal {
        // clap holds `--as` to `--personal`, so there is no named account to
        // lose here: this is the machine credential, exactly as before.
        return Ok(TokenIdentity::Instance);
    }
    let Some(account) = account else {
        return Ok(TokenIdentity::Personal(
            crystalline_service::engine::OWNER_IDENTITY_NAME.to_string(),
        ));
    };
    let name = account.trim().to_lowercase();
    if !crystalline_remote::valid_identity_name(&name) {
        bail!(
            "'{}' cannot name a personal GitHub identity: an account name may hold only \
             lowercase letters, digits, '.', '_' and '-', and at most {} bytes. Use the \
             account name Crystalline knows this person by.",
            account.escape_debug(),
            crystalline_remote::MAX_IDENTITY_NAME_BYTES
        );
    }
    Ok(TokenIdentity::Personal(name))
}

/// `crystalline connect github`: sign this machine in to GitHub, always
/// in-process (no daemon involved - signing in is this machine's identity,
/// not content, so there is nothing for a daemon to route). A personal
/// access token skips the browser sign-in entirely; otherwise runs the OAuth
/// device flow, printing the short code and verification url unmissably
/// before waiting on it to be confirmed. Works whether or not team domains
/// are turned on yet; prints a one-line hint to turn them on when they are
/// currently off.
///
/// `identity` decides which credential is written: this machine's, or one
/// person's personal one (see [`connect_identity`]). The two are separate
/// credentials in the same store, so connecting a personal identity never
/// disturbs the machine's - and `CRYSTALLINE_GITHUB_TOKEN` only refuses the
/// machine's, since that variable fixes the MACHINE's identity and an instance
/// that sets it is exactly the kind that shares personally.
///
/// `disconnect` forgets the addressed credential instead of connecting one,
/// and is allowed even while `CRYSTALLINE_GITHUB_TOKEN` is set: that variable
/// fixes which identity this machine ACTS as, while a disconnect deletes what
/// is stored, and deleting a stored credential the environment is currently
/// shadowing is a perfectly meaningful thing to want (it is how a machine stops
/// holding a token it no longer uses). The environment is untouched either way;
/// unsetting the variable is what changes who this machine is.
///
/// A daemon that already resolved that credential holds it in a
/// process-lifetime cache and never re-reads a deleted one, so a disconnect
/// here is told to it over the control socket ([`notify_daemon_forgot`]) - a
/// credential is forgotten because somebody wants it to stop working, and
/// "stops working at the next restart" is the wrong answer to that. No daemon
/// running is nothing to tell; a daemon that could not be told is said out
/// loud rather than left to be discovered.
///
/// `token_store_dir` forces the credential to a plain file under that
/// directory instead of the OS keychain, mirroring
/// [`crystalline_service::engine::Engine::with_token_store_dir`]. Test-only:
/// production passes `None` and a real connect writes through the keychain.
pub async fn connect_github(
    token: Option<&str>,
    host: Option<&str>,
    identity: &crystalline_remote::TokenIdentity,
    disconnect: bool,
    config_override: Option<&Path>,
    token_store_dir: Option<&Path>,
    json: bool,
) -> Result<()> {
    let instance = *identity == crystalline_remote::TokenIdentity::Instance;
    let loaded = load(config_override)?;
    if instance && !disconnect && loaded.overlay.github_token().is_some() {
        bail!(
            "this machine's GitHub identity comes from CRYSTALLINE_GITHUB_TOKEN; unset it to sign in interactively"
        );
    }
    let cfg = loaded.effective;
    let api_url = host
        .map(|h| format!("https://{h}/api/v3"))
        .or_else(|| cfg.github.as_ref().and_then(|g| g.api_url.clone()));
    let auth_base = crystalline_remote::github::auth::auth_base(api_url.as_deref());
    let token_host = bare_host(&auth_base);
    let state_dir = match token_store_dir {
        Some(dir) => dir.to_path_buf(),
        None => config::origins_state_dir()
            .map_err(|e| anyhow!("could not resolve the state directory: {e}"))?,
    };

    if disconnect {
        match token_store_dir {
            // The seam's own disconnect: the file beside the connect it
            // undoes, and no keychain touched on the way.
            Some(dir) => forget_file_credential(identity, dir),
            None => forget_credential(identity, token_host.as_deref(), &state_dir),
        }
        let told = notify_daemon_forgot(identity).await;
        if json {
            let mut report = serde_json::json!({
                "disconnected": identity_label(identity),
                "daemon": told.as_str(),
            });
            if let DaemonNotice::Refused(e) = &told {
                report["daemon_error"] = serde_json::json!(e);
            }
            println!("{report}");
        } else {
            println!("Disconnected {}.", identity_phrase(identity));
            if let DaemonNotice::Refused(e) = &told {
                println!(
                    "A running daemon could not be told ({e}); restart it so it stops sharing with the credential this just deleted."
                );
            }
        }
        return Ok(());
    }

    let client_id = cfg
        .github
        .as_ref()
        .and_then(|g| g.oauth_client_id.clone())
        .unwrap_or_else(|| crystalline_remote::GITHUB_CLIENT_ID.to_string());

    let (access_token, login) = match token {
        Some(pat) => {
            let login = crystalline_remote::github::auth::validate_token(api_url.as_deref(), pat)
                .await
                .map_err(|e| anyhow!("{e}"))?;
            (pat.to_string(), login)
        }
        None => device_flow_sign_in(&auth_base, &client_id, api_url.as_deref()).await?,
    };

    // One keychain write, no read: `save_resolving_for` writes straight through
    // and lands in the file store only if the keychain write itself fails.
    let stored = crystalline_remote::StoredToken {
        access_token,
        host: token_host
            .clone()
            .unwrap_or_else(|| "github.com".to_string()),
        user: login.clone(),
        created_at: chrono::Utc::now(),
    };
    let store = match token_store_dir {
        Some(dir) => {
            let store = crystalline_remote::TokenStore::file_fallback_for(identity, dir)
                .map_err(|e| anyhow!("{e}"))?;
            store.save(&stored).map_err(|e| anyhow!("{e}"))?;
            store
        }
        None => crystalline_remote::TokenStore::save_resolving_for(
            identity,
            token_host.as_deref(),
            &state_dir,
            &stored,
        )
        .map_err(|e| anyhow!("{e}"))?,
    };

    // A personal connect that lands on an instance-mode installation is a
    // credential nothing will use yet, so the mode is named where it is
    // actionable. `github.enabled` is left to the instance connect to hint at:
    // turning collaboration on is an instance-wide decision, not something a
    // person connecting their own identity is making.
    let personal_mode = matches!(
        cfg.github_share_identity(),
        crystalline_core::config::ShareIdentityMode::Personal
    );
    if json {
        let mut report = serde_json::json!({
            "connected": login,
            "token_store": store.kind(),
            "github_enabled": cfg.github_enabled(),
        });
        if !instance {
            report["identity"] = serde_json::json!("personal");
            report["account"] = serde_json::json!(identity_label(identity));
            report["share_identity"] = serde_json::json!(cfg.github_share_identity().as_str());
        }
        println!("{report}");
    } else if instance {
        println!(
            "Connected to GitHub as {login} ({} token store).",
            store.kind()
        );
        if !cfg.github_enabled() {
            println!("Run: crystalline config set github.enabled true to turn on team domains");
        }
    } else {
        println!(
            "Connected {} as {login} ({} token store).",
            identity_phrase(identity),
            store.kind()
        );
        if !personal_mode {
            println!(
                "Run: crystalline config set github.share_identity personal to share with personal identities"
            );
        }
    }
    Ok(())
}

/// Deletes `identity`'s credential wherever this machine keeps it: the backend
/// the store resolves to, plus the file fallback beside it. Both are attempted
/// and every failure is swallowed with a debug log, because this runs from
/// paths that must not fail for it - a disconnect that already found nothing,
/// and the sweep behind `users disable`/`users remove`.
///
/// Both backends rather than just the resolved one: the file fallback is
/// written whenever the keychain was unusable at connect time, and a machine
/// whose keychain works again would otherwise resolve to the keychain and leave
/// a real token behind on disk.
pub(crate) fn forget_credential(
    identity: &crystalline_remote::TokenIdentity,
    host: Option<&str>,
    state_dir: &Path,
) {
    match crystalline_remote::TokenStore::resolve_and_load_for(identity, host, state_dir) {
        Ok((store, _token)) => {
            if let Err(e) = store.delete() {
                tracing::debug!("could not forget the {} credential: {e}", store.kind());
            }
        }
        Err(e) => tracing::debug!("could not resolve the credential to forget: {e}"),
    }
    match crystalline_remote::TokenStore::file_fallback_for(identity, state_dir) {
        Ok(store) => {
            if let Err(e) = store.delete() {
                tracing::debug!("could not forget the file credential: {e}");
            }
        }
        Err(e) => tracing::debug!("could not resolve the file credential to forget: {e}"),
    }
}

/// The directory `CRYSTALLINE_TEST_TOKEN_STORE_DIR` names, when it names one.
///
/// The test-only seam behind [`connect_github`]'s `token_store_dir`, needed
/// because the CLI's own end-to-end tests drive a real child process: an
/// in-process builder override like the engine's cannot cross that boundary,
/// and a `connect` that reached the OS keychain would write to the
/// developer's own login keychain and stop for its dialog. Named `TEST` for
/// the same reason `CRYSTALLINE_TEST_POSTGRES_URL` is: it is not a knob an
/// install is meant to set, and nothing documents it as one.
///
/// Redirecting where a token is written is not a privilege escalation: a
/// process that can set this can already set `CRYSTALLINE_GITHUB_TOKEN` and
/// decide which credential this machine acts as outright.
pub(crate) fn test_token_store_dir() -> Option<PathBuf> {
    std::env::var_os("CRYSTALLINE_TEST_TOKEN_STORE_DIR").map(PathBuf::from)
}

/// Deletes `identity`'s credential from the file store under `dir` and touches
/// nothing else. The disconnect half of the token-store seam
/// ([`connect_github`]'s `token_store_dir`): a test must not reach the
/// developer's own keychain even to ask it whether it holds something.
fn forget_file_credential(identity: &crystalline_remote::TokenIdentity, dir: &Path) {
    match crystalline_remote::TokenStore::file_fallback_for(identity, dir) {
        Ok(store) => {
            if let Err(e) = store.delete() {
                tracing::debug!("could not forget the file credential: {e}");
            }
        }
        Err(e) => tracing::debug!("could not resolve the file credential to forget: {e}"),
    }
}

/// What telling a running daemon about a forgotten credential came to.
///
/// Three states rather than two, because two of them are only the same answer
/// to a person. "Nothing holds the credential now" covers both a daemon that
/// took the message and no daemon at all, and that is what the human line
/// says by staying quiet - but a machine field named for one of them and set
/// on both would assert something that did not happen.
pub(crate) enum DaemonNotice {
    /// A daemon was running and dropped its cached copy.
    Notified,
    /// No daemon was running, so nothing was holding a copy to drop.
    NotRunning,
    /// A daemon is running and did not take the message: it goes on holding a
    /// credential this machine no longer has until it restarts.
    Refused(String),
}

impl DaemonNotice {
    /// The machine-readable word for this outcome.
    fn as_str(&self) -> &'static str {
        match self {
            DaemonNotice::Notified => "notified",
            DaemonNotice::NotRunning => "not_running",
            DaemonNotice::Refused(_) => "refused",
        }
    }
}

/// Tells a running daemon that this machine just forgot `identity`'s
/// credential, so it drops the copy its process cache is holding.
///
/// Best effort in every direction: the credential is already gone from the
/// store by the time this runs, and nothing here can put it back or fail the
/// command that deleted it.
pub(crate) async fn notify_daemon_forgot(
    identity: &crystalline_remote::TokenIdentity,
) -> DaemonNotice {
    let account = match identity {
        crystalline_remote::TokenIdentity::Instance => serde_json::Value::Null,
        crystalline_remote::TokenIdentity::Personal(name) => serde_json::json!(name),
    };
    match crystalline_service::client::ctl_if_running(serde_json::json!({
        "cmd": "forget_credential",
        "account": account,
    }))
    .await
    {
        Ok(Some(_)) => DaemonNotice::Notified,
        Ok(None) => DaemonNotice::NotRunning,
        Err(e) => DaemonNotice::Refused(e.to_string()),
    }
}

/// The account name a credential is addressed by, for machine output:
/// `"instance"` for this machine's own, the account name for a personal one.
fn identity_label(identity: &crystalline_remote::TokenIdentity) -> String {
    match identity {
        crystalline_remote::TokenIdentity::Instance => "instance".to_string(),
        crystalline_remote::TokenIdentity::Personal(name) => name.clone(),
    }
}

/// The same, in words, for the line a person reads.
fn identity_phrase(identity: &crystalline_remote::TokenIdentity) -> String {
    match identity {
        crystalline_remote::TokenIdentity::Instance => "this machine's GitHub identity".to_string(),
        crystalline_remote::TokenIdentity::Personal(name)
            if name == crystalline_service::engine::OWNER_IDENTITY_NAME =>
        {
            "your personal GitHub identity".to_string()
        }
        crystalline_remote::TokenIdentity::Personal(name) => {
            format!("the personal GitHub identity for '{name}'")
        }
    }
}

/// Runs the OAuth device flow to completion: prints the user code and
/// verification url, ticks a progress indicator while waiting for it to be
/// confirmed in the browser, then validates the issued token to learn the
/// signed-in login. Returns `(access_token, login)`.
async fn device_flow_sign_in(
    auth_base: &str,
    client_id: &str,
    api_url: Option<&str>,
) -> Result<(String, String)> {
    let start = crystalline_remote::github::auth::start_device_flow(auth_base, client_id)
        .await
        .map_err(|e| anyhow!("{e}"))?;
    print_device_code(&start, auth_base);

    let ticker = tokio::spawn(async {
        let mut ticks: u32 = 0;
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            eprint!(".");
            let _ = std::io::Write::flush(&mut std::io::stderr());
            ticks += 1;
            // A minute of dots and still nothing: the most common reason is
            // the person entered the code and closed the tab without
            // clicking Authorize, so say so once rather than dotting forever.
            if ticks == 60 {
                eprintln!();
                eprintln!(
                    "Still waiting - did the page after the code show an Authorize button? The sign-in lands when it is clicked."
                );
                eprint!("Waiting for confirmation");
            }
        }
    });
    let poll =
        crystalline_remote::github::auth::run_device_flow(auth_base, client_id, &start).await;
    ticker.abort();
    eprintln!();
    let access_token = poll.map_err(|e| device_flow_error(auth_base, e))?;

    let login = crystalline_remote::github::auth::validate_token(api_url, &access_token)
        .await
        .map_err(|e| anyhow!("{e}"))?;
    Ok((access_token, login))
}

/// Prints the device flow's user code and verification url unmissably: this
/// is the moment a non-engineer copies a code into a browser. The line under
/// the box is the confirmation guidance's first sentence - what to do next,
/// not just where to type the code - so the same warning that trips people
/// up (closing the tab instead of clicking Authorize) is in view up front.
fn print_device_code(start: &crystalline_remote::DeviceFlowStart, auth_base: &str) {
    eprintln!();
    eprintln!("================================================");
    eprintln!("  Go to: {}", start.verification_url);
    eprintln!("  Enter this code: {}", start.user_code);
    eprintln!("================================================");
    eprintln!(
        "{}",
        first_sentence(&crystalline_remote::github::auth::confirmation_guidance(
            auth_base
        ))
    );
    eprint!("Waiting for confirmation");
}

/// The first sentence of `text`, period included - `confirmation_guidance`'s
/// opening sentence is the one line of it that fits under the code box; the
/// rest (the applications url, the enterprise policy note) is repeated in
/// full elsewhere rather than crammed in here.
fn first_sentence(text: &str) -> &str {
    match text.find(". ") {
        Some(period) => &text[..=period],
        None => text,
    }
}

/// Maps a `run_device_flow` outcome to the error `crystalline connect
/// github` prints. `RemoteError::AuthExpired` means the device code expired
/// before the browser side finished - GitHub's own reason for that says
/// nothing about Authorize, so this says it: what happened, the Authorize
/// reminder repeated, and where to check whether an earlier attempt already
/// landed. Every other error passes through unchanged; a declined sign-in,
/// offline and the rest already carry their own actionable message.
fn device_flow_error(auth_base: &str, e: crystalline_remote::RemoteError) -> anyhow::Error {
    if matches!(e, crystalline_remote::RemoteError::AuthExpired) {
        // "Next time:" frames the repeated Authorize sentence as advice for
        // the retry rather than an instruction to act on a code that no
        // longer exists - the sentence itself is reused verbatim from
        // `confirmation_guidance` (via `first_sentence`) rather than
        // reworded here, so there is still exactly one place that wording
        // lives.
        anyhow!(
            "The code expired before it was authorized. Next time: {} Check {} to see whether an earlier attempt already landed.",
            first_sentence(&crystalline_remote::github::auth::confirmation_guidance(
                auth_base
            )),
            crystalline_remote::github::auth::authorized_apps_url(auth_base)
        )
    } else {
        anyhow!("{e}")
    }
}

/// The bare host `TokenStore::save_resolving` and `resolve_and_load` address,
/// derived from an auth base the same way the engine's origin operations derive
/// it from
/// `github.api_url`: `None` for GitHub.com, the bare host for a GitHub
/// Enterprise Server auth base. Kept in step with
/// `crystalline_service::origin`'s private twin of this function so a token
/// saved here is found again by a later origin operation reading
/// `github.api_url` back from config. `pub(crate)` so `doctor` can resolve the
/// same token store it reports on without duplicating the derivation.
pub(crate) fn bare_host(auth_base: &str) -> Option<String> {
    let bare = auth_base
        .trim_start_matches("https://")
        .trim_start_matches("http://");
    if bare == "github.com" {
        None
    } else {
        Some(bare.to_string())
    }
}

// --- healthcheck ---------------------------------------------------------------

/// Wall-clock deadline for the whole probe in [`healthcheck`], comfortably
/// inside the container image's 5s `HEALTHCHECK` timeout.
const HEALTHCHECK_DEADLINE: std::time::Duration = std::time::Duration::from_secs(4);

/// `crystalline healthcheck`: probe a serving daemon's `GET /health` endpoint
/// with a hand-rolled HTTP/1.1 request over a plain `TcpStream` - no tokio
/// runtime, no TLS, no daemon socket, config or database touched. That
/// narrow surface is the point: this is what an external monitor (Docker's
/// own `HEALTHCHECK`, a Kubernetes `httpGet` probe, a load balancer) sees, so
/// a green result here means those see green too. `0.0.0.0` and `[::]` are
/// rewritten to `127.0.0.1` first - valid addresses to bind, never valid to
/// dial as a client. The whole probe runs under one [`HEALTHCHECK_DEADLINE`]
/// wall-clock deadline, comfortably inside the container image's 5s
/// `HEALTHCHECK` timeout: `set_read_timeout` alone only bounds a single
/// syscall, not the whole read, so a peer trickling bytes could re-arm the
/// clock indefinitely; connect, write and every read are instead each
/// capped at whatever time remains before the deadline, tracked by hand
/// since there is no thread involved to enforce it from outside. On
/// success, prints the health body (the `{"status":"ok","version":...}` JSON
/// that also lands in `docker inspect`, carrying `started_by` and `http`
/// beside those two: how the daemon was started and the endpoint it was asked
/// to bind, so the container `HEALTHCHECK` surfaces the start mode in `docker
/// inspect` too. Both are added keys, so a monitor reading `status` is
/// unaffected, and the Host allow-list is deliberately not among them - this
/// route is unguarded, so it carries nothing an unauthenticated caller should
/// not read. `crystalline status` is where the allow-list is reported) and
/// returns `Ok`; any failure - connection refused, a timeout, a non-200 status
/// or a malformed response - comes back as a single-line `Err` naming the
/// address it failed against, so the process exits nonzero through the normal
/// error path.
///
/// B14 exemption: unlike the other data verbs, this default output is not given
/// a human rendering and does not honor `--json`. The line printed here is the
/// daemon's own `/health` HTTP body echoed verbatim, not a locally composed set
/// of checks, and it is a machine-consumed contract: the container `HEALTHCHECK`
/// captures it into `docker inspect`, and `tests/service.rs` asserts the default
/// output contains `"status":"ok"`. Reshaping it would break those consumers for
/// no gain (the body is already the canonical machine JSON), so the behavior is
/// left as-is deliberately.
pub(crate) fn healthcheck(addr: &str) -> Result<()> {
    use std::io::{Read, Write};
    use std::net::{TcpStream, ToSocketAddrs};
    use std::time::{Duration, Instant};

    let deadline = Instant::now() + HEALTHCHECK_DEADLINE;
    let connect_addr = crystalline_service::instance::loopback_connect_addr(addr);

    // The one thing standing in for a real aggregate deadline: recompute the
    // time left before every blocking step and refuse to arm a timeout once
    // it hits zero (a zero-duration `set_read_timeout` is an error on some
    // platforms, so this also guards that case).
    let remaining_or_bail = || -> Result<Duration> {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            bail!(
                "health probe to {connect_addr} exceeded its {}s deadline",
                HEALTHCHECK_DEADLINE.as_secs()
            );
        }
        Ok(remaining)
    };

    let socket_addr = connect_addr
        .as_str()
        .to_socket_addrs()
        .map_err(|e| anyhow!("resolving {connect_addr}: {e}"))?
        .next()
        .ok_or_else(|| anyhow!("no address resolved for {connect_addr}"))?;

    let connect_timeout = remaining_or_bail()?.min(Duration::from_secs(2));
    let mut stream = TcpStream::connect_timeout(&socket_addr, connect_timeout)
        .map_err(|e| anyhow!("connecting to {connect_addr}: {e}"))?;

    let write_timeout = remaining_or_bail()?.min(Duration::from_secs(2));
    stream
        .set_write_timeout(Some(write_timeout))
        .map_err(|e| anyhow!("setting a write timeout for {connect_addr}: {e}"))?;

    let request =
        format!("GET /health HTTP/1.1\r\nHost: {connect_addr}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .map_err(|e| anyhow!("sending the health request to {connect_addr}: {e}"))?;

    // A manual read loop instead of a bare `read_to_string`: that call would
    // block until EOF with only a per-syscall timeout behind it, so a slow
    // peer could keep it alive well past the aggregate deadline above.
    let mut buf = [0u8; 4096];
    let mut response = Vec::new();
    loop {
        let remaining = remaining_or_bail()?;
        stream
            .set_read_timeout(Some(remaining))
            .map_err(|e| anyhow!("setting a read timeout for {connect_addr}: {e}"))?;
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => response.extend_from_slice(&buf[..n]),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                bail!(
                    "health probe to {connect_addr} exceeded its {}s deadline",
                    HEALTHCHECK_DEADLINE.as_secs()
                );
            }
            Err(e) => bail!("reading the health response from {connect_addr}: {e}"),
        }
    }
    let response = String::from_utf8_lossy(&response).into_owned();

    let status_line = response
        .lines()
        .next()
        .ok_or_else(|| anyhow!("empty response from {connect_addr}"))?;
    let mut tokens = status_line.split_ascii_whitespace();
    let (Some(_), Some(code)) = (tokens.next(), tokens.next()) else {
        bail!("malformed status line from {connect_addr}: {status_line}");
    };
    if code != "200" {
        bail!("unhealthy response from {connect_addr}: {status_line}");
    }

    let (_, body) = response.split_once("\r\n\r\n").ok_or_else(|| {
        anyhow!("malformed response from {connect_addr}: missing header separator")
    })?;
    println!("{}", body.trim());
    Ok(())
}

// --- shared helpers ----------------------------------------------------------

fn select_domains(cfg: &GlobalConfig, only: Option<&str>) -> Result<Vec<(String, DomainEntry)>> {
    match only {
        Some(name) => {
            let entry = cfg
                .domains
                .get(name)
                .ok_or_else(|| anyhow!("no domain named '{name}' is registered"))?;
            Ok(vec![(name.to_string(), entry.clone())])
        }
        None => {
            if cfg.domains.is_empty() {
                bail!("no domains registered. Add one with: crystalline domain add <name> <path>");
            }
            Ok(cfg
                .domains
                .iter()
                .map(|(n, e)| (n.clone(), e.clone()))
                .collect())
        }
    }
}

fn print_report(r: &crystalline_index::SyncReport) {
    // The deferred count only prints when non-zero, so a quiet uncontended sync
    // reads the same as before and a busy one surfaces the skipped changes.
    let deferred = if r.deferred > 0 {
        format!(", {} deferred", r.deferred)
    } else {
        String::new()
    };
    // Likewise the cross-domain pass: silent on a run that had nothing left to
    // settle, and explicit when references only resolved once every other
    // domain of the run was indexed.
    let late = if r.relations_resolved_late > 0 || r.links_resolved_late > 0 {
        format!(
            " ({} relations, {} links resolved across domains at the end)",
            r.relations_resolved_late, r.links_resolved_late
        )
    } else {
        String::new()
    };
    println!(
        "{}: {} added, {} updated, {} deleted, {} moved, {} unchanged{}, {} relations resolved, {} links resolved ({} ms){}",
        r.domain,
        r.added,
        r.updated,
        r.deleted,
        r.moved,
        r.unchanged,
        deferred,
        r.relations_resolved,
        r.links_resolved,
        r.duration_ms,
        late
    );
    for (path, err) in &r.failed {
        println!("  failed: {path}: {err}");
    }
}

#[cfg(test)]
mod origin_share_tests {
    use super::origin_share_lines;
    use serde_json::json;

    #[test]
    fn a_commit_prints_the_branch_the_link_the_summary_and_the_counts() {
        let lines = origin_share_lines(
            "kb",
            &json!({
                "outcome": "committed", "sha": "9f2c1a7deadbeef", "url": "https://github.com/acme/knowledge/commit/9f2c1a7deadbeef",
                "branch": "main", "added": ["notes/b.md"], "updated": ["notes/a.md"], "deleted": [], "skipped_large": [],
                "summary": "Shares 1 new engram and refines 1 engram.",
            }),
        );
        assert_eq!(
            lines[0],
            "Committed to main: https://github.com/acme/knowledge/commit/9f2c1a7deadbeef"
        );
        assert_eq!(lines[1], "  Shares 1 new engram and refines 1 engram.");
        assert_eq!(lines[2], "  1 added, 1 updated, 0 deleted");
        let no_url = origin_share_lines(
            "kb",
            &json!({ "outcome": "committed", "sha": "9f2c1a7deadbeef", "url": null, "branch": "main", "added": [], "updated": [], "deleted": [], "skipped_large": [], "summary": "s" }),
        );
        assert_eq!(
            no_url[0], "Committed to main: 9f2c1a7",
            "the short sha stands in for a forge with no page"
        );
    }

    #[test]
    fn the_three_direct_refusals_say_the_way_out() {
        let open = origin_share_lines(
            "kb",
            &json!({ "outcome": "proposal_open", "proposal": { "number": 4, "url": "https://github.com/acme/knowledge/pull/4", "title": "Refine" }, "guidance": "g" }),
        );
        assert_eq!(
            open[0],
            "Cannot share 'kb': proposal #4 (https://github.com/acme/knowledge/pull/4) is still open; merge or withdraw it first."
        );
        let protected = origin_share_lines(
            "kb",
            &json!({ "outcome": "branch_protected", "branch": "main", "message": "Changes must be made through a pull request.", "guidance": "g" }),
        );
        assert_eq!(
            protected[0],
            "Cannot commit to main: Changes must be made through a pull request. Set sharing: proposal in the MANIFEST, or ask a repository admin."
        );
        let moved = origin_share_lines(
            "kb",
            &json!({ "outcome": "branch_moved", "branch": "main", "guidance": "g" }),
        );
        assert_eq!(
            moved[0],
            "The branch main moved while sharing; run: crystalline origin update --domain kb and share again."
        );
    }
}

#[cfg(test)]
mod review_plan_tests {
    use super::review_plan_lines;

    /// An actor whose files could not be listed is in the plan, and the plan
    /// says so.
    ///
    /// Leaving review mode refuses outright while any part of the overlay
    /// cannot be read, so the person answering the plan has to learn it from
    /// the plan rather than from a refusal they did not expect.
    #[test]
    fn the_plan_names_an_actor_whose_files_could_not_be_read() {
        let plan = serde_json::json!({
            "actors": [
                { "actor": "ada", "entries": 0, "files_unreadable": true, "drafts": [] },
            ],
            "contested_paths": [],
            "contested_addresses": [],
        });
        let lines = review_plan_lines(&plan).join("\n");
        assert!(
            lines.contains("the files ada has drafted could not be read"),
            "the plan names them and says what could not be read: {lines}"
        );
        assert!(
            lines.contains("review mode cannot be taken off until they can"),
            "and what that means for the answer being asked for: {lines}"
        );
    }

    /// A file in the plan is named as one, on the actor's own line and on its
    /// own row.
    ///
    /// What folding does to a page and what it does to a file are different
    /// enough to be worth a word: a page becomes text the team reads, a file
    /// becomes bytes in the team's folder. A plan that called both "drafts"
    /// would leave the person answering it to find that out afterwards.
    #[test]
    fn the_plan_says_which_of_the_draft_changes_is_a_file() {
        let plan = serde_json::json!({
            "actors": [
                { "actor": "ada", "entries": 3, "drafts": [
                    { "path": "alpha.md", "permalink": "alpha", "tombstone": false, "conflict": null },
                    { "path": "assets/deck.png", "kind": "file", "tombstone": false, "conflict": null },
                    { "path": "assets/old.png", "kind": "file", "tombstone": true, "conflict": null },
                ]},
                { "actor": "bo", "entries": 1, "drafts": [
                    { "path": "beta.md", "permalink": "beta", "tombstone": false, "conflict": null }
                ]},
            ],
            "contested_paths": [],
            "contested_addresses": [],
        });
        let lines = review_plan_lines(&plan).join("\n");
        assert!(
            lines.contains("ada (3 draft changes, 2 of them files)"),
            "her line counts them and says how many are files: {lines}"
        );
        assert!(
            lines.contains("    drafted the file assets/deck.png")
                && lines.contains("    deleted the file assets/old.png"),
            "and each file row says which it is: {lines}"
        );
        assert!(
            lines.contains("  bo (1 draft(s))"),
            "an actor drafting no files reads exactly as they always did: {lines}"
        );
        assert!(
            lines.contains("    drafted alpha.md"),
            "and so does a page: {lines}"
        );
    }

    /// Both kinds of trouble a fold runs into reach the person answering the
    /// plan, because a plan that shows one and hides the other is a clean plan
    /// that then refuses - which is the one thing this whole direction is
    /// built to avoid.
    #[test]
    fn the_plan_names_a_contested_path_and_a_contested_address() {
        let plan = serde_json::json!({
            "actors": [
                { "actor": "ada", "entries": 1, "drafts": [
                    { "path": "alpha.md", "permalink": "shared", "tombstone": false, "conflict": null }
                ]},
                { "actor": "bo", "entries": 1, "drafts": [
                    { "path": "beta.md", "permalink": "shared", "tombstone": false, "conflict": null }
                ]},
            ],
            "contested_paths": [{ "path": "plan.md", "actors": ["ada", "bo"] }],
            "contested_addresses": [
                { "permalink": "shared", "paths": ["alpha.md", "beta.md"], "actors": ["ada", "bo"] }
            ],
        });
        let lines = review_plan_lines(&plan).join("\n");
        assert!(
            lines.contains("plan.md is drafted by ada and bo"),
            "the contested path: {lines}"
        );
        assert!(
            lines.contains("'shared'") && lines.contains("alpha.md") && lines.contains("beta.md"),
            "the contested address names itself and both paths: {lines}"
        );
    }

    /// A domain nobody is drafting in says so and asks for nothing.
    #[test]
    fn an_empty_plan_asks_for_no_answer() {
        let lines = review_plan_lines(&serde_json::json!({ "actors": [] })).join("\n");
        assert!(lines.contains("Nobody is drafting"), "{lines}");
        assert!(!lines.contains("--fold"), "{lines}");
    }
}

#[cfg(test)]
mod index_reach_words_tests {
    use super::{daemon_answered_badly, listing_not_reached};

    /// A daemon that answered badly is a different state from a locked file,
    /// and says so without borrowing the lock sentence.
    #[test]
    fn a_bad_answer_names_the_daemon_not_a_lock() {
        let words = daemon_answered_badly("this listing", "domain_stats failed");
        assert!(words.contains("running Crystalline daemon"), "{words}");
        assert!(words.contains("crystalline doctor --fix"), "{words}");
        assert!(
            words.ends_with("The daemon reported: domain_stats failed"),
            "{words}"
        );
        assert!(!words.contains("owns the index at"), "{words}");
    }

    /// A failure that never reached a daemon does not blame one. The daemon
    /// probe and the database-path resolution both fail through the same
    /// `reach_index` return, and only one of them is the daemon's doing.
    #[test]
    fn a_failure_that_never_reached_a_daemon_does_not_name_one() {
        let words = listing_not_reached(
            "this listing",
            "could not resolve the default database path",
        );
        assert!(!words.contains("daemon"), "{words}");
        assert!(
            words.contains("crystalline doctor"),
            "a remedy a person can paste: {words}"
        );
        assert!(
            words.ends_with("The failure was: could not resolve the default database path"),
            "and the raw text trails rather than leads: {words}"
        );
    }
}

#[cfg(test)]
mod connect_identity_tests {
    use super::connect_identity;
    use crystalline_remote::{TokenIdentity, TokenStore};
    use crystalline_service::engine::OWNER_IDENTITY_NAME;

    /// No `--personal` is the machine credential, exactly as before this flag
    /// existed: the one an install that never hears of personal identities
    /// keeps writing.
    #[test]
    fn without_the_flag_the_machine_credential_is_addressed() {
        assert_eq!(
            connect_identity(false, None).unwrap(),
            TokenIdentity::Instance
        );
    }

    /// `--personal` alone is the machine owner, under the one fixed local name
    /// the engine resolves an owner share against - taken from the engine's own
    /// constant rather than re-typed here, so the two can never drift.
    #[test]
    fn personal_alone_addresses_the_owner_slot() {
        assert_eq!(
            connect_identity(true, None).unwrap(),
            TokenIdentity::Personal(OWNER_IDENTITY_NAME.to_string())
        );
    }

    /// `--as` is normalized before it addresses anything: the auth store folds
    /// a login name to trimmed lowercase, so `--as Bot` has to reach the same
    /// credential the account 'bot' connects for itself in Fluid.
    #[test]
    fn an_account_name_is_trimmed_and_lowercased_before_it_addresses_a_credential() {
        assert_eq!(
            connect_identity(true, Some("  Release-Bot.1  ")).unwrap(),
            TokenIdentity::Personal("release-bot.1".to_string())
        );
    }

    /// A name that still cannot address a credential after normalization is
    /// taught in the CLI's own words, naming the class a name may be drawn
    /// from - not handed the token store's generic refusal.
    #[test]
    fn a_name_the_allowlist_refuses_is_taught_rather_than_passed_through() {
        let err = connect_identity(true, Some("Ann+Lee"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("Ann+Lee"), "{err}");
        assert!(err.contains("lowercase letters, digits"), "{err}");
        assert!(
            !err.contains("is not a usable account name"),
            "the token store's generic refusal must not be what a caller sees: {err}"
        );

        // A control byte in the name reaches a terminal, so it is escaped
        // rather than printed.
        let err = connect_identity(true, Some("bo\u{1b}[31mt"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("\\u{1b}"), "{err}");
        assert!(!err.contains('\u{1b}'), "{err}");
    }

    /// The credential a personal connect writes is a file of its own beside the
    /// machine's, never the machine's: this pins the addressing end to end
    /// through the same derivation `save_resolving_for` uses internally, which
    /// is as far as a test can follow it without writing to the developer's
    /// real keychain.
    #[test]
    fn the_owner_credential_is_a_separate_file_from_the_machines() {
        let dir = tempfile::tempdir().unwrap();
        let owner =
            TokenStore::file_fallback_for(&connect_identity(true, None).unwrap(), dir.path())
                .unwrap();
        let bot = TokenStore::file_fallback_for(
            &connect_identity(true, Some("BOT")).unwrap(),
            dir.path(),
        )
        .unwrap();
        let machine =
            TokenStore::file_fallback_for(&connect_identity(false, None).unwrap(), dir.path())
                .unwrap();
        let path = |store: &TokenStore| match store {
            TokenStore::File { path } => path.clone(),
            other => panic!("a file fallback is a file: {other:?}"),
        };
        assert!(
            path(&owner).ends_with("github-token-personal-owner.json"),
            "{:?}",
            path(&owner)
        );
        assert!(
            path(&bot).ends_with("github-token-personal-bot.json"),
            "{:?}",
            path(&bot)
        );
        assert!(
            path(&machine).ends_with("github-token.json"),
            "{:?}",
            path(&machine)
        );
    }
}

#[cfg(test)]
mod first_sentence_tests {
    use super::first_sentence;

    #[test]
    fn the_period_is_included_and_nothing_after_it() {
        assert_eq!(first_sentence("One. Two. Three."), "One.");
    }

    #[test]
    fn text_with_no_period_space_comes_back_whole() {
        assert_eq!(
            first_sentence("No sentence break here"),
            "No sentence break here"
        );
    }
}

#[cfg(test)]
mod device_flow_error_tests {
    use super::device_flow_error;

    /// The one mapped case: an expired code gets the Authorize reminder and
    /// the applications url, not GitHub's bare "device_code expired".
    #[test]
    fn auth_expired_repeats_the_authorize_sentence_and_the_applications_url() {
        let err = device_flow_error(
            "https://github.com",
            crystalline_remote::RemoteError::AuthExpired,
        )
        .to_string();
        assert!(err.contains("expired"), "{err}");
        assert!(err.contains("Authorize"), "{err}");
        assert!(
            err.contains("https://github.com/settings/connections/applications"),
            "{err}"
        );
    }

    /// A GHES auth base carries through to the applications url in the
    /// mapped message, same as everywhere else this is derived.
    #[test]
    fn auth_expired_derives_the_applications_url_from_a_ghes_auth_base() {
        let err = device_flow_error(
            "https://github.example.com",
            crystalline_remote::RemoteError::AuthExpired,
        )
        .to_string();
        assert!(
            err.contains("https://github.example.com/settings/connections/applications"),
            "{err}"
        );
    }

    /// Every other error passes through unchanged - it already carries its
    /// own actionable message.
    #[test]
    fn every_other_error_passes_through_unchanged() {
        let err = device_flow_error(
            "https://github.com",
            crystalline_remote::RemoteError::Offline,
        )
        .to_string();
        assert_eq!(err, crystalline_remote::RemoteError::Offline.to_string());
    }
}

#[cfg(test)]
mod relative_time_tests {
    use super::{humanize_duration, relative_time};

    #[test]
    fn humanize_duration_buckets_at_each_unit_boundary() {
        assert_eq!(humanize_duration(0), "0s");
        assert_eq!(humanize_duration(59), "59s");
        assert_eq!(humanize_duration(60), "1m");
        assert_eq!(humanize_duration(3_599), "59m");
        assert_eq!(humanize_duration(3_600), "1h");
        assert_eq!(humanize_duration(86_399), "23h");
        assert_eq!(humanize_duration(86_400), "1d");
        assert_eq!(humanize_duration(172_800), "2d");
    }

    #[test]
    fn relative_time_renders_past_instants_as_a_duration_ago() {
        let five_minutes_ago = (chrono::Utc::now() - chrono::Duration::minutes(5)).to_rfc3339();
        assert_eq!(relative_time(&five_minutes_ago), "5m ago");

        let three_days_ago = (chrono::Utc::now() - chrono::Duration::days(3)).to_rfc3339();
        assert_eq!(relative_time(&three_days_ago), "3d ago");
    }

    #[test]
    fn relative_time_falls_back_to_the_original_value_when_unparsable_or_future() {
        assert_eq!(relative_time("never"), "never");
        assert_eq!(relative_time(""), "");

        // A future instant (clock skew, a deliberately future value) must
        // never be rendered as "ago" - that would actively mislead.
        let five_minutes_from_now =
            (chrono::Utc::now() + chrono::Duration::minutes(5)).to_rfc3339();
        assert_eq!(relative_time(&five_minutes_from_now), five_minutes_from_now);
    }
}
