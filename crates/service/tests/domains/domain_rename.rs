//! Renaming a domain on this machine (`rename --local`): every store keyed
//! by the name moves, the index row keeps its id, the domain pauses while it
//! moves, a crash after any step is finished by the next engine before it
//! syncs, and each refusal names its next step.
//!
//! Every body runs against Turso (in memory, always) and Postgres (a fresh
//! per-test schema, when `CRYSTALLINE_TEST_POSTGRES_URL` is set; skipped with
//! a note otherwise), through one store shared by `Arc` so a second engine
//! over the same store stands for a restarted daemon.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crystalline_core::config::{DomainEntry, GlobalConfig, NameOrigin};
use crystalline_core::provision::receipt::{self, DomainSources, ProvisionReceipt};
use crystalline_index::{Store, TursoStore};
use crystalline_service::engine::{Engine, EngineError, RenameStep};
use crystalline_service::overlay::EnvOverlay;
use crystalline_service::params::*;
use crystalline_service::rest::{AuthStore, MemberLevel, Role};
use crystalline_service::{DomainAccess, Scope};
use tokio::sync::Mutex;

#[cfg(feature = "postgres")]
fn pg_url() -> Option<String> {
    use std::sync::Once;
    static NOTE: Once = Once::new();
    match std::env::var("CRYSTALLINE_TEST_POSTGRES_URL") {
        Ok(u) if !u.is_empty() => Some(u),
        _ => {
            NOTE.call_once(|| {
                eprintln!(
                    "note: skipping the postgres rename leg (CRYSTALLINE_TEST_POSTGRES_URL is unset); turso only"
                )
            });
            None
        }
    }
}

/// A recycled pid must never adopt a schema a panicking run left behind.
#[cfg(feature = "postgres")]
fn unique_schema() -> String {
    use std::hash::{BuildHasher, RandomState};
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!(
        "ren_{}_{}_{:x}",
        std::process::id(),
        n,
        RandomState::new().hash_one(n)
    )
}

/// Run a body against Turso (always) and Postgres (when configured).
macro_rules! both_backends {
    ($name:ident, $body:path) => {
        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn $name() {
            {
                let store = TursoStore::open_in_memory().await.unwrap();
                let store: Arc<Mutex<dyn Store>> = Arc::new(Mutex::new(store));
                $body(store).await;
            }
            #[cfg(feature = "postgres")]
            {
                if let Some(url) = pg_url() {
                    let schema = unique_schema();
                    let pg = crystalline_index::PostgresStore::open_in_schema(&url, &schema)
                        .await
                        .expect("open the postgres test schema");
                    let store: Arc<Mutex<dyn Store>> = Arc::new(Mutex::new(pg));
                    $body(store).await;
                    let cleanup = crystalline_index::PostgresStore::open_in_schema(&url, &schema)
                        .await
                        .unwrap();
                    cleanup.drop_schema().await.unwrap();
                }
            }
        }
    };
}

fn manifest(title: &str) -> String {
    format!(
        "---\ntype: manifest\ntitle: {title}\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# {title}\n\n## Scope\n\n- covers things\n\n## When to Use\n\n- when routing\n"
    )
}

fn engram(title: &str, permalink: &str, body: &str) -> String {
    format!(
        "---\ntype: engram\ntitle: {title}\npermalink: {permalink}\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# {title}\n\n{body}\n"
    )
}

fn write_params(domain: &str, title: &str, content: &str) -> WriteParams {
    WriteParams {
        domain: domain.to_string(),
        title: title.to_string(),
        content: content.to_string(),
        folder: None,
        engram_type: None,
        tags: Vec::new(),
        status: None,
        metadata: None,
        overwrite: false,
        share_link: None,
        model: None,
    }
}

fn read_params(identifier: &str, domain: &str) -> ReadParams {
    ReadParams {
        identifier: identifier.to_string(),
        domain: Some(domain.to_string()),
        share_link: None,
    }
}

/// One machine's worth of state around a shared store: a config file, a
/// state directory (origins, overlays, provision receipt, accounts database
/// and the rename journal) and two file domains, `eng` and `ops`, where an
/// `ops` engram links to `[[eng:alpha]]`. A tag suffixes every domain name,
/// so several machines can share one store without their names meeting.
struct Machine {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    store: Arc<Mutex<dyn Store>>,
    eng: String,
    ops: String,
    platform: String,
}

impl Machine {
    fn config_path(&self) -> PathBuf {
        self.root.join("config.yaml")
    }
    fn state(&self) -> PathBuf {
        self.root.join("state")
    }
    fn journal(&self) -> PathBuf {
        self.state().join("rename-journal.json")
    }

    /// An engine over this machine's config file, state directory and store,
    /// as a daemon started now would build it. `with_access` installs the
    /// accounts database the way the HTTP surface does; without it the
    /// rename reaches the database file under the state directory itself.
    async fn engine(&self, with_access: bool) -> Arc<Engine> {
        let cfg = crystalline_service::overlay::load_file(&self.config_path()).unwrap();
        self.engine_over(cfg, EnvOverlay::default(), with_access)
            .await
    }

    async fn engine_over(
        &self,
        cfg: GlobalConfig,
        overlay: EnvOverlay,
        with_access: bool,
    ) -> Arc<Engine> {
        let engine = Engine::new(self.store.clone(), cfg, None, Some(self.config_path()))
            .with_state_dir(self.state())
            .with_origins_dir(self.state().join("origins"))
            .with_env_overlay(overlay);
        if with_access {
            let auth = AuthStore::open(&self.state().join("web-auth.db"))
                .await
                .unwrap();
            engine.set_domain_access(Arc::new(DomainAccess::new(Arc::new(auth))));
        }
        Arc::new(engine)
    }
}

async fn machine(store: Arc<Mutex<dyn Store>>) -> Machine {
    machine_tagged(store, "").await
}

async fn machine_tagged(store: Arc<Mutex<dyn Store>>, tag: &str) -> Machine {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let (eng, ops) = (format!("eng{tag}"), format!("ops{tag}"));
    let eng_dir = root.join("eng");
    let ops_dir = root.join("ops");
    std::fs::create_dir_all(&eng_dir).unwrap();
    std::fs::create_dir_all(&ops_dir).unwrap();
    std::fs::write(eng_dir.join("MANIFEST.md"), manifest("Eng")).unwrap();
    std::fs::write(
        eng_dir.join("alpha.md"),
        engram("Alpha", "alpha", "the alpha runbook about turbines"),
    )
    .unwrap();
    std::fs::write(ops_dir.join("MANIFEST.md"), manifest("Ops")).unwrap();
    std::fs::write(
        ops_dir.join("pager.md"),
        engram(
            "Pager",
            "pager",
            &format!("See [[{eng}:alpha]] before paging."),
        ),
    )
    .unwrap();

    let mut cfg = GlobalConfig {
        domains_root: Some(root.join("domains-root")),
        ..GlobalConfig::default()
    };
    cfg.domains
        .insert(eng.clone(), DomainEntry::file(eng_dir.clone()));
    cfg.domains
        .insert(ops.clone(), DomainEntry::file(ops_dir.clone()));
    let m = Machine {
        _tmp: tmp,
        root,
        store,
        eng: eng.clone(),
        ops: ops.clone(),
        platform: format!("platform{tag}"),
    };
    crystalline_core::config::save_yaml(&m.config_path(), &cfg).unwrap();

    // The per-domain state a rename has to carry along.
    let state = m.state();
    std::fs::create_dir_all(state.join("origins").join(&eng)).unwrap();
    std::fs::write(state.join("origins").join(&eng).join("state.json"), "{}").unwrap();
    std::fs::create_dir_all(state.join("overlays").join(&eng).join("ada")).unwrap();
    std::fs::write(
        state.join("overlays").join(&eng).join("ada/note.md"),
        "a draft",
    )
    .unwrap();
    let mut rec = ProvisionReceipt::default();
    rec.sources.insert(eng.clone(), DomainSources::default());
    rec.sources.insert(ops.clone(), DomainSources::default());
    receipt::save(&state.join("provisions.json"), &rec).unwrap();
    let auth = AuthStore::open(&state.join("web-auth.db")).await.unwrap();
    auth.add_user("ada", "Ada", None, Role::Editor, "pw12345678")
        .await
        .unwrap();
    auth.set_domain_visibility(&eng, true, "ada").await.unwrap();

    let engine = m.engine(true).await;
    engine.sync(None).await.unwrap();
    m
}

async fn domain_id(store: &Arc<Mutex<dyn Store>>, name: &str) -> Option<i64> {
    store
        .lock()
        .await
        .domain_id(name)
        .await
        .unwrap()
        .map(|id| id.0)
}

