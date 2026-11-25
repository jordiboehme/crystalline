//! Engine-level tests for `Engine::provision`: allow persists the decision
//! into the config file this engine owns and returns a report, and a
//! read-only instance refuses allow but still answers status. There is no
//! ctl-level (socket) test harness in this crate yet (`control.rs` has no
//! dedicated test file - see `tests/domains/configure.rs`'s own note), so this
//! exercises the engine method directly, one layer below the ctl
//! `provision` command that just forwards to it and serializes the same
//! report `Engine::provision` already returns.
//!
//! Unix only: `install_receipt_path`/`receipt_path`/a harness's artifact
//! base all resolve from `HOME`/`XDG_STATE_HOME`, so every test here
//! redirects them to a scratch directory, the same shared-lock,
//! preserve-and-restore technique `crates/core/tests/it/orchestrate.rs` uses
//! for the same reason.
#![cfg(unix)]

use std::ffi::OsString;
use std::path::Path;
use std::sync::Arc;

use crystalline_core::config::{DomainEntry, GlobalConfig, Provision};
use crystalline_index::TursoStore;
use crystalline_service::engine::ProvisionAction;
use crystalline_service::rest::{AuthStore, Role};
use crystalline_service::{DomainAccess, Engine, EngineError, EnvOverlay, Scope};
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
/// `crates/cli/tests/setup/provision.rs` already covers with a shim).
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
        .provision(
            &ProvisionAction::Allow {
                domain: "harbor".to_string(),
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();

    // The decision landed on the config file this engine owns, byte for
    // byte readable back off disk - not just the in-memory config.
    let saved: GlobalConfig = crystalline_core::config::load_yaml(&config_path).unwrap();
    assert_eq!(
        saved.domains["harbor"].provision,
        Some(crystalline_core::config::Provision::all())
    );

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
        .provision(
            &ProvisionAction::Allow {
                domain: "harbor".to_string(),
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::ReadOnly), "{err}");
    // Nothing was ever written: a read-only refusal happens before any file
    // touches disk.
    assert!(!config_path.exists());
    assert!(!home.join(".claude").exists());

    let err = engine
        .provision(&ProvisionAction::Apply, &Scope::Unrestricted)
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::ReadOnly), "{err}");

    // status is still answered on a read-only instance, the same "Show is
    // always allowed" carve-out `configure` documents.
    let data = engine
        .provision(&ProvisionAction::Status, &Scope::Unrestricted)
        .await
        .unwrap();
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
        .provision(
            &ProvisionAction::Allow {
                domain: "does-not-exist".to_string(),
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::UnknownDomain { .. }), "{err}");
    let message = err.to_string();
    assert!(message.contains("does-not-exist"), "{message}");
    assert!(message.contains("harbor"), "{message}");

    let err = engine
        .provision(
            &ProvisionAction::Deny {
                domain: "notes".to_string(),
            },
            &Scope::Unrestricted,
        )
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
        .provision(
            &ProvisionAction::Allow {
                domain: "harbor".to_string(),
            },
            &Scope::Unrestricted,
        )
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
        .provision(
            &ProvisionAction::Deny {
                domain: "cove".to_string(),
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::Conflict(_)), "{err}");
    assert!(err.to_string().contains("CRYSTALLINE_DOMAIN_COVE"), "{err}");

    // Neither refused decision touched the config file.
    assert!(!config_path.exists());

    restore_env(previous);
}

