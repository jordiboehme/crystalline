//! Implementations of the data and domain-management subcommands.
//!
//! These are the first subcommands that touch the derived index. For now they
//! open the database directly in-process; the M5 daemon will route them over the
//! control socket when one is running, falling back to this direct path. The
//! spot where that dispatch slots in is [`open_store`].

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use crystalline_core::config::{self, DomainEntry, EmbeddingsConfig, GlobalConfig};
use crystalline_index::{
    ChunkParams, Store, TursoStore, configured_model_id, download_local_model,
    provider_from_config, run_embedding_pass, sync_domain_with,
};

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
async fn embed_pass(store: &TursoStore, cfg: &GlobalConfig) -> Result<()> {
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

/// Resolve the global config path from an optional override.
pub(crate) fn config_path(override_path: Option<&Path>) -> Result<PathBuf> {
    match override_path {
        Some(p) => Ok(p.to_path_buf()),
        None => config::global_config_path()
            .map_err(|e| anyhow!("could not resolve the default config path: {e}")),
    }
}

/// Load the global config, treating a missing file as an empty config.
pub(crate) fn load_config(path: &Path) -> Result<GlobalConfig> {
    if path.is_file() {
        config::load_yaml(path)
            .map_err(|e| anyhow!("failed to load config {}: {e}", path.display()))
    } else {
        Ok(GlobalConfig::default())
    }
}

/// Resolve the index database path from an optional override.
pub(crate) fn db_path(override_path: Option<&Path>) -> Result<PathBuf> {
    match override_path {
        Some(p) => Ok(p.to_path_buf()),
        None => config::index_db_path()
            .map_err(|e| anyhow!("could not resolve the default database path: {e}")),
    }
}

/// Open (creating if needed) the store at the resolved database path.
///
/// The M5 daemon dispatch slots in here: when a service socket is live this
/// would attach to it instead of opening the database in-process.
async fn open_store(db: &Path) -> Result<TursoStore> {
    if let Some(parent) = db.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating state directory {}", parent.display()))?;
    }
    TursoStore::open(db)
        .await
        .map_err(|e| anyhow!("could not open the index at {}: {e}", db.display()))
}

/// The absolute, tilde-expanded path a domain entry points at.
pub(crate) fn resolve_domain_path(entry: &DomainEntry) -> PathBuf {
    config::expand_tilde(&entry.path.to_string_lossy())
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
        std::fs::write(&manifest, manifest_template(&domain_name, &today))
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

fn manifest_template(name: &str, today: &str) -> String {
    format!(
        "---\n\
type: manifest\n\
title: {name}\n\
permalink: manifest\n\
tags:\n  - manifest\n\
status: current\n\
recorded_at: {today}\n\
---\n\n\
# {name}\n\n\
## Scope\n\n\
- Describe the knowledge this domain covers\n\n\
## When to Use\n\n\
- Describe when an agent should route here\n\n\
## Notes for Agents\n\n\
- Add guidance for agents working in this domain\n"
    )
}

// --- domain add --------------------------------------------------------------

/// Register a domain in the global config. Refuses without a MANIFEST.md.
pub fn domain_add(
    name: &str,
    path: &Path,
    config_override: Option<&Path>,
    json: bool,
) -> Result<()> {
    let manifest = path.join("MANIFEST.md");
    if !manifest.exists() {
        bail!(
            "no MANIFEST.md at {}. Run: crystalline domain init {}",
            path.display(),
            path.display()
        );
    }
    let abs = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());

    let cfg_path = config_path(config_override)?;
    let mut cfg = load_config(&cfg_path)?;
    cfg.domains
        .insert(name.to_string(), DomainEntry { path: abs.clone() });
    config::save_yaml(&cfg_path, &cfg)
        .map_err(|e| anyhow!("failed to save config {}: {e}", cfg_path.display()))?;

    if json {
        println!(
            "{}",
            serde_json::json!({ "registered": name, "path": abs.display().to_string() })
        );
    } else {
        println!("Registered domain '{name}' at {}", abs.display());
    }
    Ok(())
}

// --- domain remove -----------------------------------------------------------

