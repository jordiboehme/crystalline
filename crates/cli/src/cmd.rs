//! Implementations of the data and domain-management subcommands.
//!
//! These are the first subcommands that touch the derived index. For now they
//! open the database directly in-process; the M5 daemon will route them over the
//! control socket when one is running, falling back to this direct path. The
//! spot where that dispatch slots in is [`open_store`].

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, anyhow, bail};
use crystalline_core::config::{
    self, DatabaseBackend, DomainEntry, EmbeddingsConfig, GlobalConfig,
};
use crystalline_index::{
    ChunkParams, DomainKind, Store, configured_model_id, download_local_model,
    provider_from_config, run_embedding_pass, sync_domain_with,
};
use tokio::sync::Mutex as TokioMutex;

/// The embeddings config to use: the configured one, or the local bge default.
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

/// Open the configured backend as a `dyn Store` through the shared factory, so
/// these standalone commands honor `backend: postgres` (or a Turso file at the
/// resolved path) without a running daemon, exactly like the daemon and doctor
/// paths do. `resilient` selects the corruption-recovery open for Turso (the
/// `reindex --full` recovery path) and is ignored by Postgres.
///
/// The M5 daemon dispatch still slots in above this: when a service socket is
/// live the command routes over it instead of opening the database in-process.
async fn open_backend(
    cfg: &GlobalConfig,
    db_override: Option<&Path>,
    resilient: bool,
) -> Result<Arc<TokioMutex<dyn Store>>> {
    crystalline_index::open_store(&cfg.database(), db_override, resilient)
        .await
        .map_err(|e| anyhow!("could not open the index: {e}"))
}

/// Whether the effective backend is the local Turso file (so an absent file
/// means "no index yet"). Postgres has no local file and is always opened.
fn backend_is_turso(cfg: &GlobalConfig) -> bool {
    cfg.database().backend == DatabaseBackend::Turso
}

