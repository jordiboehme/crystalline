//! Engine-level tests for `Engine::provision`: allow persists the decision
//! into the config file this engine owns and returns a report, and a
//! read-only instance refuses allow but still answers status. There is no
//! ctl-level (socket) test harness in this crate yet (`control.rs` has no
//! dedicated test file - see `tests/configure.rs`'s own note), so this
//! exercises the engine method directly, one layer below the ctl
//! `provision` command that just forwards to it and serializes the same
//! report `Engine::provision` already returns.
//!
//! Unix only: `install_receipt_path`/`receipt_path`/a harness's artifact
//! base all resolve from `HOME`/`XDG_STATE_HOME`, so every test here
//! redirects them to a scratch directory, the same shared-lock,
//! preserve-and-restore technique `crates/core/tests/orchestrate.rs` uses
//! for the same reason.
#![cfg(unix)]

use std::ffi::OsString;
use std::path::Path;
use std::sync::Arc;

use crystalline_core::config::{DomainEntry, GlobalConfig};
use crystalline_index::TursoStore;
use crystalline_service::engine::ProvisionAction;
use crystalline_service::{Engine, EngineError, EnvOverlay};
use tokio::sync::Mutex as TokioMutex;

/// Serializes every `HOME`/`XDG_STATE_HOME`-mutating test in this binary. A
/// tokio mutex, not `std::sync::Mutex`: every test holds the guard across an
/// `.await`, which clippy's `await_holding_lock` flags for a std lock.
static HOME_LOCK: TokioMutex<()> = TokioMutex::const_new(());

fn set_env(home: &Path, xdg_state_home: &Path) -> (Option<OsString>, Option<OsString>) {
    let previous = (std::env::var_os("HOME"), std::env::var_os("XDG_STATE_HOME"));
    // SAFETY: guarded by HOME_LOCK; restored via restore_env before the test
    // returns.
    unsafe {
        std::env::set_var("HOME", home);
        std::env::set_var("XDG_STATE_HOME", xdg_state_home);
    }
    previous
}

fn restore_env(previous: (Option<OsString>, Option<OsString>)) {
    match previous.0 {
        Some(v) => unsafe { std::env::set_var("HOME", v) },
        None => unsafe { std::env::remove_var("HOME") },
    }
    match previous.1 {
        Some(v) => unsafe { std::env::set_var("XDG_STATE_HOME", v) },
        None => unsafe { std::env::remove_var("XDG_STATE_HOME") },
    }
}

// --- fixture helpers ---------------------------------------------------------

fn write(dir: &Path, rel: &str, content: &str) {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

/// A harbor-shaped MANIFEST declaring skills, commands and agents (no mcps -
/// this engine-level suite never wants a real harness CLI on `PATH`, which
/// `crates/cli/tests/provision.rs` already covers with a shim).
fn write_harbor(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(
        dir.join("MANIFEST.md"),
        "---\ntype: manifest\ntitle: harbor\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n\
         # harbor\n\n\
         ## Scope\n\n- Coastal navigation knowledge\n\n\
         ## When to Use\n\n- When docking\n\n\
         ## Provisioning\n\n- skills: skills\n- commands: commands\n- agents: agents\n",
    )
    .unwrap();
    write(
        dir,
        "skills/tide-tables/SKILL.md",
        "---\nname: tide-tables\n---\n\nReads the harbor's tide tables.\n",
    );
    write(
        dir,
        "commands/charts/plot-route.md",
        "Plot a route between two buoys.\n",
    );
    write(
        dir,
        "agents/quartermaster.md",
        "# Quartermaster\n\nKeeps the manifest of stores.\n",
    );
}

/// Mark claude-code onboarded in the install receipt this test's isolated
/// `XDG_STATE_HOME` resolves to, so `Engine::provision` finds a harness to
/// reconcile into.
fn write_install_receipt(xdg_state_home: &Path) {
    let path = xdg_state_home.join("crystalline").join("installs.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&serde_json::json!({
            "format": 1,
            "installs": [
                {
                    "harness": "claude-code",
                    "scope": "user",
                    "version": "0.0.0",
                    "parts": { "mcp": true, "hooks": true, "skills": true },
                    "skills": []
                }
            ]
        }))
        .unwrap(),
    )
    .unwrap();
}