/// **The reconcile runs over the machine; the report goes to one caller.**
///
/// `provision` with `action: "apply"` names no domain, so nothing refuses it
/// up front and every caller reaches it - an authenticated stranger, and on an
/// instance with `auth.mcp` off every anonymous agent. What comes back is not
/// only a list of actions: `notices` is free prose raised while reconciling,
/// and several of those notices name the domain they are about. A private
/// domain that ships something a harness has no surface for, or that is
/// virtual, or that collides with another domain, would announce itself there
/// to anybody who asked - through the one arm of `provision` that takes no
/// argument to gate.
///
/// The virtual-domain skip is the cheapest of those notices to raise and needs
/// no harness at all, so it is the one this pins. `shed` is the control: it
/// raises the same notice, it is not private, and it has to survive.
#[tokio::test]
async fn an_apply_report_names_no_domain_the_caller_may_not_see() {
    let _guard = HOME_LOCK.lock().await;
    let work = tempfile::tempdir().unwrap();
    let home = work.path().join("home");
    let xdg_state_home = work.path().join("state");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&xdg_state_home).unwrap();
    let previous = set_env(&home, &xdg_state_home);

    // No install receipt is written, so this machine has no harness onboarded:
    // `apply` raises its notices and stops without touching a single file.
    let harbor_dir = work.path().join("kb-harbor");
    write_harbor(&harbor_dir);
    let mut cfg = config_with_harbor(&harbor_dir);
    for name in ["shed", "vault"] {
        cfg.domains.insert(
            name.to_string(),
            DomainEntry {
                provision: Some(crystalline_core::config::Provision::all()),
                ..DomainEntry::virtual_domain()
            },
        );
    }

    let config_path = work.path().join("config.yaml");
    let engine = engine_at(&config_path, cfg, false).await;

    let auth = Arc::new(
        AuthStore::open(&work.path().join("web-auth.db"))
            .await
            .unwrap(),
    );
    auth.add_user("keeper", "keeper", None, Role::Admin, "pw12345678")
        .await
        .unwrap();
    auth.set_domain_visibility("vault", true, "keeper")
        .await
        .unwrap();
    engine.set_domain_access(Arc::new(DomainAccess::new(auth)));

    // Guard against a vacuous pass: the assertions below say nothing unless
    // this caller really does have `vault` hidden from it.
    assert_eq!(
        engine.hidden_domains(&Scope::Anonymous).await.unwrap(),
        Some(std::collections::HashSet::from(["vault".to_string()])),
    );

    let theirs = engine
        .provision(&ProvisionAction::Apply, &Scope::Anonymous)
        .await
        .unwrap();
    let notices = theirs["notices"].to_string();
    assert!(
        notices.contains("`shed`"),
        "the report is a real one, with the notice a visible domain raises:\n{notices}"
    );
    assert!(
        !notices.contains("vault"),
        "and no notice about the domain this caller may not see:\n{notices}"
    );
    assert!(
        !theirs.to_string().contains("vault"),
        "nor anywhere else in the report:\n{theirs}"
    );
    assert_eq!(
        theirs["pending"][0]["domain"], "harbor",
        "the rest of the report is untouched:\n{theirs}"
    );

    // The machine owner sees both, which is what says the filter is about the
    // caller rather than about the notice.
    let mine = engine
        .provision(&ProvisionAction::Apply, &Scope::Unrestricted)
        .await
        .unwrap();
    let notices = mine["notices"].to_string();
    assert!(
        notices.contains("`shed`") && notices.contains("`vault`"),
        "the machine owner is told about both:\n{notices}"
    );

    restore_env(previous);
}