/// The absolute, tilde-expanded filesystem root a file domain points at, or
/// `None` for a virtual domain (which has no path).
pub(crate) fn resolve_domain_path(entry: &DomainEntry) -> Option<PathBuf> {
    entry.file_path().filter(|_| !entry.is_virtual())
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
pub(crate) fn domain_add_register(
    name: &str,
    path: &Path,
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

    let manifest = path.join("MANIFEST.md");
    if !manifest.exists() {
        bail!(
            "no MANIFEST.md at {}. Run: crystalline domain init {}",
            path.display(),
            path.display()
        );
    }
    let abs = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());

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

/// Sync a single, just-registered domain directly (no daemon involved) and
/// return its report. Parse failures in individual files land in the
/// report's `failed` list rather than aborting; only a harder error (the
/// store will not open, the transaction fails) is propagated.
pub(crate) async fn sync_domain_direct(
    name: &str,
    root: &Path,
    config_override: Option<&Path>,
    db_override: Option<&Path>,
) -> Result<crystalline_index::SyncReport> {
    let cfg = load(config_override)?.effective;
    let store = open_backend(&cfg, db_override, false).await?;
    let store = store.lock().await;
    let params = chunk_params(&cfg);
    sync_domain_with(&*store, name, root, &params)
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

// --- origin share, discard and resolve ----------------------------------------

/// Print `origin share`'s result: the proposal URL and change summary when
/// one was opened, the friendly "nothing to share" line when the team
/// already has everything the domain knows, or (when conflicts are still
/// pending) every conflicting path plus a pointer at `origin resolve`.
pub(crate) fn print_origin_share(domain: &str, data: &serde_json::Value, json: bool) {
    if json {
        println!("{data}");
        return;
    }
    let empty = Vec::new();
    match data["outcome"].as_str().unwrap_or("") {
        "proposed" => {
            println!("Shared: {}", data["url"].as_str().unwrap_or(""));
            if let Some(summary) = data["summary"].as_str() {
                println!("  {summary}");
            }
            let added = data["added"].as_array().map(Vec::len).unwrap_or(0);
            let updated = data["updated"].as_array().map(Vec::len).unwrap_or(0);
            let deleted = data["deleted"].as_array().map(Vec::len).unwrap_or(0);
            println!("  {added} added, {updated} updated, {deleted} deleted");
            print_skipped_large(&data["skipped_large"]);
        }
        "nothing_to_share" => {
            println!("Nothing to share: '{domain}' already matches its origin.");
            print_skipped_large(&data["skipped_large"]);
        }
        "conflicts_pending" => {
            println!(
                "Cannot share '{domain}': {} conflict(s) need to be resolved first.",
                data["count"].as_u64().unwrap_or(0)
            );
            for c in data["conflicts"].as_array().unwrap_or(&empty) {
                println!("  conflict: {}", c["path"].as_str().unwrap_or(""));
            }
            println!("Run: crystalline origin resolve {domain} <path> --keep mine|theirs");
        }
        other => println!("origin share '{domain}': unexpected outcome '{other}'"),
    }
}

fn print_skipped_large(skipped_large: &serde_json::Value) {
    let empty = Vec::new();
    for s in skipped_large.as_array().unwrap_or(&empty) {
        println!(
            "  skipped (too large): {} ({} bytes)",
            s[0].as_str().unwrap_or(""),
            s[1].as_u64().unwrap_or(0)
        );
    }
}

/// Print `origin discard`'s result: the restored, deleted and skipped
/// (diverged since sharing) paths, or a friendly line when there was
/// nothing to discard.
pub(crate) fn print_origin_discard(data: &serde_json::Value, json: bool) {
    if json {
        println!("{data}");
        return;
    }
    let empty = Vec::new();
    let restored = data["restored"].as_array().unwrap_or(&empty);
    let deleted = data["deleted"].as_array().unwrap_or(&empty);
    let skipped = data["skipped_diverged"].as_array().unwrap_or(&empty);
    if restored.is_empty() && deleted.is_empty() && skipped.is_empty() {
        println!("Nothing to discard.");
        return;
    }
    for p in restored {
        println!("restored: {}", p.as_str().unwrap_or(""));
    }
    for p in deleted {
        println!("deleted: {}", p.as_str().unwrap_or(""));
    }
    for p in skipped {
        println!(
            "left alone (diverged since sharing): {}",
            p.as_str().unwrap_or("")
        );
    }
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

/// Remove a domain from the global config. Leaves its files and index rows
/// untouched; the rows are only dropped by a later full reindex.
pub fn domain_remove(name: &str, config_override: Option<&Path>, json: bool) -> Result<()> {
    let loaded = load(config_override)?;
    let mut cfg = loaded.file;
    if cfg.domains.shift_remove(name).is_none() {
        // A miss in the file config may be an env-defined domain: those are
        // immune to `domain remove` (the variable is their source of truth).
        if let Some(env) = loaded.overlay.env_domain(name) {
            bail!(
                "domain '{name}' is defined by the environment variable {}; unset it to manage this domain in the config file",
                env.var
            );
        }
        bail!("no domain named '{name}' is registered");
    }
    config::save_yaml(&loaded.path, &cfg)
        .map_err(|e| anyhow!("failed to save config {}: {e}", loaded.path.display()))?;
    if json {
        println!(
            "{}",
            serde_json::json!({
                "removed": name,
                "note": "index rows for this domain remain until the next full reindex",
            })
        );
    } else {
        println!("Removed domain '{name}' (files and index rows left untouched)");
        println!("Run: crystalline reindex --full to drop its rows from the index");
    }
    Ok(())
}

// --- domain list -------------------------------------------------------------

/// List registered domains, with engram counts when the index is present.
pub async fn domain_list(
    config_override: Option<&Path>,
    db_override: Option<&Path>,
    json: bool,
) -> Result<()> {
    // Keep the whole `LoadedConfig`: reads use the effective config, and the
    // overlay marks which rows an environment variable defines.
    let loaded = load(config_override)?;
    let cfg = loaded.effective;
    let should_open = !backend_is_turso(&cfg) || db_path(db_override)?.exists();
    let stats = if should_open {
        match open_backend(&cfg, db_override, false).await {
            Ok(store) => store.lock().await.domain_stats().await.ok(),
            Err(_) => None,
        }
    } else {
        None
    };
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
        println!("{}", serde_json::json!({ "domains": domains }));
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
        // In a shared database a file domain names the instance that hosts it.
        let host = host_for(name)
            .map(|(id, _)| format!("\thosted by {id}"))
            .unwrap_or_default();
        match count_for(name) {
            Some(n) => println!("{name}\t{location}\t{n} engrams{host}"),
            None => println!("{name}\t{location}\t(not indexed){host}"),
        }
    }
    Ok(())
}

// --- sync --------------------------------------------------------------------

/// Sync one or all registered domains, optionally embedding new chunks after.
pub async fn sync(
    only: Option<&str>,
    embed: bool,
    config_override: Option<&Path>,
    db_override: Option<&Path>,
    json: bool,
) -> Result<()> {
    let cfg = load(config_override)?.effective;
    let targets = select_domains(&cfg, only)?;
    let store = open_backend(&cfg, db_override, false).await?;
    let store = store.lock().await;
    let params = chunk_params(&cfg);

    let mut reports = Vec::new();
    for (name, entry) in targets {
        // Virtual domains have no files to sync.
        let Some(path) = resolve_domain_path(&entry) else {
            continue;
        };
        let report = sync_domain_with(&*store, &name, &path, &params)
            .await
            .map_err(|e| anyhow!("sync of '{name}' failed: {e}"))?;
        reports.push(report);
    }

    if json {
        println!("{}", serde_json::to_string(&reports)?);
    } else {
        for r in &reports {
            print_report(r);
        }
    }

    if embed {
        embed_pass(&*store, &cfg).await?;
    }
    Ok(())
}

// --- reindex -----------------------------------------------------------------

/// Reindex all domains. `--full` wipes the index first (the corruption-recovery
/// path), opening resiliently so a database that will not open is rebuilt.
pub async fn reindex(
    full: bool,
    embed: bool,
    config_override: Option<&Path>,
    db_override: Option<&Path>,
    json: bool,
) -> Result<()> {
    let cfg = load(config_override)?.effective;
    let targets = select_domains(&cfg, None)?;
    let params = chunk_params(&cfg);

    // `--full` opens resiliently (Turso rebuilds a database that will not open;
    // a no-op for Postgres). Rather than a global wipe, it clears each file
    // domain's rows per-domain and resyncs, so virtual-domain rows, whose only
    // source of truth is the database, survive the reindex.
    let store = open_backend(&cfg, db_override, full).await?;
    let store = store.lock().await;
    // Only the file domains have files to (re)index.
    let file_targets: Vec<(String, PathBuf)> = targets
        .into_iter()
        .filter_map(|(name, entry)| resolve_domain_path(&entry).map(|p| (name, p)))
        .collect();
    if full {
        for (name, path) in &file_targets {
            let domain_id = store
                .upsert_domain(name, Some(&path.to_string_lossy()), DomainKind::File)
                .await
                .map_err(|e| anyhow!("failed to resolve domain '{name}': {e}"))?;
            store
                .clear_domain(domain_id)
                .await
                .map_err(|e| anyhow!("failed to clear domain '{name}': {e}"))?;
        }
    }

    let mut reports = Vec::new();
    for (name, path) in &file_targets {
        let report = sync_domain_with(&*store, name, path, &params)
            .await
            .map_err(|e| anyhow!("reindex of '{name}' failed: {e}"))?;
        reports.push(report);
    }

    if json {
        println!(
            "{}",
            serde_json::json!({ "full": full, "reports": reports })
        );
    } else {
        println!(
            "Reindex ({}) complete",
            if full { "full" } else { "incremental" }
        );
        for r in &reports {
            print_report(r);
        }
    }

    if embed {
        embed_pass(&*store, &cfg).await?;
    }
    Ok(())
}

// --- status ------------------------------------------------------------------

/// Build the in-process status report in the same shape the daemon's ctl
/// `status` returns (minus its liveness fields), so both paths render through
/// [`render_status`] and `--json` yields one stable shape either way.
pub async fn status_value(
    config_override: Option<&Path>,
    db_override: Option<&Path>,
) -> Result<serde_json::Value> {
    let cfg = load(config_override)?.effective;
    let registered: Vec<String> = cfg.domains.keys().cloned().collect();
    // Only the Turso backend has a local file whose absence means "no index
    // yet"; Postgres is always opened.
    if backend_is_turso(&cfg) {
        let db = db_path(db_override)?;
        if !db.exists() {
            return Ok(serde_json::json!({
                "indexed": false,
                "db_path": db.display().to_string(),
                "registered": registered,
            }));
        }
    }

    let store = open_backend(&cfg, db_override, false).await?;
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
    let registered: Vec<&str> = data["registered"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();

    if !data.get("indexed").and_then(Value::as_bool).unwrap_or(true) {
        println!(
            "No index at {} yet. Run: crystalline sync",
            data["db_path"].as_str().unwrap_or("(unknown)")
        );
        for name in registered {
            println!("{name}\t(not indexed yet)");
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
        println!(
            "{}\t{} engrams, {} observations, {} relations ({} unresolved)\tlast sync {}",
            name,
            d["engrams"].as_u64().unwrap_or(0),
            d["observations"].as_u64().unwrap_or(0),
            d["relations"].as_u64().unwrap_or(0),
            d["unresolved_relations"].as_u64().unwrap_or(0),
            d["last_sync"].as_str().unwrap_or("never")
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

/// Show per-domain counts and index diagnostics from a directly opened
/// index. `daemon_note` explains why the daemon was not consulted.
pub async fn status(
    config_override: Option<&Path>,
    db_override: Option<&Path>,
    json: bool,
    daemon_note: &str,
) -> Result<()> {
    let value = status_value(config_override, db_override).await?;
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
/// sentinel open-ended dates, strip a source permalink prefix and add a
/// missing `timestamp`. Pure file transformation: never touches the index,
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
    if dry_run {
        println!("Dry run: no files were written.");
    }
    println!(
        "{} converted, {} copied, {} skipped",
        r.files_converted, r.files_copied, r.files_skipped
    );
    println!(
        "type mapped: {}, temporal backfilled: {}, sentinels dropped: {}, prefixes stripped: {}, collisions: {}",
        r.type_mapped,
        r.temporal_backfilled,
        r.sentinels_dropped,
        r.prefixes_stripped,
        r.collisions
    );
    for w in &r.warnings {
        println!("  warning: {w}");
    }
    for f in &r.files {
        if !f.changes.is_empty() {
            println!("  {}", f.path);
            for c in &f.changes {
                println!("    {c}");
            }
        }
    }
}

// --- connect github ------------------------------------------------------------

/// `crystalline connect github`: sign this machine in to GitHub, always
/// in-process (no daemon involved - signing in is this machine's identity,
/// not content, so there is nothing for a daemon to route). A personal
/// access token skips the browser sign-in entirely; otherwise runs the OAuth
/// device flow, printing the short code and verification url unmissably
/// before waiting on it to be confirmed. Works whether or not team domains
/// are turned on yet; prints a one-line hint to turn them on when they are
/// currently off. Refuses up front when `CRYSTALLINE_GITHUB_TOKEN` is set:
/// this machine's identity is already fixed by the environment, so there is
/// nothing for an interactive sign-in to change.
pub async fn connect_github(
    token: Option<&str>,
    host: Option<&str>,
    config_override: Option<&Path>,
    json: bool,
) -> Result<()> {
    let loaded = load(config_override)?;
    if loaded.overlay.github_token().is_some() {
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

    let state_dir = config::origins_state_dir()
        .map_err(|e| anyhow!("could not resolve the state directory: {e}"))?;
    // One keychain write, no read: `save_resolving` writes straight through
    // and lands in the file store only if the keychain write itself fails.
    let stored = crystalline_remote::StoredToken {
        access_token,
        host: token_host
            .clone()
            .unwrap_or_else(|| "github.com".to_string()),
        user: login.clone(),
        created_at: chrono::Utc::now(),
    };
    let store =
        crystalline_remote::TokenStore::save_resolving(token_host.as_deref(), &state_dir, &stored)
            .map_err(|e| anyhow!("{e}"))?;

    if json {
        println!(
            "{}",
            serde_json::json!({
                "connected": login,
                "token_store": store.kind(),
                "github_enabled": cfg.github_enabled(),
            })
        );
    } else {
        println!(
            "Connected to GitHub as {login} ({} token store).",
            store.kind()
        );
        if !cfg.github_enabled() {
            println!("Run: crystalline config set github.enabled true to turn on team domains");
        }
    }
    Ok(())
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
    print_device_code(&start);

    let ticker = tokio::spawn(async {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            eprint!(".");
            let _ = std::io::Write::flush(&mut std::io::stderr());
        }
    });
    let poll =
        crystalline_remote::github::auth::run_device_flow(auth_base, client_id, &start).await;
    ticker.abort();
    eprintln!();
    let access_token = poll.map_err(|e| anyhow!("{e}"))?;

    let login = crystalline_remote::github::auth::validate_token(api_url, &access_token)
        .await
        .map_err(|e| anyhow!("{e}"))?;
    Ok((access_token, login))
}

/// Prints the device flow's user code and verification url unmissably: this
/// is the moment a non-engineer copies a code into a browser.
fn print_device_code(start: &crystalline_remote::DeviceFlowStart) {
    eprintln!();
    eprintln!("================================================");
    eprintln!("  Go to: {}", start.verification_url);
    eprintln!("  Enter this code: {}", start.user_code);
    eprintln!("================================================");
    eprint!("Waiting for confirmation");
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
/// that also lands in `docker inspect`) and returns `Ok`; any failure -
/// connection refused, a timeout, a non-200 status or a malformed response -
/// comes back as a single-line `Err` naming the address it failed against,
/// so the process exits nonzero through the normal error path.
pub(crate) fn healthcheck(addr: &str) -> Result<()> {
    use std::io::{Read, Write};
    use std::net::{TcpStream, ToSocketAddrs};
    use std::time::{Duration, Instant};

    let deadline = Instant::now() + HEALTHCHECK_DEADLINE;
    let connect_addr = loopback_connect_addr(addr);

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

/// Rewrite an unroutable bind address to its loopback equivalent: `0.0.0.0`
/// and `[::]` are addresses a server can listen on but a client can never
/// dial, and people naturally paste the same address they gave `serve --http`.
fn loopback_connect_addr(addr: &str) -> String {
    if let Some(port) = addr.strip_prefix("0.0.0.0:") {
        format!("127.0.0.1:{port}")
    } else if let Some(port) = addr.strip_prefix("[::]:") {
        format!("127.0.0.1:{port}")
    } else {
        addr.to_string()
    }
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
    println!(
        "{}: {} added, {} updated, {} deleted, {} moved, {} unchanged, {} resolved ({} ms)",
        r.domain,
        r.added,
        r.updated,
        r.deleted,
        r.moved,
        r.unchanged,
        r.relations_resolved,
        r.duration_ms
    );
    for (path, err) in &r.failed {
        println!("  failed: {path}: {err}");
    }
}