/// A single-domain config: harbor, rooted at `harbor_dir`, undecided.
fn config_with_harbor(harbor_dir: &Path) -> GlobalConfig {
    let mut cfg = GlobalConfig::default();
    cfg.domains
        .insert("harbor".to_string(), DomainEntry::file(harbor_dir));
    cfg
}

async fn engine_at(config_path: &Path, config: GlobalConfig, read_only: bool) -> Engine {
    let store = TursoStore::open_in_memory().await.unwrap();
    Engine::new(
        Arc::new(TokioMutex::new(store)),
        config,
        None,
        Some(config_path.to_path_buf()),
    )
    .with_read_only(read_only)
}

// --- tests ---------------------------------------------------------------

#[tokio::test]
async fn allow_persists_the_decision_into_the_config_file_and_returns_a_report() {
    let _guard = HOME_LOCK.lock().await;
    let work = tempfile::tempdir().unwrap();
    let home = work.path().join("home");
    let xdg_state_home = work.path().join("state");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&xdg_state_home).unwrap();
    let previous = set_env(&home, &xdg_state_home);

    let harbor_dir = work.path().join("kb-harbor");
    write_harbor(&harbor_dir);
    write_install_receipt(&xdg_state_home);

    let config_path = work.path().join("config.yaml");
    let engine = engine_at(&config_path, config_with_harbor(&harbor_dir), false).await;

    let data = engine
        .provision(&ProvisionAction::Allow {
            domain: "harbor".to_string(),
        })
        .await
        .unwrap();

    // The decision landed on the config file this engine owns, byte for
    // byte readable back off disk - not just the in-memory config.
    let saved: GlobalConfig = crystalline_core::config::load_yaml(&config_path).unwrap();
    assert_eq!(saved.domains["harbor"].provision, Some(true));

    // A report comes back naming the one harness this machine onboarded and
    // what it did.
    let harnesses = data["harnesses"].as_array().unwrap();
    assert_eq!(harnesses.len(), 1, "{data}");
    assert_eq!(harnesses[0]["harness"], "claude-code");
    let actions = harnesses[0]["actions"].as_array().unwrap();
    assert!(actions.iter().any(|a| a["status"] == "installed"), "{data}");
    assert!(
        data["pending"].as_array().unwrap().is_empty(),
        "harbor is decided, not pending: {data}"
    );

    // The files actually landed under the isolated HOME.
    assert!(home.join(".claude/skills/tide-tables/SKILL.md").exists());
    assert!(home.join(".claude/commands/charts/plot-route.md").exists());
    assert!(home.join(".claude/agents/quartermaster.md").exists());

    restore_env(previous);
}

#[tokio::test]
async fn read_only_refuses_allow_but_answers_status() {
    let _guard = HOME_LOCK.lock().await;
    let work = tempfile::tempdir().unwrap();
    let home = work.path().join("home");
    let xdg_state_home = work.path().join("state");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&xdg_state_home).unwrap();
    let previous = set_env(&home, &xdg_state_home);

    let harbor_dir = work.path().join("kb-harbor");
    write_harbor(&harbor_dir);
    write_install_receipt(&xdg_state_home);

    let config_path = work.path().join("config.yaml");
    let engine = engine_at(&config_path, config_with_harbor(&harbor_dir), true).await;

    let err = engine
        .provision(&ProvisionAction::Allow {
            domain: "harbor".to_string(),
        })
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::ReadOnly), "{err}");
    // Nothing was ever written: a read-only refusal happens before any file
    // touches disk.
    assert!(!config_path.exists());
    assert!(!home.join(".claude").exists());

    let err = engine.provision(&ProvisionAction::Apply).await.unwrap_err();
    assert!(matches!(err, EngineError::ReadOnly), "{err}");

    // status is still answered on a read-only instance, the same "Show is
    // always allowed" carve-out `configure` documents.
    let data = engine.provision(&ProvisionAction::Status).await.unwrap();
    let domains = data["domains"].as_array().unwrap();
    assert_eq!(domains.len(), 1, "{data}");
    assert_eq!(domains[0]["domain"], "harbor");
    assert_eq!(domains[0]["decision"], "undecided");
    let pending = data["pending"].as_array().unwrap();
    assert_eq!(pending[0]["domain"], "harbor", "{data}");

    restore_env(previous);
}