/// **A decision about a name nobody registered lists only the domains the
/// caller may see.** The open tier (`auth.mcp` off) lets an anonymous agent
/// reach `allow` and `deny`, and the refusal for an unknown name used to list
/// every registered file domain, private ones included. That turned a typo
/// into a way to read the names of domains this caller is not shown anywhere
/// else. `harbor` is the control: it is visible and has to stay in the list.
#[tokio::test]
async fn an_unknown_domain_refusal_names_no_domain_the_caller_may_not_see() {
    let _guard = HOME_LOCK.lock().await;
    let work = tempfile::tempdir().unwrap();
    let home = work.path().join("home");
    let xdg_state_home = work.path().join("state");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&xdg_state_home).unwrap();
    let previous = set_env(&home, &xdg_state_home);

    let harbor_dir = work.path().join("kb-harbor");
    write_harbor(&harbor_dir);
    let vault_dir = work.path().join("kb-vault");
    std::fs::create_dir_all(&vault_dir).unwrap();
    let mut cfg = config_with_harbor(&harbor_dir);
    cfg.domains
        .insert("vault".to_string(), DomainEntry::file(&vault_dir));

    let config_path = work.path().join("config.yaml");
    let engine = engine_at(&config_path, cfg, false).await;

    let auth = Arc::new(
        AuthStore::open(&work.path().join("web-auth.db"))
            .await
            .unwrap(),
    );
    auth.add_user("keeper", "keeper", None, Role::Admin, "pw12345678")
        .await
        .unwrap();
    auth.set_domain_visibility("vault", true, "keeper")
        .await
        .unwrap();
    engine.set_domain_access(Arc::new(DomainAccess::new(auth)));

    // Guard against a vacuous pass: this caller really has `vault` hidden.
    assert_eq!(
        engine.hidden_domains(&Scope::Anonymous).await.unwrap(),
        Some(std::collections::HashSet::from(["vault".to_string()])),
    );

    for action in [
        ProvisionAction::Allow {
            domain: "does-not-exist".to_string(),
        },
        ProvisionAction::Deny {
            domain: "does-not-exist".to_string(),
        },
    ] {
        let err = engine
            .provision(&action, &Scope::Anonymous)
            .await
            .unwrap_err();
        assert!(matches!(err, EngineError::UnknownDomain { .. }), "{err}");
        let message = err.to_string();
        assert!(message.contains("does-not-exist"), "{message}");
        assert!(message.contains("harbor"), "{message}");
        assert!(
            !message.contains("vault"),
            "the refusal must not name a domain this caller may not see: {message}"
        );
    }

    // Naming the hidden domain itself is refused in the same words, without a
    // decision being recorded: the engine does not lean on the MCP gate alone.
    let err = engine
        .provision(
            &ProvisionAction::Allow {
                domain: "vault".to_string(),
            },
            &Scope::Anonymous,
        )
        .await
        .unwrap_err();
    match &err {
        EngineError::UnknownDomain { registered, .. } => {
            assert_eq!(registered, &vec!["harbor".to_string()], "{err}")
        }
        other => panic!("expected an unknown-domain refusal, got {other}"),
    }

    // The machine owner still gets the whole list.
    let message = engine
        .provision(
            &ProvisionAction::Allow {
                domain: "does-not-exist".to_string(),
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(
        message.contains("harbor") && message.contains("vault"),
        "{message}"
    );

    assert!(!config_path.exists());
    restore_env(previous);
}

/// **A private domain defined by the environment is refused like an unknown
/// name, before the environment check can name it or its variable.** The two
/// refusals also come from one helper, so a hidden name and a typo read back
/// the same visible set, env domains included.
#[tokio::test]
async fn a_hidden_env_domain_is_refused_like_an_unknown_name() {
    let _guard = HOME_LOCK.lock().await;
    let work = tempfile::tempdir().unwrap();
    let home = work.path().join("home");
    let xdg_state_home = work.path().join("state");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&xdg_state_home).unwrap();
    let previous = set_env(&home, &xdg_state_home);

    let harbor_dir = work.path().join("kb-harbor");
    write_harbor(&harbor_dir);
    let overlay = EnvOverlay::from_vars(vec![
        (
            "CRYSTALLINE_DOMAIN_COVE".to_string(),
            work.path().join("kb-cove").display().to_string(),
        ),
        (
            "CRYSTALLINE_DOMAIN_REEF".to_string(),
            work.path().join("kb-reef").display().to_string(),
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

    let auth = Arc::new(
        AuthStore::open(&work.path().join("web-auth.db"))
            .await
            .unwrap(),
    );
    auth.add_user("keeper", "keeper", None, Role::Admin, "pw12345678")
        .await
        .unwrap();
    auth.set_domain_visibility("cove", true, "keeper")
        .await
        .unwrap();
    engine.set_domain_access(Arc::new(DomainAccess::new(auth)));
    assert_eq!(
        engine.hidden_domains(&Scope::Anonymous).await.unwrap(),
        Some(std::collections::HashSet::from(["cove".to_string()])),
    );

    let decide = |name: &str| ProvisionAction::Allow {
        domain: name.to_string(),
    };
    let hidden = engine
        .provision(&decide("cove"), &Scope::Anonymous)
        .await
        .unwrap_err();
    assert!(
        matches!(hidden, EngineError::UnknownDomain { .. }),
        "a hidden env domain is refused as unknown, not with the env message: {hidden}"
    );
    let unknown = engine
        .provision(&decide("typo"), &Scope::Anonymous)
        .await
        .unwrap_err();
    let hidden = hidden.to_string();
    let unknown = unknown.to_string();
    assert!(
        !hidden.contains("CRYSTALLINE_DOMAIN_COVE") && !unknown.contains("cove"),
        "{hidden}\n{unknown}"
    );
    assert!(
        unknown.contains("reef"),
        "the visible env domain is listed: {unknown}"
    );
    assert_eq!(
        hidden.replace("'cove'", "'NAME'"),
        unknown.replace("'typo'", "'NAME'"),
        "a hidden name and a typo read back the same bytes"
    );

    assert!(!config_path.exists());
    restore_env(previous);
}

// --- take and skip -------------------------------------------------------

/// A scratch `HOME` and `XDG_STATE_HOME` under one temp directory, with
/// claude-code onboarded and harbor (plus an optional skill) written at
/// `kb-harbor`. The caller holds [`HOME_LOCK`] and calls
/// [`Sandbox::finish`] before it returns.
struct Sandbox {
    work: tempfile::TempDir,
    home: std::path::PathBuf,
    harbor_dir: std::path::PathBuf,
    config_path: std::path::PathBuf,
    previous: (Option<OsString>, Option<OsString>),
}

impl Sandbox {
    fn new() -> Sandbox {
        let work = tempfile::tempdir().unwrap();
        let home = work.path().join("home");
        let xdg_state_home = work.path().join("state");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&xdg_state_home).unwrap();
        let previous = set_env(&home, &xdg_state_home);
        let harbor_dir = work.path().join("kb-harbor");
        write_harbor(&harbor_dir);
        write_optional_skill(&harbor_dir);
        write_install_receipt(&xdg_state_home);
        let config_path = work.path().join("config.yaml");
        Sandbox {
            work,
            home,
            harbor_dir,
            config_path,
            previous,
        }
    }

    fn installed(&self, rel: &str) -> bool {
        self.home.join(".claude").join(rel).exists()
    }

    fn saved_decision(&self) -> Option<Provision> {
        let saved: GlobalConfig = crystalline_core::config::load_yaml(&self.config_path).unwrap();
        saved.domains["harbor"].provision.clone()
    }

    fn finish(self) {
        restore_env(self.previous);
    }
}

/// A second harbor skill the author marked optional.
fn write_optional_skill(dir: &Path) {
    write(
        dir,
        "skills/storm-charts/SKILL.md",
        "---\nname: storm-charts\nmetadata:\n  provision: optional\n---\n\nReads storm charts.\n",
    );
}

fn take(domain: &str, names: &[&str]) -> ProvisionAction {
    ProvisionAction::Take {
        domain: domain.to_string(),
        artifacts: strings(names),
    }
}

fn skip(domain: &str, names: &[&str]) -> ProvisionAction {
    ProvisionAction::Skip {
        domain: domain.to_string(),
        artifacts: strings(names),
    }
}

fn allow(domain: &str) -> ProvisionAction {
    ProvisionAction::Allow {
        domain: domain.to_string(),
    }
}

fn deny(domain: &str) -> ProvisionAction {
    ProvisionAction::Deny {
        domain: domain.to_string(),
    }
}

fn strings(names: &[&str]) -> Vec<String> {
    names.iter().map(|n| n.to_string()).collect()
}

/// The text of every notice in a report.
fn notices(report: &serde_json::Value) -> Vec<String> {
    report["notices"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n.as_str().unwrap().to_string())
        .collect()
}

/// Whether a report carries the notice that older versions cannot read the
/// decision.
fn has_older_versions_notice(report: &serde_json::Value) -> bool {
    notices(report)
        .iter()
        .any(|n| n.starts_with("Older Crystalline versions cannot read"))
}

#[tokio::test]
async fn skip_persists_an_except_list_and_reconciles() {
    let _guard = HOME_LOCK.lock().await;
    let sandbox = Sandbox::new();
    let engine = engine_at(
        &sandbox.config_path,
        config_with_harbor(&sandbox.harbor_dir),
        false,
    )
    .await;

    let data = engine
        .provision(
            &skip("harbor", &["skills/tide-tables"]),
            &Scope::Unrestricted,
        )
        .await
        .unwrap();

    // Skip on a domain with no decision yet installs everything else.
    assert_eq!(
        sandbox.saved_decision(),
        Some(Provision::All {
            except: strings(&["skills/tide-tables"]),
            also: Vec::new(),
        })
    );
    assert_eq!(
        data["decided"],
        serde_json::json!({
            "domain": "harbor",
            "decision": "allowed",
            "selection": { "except": ["skills/tide-tables"], "also": [] },
        }),
        "{data}"
    );
    assert!(!sandbox.installed("skills/tide-tables"), "{data}");
    assert!(sandbox.installed("commands/charts/plot-route.md"));
    assert!(sandbox.installed("agents/quartermaster.md"));
    // The optional skill was not taken, so it stays out.
    assert!(!sandbox.installed("skills/storm-charts"));
    sandbox.finish();
}

#[tokio::test]
async fn take_on_an_undecided_domain_persists_only_and_reconciles() {
    let _guard = HOME_LOCK.lock().await;
    let sandbox = Sandbox::new();
    let engine = engine_at(
        &sandbox.config_path,
        config_with_harbor(&sandbox.harbor_dir),
        false,
    )
    .await;

    let data = engine
        .provision(
            &take(
                "harbor",
                &["commands/charts/plot-route", "skills/storm-charts"],
            ),
            &Scope::Unrestricted,
        )
        .await
        .unwrap();

    assert_eq!(
        sandbox.saved_decision(),
        Some(Provision::Only(strings(&[
            "commands/charts/plot-route",
            "skills/storm-charts"
        ])))
    );
    assert_eq!(
        data["decided"],
        serde_json::json!({
            "domain": "harbor",
            "decision": "only",
            "selection": { "only": ["commands/charts/plot-route", "skills/storm-charts"] },
        }),
        "{data}"
    );
    assert!(sandbox.installed("commands/charts/plot-route.md"), "{data}");
    assert!(sandbox.installed("skills/storm-charts/SKILL.md"), "{data}");
    assert!(!sandbox.installed("skills/tide-tables"));
    assert!(!sandbox.installed("agents/quartermaster.md"));
    sandbox.finish();
}

#[tokio::test]
async fn allow_after_a_pick_writes_plain_true() {
    let _guard = HOME_LOCK.lock().await;
    let sandbox = Sandbox::new();
    let engine = engine_at(
        &sandbox.config_path,
        config_with_harbor(&sandbox.harbor_dir),
        false,
    )
    .await;

    engine
        .provision(
            &skip("harbor", &["skills/tide-tables"]),
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert!(!sandbox.installed("skills/tide-tables"));

    let data = engine
        .provision(&allow("harbor"), &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(sandbox.saved_decision(), Some(Provision::all()));
    let yaml = std::fs::read_to_string(&sandbox.config_path).unwrap();
    assert!(yaml.contains("provision: true"), "{yaml}");
    assert_eq!(
        data["decided"],
        serde_json::json!({ "domain": "harbor", "decision": "allowed", "selection": null }),
        "{data}"
    );
    assert!(!has_older_versions_notice(&data), "{data}");
    assert!(sandbox.installed("skills/tide-tables/SKILL.md"));
    sandbox.finish();
}

/// `allow` clears `except` and `only` but keeps the optional artifacts that
/// were taken, so a pick never silently retires one.
#[tokio::test]
async fn allow_after_a_pick_keeps_taken_optional_artifacts() {
    let _guard = HOME_LOCK.lock().await;
    let sandbox = Sandbox::new();
    let engine = engine_at(
        &sandbox.config_path,
        config_with_harbor(&sandbox.harbor_dir),
        false,
    )
    .await;

    // `{except: [skills/tide-tables], also: [skills/storm-charts]}`.
    engine
        .provision(
            &skip("harbor", &["skills/tide-tables"]),
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    engine
        .provision(
            &take("harbor", &["skills/storm-charts"]),
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert_eq!(
        sandbox.saved_decision(),
        Some(Provision::All {
            except: strings(&["skills/tide-tables"]),
            also: strings(&["skills/storm-charts"]),
        })
    );
    assert!(sandbox.installed("skills/storm-charts/SKILL.md"));

    let data = engine
        .provision(&allow("harbor"), &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(
        sandbox.saved_decision(),
        Some(Provision::All {
            except: Vec::new(),
            also: strings(&["skills/storm-charts"]),
        })
    );
    assert_eq!(
        data["decided"]["selection"],
        serde_json::json!({ "except": [], "also": ["skills/storm-charts"] }),
        "{data}"
    );
    assert!(has_older_versions_notice(&data), "{data}");
    assert!(sandbox.installed("skills/storm-charts/SKILL.md"), "{data}");
    assert!(sandbox.installed("skills/tide-tables/SKILL.md"), "{data}");

    // From `only`, allow keeps the taken names that are optional.
    engine
        .provision(&deny("harbor"), &Scope::Unrestricted)
        .await
        .unwrap();
    engine
        .provision(
            &take("harbor", &["agents/quartermaster", "skills/storm-charts"]),
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    engine
        .provision(&allow("harbor"), &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(
        sandbox.saved_decision(),
        Some(Provision::All {
            except: Vec::new(),
            also: strings(&["skills/storm-charts"]),
        })
    );
    assert!(sandbox.installed("skills/storm-charts/SKILL.md"));
    assert!(sandbox.installed("commands/charts/plot-route.md"));

    // With no optional artifact taken, allow writes plain `true`.
    engine
        .provision(
            &skip("harbor", &["skills/storm-charts"]),
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    let data = engine
        .provision(&allow("harbor"), &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(sandbox.saved_decision(), Some(Provision::all()));
    assert!(!has_older_versions_notice(&data), "{data}");
    assert!(!sandbox.installed("skills/storm-charts"));
    sandbox.finish();
}

#[tokio::test]
async fn skip_with_an_unknown_name_refuses_and_writes_nothing() {
    let _guard = HOME_LOCK.lock().await;
    let sandbox = Sandbox::new();
    let engine = engine_at(
        &sandbox.config_path,
        config_with_harbor(&sandbox.harbor_dir),
        false,
    )
    .await;

    // One unknown name refuses the whole call, the good name included.
    let err = engine
        .provision(
            &skip("harbor", &["skills/tide-tables", "skills/fog-horns"]),
            &Scope::Unrestricted,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::Invalid(_)), "{err}");
    let message = err.to_string();
    assert!(
        message.contains("does not ship `skills/fog-horns`"),
        "{message}"
    );
    assert!(message.contains("`skills/tide-tables`"), "{message}");

    // A malformed name is refused too.
    let err = engine
        .provision(&take("harbor", &["skills"]), &Scope::Unrestricted)
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::Invalid(_)), "{err}");
    assert!(err.to_string().contains("is not an artifact name"), "{err}");

    assert!(!sandbox.config_path.exists());
    assert!(!sandbox.home.join(".claude").exists());
    sandbox.finish();
}

#[tokio::test]
async fn read_only_refuses_take_and_skip_but_answers_status() {
    let _guard = HOME_LOCK.lock().await;
    let sandbox = Sandbox::new();
    let engine = engine_at(
        &sandbox.config_path,
        config_with_harbor(&sandbox.harbor_dir),
        true,
    )
    .await;

    for action in [
        take("harbor", &["skills/tide-tables"]),
        skip("harbor", &["skills/tide-tables"]),
    ] {
        let err = engine
            .provision(&action, &Scope::Unrestricted)
            .await
            .unwrap_err();
        assert!(matches!(err, EngineError::ReadOnly), "{err}");
    }
    assert!(!sandbox.config_path.exists());
    assert!(!sandbox.home.join(".claude").exists());

    let data = engine
        .provision(&ProvisionAction::Status, &Scope::Unrestricted)
        .await
        .unwrap();
    let harbor = &data["domains"][0];
    assert_eq!(harbor["decision"], "undecided", "{data}");
    assert_eq!(harbor["selection"], serde_json::Value::Null, "{data}");
    let artifacts = harbor["artifacts"].as_array().unwrap();
    assert_eq!(artifacts.len(), 4, "{data}");
    assert!(artifacts.iter().all(|a| a["on"] == false), "{data}");
    sandbox.finish();
}

#[tokio::test]
async fn take_and_skip_on_an_env_defined_or_virtual_domain_are_refused() {
    let _guard = HOME_LOCK.lock().await;
    let sandbox = Sandbox::new();
    let overlay = EnvOverlay::from_vars(vec![
        (
            "CRYSTALLINE_DOMAIN_HARBOR".to_string(),
            sandbox.harbor_dir.display().to_string(),
        ),
        (
            "CRYSTALLINE_DOMAIN_COVE".to_string(),
            sandbox.work.path().join("kb-cove").display().to_string(),
        ),
    ])
    .unwrap();
    let mut cfg = config_with_harbor(&sandbox.harbor_dir);
    cfg.domains
        .insert("notes".to_string(), DomainEntry::virtual_domain());
    let store = TursoStore::open_in_memory().await.unwrap();
    let engine = Engine::new(
        Arc::new(TokioMutex::new(store)),
        cfg,
        None,
        Some(sandbox.config_path.clone()),
    )
    .with_env_overlay(overlay);

    for domain in ["harbor", "cove", "notes"] {
        let allowed = engine
            .provision(&allow(domain), &Scope::Unrestricted)
            .await
            .unwrap_err()
            .to_string();
        for action in [
            take(domain, &["skills/tide-tables"]),
            skip(domain, &["skills/tide-tables"]),
        ] {
            let err = engine
                .provision(&action, &Scope::Unrestricted)
                .await
                .unwrap_err()
                .to_string();
            assert_eq!(err, allowed, "{domain}: the same refusal as allow");
        }
        let expected = match domain {
            "harbor" => "CRYSTALLINE_DOMAIN_HARBOR",
            "cove" => "CRYSTALLINE_DOMAIN_COVE",
            _ => "virtual",
        };
        assert!(allowed.contains(expected), "{allowed}");
    }

    assert!(!sandbox.config_path.exists());
    sandbox.finish();
}

/// A private domain is refused like a name nobody registered, before its
/// listing is read, so the refusal never says what the domain ships.
#[tokio::test]
async fn a_hidden_domain_take_is_refused_like_an_unknown_name() {
    let _guard = HOME_LOCK.lock().await;
    let sandbox = Sandbox::new();
    let vault_dir = sandbox.work.path().join("kb-vault");
    write_harbor(&vault_dir);
    let mut cfg = config_with_harbor(&sandbox.harbor_dir);
    cfg.domains
        .insert("vault".to_string(), DomainEntry::file(&vault_dir));
    let engine = engine_at(&sandbox.config_path, cfg, false).await;

    let auth = Arc::new(
        AuthStore::open(&sandbox.work.path().join("web-auth.db"))
            .await
            .unwrap(),
    );
    auth.add_user("keeper", "keeper", None, Role::Admin, "pw12345678")
        .await
        .unwrap();
    auth.set_domain_visibility("vault", true, "keeper")
        .await
        .unwrap();
    engine.set_domain_access(Arc::new(DomainAccess::new(auth)));
    assert_eq!(
        engine.hidden_domains(&Scope::Anonymous).await.unwrap(),
        Some(std::collections::HashSet::from(["vault".to_string()])),
    );

    // A name the hidden domain does not ship: were the domain listed, the
    // refusal would name what it ships.
    let actions: [fn(&str) -> ProvisionAction; 2] = [
        |d| take(d, &["skills/fog-horns"]),
        |d| skip(d, &["skills/fog-horns"]),
    ];
    for action in actions {
        let hidden = engine
            .provision(&action("vault"), &Scope::Anonymous)
            .await
            .unwrap_err();
        assert!(
            matches!(hidden, EngineError::UnknownDomain { .. }),
            "{hidden}"
        );
        let unknown = engine
            .provision(&action("typo"), &Scope::Anonymous)
            .await
            .unwrap_err();
        let hidden = hidden.to_string();
        assert!(!hidden.contains("tide-tables"), "{hidden}");
        assert_eq!(
            hidden.replace("'vault'", "'NAME'"),
            unknown.to_string().replace("'typo'", "'NAME'"),
            "a hidden name and a typo read back the same bytes"
        );
    }

    assert!(!sandbox.config_path.exists());
    sandbox.finish();
}

#[tokio::test]
async fn status_json_carries_selection_artifacts_stale_and_malformed() {
    let _guard = HOME_LOCK.lock().await;
    let sandbox = Sandbox::new();
    let mut cfg = config_with_harbor(&sandbox.harbor_dir);
    cfg.domains.get_mut("harbor").unwrap().provision = Some(Provision::All {
        except: strings(&["skills", "skills/old-charts", "skills/tide-tables"]),
        also: strings(&["skills/storm-charts"]),
    });
    cfg.domains
        .insert("notes".to_string(), DomainEntry::virtual_domain());
    let engine = engine_at(&sandbox.config_path, cfg, false).await;

    let data = engine
        .provision(&ProvisionAction::Status, &Scope::Unrestricted)
        .await
        .unwrap();
    let domains = data["domains"].as_array().unwrap();
    let harbor = domains.iter().find(|d| d["domain"] == "harbor").unwrap();
    assert_eq!(harbor["decision"], "allowed", "{data}");
    assert_eq!(
        harbor["selection"],
        serde_json::json!({
            "except": ["skills", "skills/old-charts", "skills/tide-tables"],
            "also": ["skills/storm-charts"],
        }),
        "{data}"
    );
    let row = |key: &str| {
        harbor["artifacts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["key"] == key)
            .cloned()
            .unwrap_or_else(|| panic!("no row for {key}: {data}"))
    };
    assert_eq!(
        row("skills/storm-charts"),
        serde_json::json!({
            "key": "skills/storm-charts",
            "kind": "skills",
            "name": "storm-charts",
            "optional": true,
            "on": true,
        })
    );
    assert_eq!(row("skills/tide-tables")["on"], false);
    assert_eq!(row("skills/tide-tables")["optional"], false);
    assert_eq!(
        row("commands/charts/plot-route")["name"],
        "charts/plot-route"
    );
    assert_eq!(row("commands/charts/plot-route")["kind"], "commands");
    assert_eq!(row("agents/quartermaster")["on"], true);
    assert_eq!(harbor["artifacts"].as_array().unwrap().len(), 4, "{data}");
    assert_eq!(harbor["stale"], serde_json::json!(["skills/old-charts"]));
    assert_eq!(harbor["malformed"], serde_json::json!(["skills"]));
    // The keys that were there before are unchanged.
    assert_eq!(harbor["declares"], true);
    assert_eq!(harbor["parse_problems"], 0);
    assert_eq!(harbor["counts"]["skills"], 2, "{data}");

    // A virtual domain keeps empty lists and no selection.
    let notes = domains.iter().find(|d| d["domain"] == "notes").unwrap();
    assert_eq!(notes["selection"], serde_json::Value::Null);
    assert_eq!(notes["artifacts"], serde_json::json!([]));
    assert_eq!(notes["stale"], serde_json::json!([]));
    assert_eq!(notes["malformed"], serde_json::json!([]));
    sandbox.finish();
}

#[tokio::test]
async fn skip_and_take_add_the_older_versions_notice_only_for_a_map() {
    let _guard = HOME_LOCK.lock().await;
    let sandbox = Sandbox::new();
    let engine = engine_at(
        &sandbox.config_path,
        config_with_harbor(&sandbox.harbor_dir),
        false,
    )
    .await;

    // A map: one notice, naming the domain between backticks so the
    // hidden-domain filter can find it.
    let data = engine
        .provision(
            &skip("harbor", &["skills/tide-tables"]),
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    let older: Vec<String> = notices(&data)
        .into_iter()
        .filter(|n| n.starts_with("Older Crystalline versions"))
        .collect();
    assert_eq!(older.len(), 1, "{data}");
    assert!(older[0].contains("`harbor`"), "{}", older[0]);
    assert!(older[0].contains("`config.yaml`"), "{}", older[0]);

    // Taking the name back normalizes to plain `true`: no notice.
    let data = engine
        .provision(
            &take("harbor", &["skills/tide-tables"]),
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert_eq!(sandbox.saved_decision(), Some(Provision::all()));
    assert!(!has_older_versions_notice(&data), "{data}");

    // A name that changes nothing gets a note naming the domain.
    let data = engine
        .provision(
            &take("harbor", &["skills/tide-tables"]),
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert!(
        notices(&data).contains(&"`skills/tide-tables` is already on in `harbor`.".to_string()),
        "{data}"
    );
    assert!(!has_older_versions_notice(&data), "{data}");

    // Deny then take: `only` is a map again.
    engine
        .provision(&deny("harbor"), &Scope::Unrestricted)
        .await
        .unwrap();
    let data = engine
        .provision(
            &take("harbor", &["skills/tide-tables"]),
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert!(has_older_versions_notice(&data), "{data}");

    // Skipping the last name of `only` normalizes to plain `false`.
    let data = engine
        .provision(
            &skip("harbor", &["skills/tide-tables"]),
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert_eq!(sandbox.saved_decision(), Some(Provision::Deny));
    assert!(!has_older_versions_notice(&data), "{data}");
    assert_eq!(data["decided"]["decision"], "denied", "{data}");
    sandbox.finish();
}

/// The daemon path and the static fallback write a pick through one seam,
/// so the same steps on the same config give the same bytes on disk.
#[tokio::test]
async fn the_daemon_and_the_fallback_write_the_same_pick() {
    let _guard = HOME_LOCK.lock().await;
    let sandbox = Sandbox::new();
    let daemon_path = sandbox.config_path.clone();
    let fallback_path = sandbox.work.path().join("fallback.yaml");
    let cfg = config_with_harbor(&sandbox.harbor_dir);
    crystalline_core::config::save_yaml(&daemon_path, &cfg).unwrap();
    crystalline_core::config::save_yaml(&fallback_path, &cfg).unwrap();
    let engine = engine_at(&daemon_path, cfg, false).await;

    let steps: [(&str, &[&str]); 3] = [
        ("skip", &["skills/tide-tables"]),
        ("take", &["skills/storm-charts"]),
        ("skip", &["agents/quartermaster"]),
    ];
    for (action, names) in steps {
        let engine_action = if action == "skip" {
            skip("harbor", names)
        } else {
            take("harbor", names)
        };
        let daemon = engine
            .provision(&engine_action, &Scope::Unrestricted)
            .await
            .unwrap();
        let fallback = crystalline_service::client::provision(
            action,
            Some("harbor"),
            &strings(names),
            Some(&fallback_path),
        )
        .await
        .unwrap();
        assert_eq!(daemon["decided"], fallback["decided"], "{action} {names:?}");
        assert_eq!(
            has_older_versions_notice(&daemon),
            has_older_versions_notice(&fallback)
        );
        let daemon_yaml = std::fs::read(&daemon_path).unwrap();
        let fallback_yaml = std::fs::read(&fallback_path).unwrap();
        assert_eq!(daemon_yaml, fallback_yaml, "after {action} {names:?}");
    }
    let yaml = std::fs::read_to_string(&daemon_path).unwrap();
    assert!(yaml.contains("except:"), "{yaml}");
    assert!(yaml.contains("also:"), "{yaml}");
    sandbox.finish();
}
