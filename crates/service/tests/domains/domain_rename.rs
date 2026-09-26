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
use crystalline_service::rest::{AuthStore, Role};
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
        msg.contains("a rename of 'eng' is still finishing; try again in a moment"),
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