#[tokio::test]
async fn allow_on_an_unknown_or_virtual_domain_errors_through_the_normal_mapping() {
    let _guard = HOME_LOCK.lock().await;
    let work = tempfile::tempdir().unwrap();
    let home = work.path().join("home");
    let xdg_state_home = work.path().join("state");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&xdg_state_home).unwrap();
    let previous = set_env(&home, &xdg_state_home);

    let harbor_dir = work.path().join("kb-harbor");
    write_harbor(&harbor_dir);
    write_install_receipt(&xdg_state_home);

    let mut cfg = config_with_harbor(&harbor_dir);
    cfg.domains
        .insert("notes".to_string(), DomainEntry::virtual_domain());

    let config_path = work.path().join("config.yaml");
    let engine = engine_at(&config_path, cfg, false).await;

    let err = engine
        .provision(&ProvisionAction::Allow {
            domain: "does-not-exist".to_string(),
        })
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::UnknownDomain { .. }), "{err}");
    let message = err.to_string();
    assert!(message.contains("does-not-exist"), "{message}");
    assert!(message.contains("harbor"), "{message}");

    let err = engine
        .provision(&ProvisionAction::Deny {
            domain: "notes".to_string(),
        })
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::Invalid(_)), "{err}");
    assert!(err.to_string().contains("virtual"), "{err}");

    // Neither failed decision touched the config file.
    assert!(!config_path.exists());

    restore_env(previous);
}

#[tokio::test]
async fn env_defined_domain_decisions_are_refused_naming_the_variable() {
    let _guard = HOME_LOCK.lock().await;
    let work = tempfile::tempdir().unwrap();
    let home = work.path().join("home");
    let xdg_state_home = work.path().join("state");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&xdg_state_home).unwrap();
    let previous = set_env(&home, &xdg_state_home);

    let harbor_dir = work.path().join("kb-harbor");
    write_harbor(&harbor_dir);
    write_install_receipt(&xdg_state_home);

    // Harbor is both in the config (shadowed) and defined by the
    // environment; cove is env-only. The overlay is injected the seam-safe
    // way, never through the process environment.
    let overlay = EnvOverlay::from_vars(vec![
        (
            "CRYSTALLINE_DOMAIN_HARBOR".to_string(),
            harbor_dir.display().to_string(),
        ),
        (
            "CRYSTALLINE_DOMAIN_COVE".to_string(),
            work.path().join("kb-cove").display().to_string(),
        ),
    ])
    .unwrap();
    let config_path = work.path().join("config.yaml");
    let store = TursoStore::open_in_memory().await.unwrap();
    let engine = Engine::new(
        Arc::new(TokioMutex::new(store)),
        config_with_harbor(&harbor_dir),
        None,
        Some(config_path.clone()),
    )
    .with_env_overlay(overlay);

    // Shadowed: the variable is the domain's source of truth, so a decision
    // written to the file would be silently discarded on the next overlay
    // apply - refused up front, naming the variable.
    let err = engine
        .provision(&ProvisionAction::Allow {
            domain: "harbor".to_string(),
        })
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::Conflict(_)), "{err}");
    assert!(
        err.to_string().contains("CRYSTALLINE_DOMAIN_HARBOR"),
        "{err}"
    );

    // Env-only: the env message too, never UnknownDomain - status lists the
    // domain, so "not registered" would be a lie.
    let err = engine
        .provision(&ProvisionAction::Deny {
            domain: "cove".to_string(),
        })
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::Conflict(_)), "{err}");
    assert!(err.to_string().contains("CRYSTALLINE_DOMAIN_COVE"), "{err}");

    // Neither refused decision touched the config file.
    assert!(!config_path.exists());

    restore_env(previous);
}