async fn row_names(store: &Arc<Mutex<dyn Store>>) -> Vec<String> {
    store
        .lock()
        .await
        .domain_stats()
        .await
        .unwrap()
        .into_iter()
        .map(|d| d.name)
        .collect()
}

/// Everything a finished rename of `eng` to `platform` leaves behind.
async fn assert_renamed(m: &Machine, engine: &Engine, eng_id: i64) {
    let (eng, ops, platform) = (m.eng.as_str(), m.ops.as_str(), m.platform.as_str());
    let cfg = crystalline_service::overlay::load_file(&m.config_path()).unwrap();
    assert!(!cfg.domains.contains_key(eng), "{:?}", cfg.domains.keys());
    let entry = cfg.domains.get(platform).expect("platform registered");
    assert_eq!(entry.name_origin, Some(NameOrigin::Explicit));
    assert_eq!(entry.aliases, vec![eng.to_string()]);
    assert_eq!(
        entry.file_path(),
        Some(m.root.join("eng")),
        "the folder stays"
    );

    assert_eq!(domain_id(&m.store, platform).await, Some(eng_id));
    assert_eq!(domain_id(&m.store, eng).await, None);
    let rows = row_names(&m.store).await;
    assert!(
        !rows.contains(&eng.to_string()),
        "no row named {eng}: {rows:?}"
    );

    // The link keeps the text as written and still reaches the runbook.
    let pager = engine
        .read_engram(&read_params("pager", ops), &Scope::Unrestricted)
        .await
        .unwrap();
    let links = pager["links"].as_array().unwrap();
    assert_eq!(links.len(), 1, "{pager}");
    assert_eq!(
        links[0]["target"],
        serde_json::json!({ "domain": eng, "target": "alpha" }),
        "{pager}"
    );
    assert_eq!(links[0]["resolved"], true, "{pager}");

    let state = m.state();
    assert!(
        state
            .join("origins")
            .join(platform)
            .join("state.json")
            .is_file()
    );
    assert!(!state.join("origins").join(eng).exists());
    assert!(
        state
            .join("overlays")
            .join(platform)
            .join("ada/note.md")
            .is_file()
    );
    assert!(!state.join("overlays").join(eng).exists());

    let rec = receipt::load(&state.join("provisions.json")).unwrap();
    assert!(
        rec.sources.contains_key(platform),
        "{:?}",
        rec.sources.keys()
    );
    assert!(!rec.sources.contains_key(eng));
    assert!(rec.sources.contains_key(ops));

    let auth = AuthStore::open(&state.join("web-auth.db")).await.unwrap();
    let acl = auth.domain_visibility(platform).await.unwrap();
    assert_eq!(acl.map(|a| a.owner), Some("ada".to_string()));
    assert!(auth.domain_visibility(eng).await.unwrap().is_none());

    assert!(!m.journal().exists(), "the journal is gone");
    assert_eq!(
        engine.local_domain_name(eng).await.as_deref(),
        Some(platform)
    );
    assert!(!engine.is_renaming(eng) && !engine.is_renaming(platform));

    // The old name still reaches the domain's content.
    let alpha = engine
        .read_engram(&read_params("alpha", eng), &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(alpha["domain"], platform, "{alpha}");
}

async fn a_local_rename_moves_everything_and_keeps_the_id_body(store: Arc<Mutex<dyn Store>>) {
    let m = machine(store).await;
    let engine = m.engine(true).await;
    let eng_id = domain_id(&m.store, "eng").await.expect("eng indexed");

    let report = engine
        .rename_domain_local(
            "eng",
            "platform",
            NameOrigin::Explicit,
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert_eq!(report["domain"], "platform", "{report}");
    assert_eq!(report["previous"], "eng", "{report}");
    assert_eq!(report["local_only"], true, "{report}");
    assert_eq!(report["aliases"], serde_json::json!(["eng"]), "{report}");
    let moved: Vec<&str> = report["moved"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(
        moved,
        vec![
            "index_row",
            "auth_tables",
            "origins_dir",
            "overlays_dir",
            "provision_receipt",
            "config"
        ],
        "{report}"
    );

    assert_renamed(&m, &engine, eng_id).await;

    // A restart over what the rename left behind finds nothing to finish.
    let restarted = m.engine(false).await;
    assert!(restarted.recover_rename_journal().await.unwrap().is_none());
    restarted.sync(None).await.unwrap();
    assert_renamed(&m, &restarted, eng_id).await;
}
both_backends!(
    a_local_rename_moves_everything_and_keeps_the_id,
    a_local_rename_moves_everything_and_keeps_the_id_body
);

/// A crash after each step, simulated by the failpoint: the rename errors,
/// the journal stays, and a new engine over the same config, state directory
/// and store finishes it before its first sync. The new engine has no
/// accounts resolver installed, as a daemon at startup does not.
///
/// Each step is crashed twice: once after the journal recorded it, and once
/// in the real crash window, where the step's effect landed and the journal
/// did not record it yet (the last `done` entry is dropped again), so the
/// recovery runs that step a second time over its own result.
async fn a_leftover_journal_is_completed_before_the_first_sync_body(store: Arc<Mutex<dyn Store>>) {
    let steps = [
        RenameStep::IndexRow,
        RenameStep::AuthTables,
        RenameStep::OriginsDir,
        RenameStep::OverlaysDir,
        RenameStep::ProvisionReceipt,
        RenameStep::Config,
    ];
    for (step, unrecorded) in steps
        .iter()
        .flat_map(|step| [(*step, false), (*step, true)])
    {
        // A machine per turn over the one store, its names tagged with the
        // turn so the turns never meet in the shared index.
        let tag = format!("-{step:?}{}", if unrecorded { "-u" } else { "" }).to_lowercase();
        let m = machine_tagged(store.clone(), &tag).await;
        let engine = m.engine(true).await;
        let eng_id = domain_id(&m.store, &m.eng).await.expect("eng indexed");

        engine.fail_rename_after(Some(step));
        let err = engine
            .rename_domain_local(
                &m.eng,
                &m.platform,
                NameOrigin::Explicit,
                &Scope::Unrestricted,
            )
            .await
            .expect_err("the failpoint stops the rename");
        assert!(
            format!("{err}").contains("rename"),
            "{step:?}: the error says a rename stopped: {err}"
        );
        assert!(m.journal().is_file(), "{step:?}: the journal stays");
        assert!(
            engine.is_renaming(&m.eng),
            "{step:?}: a stopped rename keeps the domain paused"
        );
        drop(engine);
        if unrecorded {
            let mut journal: serde_json::Value =
                serde_json::from_slice(&std::fs::read(m.journal()).unwrap()).unwrap();
            let done = journal["done"].as_array_mut().unwrap();
            assert_eq!(done.last(), Some(&serde_json::json!(step.name())));
            done.pop();
            std::fs::write(m.journal(), serde_json::to_vec(&journal).unwrap()).unwrap();
        }

        let restarted = m.engine(false).await;
        let recovered = restarted
            .recover_rename_journal()
            .await
            .unwrap()
            .unwrap_or_else(|| panic!("{step:?}: a journal was left to finish"));
        assert_eq!(recovered["domain"], m.platform, "{step:?}: {recovered}");
        assert_eq!(recovered["previous"], m.eng, "{step:?}: {recovered}");
        restarted.sync(None).await.unwrap();
        assert_renamed(&m, &restarted, eng_id).await;
    }
}
both_backends!(
    a_leftover_journal_is_completed_before_the_first_sync,
    a_leftover_journal_is_completed_before_the_first_sync_body
);

/// A journal records the index, configuration and state directory it was
/// started against, and an engine that opened another index leaves it alone:
/// no step runs, the state folders and the configuration stay where they
/// are, and a resend of the same rename is refused rather than finished
/// there. Put back to its own index, the same journal is finished as usual.
async fn a_journal_for_another_index_is_left_alone_body(store: Arc<Mutex<dyn Store>>) {
    let m = machine(store).await;
    let engine = m.engine(true).await;
    engine.fail_rename_after(Some(RenameStep::IndexRow));
    engine
        .rename_domain_local(
            &m.eng,
            &m.platform,
            NameOrigin::Explicit,
            &Scope::Unrestricted,
        )
        .await
        .expect_err("the failpoint stops the rename");
    drop(engine);

    let own: serde_json::Value =
        serde_json::from_slice(&std::fs::read(m.journal()).unwrap()).unwrap();
    let state = std::fs::canonicalize(m.state()).unwrap();
    assert_eq!(
        own["owner"]["state_dir"],
        state.display().to_string(),
        "{own}"
    );
    assert_eq!(
        own["owner"]["config"],
        std::fs::canonicalize(m.config_path())
            .unwrap()
            .display()
            .to_string(),
        "{own}"
    );
    let mut foreign = own.clone();
    foreign["owner"]["index"] = serde_json::json!("/elsewhere/other-index.db");
    std::fs::write(m.journal(), serde_json::to_vec(&foreign).unwrap()).unwrap();

    let other = m.engine(false).await;
    assert!(
        other.recover_rename_journal().await.unwrap().is_none(),
        "a journal of another index is not finished here"
    );
    assert!(m.journal().is_file(), "the journal stays for its own index");
    assert!(m.state().join("origins").join(&m.eng).is_dir());
    assert!(!m.state().join("origins").join(&m.platform).exists());
    let cfg: GlobalConfig = crystalline_core::config::load_yaml(&m.config_path()).unwrap();
    assert!(
        cfg.domains.contains_key(&m.eng) && !cfg.domains.contains_key(&m.platform),
        "{:?}",
        cfg.domains.keys()
    );
    let err = other
        .rename_domain_local(
            &m.eng,
            &m.platform,
            NameOrigin::Explicit,
            &Scope::Unrestricted,
        )
        .await
        .expect_err("a resend against another index is refused");
    let text = conflict(err);
    assert!(
        text.contains("belongs to another index") && text.contains("without --db and --config"),
        "{text}"
    );
    assert!(m.journal().is_file());
    drop(other);

    std::fs::write(m.journal(), serde_json::to_vec(&own).unwrap()).unwrap();
    let restarted = m.engine(false).await;
    let recovered = restarted
        .recover_rename_journal()
        .await
        .unwrap()
        .expect("its own index finishes it");
    assert_eq!(recovered["domain"], m.platform, "{recovered}");
    assert!(!m.journal().exists());
}
both_backends!(
    a_journal_for_another_index_is_left_alone,
    a_journal_for_another_index_is_left_alone_body
);

/// An engine that does not hold this machine's state directory never runs a
/// journal: recovery leaves it and a resend of the same rename is refused,
/// so a one-shot command and the daemon holding the directory never run one
/// journal at once. Holding it, the same engine finishes the rename.
async fn a_journal_waits_for_the_state_directory_holder_body(store: Arc<Mutex<dyn Store>>) {
    let m = machine(store).await;
    let engine = m.engine(true).await;
    engine.fail_rename_after(Some(RenameStep::IndexRow));
    engine
        .rename_domain_local(
            &m.eng,
            &m.platform,
            NameOrigin::Explicit,
            &Scope::Unrestricted,
        )
        .await
        .expect_err("the failpoint stops the rename");
    drop(engine);

    let standalone = m.engine(false).await;
    standalone.set_holds_state_dir(false);
    assert!(standalone.recover_rename_journal().await.unwrap().is_none());
    let err = standalone
        .rename_domain_local(
            &m.eng,
            &m.platform,
            NameOrigin::Explicit,
            &Scope::Unrestricted,
        )
        .await
        .expect_err("a resend without the state directory is refused");
    assert!(
        conflict(err).contains("holds this machine's state directory"),
        "the refusal names the holder"
    );
    assert!(m.journal().is_file());
    assert!(m.state().join("origins").join(&m.eng).is_dir());

    standalone.set_holds_state_dir(true);
    let recovered = standalone
        .recover_rename_journal()
        .await
        .unwrap()
        .expect("the holder finishes it");
    assert_eq!(recovered["domain"], m.platform, "{recovered}");
    assert!(!m.journal().exists());
}
both_backends!(
    a_journal_waits_for_the_state_directory_holder,
    a_journal_waits_for_the_state_directory_holder_body
);

/// A write that arrives while the domain is paused waits for the rename and
/// then lands in the renamed domain, under the name it was sent with.
async fn a_write_during_a_rename_waits_and_then_lands_in_the_new_name_body(
    store: Arc<Mutex<dyn Store>>,
) {
    let m = machine(store).await;
    let engine = m.engine(true).await;
    let hold = engine.hold_rename_after(RenameStep::IndexRow);

    let renamer = {
        let engine = engine.clone();
        tokio::spawn(async move {
            engine
                .rename_domain_local(
                    "eng",
                    "platform",
                    NameOrigin::Explicit,
                    &Scope::Unrestricted,
                )
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(20), hold.reached())
        .await
        .expect("the rename reaches its hold");
    assert!(engine.is_renaming("eng") && engine.is_renaming("platform"));

    // A refresh of the names between the index row step and the config step
    // (a new MCP connection, a synced MANIFEST) leaves the alias the index
    // row step kept for the old name, which the configuration does not list
    // yet.
    engine.refresh_routing_cache().await;
    let eng_id = domain_id(&m.store, "platform").await.expect("renamed row");
    let spellings = m.store.lock().await.domain_spellings().await.unwrap();
    assert!(
        spellings
            .iter()
            .any(|(spelling, id)| spelling == "eng" && id.0 == eng_id),
        "the old name still spells the row: {spellings:?}"
    );

    // A sync or a watcher pass skips the paused domain rather than register
    // a second row under the old name; the sync that ends the rename covers
    // it.
    let synced = engine.sync(None).await.unwrap();
    assert!(
        synced["skipped"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!({ "domain": "eng", "renaming": true })),
        "{synced}"
    );
    let targeted = engine
        .sync_paths("eng", vec!["alpha.md".to_string()])
        .await
        .unwrap();
    assert_eq!(targeted.domain, "eng");
    assert_eq!(domain_id(&m.store, "eng").await, None, "no second row");

    let writer = {
        let engine = engine.clone();
        tokio::spawn(async move {
            engine
                .write_engram(&write_params(
                    "eng",
                    "Beta",
                    "written while the domain was renamed",
                ))
                .await
        })
    };
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!writer.is_finished(), "the write waits for the rename");

    hold.release();
    renamer.await.unwrap().unwrap();
    let written = writer.await.unwrap().unwrap();
    assert_eq!(written["domain"], "platform", "{written}");
    assert!(
        m.root.join("eng/beta.md").is_file(),
        "the file is in the folder"
    );
    let beta = engine
        .read_engram(&read_params("beta", "platform"), &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(beta["domain"], "platform");
    let rows = row_names(&m.store).await;
    assert!(
        !rows.contains(&"eng".to_string()),
        "no row named eng: {rows:?}"
    );
}
both_backends!(
    a_write_during_a_rename_waits_and_then_lands_in_the_new_name,
    a_write_during_a_rename_waits_and_then_lands_in_the_new_name_body
);

/// A write already running when the rename starts is waited for: the rename
/// does not touch the index row until the write is done, the write lands
/// under the old name, and the rename then carries it to the new one.
async fn a_rename_waits_for_a_write_already_running_body(store: Arc<Mutex<dyn Store>>) {
    let m = machine(store).await;
    let engine = m.engine(true).await;
    let eng_id = domain_id(&m.store, "eng").await.expect("eng indexed");
    let write_hold = engine.hold_next_write();

    let writer = {
        let engine = engine.clone();
        tokio::spawn(async move {
            engine
                .write_engram(&write_params("eng", "Beta", "written before the rename"))
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(20), write_hold.reached())
        .await
        .expect("the write is counted and held");

    let renamer = {
        let engine = engine.clone();
        tokio::spawn(async move {
            engine
                .rename_domain_local(
                    "eng",
                    "platform",
                    NameOrigin::Explicit,
                    &Scope::Unrestricted,
                )
                .await
        })
    };
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        engine.is_renaming("eng"),
        "the rename has paused the domain"
    );
    assert!(
        !renamer.is_finished(),
        "the rename waits for the running write"
    );
    assert!(!m.journal().exists(), "no step has started");
    assert_eq!(
        domain_id(&m.store, "eng").await,
        Some(eng_id),
        "the index row still has its old name"
    );

    write_hold.release();
    let written = writer.await.unwrap().unwrap();
    assert_eq!(
        written["domain"], "eng",
        "the write landed under the old name"
    );
    renamer.await.unwrap().unwrap();

    let beta = engine
        .read_engram(&read_params("beta", "platform"), &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(beta["domain"], "platform", "{beta}");
    assert_eq!(domain_id(&m.store, "platform").await, Some(eng_id));
    let rows = row_names(&m.store).await;
    assert!(
        !rows.contains(&"eng".to_string()),
        "no row named eng: {rows:?}"
    );
}
both_backends!(
    a_rename_waits_for_a_write_already_running,
    a_rename_waits_for_a_write_already_running_body
);

/// A move from another domain into the domain being renamed is counted in
/// its destination too: one that passed its source's pause check before the
/// rename started waits for the rename and is then refused, rather than
/// registering a second row under the old name.
async fn a_move_into_a_domain_being_renamed_never_registers_the_old_name_body(
    store: Arc<Mutex<dyn Store>>,
) {
    let m = machine(store).await;
    let engine = m.engine(true).await;
    // The move is held right after its source (ops) counted it, before it
    // reached its destination.
    let write_hold = engine.hold_next_write();
    let mover = {
        let engine = engine.clone();
        tokio::spawn(async move {
            engine
                .move_engram(
                    &MoveParams {
                        identifier: "pager".to_string(),
                        domain: "ops".to_string(),
                        destination: "pager.md".to_string(),
                        destination_domain: Some("eng".to_string()),
                        permalink: None,
                        update_links: None,
                    },
                    &Scope::Unrestricted,
                )
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(20), write_hold.reached())
        .await
        .expect("the move is counted in its source and held");

    let rename_hold = engine.hold_rename_after(RenameStep::IndexRow);
    let renamer = {
        let engine = engine.clone();
        tokio::spawn(async move {
            engine
                .rename_domain_local(
                    "eng",
                    "platform",
                    NameOrigin::Explicit,
                    &Scope::Unrestricted,
                )
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(20), rename_hold.reached())
        .await
        .expect("the rename is past its index row step");

    write_hold.release();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        domain_id(&m.store, "eng").await,
        None,
        "the move did not register the old name"
    );
    rename_hold.release();
    renamer.await.unwrap().unwrap();
    let err = mover.await.unwrap().expect_err("the move is refused");
    assert!(
        conflict(err).contains("was renamed while this request waited; send it again"),
        "the refusal says to send the move again"
    );
    let rows = row_names(&m.store).await;
    assert!(
        !rows.contains(&"eng".to_string()),
        "no row named eng: {rows:?}"
    );
    let pager = engine
        .read_engram(&read_params("pager", "ops"), &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(pager["domain"], "ops", "the engram stayed where it was");
}
both_backends!(
    a_move_into_a_domain_being_renamed_never_registers_the_old_name,
    a_move_into_a_domain_being_renamed_never_registers_the_old_name_body
);

fn conflict(err: EngineError) -> String {
    match err {
        EngineError::Conflict(msg) => msg,
        other => panic!("expected a conflict, got {other:?}"),
    }
}

async fn refusals_name_the_next_step_body(store: Arc<Mutex<dyn Store>>) {
    let m = machine(store).await;
    let engine = m.engine(true).await;

    // Another domain's local name.
    let msg = conflict(
        engine
            .rename_domain_local("eng", "ops", NameOrigin::Explicit, &Scope::Unrestricted)
            .await
            .unwrap_err(),
    );
    assert!(
        msg.contains("'ops' is already a domain here")
            && msg.contains("pick another name or rename that domain first"),
        "{msg}"
    );

    // An invalid name.
    match engine
        .rename_domain_local("eng", "../up", NameOrigin::Explicit, &Scope::Unrestricted)
        .await
        .unwrap_err()
    {
        EngineError::Invalid(msg) => assert!(
            msg.contains("'../up' cannot name a domain: use letters, digits, hyphens"),
            "the registration error text: {msg}"
        ),
        other => panic!("expected invalid, got {other:?}"),
    }

    // A caller who does not own the private domain.
    let stranger = Scope::User {
        account: "out".into(),
        admin: false,
    };
    match engine
        .rename_domain_local("eng", "platform", NameOrigin::Explicit, &stranger)
        .await
        .unwrap_err()
    {
        EngineError::Forbidden(_) | EngineError::UnknownDomain { .. } => {}
        other => panic!("expected forbidden, got {other:?}"),
    }

    // A second rename while one is running.
    let hold = engine.hold_rename_after(RenameStep::IndexRow);
    let renamer = {
        let engine = engine.clone();
        tokio::spawn(async move {
            engine
                .rename_domain_local(
                    "eng",
                    "platform",
                    NameOrigin::Explicit,
                    &Scope::Unrestricted,
                )
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(20), hold.reached())
        .await
        .expect("the rename reaches its hold");
    let msg = conflict(
        engine
            .rename_domain_local(
                "ops",
                "runbooks",
                NameOrigin::Explicit,
                &Scope::Unrestricted,
            )
            .await
            .unwrap_err(),
    );
    assert!(
        msg.contains("the rename of 'eng' to 'platform' is still running")
            && msg.contains(
                "send the same rename again (`crystalline domain rename eng platform --local`)"
            )
            && msg.contains("restart the daemon"),
        "{msg}"
    );
    hold.release();
    renamer.await.unwrap().unwrap();

    // Nothing the refusals touched moved.
    let cfg = crystalline_service::overlay::load_file(&m.config_path()).unwrap();
    assert!(cfg.domains.contains_key("ops") && cfg.domains.contains_key("platform"));

    // A read-only instance refuses before anything else.
    let cfg = crystalline_service::overlay::load_file(&m.config_path()).unwrap();
    let read_only = Engine::new(m.store.clone(), cfg, None, Some(m.config_path()))
        .with_state_dir(m.state())
        .with_read_only(true);
    assert!(matches!(
        read_only
            .rename_domain_local(
                "ops",
                "runbooks",
                NameOrigin::Explicit,
                &Scope::Unrestricted
            )
            .await
            .unwrap_err(),
        EngineError::ReadOnly
    ));
}
both_backends!(
    refusals_name_the_next_step,
    refusals_name_the_next_step_body
);

/// A domain an environment variable defines is renamed by renaming the
/// variable, never here.
async fn an_env_domain_is_not_renamed_body(store: Arc<Mutex<dyn Store>>) {
    let m = machine(store).await;
    let env_root = m.root.join("envdom");
    std::fs::create_dir_all(&env_root).unwrap();
    std::fs::write(env_root.join("MANIFEST.md"), manifest("Env")).unwrap();
    let overlay = EnvOverlay::from_vars(vec![(
        "CRYSTALLINE_DOMAIN_ENVDOM".to_string(),
        env_root.display().to_string(),
    )])
    .unwrap();
    let cfg = crystalline_service::overlay::load_file(&m.config_path()).unwrap();
    let engine = m.engine_over(cfg, overlay, true).await;
    let msg = conflict(
        engine
            .rename_domain_local(
                "envdom",
                "moved",
                NameOrigin::Explicit,
                &Scope::Unrestricted,
            )
            .await
            .unwrap_err(),
    );
    assert!(
        msg.contains("defined by the environment variable CRYSTALLINE_DOMAIN_ENVDOM")
            && msg.contains("rename the variable to rename the domain"),
        "{msg}"
    );
    assert!(!m.journal().exists());
}
both_backends!(
    an_env_domain_is_not_renamed,
    an_env_domain_is_not_renamed_body
);

/// Renaming onto a name another domain declares in its MANIFEST (or keeps as
/// an alias) is allowed: the local name wins, and the report names the
/// domain that is now shadowed.
async fn renaming_onto_another_domains_canonical_reports_the_shadow_body(
    store: Arc<Mutex<dyn Store>>,
) {
    let m = machine(store).await;
    let ops_manifest = manifest("Ops").replacen(
        "status: current\n",
        "status: current\ndomain_name: platform\n",
        1,
    );
    std::fs::write(m.root.join("ops/MANIFEST.md"), ops_manifest).unwrap();
    let engine = m.engine(true).await;
    engine.sync(None).await.unwrap();
    assert_eq!(
        engine.local_domain_name("platform").await.as_deref(),
        Some("ops")
    );

    let report = engine
        .rename_domain_local(
            "eng",
            "platform",
            NameOrigin::Explicit,
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert_eq!(report["shadows"], serde_json::json!(["ops"]), "{report}");
    assert!(
        report["note"].as_str().unwrap_or_default().contains("ops"),
        "{report}"
    );
    assert_eq!(
        engine.local_domain_name("platform").await.as_deref(),
        Some("platform")
    );
}
both_backends!(
    renaming_onto_another_domains_canonical_reports_the_shadow,
    renaming_onto_another_domains_canonical_reports_the_shadow_body
);

/// On a shared index another live instance serves, a rename on this machine
/// alone would rename the domain under that instance too: refused before any
/// step.
async fn a_shared_index_another_instance_uses_refuses_body(store: Arc<Mutex<dyn Store>>) {
    let m = machine(store).await;
    let cfg = crystalline_service::overlay::load_file(&m.config_path()).unwrap();
    let other = Engine::new(m.store.clone(), cfg.clone(), None, None)
        .with_instance_id("inst-b".to_string());
    other.sync(Some("ops")).await.unwrap();

    let engine = Arc::new(
        Engine::new(m.store.clone(), cfg, None, Some(m.config_path()))
            .with_state_dir(m.state())
            .with_origins_dir(m.state().join("origins"))
            .with_instance_id("inst-a".to_string()),
    );
    let msg = conflict(
        engine
            .rename_domain_local(
                "eng",
                "platform",
                NameOrigin::Explicit,
                &Scope::Unrestricted,
            )
            .await
            .unwrap_err(),
    );
    assert!(msg.contains("inst-b"), "{msg}");
    assert!(!m.journal().exists(), "nothing started");
    assert!(domain_id(&m.store, "eng").await.is_some());
}
both_backends!(
    a_shared_index_another_instance_uses_refuses,
    a_shared_index_another_instance_uses_refuses_body
);

/// Records a removed domain left under the new name never reach the renamed
/// domain: the renamed domain is shared, and stays shared.
async fn stale_records_under_the_new_name_are_forgotten_body(store: Arc<Mutex<dyn Store>>) {
    let m = machine(store).await;
    let auth = AuthStore::open(&m.state().join("web-auth.db"))
        .await
        .unwrap();
    auth.add_user("out", "Out", None, Role::Editor, "pw12345678")
        .await
        .unwrap();
    auth.set_domain_visibility("runbooks", true, "out")
        .await
        .unwrap();
    let engine = m.engine(true).await;
    engine
        .rename_domain_local(
            "ops",
            "runbooks",
            NameOrigin::Explicit,
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert!(
        auth.domain_visibility("runbooks").await.unwrap().is_none(),
        "the stale owner does not own the renamed domain"
    );
}
both_backends!(
    stale_records_under_the_new_name_are_forgotten,
    stale_records_under_the_new_name_are_forgotten_body
);

// --- the full rename: MANIFEST, relink, report ---------------------------------

/// A MANIFEST that declares `domain_name: <name>`.
fn manifest_named(title: &str, name: &str) -> String {
    manifest(title).replacen(
        "status: current\n",
        &format!("status: current\ndomain_name: {name}\n"),
        1,
    )
}

/// The full-rename machine: [`machine_tagged`], with `eng` declaring
/// `domain_name: eng-team` and keeping the alias `engineering` here, three
/// engrams `a`, `b` and `c` in it, and `ops`'s pager linking to each under a
/// different spelling: the local name, the canonical name and the alias.
struct Full {
    m: Machine,
    canonical: String,
    alias: String,
}

async fn full_machine(store: Arc<Mutex<dyn Store>>, tag: &str) -> Full {
    let m = machine_tagged(store, tag).await;
    let (canonical, alias) = (format!("eng-team{tag}"), format!("engineering{tag}"));
    std::fs::write(
        m.root.join("eng/MANIFEST.md"),
        manifest_named("Eng", &canonical),
    )
    .unwrap();
    for t in ["a", "b", "c"] {
        std::fs::write(
            m.root.join(format!("eng/{t}.md")),
            engram(&t.to_uppercase(), t, "an engineering note"),
        )
        .unwrap();
    }
    std::fs::write(
        m.root.join("ops/pager.md"),
        engram(
            "Pager",
            "pager",
            &format!(
                "See [[{}:a]], then [[{canonical}:b]], then crystalline://{alias}/c before paging.",
                m.eng
            ),
        ),
    )
    .unwrap();
    let mut cfg = crystalline_service::overlay::load_file(&m.config_path()).unwrap();
    cfg.domains.get_mut(&m.eng).unwrap().aliases = vec![alias.clone()];
    crystalline_core::config::save_yaml(&m.config_path(), &cfg).unwrap();
    let engine = m.engine(true).await;
    engine.sync(None).await.unwrap();
    Full {
        m,
        canonical,
        alias,
    }
}

fn sorted(mut names: Vec<String>) -> Vec<String> {
    names.sort();
    names
}

/// Everything a finished full rename of `eng` to `platform` leaves behind.
async fn assert_fully_renamed(f: &Full, engine: &Engine) {
    let m = &f.m;
    let manifest_text = std::fs::read_to_string(m.root.join("eng/MANIFEST.md")).unwrap();
    assert!(
        manifest_text.contains(&format!("domain_name: {}\n", m.platform)),
        "{manifest_text}"
    );
    assert!(!manifest_text.contains(&f.canonical), "{manifest_text}");

    let pager = std::fs::read_to_string(m.root.join("ops/pager.md")).unwrap();
    for rewritten in [
        format!("[[{}:a]]", m.platform),
        format!("[[{}:b]]", m.platform),
        format!("crystalline://{}/c", m.platform),
    ] {
        assert!(pager.contains(&rewritten), "{rewritten} in {pager}");
    }
    for old in [&m.eng, &f.canonical, &f.alias] {
        assert!(
            !pager.contains(&format!("[[{old}:")) && !pager.contains(&format!("//{old}/")),
            "no {old} left in {pager}"
        );
    }

    let cfg = crystalline_service::overlay::load_file(&m.config_path()).unwrap();
    assert!(
        !cfg.domains.contains_key(&m.eng),
        "{:?}",
        cfg.domains.keys()
    );
    let entry = cfg.domains.get(&m.platform).expect("platform registered");
    assert_eq!(entry.name_origin, Some(NameOrigin::Explicit));
    assert_eq!(
        sorted(entry.aliases.clone()),
        sorted(vec![m.eng.clone(), f.canonical.clone(), f.alias.clone()]),
        "the old local name, the previous canonical and the alias"
    );
    assert!(!m.journal().exists(), "the journal is gone");
    assert!(!engine.is_renaming(&m.eng) && !engine.is_renaming(&m.platform));

    // The rewritten links reach the renamed domain.
    let read = engine
        .read_engram(&read_params("pager", &m.ops), &Scope::Unrestricted)
        .await
        .unwrap();
    let links = read["links"].as_array().unwrap();
    assert!(!links.is_empty(), "{read}");
    for link in links {
        assert_eq!(link["target"]["domain"], m.platform, "{read}");
        assert_eq!(link["resolved"], true, "{read}");
    }
    // Every old spelling still reaches the domain.
    for old in [&m.eng, &f.canonical, &f.alias] {
        assert_eq!(
            engine.local_domain_name(old).await.as_deref(),
            Some(m.platform.as_str()),
            "{old}"
        );
    }
}

async fn a_full_rename_rewrites_links_in_every_writable_domain_body(store: Arc<Mutex<dyn Store>>) {
    let f = full_machine(store, "").await;
    let engine = f.m.engine(true).await;

    let report = engine
        .rename_domain("eng", "platform", false, &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(report["domain"], "platform", "{report}");
    assert_eq!(report["previous"], "eng", "{report}");
    assert_eq!(report["local_only"], false, "{report}");
    assert_eq!(report["manifest_written"], true, "{report}");
    assert_eq!(report["manifest_draft"], false, "{report}");
    assert_eq!(
        report["rewritten"],
        serde_json::json!([{ "domain": "ops", "engrams": 1, "references": 3 }]),
        "{report}"
    );
    assert_eq!(report["left_behind"], serde_json::json!([]), "{report}");
    assert_eq!(report["shadows"], serde_json::json!([]), "{report}");
    let aliases: Vec<String> = serde_json::from_value(report["aliases"].clone()).unwrap();
    assert_eq!(
        sorted(aliases),
        sorted(vec![
            "eng".to_string(),
            "eng-team".to_string(),
            "engineering".to_string()
        ]),
        "{report}"
    );

    assert_fully_renamed(&f, &engine).await;

    // A restart finds nothing to finish, and a sync changes nothing.
    let restarted = f.m.engine(false).await;
    assert!(restarted.recover_rename_journal().await.unwrap().is_none());
    restarted.sync(None).await.unwrap();
    assert_fully_renamed(&f, &restarted).await;
}
both_backends!(
    a_full_rename_rewrites_links_in_every_writable_domain,
    a_full_rename_rewrites_links_in_every_writable_domain_body
);

/// `--local` through the same entry point: the MANIFEST and the content stay
/// as they are, and the report has the full rename's shape.
async fn a_local_rename_through_rename_domain_leaves_manifest_and_links_body(
    store: Arc<Mutex<dyn Store>>,
) {
    let f = full_machine(store, "").await;
    let engine = f.m.engine(true).await;
    let manifest_before = std::fs::read_to_string(f.m.root.join("eng/MANIFEST.md")).unwrap();
    let pager_before = std::fs::read_to_string(f.m.root.join("ops/pager.md")).unwrap();

    let report = engine
        .rename_domain("eng", "platform", true, &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(report["local_only"], true, "{report}");
    assert_eq!(report["manifest_written"], false, "{report}");
    assert_eq!(report["manifest_draft"], false, "{report}");
    assert_eq!(report["rewritten"], serde_json::json!([]), "{report}");
    assert_eq!(report["left_behind"], serde_json::json!([]), "{report}");
    assert_eq!(
        std::fs::read_to_string(f.m.root.join("eng/MANIFEST.md")).unwrap(),
        manifest_before
    );
    assert_eq!(
        std::fs::read_to_string(f.m.root.join("ops/pager.md")).unwrap(),
        pager_before
    );
    let cfg = crystalline_service::overlay::load_file(&f.m.config_path()).unwrap();
    assert_eq!(
        cfg.domains.get("platform").unwrap().name_origin,
        Some(NameOrigin::Explicit)
    );
}
both_backends!(
    a_local_rename_through_rename_domain_leaves_manifest_and_links,
    a_local_rename_through_rename_domain_leaves_manifest_and_links_body
);

/// Two private domains of bob's beside the full-rename machine, `archive`
/// (ada is a viewer) and `vault` (ada cannot see it), each with an `old.md`
/// linking to `eng` by its canonical name. Answers that engram's text.
async fn add_archive_and_vault(f: &Full) -> String {
    let m = &f.m;
    let old_link = engram(
        "Old",
        "old",
        &format!("The old runbook was [[{}:a]].", f.canonical),
    );
    let mut cfg = crystalline_service::overlay::load_file(&m.config_path()).unwrap();
    for name in ["archive", "vault"] {
        let dir = m.root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("MANIFEST.md"), manifest(name)).unwrap();
        std::fs::write(dir.join("old.md"), &old_link).unwrap();
        cfg.domains
            .insert(name.to_string(), DomainEntry::file(dir.clone()));
    }
    crystalline_core::config::save_yaml(&m.config_path(), &cfg).unwrap();
    let auth = AuthStore::open(&m.state().join("web-auth.db"))
        .await
        .unwrap();
    auth.add_user("bob", "Bob", None, Role::Editor, "pw12345678")
        .await
        .unwrap();
    for name in ["archive", "vault"] {
        auth.set_domain_visibility(name, true, "bob").await.unwrap();
    }
    auth.upsert_domain_member("archive", "ada", MemberLevel::Viewer, "bob")
        .await
        .unwrap();
    m.engine(true).await.sync(None).await.unwrap();
    old_link
}

fn ada() -> Scope {
    Scope::User {
        account: "ada".into(),
        admin: false,
    }
}

/// A caller who owns `eng`, writes `ops`, only reads `archive` and cannot see
/// `vault`: `ops` is rewritten, `archive`'s link is left as it is and listed,
/// and `vault` is named nowhere. The link left behind still resolves, through
/// the previous canonical name the rename kept as an alias.
async fn a_link_in_a_read_only_domain_is_left_behind_and_listed_body(store: Arc<Mutex<dyn Store>>) {
    let f = full_machine(store, "").await;
    let m = &f.m;
    let old_link = add_archive_and_vault(&f).await;
    let engine = m.engine(true).await;

    let report = engine
        .rename_domain("eng", "platform", false, &ada())
        .await
        .unwrap();
    assert_eq!(
        report["rewritten"],
        serde_json::json!([{ "domain": "ops", "engrams": 1, "references": 3 }]),
        "{report}"
    );
    assert_eq!(
        report["left_behind"],
        serde_json::json!([{ "domain": "archive", "path": "old.md", "references": 1 }]),
        "{report}"
    );
    assert!(!report.to_string().contains("vault"), "{report}");
    for name in ["archive", "vault"] {
        assert_eq!(
            std::fs::read_to_string(m.root.join(name).join("old.md")).unwrap(),
            old_link,
            "{name} is untouched"
        );
    }
    let pager = std::fs::read_to_string(m.root.join("ops/pager.md")).unwrap();
    assert!(pager.contains("[[platform:a]]"), "{pager}");
    let old = engine
        .read_engram(&read_params("old", "archive"), &Scope::Unrestricted)
        .await
        .unwrap();
    let links = old["links"].as_array().unwrap();
    assert_eq!(links.len(), 1, "{old}");
    assert_eq!(links[0]["resolved"], true, "{old}");
}
both_backends!(
    a_link_in_a_read_only_domain_is_left_behind_and_listed,
    a_link_in_a_read_only_domain_is_left_behind_and_listed_body
);

/// A rename a crash stopped after its MANIFEST step is finished at startup,
/// where no accounts resolver is installed and every domain would read as
/// writable: the relink still respells only where the caller could write.
async fn a_recovered_relink_writes_only_where_the_caller_could_body(store: Arc<Mutex<dyn Store>>) {
    let f = full_machine(store, "").await;
    let m = &f.m;
    let old_link = add_archive_and_vault(&f).await;
    let engine = m.engine(true).await;
    engine.fail_rename_after(Some(RenameStep::Manifest));
    engine
        .rename_domain("eng", "platform", false, &ada())
        .await
        .expect_err("the failpoint stops the rename");
    drop(engine);

    let restarted = m.engine(false).await;
    restarted
        .recover_rename_journal()
        .await
        .unwrap()
        .expect("a journal was left to finish");
    for name in ["archive", "vault"] {
        assert_eq!(
            std::fs::read_to_string(m.root.join(name).join("old.md")).unwrap(),
            old_link,
            "{name} is untouched"
        );
    }
    let pager = std::fs::read_to_string(m.root.join("ops/pager.md")).unwrap();
    assert!(pager.contains("[[platform:a]]"), "{pager}");
}
both_backends!(
    a_recovered_relink_writes_only_where_the_caller_could,
    a_recovered_relink_writes_only_where_the_caller_could_body
);

/// A spelling that does not reach `eng` today is not `eng`'s to respell: a
/// declared `domain_name` the local name `ops` shadows, and an alias the
/// table dropped because `ops` holds it. A link `[[ops:pager]]` keeps its
/// text and still reaches `ops` after the full rename.
async fn a_spelling_another_domain_holds_is_never_respelled_body(store: Arc<Mutex<dyn Store>>) {
    for case in ["declared", "alias"] {
        let m = machine_tagged(store.clone(), &format!("-{case}")).await;
        if case == "declared" {
            std::fs::write(
                m.root.join("eng/MANIFEST.md"),
                manifest_named("Eng", &m.ops),
            )
            .unwrap();
        } else {
            let mut cfg = crystalline_service::overlay::load_file(&m.config_path()).unwrap();
            cfg.domains.get_mut(&m.eng).unwrap().aliases = vec![m.ops.clone()];
            crystalline_core::config::save_yaml(&m.config_path(), &cfg).unwrap();
        }
        let runbook = engram(
            "Runbook",
            "runbook",
            &format!("Page with [[{}:pager]].", m.ops),
        );
        std::fs::write(m.root.join("ops/runbook.md"), &runbook).unwrap();
        let engine = m.engine(true).await;
        engine.sync(None).await.unwrap();

        let report = engine
            .rename_domain(&m.eng, &m.platform, false, &Scope::Unrestricted)
            .await
            .unwrap();
        assert_eq!(
            report["rewritten"],
            serde_json::json!([{ "domain": m.ops, "engrams": 1, "references": 1 }]),
            "{case}: only the pager's link to eng: {report}"
        );
        assert_eq!(
            std::fs::read_to_string(m.root.join("ops/runbook.md")).unwrap(),
            runbook,
            "{case}"
        );
        let read = engine
            .read_engram(&read_params("runbook", &m.ops), &Scope::Unrestricted)
            .await
            .unwrap();
        let links = read["links"].as_array().unwrap();
        assert_eq!(links.len(), 1, "{case}: {read}");
        assert_eq!(links[0]["target"]["domain"], m.ops, "{case}: {read}");
        assert_eq!(links[0]["resolved"], true, "{case}: {read}");
    }
}
both_backends!(
    a_spelling_another_domain_holds_is_never_respelled,
    a_spelling_another_domain_holds_is_never_respelled_body
);

/// A MANIFEST this machine cannot write refuses the full rename before any
/// step and names `--local`, and nothing changes. A caller who does not own
/// the domain is refused as forbidden, full or `--local`.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unwritable_manifest_refuses_and_names_local() {
    use std::os::unix::fs::PermissionsExt;

    let store: Arc<Mutex<dyn Store>> =
        Arc::new(Mutex::new(TursoStore::open_in_memory().await.unwrap()));
    let f = full_machine(store, "").await;
    let m = &f.m;
    let engine = m.engine(true).await;
    let config_before = std::fs::read_to_string(m.config_path()).unwrap();
    let manifest_before = std::fs::read_to_string(m.root.join("eng/MANIFEST.md")).unwrap();

    let eng_dir = m.root.join("eng");
    std::fs::set_permissions(&eng_dir, std::fs::Permissions::from_mode(0o555)).unwrap();
    // Running as root writes into a read-only folder anyway: nothing to test.
    let probe = eng_dir.join("probe");
    if std::fs::write(&probe, "x").is_ok() {
        let _ = std::fs::remove_file(&probe);
        std::fs::set_permissions(&eng_dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        eprintln!("skipped: a read-only folder is writable here (running as root?)");
        return;
    }
    let result = engine
        .rename_domain("eng", "platform", false, &Scope::Unrestricted)
        .await;
    std::fs::set_permissions(&eng_dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    match result.unwrap_err() {
        EngineError::Invalid(msg) => assert!(
            msg.contains("--local") && msg.contains("This machine only"),
            "{msg}"
        ),
        other => panic!("expected invalid, got {other:?}"),
    }
    assert!(!m.journal().exists(), "no step started");
    assert_eq!(
        std::fs::read_to_string(m.config_path()).unwrap(),
        config_before
    );
    assert_eq!(
        std::fs::read_to_string(m.root.join("eng/MANIFEST.md")).unwrap(),
        manifest_before
    );
    assert!(!engine.is_renaming("eng"));

    // `ops` is shared: ada writes it and does not own it.
    let ada = Scope::User {
        account: "ada".into(),
        admin: false,
    };
    for local_only in [false, true] {
        match engine
            .rename_domain("ops", "runbooks", local_only, &ada)
            .await
            .unwrap_err()
        {
            EngineError::Forbidden(_) => {}
            other => panic!("expected forbidden (local_only {local_only}), got {other:?}"),
        }
    }
    assert!(!m.journal().exists());
}

/// Open issue 8: a full rename onto another domain's canonical name goes
/// through and names that domain under `shadows`; onto another domain's
/// local name it is refused.
async fn a_full_rename_onto_a_canonical_shadows_and_onto_a_local_name_refuses_body(
    store: Arc<Mutex<dyn Store>>,
) {
    let f = full_machine(store, "").await;
    let m = &f.m;
    std::fs::write(
        m.root.join("ops/MANIFEST.md"),
        manifest_named("Ops", "platform"),
    )
    .unwrap();
    let engine = m.engine(true).await;
    engine.sync(None).await.unwrap();

    let msg = conflict(
        engine
            .rename_domain("eng", "ops", false, &Scope::Unrestricted)
            .await
            .unwrap_err(),
    );
    assert!(msg.contains("'ops' is already a domain here"), "{msg}");
    assert!(!m.journal().exists());

    let report = engine
        .rename_domain("eng", "platform", false, &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(report["shadows"], serde_json::json!(["ops"]), "{report}");
    assert_eq!(
        engine.local_domain_name("platform").await.as_deref(),
        Some("platform")
    );
    // The respelled links reach the renamed domain, not `ops`, which holds
    // no `a`, `b` or `c`.
    let read = engine
        .read_engram(&read_params("pager", "ops"), &Scope::Unrestricted)
        .await
        .unwrap();
    let links = read["links"].as_array().unwrap();
    assert!(!links.is_empty(), "{read}");
    for link in links {
        assert_eq!(link["target"]["domain"], "platform", "{read}");
        assert_eq!(link["resolved"], true, "{read}");
    }
}
both_backends!(
    a_full_rename_onto_a_canonical_shadows_and_onto_a_local_name_refuses,
    a_full_rename_onto_a_canonical_shadows_and_onto_a_local_name_refuses_body
);

/// A crash after the MANIFEST step and after the relink step: a new engine
/// over the same state finishes the rename and reaches the end state of a
/// rename nothing interrupted, including the previous canonical among the
/// aliases although the MANIFEST no longer names it. Each step is crashed
/// once after the journal recorded it and once with its effect landed and
/// the journal not yet saying so.
async fn a_full_rename_a_crash_stopped_is_finished_body(store: Arc<Mutex<dyn Store>>) {
    for (step, unrecorded) in [RenameStep::Manifest, RenameStep::Relink]
        .iter()
        .flat_map(|step| [(*step, false), (*step, true)])
    {
        let tag = format!("-{step:?}{}", if unrecorded { "-u" } else { "" }).to_lowercase();
        let f = full_machine(store.clone(), &tag).await;
        let m = &f.m;
        let engine = m.engine(true).await;

        engine.fail_rename_after(Some(step));
        let err = engine
            .rename_domain(&m.eng, &m.platform, false, &Scope::Unrestricted)
            .await
            .expect_err("the failpoint stops the rename");
        assert!(format!("{err}").contains("rename"), "{step:?}: {err}");
        assert!(m.journal().is_file(), "{step:?}: the journal stays");
        // The failpoint fired right after its own step: the MANIFEST is
        // written either way, the links only once the relink step ran.
        let manifest_text = std::fs::read_to_string(m.root.join("eng/MANIFEST.md")).unwrap();
        assert!(
            manifest_text.contains(&format!("domain_name: {}\n", m.platform)),
            "{step:?}: {manifest_text}"
        );
        let pager = std::fs::read_to_string(m.root.join("ops/pager.md")).unwrap();
        assert_eq!(
            pager.contains(&format!("[[{}:a]]", m.eng)),
            step == RenameStep::Manifest,
            "{step:?}: {pager}"
        );
        let cfg = crystalline_service::overlay::load_file(&m.config_path()).unwrap();
        assert!(
            cfg.domains.contains_key(&m.eng),
            "{step:?}: the config is written last"
        );
        drop(engine);
        if unrecorded {
            let mut journal: serde_json::Value =
                serde_json::from_slice(&std::fs::read(m.journal()).unwrap()).unwrap();
            let done = journal["done"].as_array_mut().unwrap();
            assert_eq!(done.last(), Some(&serde_json::json!(step.name())));
            done.pop();
            std::fs::write(m.journal(), serde_json::to_vec(&journal).unwrap()).unwrap();
        }

        let restarted = m.engine(false).await;
        let recovered = restarted
            .recover_rename_journal()
            .await
            .unwrap()
            .unwrap_or_else(|| panic!("{step:?}: a journal was left to finish"));
        assert_eq!(recovered["domain"], m.platform, "{step:?}: {recovered}");
        assert_eq!(recovered["manifest_written"], true, "{step:?}: {recovered}");
        // The relink counts travel in the journal: a recovery after a
        // recorded relink step reports what the first run respelled. After
        // an unrecorded one the step runs again and finds nothing left.
        let references = if step == RenameStep::Relink && unrecorded {
            serde_json::json!([])
        } else {
            serde_json::json!([{ "domain": m.ops, "engrams": 1, "references": 3 }])
        };
        assert_eq!(recovered["rewritten"], references, "{step:?}: {recovered}");
        assert_eq!(
            recovered["moved"],
            serde_json::json!([
                "index_row",
                "auth_tables",
                "origins_dir",
                "overlays_dir",
                "provision_receipt",
                "config"
            ]),
            "{step:?}: {recovered}"
        );
        restarted.sync(None).await.unwrap();
        assert_fully_renamed(&f, &restarted).await;
    }
}
both_backends!(
    a_full_rename_a_crash_stopped_is_finished,
    a_full_rename_a_crash_stopped_is_finished_body
);

/// A write still running in the domain once the MANIFEST and relink steps are
/// done: the drain gives up, the rename answers a conflict that names how to
/// finish it, keeps its journal and lifts the pause. Another rename is
/// refused naming the pending one; sending the same rename again finishes it
/// and reports the relink counts of the first run.
async fn a_busy_domain_after_relink_answers_a_conflict_and_a_resend_finishes_body(
    store: Arc<Mutex<dyn Store>>,
) {
    let f = full_machine(store, "").await;
    let m = &f.m;
    let engine = m.engine(true).await;
    engine.set_rename_drain_wait(Some(Duration::from_millis(300)));
    let hold = engine.hold_rename_after(RenameStep::Relink);
    let renamer = {
        let engine = engine.clone();
        tokio::spawn(async move {
            engine
                .rename_domain("eng", "platform", false, &Scope::Unrestricted)
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(20), hold.reached())
        .await
        .expect("the rename reaches its hold after the relink step");
    let write_hold = engine.hold_next_write();
    let writer = {
        let engine = engine.clone();
        tokio::spawn(async move {
            engine
                .write_engram(&write_params(
                    "eng",
                    "Beta",
                    "written while the rename waits",
                ))
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(20), write_hold.reached())
        .await
        .expect("the write is counted and held");
    hold.release();

    let msg = conflict(renamer.await.unwrap().unwrap_err());
    assert!(
        msg.contains("domain 'eng' is busy with a write that has not finished")
            && msg
                .contains("send the same rename again (`crystalline domain rename eng platform`)")
            && msg.contains("restart the daemon"),
        "{msg}"
    );
    let journal: serde_json::Value =
        serde_json::from_slice(&std::fs::read(m.journal()).unwrap()).unwrap();
    assert_eq!(
        journal["done"],
        serde_json::json!(["manifest", "relink"]),
        "{journal}"
    );
    assert!(
        !engine.is_renaming("eng") && !engine.is_renaming("platform"),
        "the pause is lifted"
    );
    let cfg = crystalline_service::overlay::load_file(&m.config_path()).unwrap();
    assert!(cfg.domains.contains_key("eng"), "the config is untouched");

    write_hold.release();
    let written = writer.await.unwrap().unwrap();
    assert_eq!(written["domain"], "eng", "{written}");

    let msg = conflict(
        engine
            .rename_domain("ops", "runbooks", false, &Scope::Unrestricted)
            .await
            .unwrap_err(),
    );
    assert!(
        msg.contains("the rename of 'eng' to 'platform' has not finished")
            && msg.contains("`crystalline domain rename eng platform`")
            && msg.contains("restart the daemon"),
        "{msg}"
    );

    let report = engine
        .rename_domain("eng", "platform", false, &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(report["manifest_written"], true, "{report}");
    assert_eq!(
        report["rewritten"],
        serde_json::json!([{ "domain": "ops", "engrams": 1, "references": 3 }]),
        "the counts of the first run: {report}"
    );
    assert_fully_renamed(&f, &engine).await;
    assert!(m.root.join("eng/beta.md").is_file());
}
both_backends!(
    a_busy_domain_after_relink_answers_a_conflict_and_a_resend_finishes,
    a_busy_domain_after_relink_answers_a_conflict_and_a_resend_finishes_body
);

/// The relink step writes through the ordinary edit path: in a reviewing
/// domain the respelled engram becomes the caller's draft and the file
/// stays as the team reviewed it; in a virtual domain the row is respelled.
async fn relink_drafts_in_a_reviewing_domain_and_rewrites_a_virtual_one_body(
    store: Arc<Mutex<dyn Store>>,
) {
    let m = machine(store).await;
    let mut cfg = crystalline_service::overlay::load_file(&m.config_path()).unwrap();
    cfg.domains.get_mut("ops").unwrap().review =
        Some(crystalline_core::config::ReviewMode::Overlay);
    cfg.domains
        .insert("notes".to_string(), DomainEntry::virtual_domain());
    crystalline_core::config::save_yaml(&m.config_path(), &cfg).unwrap();
    let engine = m.engine(true).await;
    engine.sync(None).await.unwrap();
    engine
        .write_engram(&write_params("notes", "Links", "See [[eng:alpha]] first."))
        .await
        .unwrap();
    let pager_before = std::fs::read_to_string(m.root.join("ops/pager.md")).unwrap();

    let report = engine
        .rename_domain("eng", "platform", false, &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(
        report["rewritten"],
        serde_json::json!([
            { "domain": "notes", "engrams": 1, "references": 1 },
            { "domain": "ops", "engrams": 1, "references": 1 }
        ]),
        "{report}"
    );

    assert_eq!(
        std::fs::read_to_string(m.root.join("ops/pager.md")).unwrap(),
        pager_before,
        "the reviewed file is untouched"
    );
    let drafts = {
        let store = m.store.lock().await;
        let id = store.domain_id("ops").await.unwrap().unwrap();
        store.overlay_entries(id, "owner").await.unwrap()
    };
    let pager = drafts
        .iter()
        .find(|e| e.path == "pager.md")
        .unwrap_or_else(|| panic!("the owner holds a draft of the pager"));
    assert!(
        pager.content.contains("[[platform:alpha]]"),
        "{}",
        pager.content
    );

    let links = engine
        .read_engram(&read_params("links", "notes"), &Scope::Unrestricted)
        .await
        .unwrap();
    let content = links["content"].as_str().unwrap();
    assert!(content.contains("[[platform:alpha]]"), "{content}");
    assert!(!content.contains("[[eng:alpha]]"), "{content}");
}
both_backends!(
    relink_drafts_in_a_reviewing_domain_and_rewrites_a_virtual_one,
    relink_drafts_in_a_reviewing_domain_and_rewrites_a_virtual_one_body
);

/// After a full rename the REST paths under the old local name and the
/// previous canonical name answer the renamed domain.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn old_rest_paths_answer_the_renamed_domain() {
    use crystalline_core::config::{AuthConfig, ResponseFormat, ServiceConfig};

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let eng_dir = root.join("eng");
    std::fs::create_dir_all(&eng_dir).unwrap();
    std::fs::write(
        eng_dir.join("MANIFEST.md"),
        manifest_named("Eng", "eng-team"),
    )
    .unwrap();
    std::fs::write(
        eng_dir.join("a.md"),
        engram("A", "a", "an engineering note"),
    )
    .unwrap();
    let mut cfg = GlobalConfig {
        auth: Some(AuthConfig {
            anonymous: Some(true),
            ..AuthConfig::default()
        }),
        service: Some(ServiceConfig {
            response_format: Some(ResponseFormat::Json),
            ..ServiceConfig::default()
        }),
        ..GlobalConfig::default()
    };
    cfg.domains
        .insert("eng".to_string(), DomainEntry::file(eng_dir.clone()));
    let config_path = root.join("config.yaml");
    crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();
    let engine = Arc::new(
        Engine::new(
            Arc::new(Mutex::new(TursoStore::open_in_memory().await.unwrap())),
            cfg,
            None,
            Some(config_path),
        )
        .with_state_dir(root.join("state")),
    );
    engine.sync(None).await.unwrap();
    // Spawned, as a request handler would run it: the rename is `Send`.
    let renamer = engine.clone();
    tokio::spawn(async move {
        renamer
            .rename_domain("eng", "platform", false, &Scope::Unrestricted)
            .await
    })
    .await
    .unwrap()
    .unwrap();

    let auth = Arc::new(AuthStore::open(&root.join("web-auth.db")).await.unwrap());
    let router = crystalline_service::daemon::http_router(
        engine.clone(),
        Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        &[],
        auth,
        None,
    )
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    let client = reqwest::Client::new();
    let mut bodies = Vec::new();
    for spelling in ["platform", "eng", "eng-team"] {
        let resp = client
            .get(format!(
                "http://{addr}/api/v1/domains/{spelling}/tree?depth=2"
            ))
            .send()
            .await
            .unwrap();
        let status = resp.status();
        let text = resp.text().await.unwrap();
        assert_eq!(status, 200, "{spelling}: {text}");
        assert!(text.contains("a.md"), "{spelling}: {text}");
        bodies.push(text);
    }
    assert_eq!(bodies[0], bodies[1], "eng answers the renamed domain");
    assert_eq!(bodies[0], bodies[2], "eng-team answers the renamed domain");
}

/// A full rename that stopped after its relink step leaves a MANIFEST that
/// already declares the new name, a config that still holds the old one and,
/// once the engine is gone, a domain nothing pauses. The adoption after a
/// sync must not rename that domain outside the journal, nor infer or write
/// anything for it: the journal is how the rename finishes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn adoption_leaves_a_domain_with_a_pending_rename_journal_alone() {
    let store: Arc<Mutex<dyn Store>> =
        Arc::new(Mutex::new(TursoStore::open_in_memory().await.unwrap()));
    let f = full_machine(store, "").await;
    let m = &f.m;
    let engine = m.engine(true).await;
    engine.fail_rename_after(Some(RenameStep::Relink));
    engine
        .rename_domain("eng", "platform", false, &Scope::Unrestricted)
        .await
        .expect_err("the failpoint stops the rename after its relink step");
    drop(engine);
    let manifest_text = std::fs::read_to_string(m.root.join("eng/MANIFEST.md")).unwrap();
    assert!(
        manifest_text.contains("domain_name: platform\n"),
        "{manifest_text}"
    );
    let eng_before = crystalline_service::overlay::load_file(&m.config_path())
        .unwrap()
        .domains["eng"]
        .clone();
    let journal_before = std::fs::read(m.journal()).unwrap();

    // A fresh engine over the same state: nothing pauses `eng` now.
    let engine = m.engine(true).await;
    assert!(!engine.is_renaming("eng"));
    engine.sync(None).await.unwrap();
    let report = engine.adopt_domain_names().await.unwrap();

    assert!(
        report
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["domain"] != "eng" && r["domain"] != "platform"),
        "{report}"
    );
    let cfg = crystalline_service::overlay::load_file(&m.config_path()).unwrap();
    assert!(cfg.domains.contains_key("eng"), "{:?}", cfg.domains.keys());
    assert!(!cfg.domains.contains_key("platform"));
    assert_eq!(cfg.domains["eng"].name_origin, None, "nothing inferred");
    assert_eq!(cfg.domains["eng"], eng_before, "nothing written for it");
    assert_eq!(std::fs::read(m.journal()).unwrap(), journal_before);

    // The journal still finishes the rename as the user asked for it.
    let recovered = engine.recover_rename_journal().await.unwrap().unwrap();
    assert_eq!(recovered["domain"], "platform", "{recovered}");
    engine.sync(None).await.unwrap();
    assert_fully_renamed(&f, &engine).await;
}
