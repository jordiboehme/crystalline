//! Implementations of the data and domain-management subcommands.
//!
//! These are the first subcommands that touch the derived index. For now they
//! open the database directly in-process; the M5 daemon will route them over the
//! control socket when one is running, falling back to this direct path. The
//! spot where that dispatch slots in is [`open_store`].

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use crystalline_core::config::{self, DomainEntry, GlobalConfig};
use crystalline_index::{Store, TursoStore, sync_domain};

/// Resolve the global config path from an optional override.
fn config_path(override_path: Option<&Path>) -> Result<PathBuf> {
    match override_path {
        Some(p) => Ok(p.to_path_buf()),
        None => config::global_config_path()
            .map_err(|e| anyhow!("could not resolve the default config path: {e}")),
    }
}

/// Load the global config, treating a missing file as an empty config.
fn load_config(path: &Path) -> Result<GlobalConfig> {
    if path.is_file() {
        config::load_yaml(path)
            .map_err(|e| anyhow!("failed to load config {}: {e}", path.display()))
    } else {
        Ok(GlobalConfig::default())
    }
}

/// Resolve the index database path from an optional override.
fn db_path(override_path: Option<&Path>) -> Result<PathBuf> {
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
fn resolve_domain_path(entry: &DomainEntry) -> PathBuf {
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

/// Sync one or all registered domains.
pub async fn sync(
    only: Option<&str>,
    config_override: Option<&Path>,
    db_override: Option<&Path>,
    json: bool,
) -> Result<()> {
    let cfg = load_config(&config_path(config_override)?)?;
    let targets = select_domains(&cfg, only)?;
    let store = open_store(&db_path(db_override)?).await?;

    let mut reports = Vec::new();
    for (name, entry) in targets {
        let path = resolve_domain_path(&entry);
        let report = sync_domain(&store, &name, &path)
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
    Ok(())
}

// --- reindex -----------------------------------------------------------------

/// Reindex all domains. `--full` wipes the index first (the corruption-recovery
/// path), opening resiliently so a database that will not open is rebuilt.
pub async fn reindex(
    full: bool,
    config_override: Option<&Path>,
    db_override: Option<&Path>,
    json: bool,
) -> Result<()> {
    let cfg = load_config(&config_path(config_override)?)?;
    let targets = select_domains(&cfg, None)?;
    let db = db_path(db_override)?;

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
        let report = sync_domain(&store, &name, &path)
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

    if json {
        println!(
            "{}",
            serde_json::json!({
                "indexed": true,
                "store": info,
                "domains": stats,
                "registered": cfg.domains.keys().collect::<Vec<_>>(),
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