/// Remove a domain from the global config. Leaves its files untouched.
pub fn domain_remove(name: &str, config_override: Option<&Path>, json: bool) -> Result<()> {
    let cfg_path = config_path(config_override)?;
    let mut cfg = load_config(&cfg_path)?;
    if cfg.domains.shift_remove(name).is_none() {
        bail!("no domain named '{name}' is registered");
    }
    config::save_yaml(&cfg_path, &cfg)
        .map_err(|e| anyhow!("failed to save config {}: {e}", cfg_path.display()))?;
    if json {
        println!("{}", serde_json::json!({ "removed": name }));
    } else {
        println!("Removed domain '{name}' (files left untouched)");
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
    let cfg = load_config(&config_path(config_override)?)?;
    let db = db_path(db_override)?;
    let stats = if db.exists() {
        open_store(&db).await?.domain_stats().await.ok()
    } else {
        None
    };
    let count_for = |name: &str| -> Option<i64> {
        stats
            .as_ref()
            .and_then(|s| s.iter().find(|d| d.name == name))
            .map(|d| d.engrams)
    };

    if json {
        let domains: Vec<serde_json::Value> = cfg
            .domains
            .iter()
            .map(|(name, entry)| {
                serde_json::json!({
                    "name": name,
                    "path": entry.path.display().to_string(),
                    "engrams": count_for(name),
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
        match count_for(name) {
            Some(n) => println!("{name}\t{}\t{n} engrams", entry.path.display()),
            None => println!("{name}\t{}\t(not indexed)", entry.path.display()),
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
    let cfg = load_config(&config_path(config_override)?)?;
    let targets = select_domains(&cfg, only)?;
    let store = open_store(&db_path(db_override)?).await?;
    let params = chunk_params(&cfg);

    let mut reports = Vec::new();
    for (name, entry) in targets {
        let path = resolve_domain_path(&entry);
        let report = sync_domain_with(&store, &name, &path, &params)
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
        embed_pass(&store, &cfg).await?;
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
    let cfg = load_config(&config_path(config_override)?)?;
    let targets = select_domains(&cfg, None)?;
    let db = db_path(db_override)?;
    let params = chunk_params(&cfg);

    let store = if full {
        if let Some(parent) = db.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent).ok();
        }
        let store = TursoStore::open_resilient(&db).await.map_err(|e| {
            anyhow!(
                "could not open or rebuild the index at {}: {e}",
                db.display()
            )
        })?;
        store
            .wipe()
            .await
            .map_err(|e| anyhow!("failed to wipe the index: {e}"))?;
        store
    } else {
        open_store(&db).await?
    };

    let mut reports = Vec::new();
    for (name, entry) in targets {
        let path = resolve_domain_path(&entry);
        let report = sync_domain_with(&store, &name, &path, &params)
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
        embed_pass(&store, &cfg).await?;
    }
    Ok(())
}

// --- status ------------------------------------------------------------------

/// Show per-domain counts and index diagnostics.
pub async fn status(
    config_override: Option<&Path>,
    db_override: Option<&Path>,
    json: bool,
) -> Result<()> {
    let cfg = load_config(&config_path(config_override)?)?;
    let db = db_path(db_override)?;
    if !db.exists() {
        if json {
            println!(
                "{}",
                serde_json::json!({ "indexed": false, "db_path": db.display().to_string() })
            );
        } else {
            println!("No index at {} yet. Run: crystalline sync", db.display());
        }
        return Ok(());
    }

    let store = open_store(&db).await?;
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

    if json {
        println!(
            "{}",
            serde_json::json!({
                "indexed": true,
                "store": info,
                "domains": stats,
                "registered": cfg.domains.keys().collect::<Vec<_>>(),
                "embeddings": {
                    "active_model": active_model,
                    "embedded_chunks": active_embedded,
                    "total_chunks": coverage.total_chunks,
                    "hybrid_available": hybrid_available,
                    "models": coverage.models,
                },
            })
        );
        return Ok(());
    }

    println!(
        "Index: {} ({} bytes, schema v{}, fts {})",
        info.db_path.as_deref().unwrap_or("(memory)"),
        info.db_size.unwrap_or(0),
        info.schema_version,
        match info.fts_mode {
            crystalline_index::FtsMode::Native => "native",
            crystalline_index::FtsMode::CandidateScan => "candidate-scan",
        }
    );
    println!(
        "Embeddings: {active_embedded}/{} chunks embedded with '{active_model}' ({} dims), default search: {}",
        coverage.total_chunks,
        coverage
            .models
            .iter()
            .find(|m| m.model == active_model)
            .map(|m| m.dims)
            .unwrap_or(0),
        if hybrid_available { "hybrid" } else { "text" }
    );
    if stats.is_empty() {
        println!("No domains indexed yet.");
    }
    for d in &stats {
        println!(
            "{}\t{} engrams, {} observations, {} relations ({} unresolved)\tlast sync {}",
            d.name,
            d.engrams,
            d.observations,
            d.relations,
            d.unresolved_relations,
            d.last_sync.as_deref().unwrap_or("never")
        );
    }
    Ok(())
}

// --- model download ----------------------------------------------------------

/// Pre-fetch the local embedding model, printing the cache path and size. Exits
/// non-zero (via the returned error) when the fetch fails or the build has no
/// local embedding support.
pub async fn model_download(config_override: Option<&Path>, json: bool) -> Result<()> {
    let cfg = load_config(&config_path(config_override)?)?;
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
    let cfg = load_config(&config_path(config_override)?)?;
    let entry = cfg.domains.get(domain).ok_or_else(|| {
        anyhow!(
            "no domain named '{domain}' is registered. Register it first: crystalline domain add {domain} <path>"
        )
    })?;
    let domain_dir = resolve_domain_path(entry);

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
