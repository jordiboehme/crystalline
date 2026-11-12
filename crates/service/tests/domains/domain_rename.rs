//! Renaming a domain on this machine (`rename --local`): every store keyed
//! by the name moves, the index row keeps its id, the domain pauses while it
//! moves, a crash after any step is finished by the next engine before it
//! syncs, and each refusal names its next step.
//!
//! Every body runs against Turso (in memory, always) and Postgres (a fresh
//! per-test schema, when `CRYSTALLINE_TEST_POSTGRES_URL` is set; skipped with
//! a note otherwise), through one store shared by `Arc` so a second engine
//! over the same store stands for a restarted daemon.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crystalline_core::config::{
    DomainEntry, GlobalConfig, NameOrigin, RenameAnswer, RenameQuestion,
};
use crystalline_core::provision::receipt::{self, DomainSources, ProvisionReceipt};
use crystalline_index::{Store, TursoStore};
use crystalline_service::changes::{Change, DomainAudience, Envelope};
use crystalline_service::engine::{Engine, EngineError, RenameStep};
use crystalline_service::overlay::EnvOverlay;
use crystalline_service::params::*;
use crystalline_service::rest::{AuthStore, MemberLevel, Role};
use crystalline_service::{DomainAccess, Scope};
use tokio::sync::Mutex;

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
                if let Some(scratch) = crate::support::scratch_schema("ren") {
                    let pg = crystalline_index::PostgresStore::open_in_schema(
                        scratch.url(),
                        scratch.name(),
                    )
                    .await
                    .expect("open the postgres test schema");
                    let store: Arc<Mutex<dyn Store>> = Arc::new(Mutex::new(pg));
                    $body(store).await;
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
        path: None,
        base64: None,
        domain: domain.to_string(),
        title: title.to_string(),
        content: Some(content.to_string()),
        folder: None,
        engram_type: None,
        tags: Vec::new(),
        status: None,
        metadata: None,
        overwrite: false,
        share_link: None,
        model: None,
        expected_checksum: None,
    }
}

fn read_params(identifier: &str, domain: &str) -> ReadParams {
    ReadParams {
        identifier: identifier.to_string(),
        domain: Some(domain.to_string()),
        share_link: None,
        include_images: None,
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
        let engine = crate::support::as_this_machine(engine, &self.store).await;
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
            "kept_dir",
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

/// A rename over a configuration file that does not parse is refused before
/// any step: its last step would refuse to save over the broken file, after
/// the domain was moved and paused. Nothing moves, no journal is left, the
/// domain is not paused and the file stays byte for byte. Once the file
/// parses again, the same rename lands.
async fn a_rename_over_a_config_that_does_not_parse_moves_nothing_body(
    store: Arc<Mutex<dyn Store>>,
) {
    let m = machine(store).await;
    let engine = m.engine(true).await;
    let eng_id = domain_id(&m.store, "eng").await.expect("eng indexed");
    let good = std::fs::read_to_string(m.config_path()).unwrap();
    let broken = "domains: [unclosed\n";
    std::fs::write(m.config_path(), broken).unwrap();

    let err = engine
        .rename_domain_local(
            "eng",
            "platform",
            NameOrigin::Explicit,
            &Scope::Unrestricted,
        )
        .await
        .expect_err("a config that does not parse refuses the rename");
    assert!(format!("{err}").contains("left as it is"), "{err}");
    assert_eq!(std::fs::read_to_string(m.config_path()).unwrap(), broken);
    assert!(!m.journal().exists(), "no journal");
    assert!(
        !engine.is_renaming("eng") && !engine.is_renaming("platform"),
        "nothing is paused"
    );
    assert_eq!(domain_id(&m.store, "eng").await, Some(eng_id));
    assert_eq!(domain_id(&m.store, "platform").await, None);
    let state = m.state();
    assert!(state.join("origins/eng/state.json").is_file());
    assert!(!state.join("origins/platform").exists());
    assert!(state.join("overlays/eng/ada/note.md").is_file());
    assert!(!state.join("overlays/platform").exists());
    let alpha = engine
        .read_engram(&read_params("alpha", "eng"), &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(alpha["domain"], "eng", "{alpha}");

    std::fs::write(m.config_path(), good).unwrap();
    engine
        .rename_domain_local(
            "eng",
            "platform",
            NameOrigin::Explicit,
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert_renamed(&m, &engine, eng_id).await;
}
both_backends!(
    a_rename_over_a_config_that_does_not_parse_moves_nothing,
    a_rename_over_a_config_that_does_not_parse_moves_nothing_body
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
        RenameStep::KeptDir,
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
    // Waited for, not slept on: the renamer task may not have run at all yet
    // on a loaded machine. The pause is set right before the drain, and the
    // drain cannot end while the write above is held, so once the pause shows
    // the rename is parked on that write.
    tokio::time::timeout(Duration::from_secs(20), async {
        while !engine.is_renaming("eng") {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the rename has paused the domain");
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
    // The fixture's engine ran as this machine and holds the locks on a
    // shared database; the two instances of this test are others.
    {
        let store = m.store.lock().await;
        for name in [&m.eng, &m.ops] {
            if let Some(id) = store.domain_id(name).await.unwrap() {
                // A release names its claim's token: claim once more as
                // this machine and give that claim back.
                let now = "2026-01-01T00:00:00+00:00";
                let crystalline_index::HostClaim::Acquired { token } = store
                    .claim_domain_host(id, crate::support::THIS_MACHINE, "", now, now, true)
                    .await
                    .unwrap()
                else {
                    panic!("a take-over acquires");
                };
                store
                    .release_domain_host(id, crate::support::THIS_MACHINE, &token)
                    .await
                    .unwrap();
            }
        }
    }
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

/// A full rename of `eng` over `manifest_text`, answering the MANIFEST it
/// leaves behind without the `generated` stamp the edit path adds.
async fn manifest_after_rename(store: Arc<Mutex<dyn Store>>, manifest_text: &str) -> String {
    let m = machine(store).await;
    std::fs::write(m.root.join("eng/MANIFEST.md"), manifest_text).unwrap();
    let engine = m.engine(true).await;
    engine.sync(None).await.unwrap();
    let report = engine
        .rename_domain("eng", "platform", false, &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(report["manifest_written"], true, "{report}");
    std::fs::read_to_string(m.root.join("eng/MANIFEST.md"))
        .unwrap()
        .split_inclusive('\n')
        .filter(|line| !line.starts_with("generated: "))
        .collect()
}

/// A MANIFEST that names no domain yet gets `domain_name` right after the
/// title, where a person adding it by hand puts it.
async fn a_rename_puts_a_new_domain_name_right_after_the_title_body(store: Arc<Mutex<dyn Store>>) {
    let renamed = manifest_after_rename(store, &manifest("Eng")).await;
    assert_eq!(
        renamed,
        manifest("Eng").replacen("title: Eng\n", "title: Eng\ndomain_name: platform\n", 1)
    );
}
both_backends!(
    a_rename_puts_a_new_domain_name_right_after_the_title,
    a_rename_puts_a_new_domain_name_right_after_the_title_body
);

/// A `domain_name` key spelled with quotes is the key: it is rewritten where
/// it stands, never joined by a second copy the MANIFEST could not parse
/// with.
async fn a_rename_rewrites_a_quoted_domain_name_key_in_place_body(store: Arc<Mutex<dyn Store>>) {
    let quoted = manifest("Eng").replacen(
        "status: current\n",
        "status: current\n\"domain_name\": eng-team\n",
        1,
    );
    let renamed = manifest_after_rename(store, &quoted).await;
    assert_eq!(
        renamed,
        manifest("Eng").replacen(
            "status: current\n",
            "status: current\ndomain_name: platform\n",
            1
        )
    );
}
both_backends!(
    a_rename_rewrites_a_quoted_domain_name_key_in_place,
    a_rename_rewrites_a_quoted_domain_name_key_in_place_body
);

/// A MANIFEST in flow style cannot take the new name key by key: the
/// rename leaves it byte for byte, reports it as not written and says why.
async fn a_rename_reports_a_flow_style_manifest_as_not_written_body(store: Arc<Mutex<dyn Store>>) {
    let m = machine(store).await;
    let flow = "---\n{type: manifest, title: Eng, permalink: manifest, tags: [manifest], status: current, recorded_at: 2026-01-01, domain_name: eng-team}\n---\n\n# Eng\n\n## Scope\n\n- covers things\n\n## When to Use\n\n- when routing\n";
    std::fs::write(m.root.join("eng/MANIFEST.md"), flow).unwrap();
    let engine = m.engine(true).await;
    engine.sync(None).await.unwrap();
    let report = engine
        .rename_domain("eng", "platform", false, &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(report["manifest_written"], false, "{report}");
    let note = report["note"].as_str().unwrap_or_default();
    assert!(
        note.contains("domain_name") && note.contains("by hand"),
        "{report}"
    );
    assert_eq!(
        std::fs::read_to_string(m.root.join("eng/MANIFEST.md")).unwrap(),
        flow
    );
    // The MANIFEST still declares the old name, and so does the record.
    let cfg = crystalline_service::overlay::load_file(&m.config_path()).unwrap();
    assert_eq!(
        cfg.domains["platform"].canonical_seen.as_deref(),
        Some("eng-team")
    );
}
both_backends!(
    a_rename_reports_a_flow_style_manifest_as_not_written,
    a_rename_reports_a_flow_style_manifest_as_not_written_body
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
    auth.upsert_domain_member("archive", "ada", MemberLevel::Viewer, None, "bob")
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

/// A full rename onto another domain's canonical name goes
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
                "kept_dir",
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
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
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

// --- item 11: a reviewing domain's MANIFEST ------------------------------------

const FLOW_MANIFEST: &str = "---\n{type: manifest, title: Eng, permalink: manifest, tags: [manifest], status: current, recorded_at: 2026-01-01, domain_name: eng-team}\n---\n\n# Eng\n\n## Scope\n\n- covers things\n\n## When to Use\n\n- when routing\n";

const KEPT_IN_DRAFT: &str = "The MANIFEST's frontmatter is in a form Crystalline cannot change key by key (such as {title: ...}), so its domain_name was left as it is; set it to 'platform' by hand in your draft of the MANIFEST.";

fn draft_record(text: &str, path: &str) -> crystalline_index::EngramRecord {
    let mut record = crystalline_index::EngramRecord::from_engram(
        &crystalline_core::parse_engram(text).unwrap(),
        path,
        crystalline_index::FileStamp {
            mtime: 0,
            size: text.len() as u64,
            sha256: "0".repeat(64),
        },
    );
    record.content = text.to_string();
    record
}

/// [`machine`], with `eng` reviewing changes, its MANIFEST holding `base`,
/// and the owner's draft of that MANIFEST holding `draft` when one is given.
async fn reviewing_eng(
    store: Arc<Mutex<dyn Store>>,
    base: &str,
    draft: Option<&str>,
) -> (Machine, Arc<Engine>) {
    let m = machine(store).await;
    std::fs::write(m.root.join("eng/MANIFEST.md"), base).unwrap();
    let mut cfg = crystalline_service::overlay::load_file(&m.config_path()).unwrap();
    cfg.domains.get_mut("eng").unwrap().review =
        Some(crystalline_core::config::ReviewMode::Overlay);
    crystalline_core::config::save_yaml(&m.config_path(), &cfg).unwrap();
    let engine = m.engine(true).await;
    engine.sync(None).await.unwrap();
    if let Some(draft) = draft {
        let id = domain_id(&m.store, "eng").await.expect("eng indexed");
        m.store
            .lock()
            .await
            .upsert_overlay(
                crystalline_index::DomainId(id),
                "owner",
                &draft_record(draft, "MANIFEST.md"),
            )
            .await
            .unwrap();
        crystalline_service::overlay_journal::journal_write(
            &m.state(),
            "eng",
            "owner",
            "MANIFEST.md",
            draft,
        )
        .unwrap();
    }
    (m, engine)
}

/// The owner's draft of the renamed domain's MANIFEST, if there is one.
async fn owner_manifest_draft(m: &Machine) -> Option<String> {
    let id = domain_id(&m.store, "platform").await.expect("renamed row");
    let store = m.store.lock().await;
    store
        .overlay_entries(crystalline_index::DomainId(id), "owner")
        .await
        .unwrap()
        .into_iter()
        .find(|entry| entry.path == "MANIFEST.md" && !entry.tombstone)
        .map(|entry| entry.content)
}

async fn a_reviewing_domain_with_a_flow_style_manifest_renames_and_leaves_it_body(
    store: Arc<Mutex<dyn Store>>,
) {
    let (m, engine) = reviewing_eng(store, FLOW_MANIFEST, None).await;
    let report = engine
        .rename_domain("eng", "platform", false, &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(report["manifest_written"], false, "{report}");
    assert_eq!(report["manifest_draft"], false, "{report}");
    assert_eq!(report["note"], KEPT_IN_DRAFT, "{report}");
    assert_eq!(owner_manifest_draft(&m).await, None, "nothing was drafted");
    assert_eq!(
        std::fs::read_to_string(m.root.join("eng/MANIFEST.md")).unwrap(),
        FLOW_MANIFEST
    );
    assert!(!m.journal().is_file(), "the rename finished");
    assert!(!engine.is_renaming("eng") && !engine.is_renaming("platform"));
}
both_backends!(
    a_reviewing_domain_with_a_flow_style_manifest_renames_and_leaves_it,
    a_reviewing_domain_with_a_flow_style_manifest_renames_and_leaves_it_body
);

async fn a_flow_style_draft_over_a_block_style_base_is_judged_by_the_draft_body(
    store: Arc<Mutex<dyn Store>>,
) {
    let (m, engine) = reviewing_eng(
        store,
        &manifest_named("Eng", "eng-team"),
        Some(FLOW_MANIFEST),
    )
    .await;
    let report = engine
        .rename_domain("eng", "platform", false, &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(report["manifest_draft"], false, "{report}");
    assert_eq!(report["note"], KEPT_IN_DRAFT, "{report}");
    assert_eq!(
        owner_manifest_draft(&m).await.as_deref(),
        Some(FLOW_MANIFEST),
        "the draft is left as it is"
    );
}
both_backends!(
    a_flow_style_draft_over_a_block_style_base_is_judged_by_the_draft,
    a_flow_style_draft_over_a_block_style_base_is_judged_by_the_draft_body
);

async fn a_block_style_draft_over_a_flow_style_base_is_drafted_body(store: Arc<Mutex<dyn Store>>) {
    let (m, engine) = reviewing_eng(
        store,
        FLOW_MANIFEST,
        Some(&manifest_named("Eng", "eng-team")),
    )
    .await;
    let report = engine
        .rename_domain("eng", "platform", false, &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(report["manifest_draft"], true, "{report}");
    assert!(report["note"].is_null(), "{report}");
    let draft = owner_manifest_draft(&m).await.expect("the draft is there");
    assert!(draft.contains("domain_name: platform"), "{draft}");
    assert_eq!(
        std::fs::read_to_string(m.root.join("eng/MANIFEST.md")).unwrap(),
        FLOW_MANIFEST,
        "the reviewed file is untouched"
    );
}
both_backends!(
    a_block_style_draft_over_a_flow_style_base_is_drafted,
    a_block_style_draft_over_a_flow_style_base_is_drafted_body
);

async fn a_full_rename_of_a_domain_without_a_manifest_is_still_refused_naming_local_body(
    store: Arc<Mutex<dyn Store>>,
) {
    let m = machine(store).await;
    std::fs::remove_file(m.root.join("eng/MANIFEST.md")).unwrap();
    let engine = m.engine(true).await;
    engine.sync(None).await.unwrap();
    let err = engine
        .rename_domain("eng", "platform", false, &Scope::Unrestricted)
        .await
        .expect_err("a full rename needs a MANIFEST to write");
    let said = err.to_string();
    assert!(
        said.contains("domain 'eng' has no MANIFEST to write the new name into")
            && said.contains("--local"),
        "{said}"
    );
}
both_backends!(
    a_full_rename_of_a_domain_without_a_manifest_is_still_refused_naming_local,
    a_full_rename_of_a_domain_without_a_manifest_is_still_refused_naming_local_body
);

async fn a_reviewing_domain_without_a_manifest_is_refused_naming_local_body(
    store: Arc<Mutex<dyn Store>>,
) {
    let (m, engine) = reviewing_eng(store, FLOW_MANIFEST, None).await;
    std::fs::remove_file(m.root.join("eng/MANIFEST.md")).unwrap();
    engine.sync(None).await.unwrap();
    let err = engine
        .rename_domain("eng", "platform", false, &Scope::Unrestricted)
        .await
        .expect_err("a full rename needs a MANIFEST to write");
    let said = err.to_string();
    assert!(
        said.contains("domain 'eng' has no MANIFEST to write the new name into")
            && said.contains("--local"),
        "{said}"
    );
    assert!(!m.journal().is_file(), "nothing was started");
    assert!(!engine.is_renaming("eng"));
}
both_backends!(
    a_reviewing_domain_without_a_manifest_is_refused_naming_local,
    a_reviewing_domain_without_a_manifest_is_refused_naming_local_body
);

async fn a_resumed_rename_of_a_reviewing_domain_keeps_the_manifest_and_the_note_body(
    store: Arc<Mutex<dyn Store>>,
) {
    let (m, engine) = reviewing_eng(store, FLOW_MANIFEST, None).await;
    engine.fail_rename_after(Some(RenameStep::IndexRow));
    engine
        .rename_domain("eng", "platform", false, &Scope::Unrestricted)
        .await
        .expect_err("the failpoint stops the rename");
    assert!(m.journal().is_file(), "the journal stays");
    drop(engine);

    let restarted = m.engine(false).await;
    let recovered = restarted
        .recover_rename_journal()
        .await
        .unwrap()
        .expect("a journal was left to finish");
    assert_eq!(recovered["manifest_written"], false, "{recovered}");
    assert_eq!(recovered["manifest_draft"], false, "{recovered}");
    assert_eq!(recovered["note"], KEPT_IN_DRAFT, "{recovered}");
    assert_eq!(owner_manifest_draft(&m).await, None, "nothing was drafted");
    assert_eq!(
        std::fs::read_to_string(m.root.join("eng/MANIFEST.md")).unwrap(),
        FLOW_MANIFEST
    );
    assert!(!m.journal().is_file(), "the rename finished");
}
both_backends!(
    a_resumed_rename_of_a_reviewing_domain_keeps_the_manifest_and_the_note,
    a_resumed_rename_of_a_reviewing_domain_keeps_the_manifest_and_the_note_body
);

// --- item 14: a resumed rename's audience --------------------------------------

/// Every `domain` frame heard, as `(domain, audience)`.
fn domain_frames(
    rx: &mut tokio::sync::broadcast::Receiver<Envelope>,
) -> Vec<(String, Option<DomainAudience>)> {
    let mut frames = Vec::new();
    while let Ok(envelope) = rx.try_recv() {
        if let Change::Domain(domain) = envelope.change {
            frames.push((domain.domain, domain.audience));
        }
    }
    frames
}

fn nobody() -> Option<DomainAudience> {
    Some(DomainAudience::Accounts(HashSet::new()))
}

/// A rename stopped after its index row, before the privacy records moved,
/// so a resume that read the audience afresh would find ada among the old
/// name's readers.
async fn stopped_rename(m: &Machine) {
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
    assert!(m.journal().is_file(), "the journal stays");
}

fn assert_owner_only(m: &Machine, frames: &[(String, Option<DomainAudience>)]) {
    let old: Vec<_> = frames.iter().filter(|(d, _)| *d == m.eng).collect();
    assert!(!old.is_empty(), "the old name is announced: {frames:?}");
    assert!(
        old.iter().all(|(_, audience)| *audience == nobody()),
        "to the machine owner only: {frames:?}"
    );
    assert!(
        frames.contains(&(m.platform.clone(), None)),
        "the new name keeps the ordinary check: {frames:?}"
    );
}

async fn a_rename_recovered_at_start_announces_the_old_name_to_the_owner_only_body(
    store: Arc<Mutex<dyn Store>>,
) {
    let m = machine(store).await;
    stopped_rename(&m).await;
    let restarted = m.engine(true).await;
    let mut rx = restarted.changes().subscribe();
    restarted
        .recover_rename_journal()
        .await
        .unwrap()
        .expect("a journal was left to finish");
    assert_owner_only(&m, &domain_frames(&mut rx));
}
both_backends!(
    a_rename_recovered_at_start_announces_the_old_name_to_the_owner_only,
    a_rename_recovered_at_start_announces_the_old_name_to_the_owner_only_body
);

async fn a_rename_sent_again_announces_the_old_name_to_the_owner_only_body(
    store: Arc<Mutex<dyn Store>>,
) {
    let m = machine(store).await;
    stopped_rename(&m).await;
    let again = m.engine(true).await;
    let mut rx = again.changes().subscribe();
    again
        .rename_domain_local(
            &m.eng,
            &m.platform,
            NameOrigin::Explicit,
            &Scope::Unrestricted,
        )
        .await
        .expect("the same rename sent again finishes the journal");
    assert_owner_only(&m, &domain_frames(&mut rx));
}
both_backends!(
    a_rename_sent_again_announces_the_old_name_to_the_owner_only,
    a_rename_sent_again_announces_the_old_name_to_the_owner_only_body
);

/// Lock `name`'s MANIFEST on disk: the machine fixtures write
/// `status: current` in every MANIFEST.
fn lock(m: &Machine, dir: &str) {
    let path = m.root.join(dir).join("MANIFEST.md");
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::write(
        &path,
        text.replacen("status: current\n", "status: current\nread_only: on\n", 1),
    )
    .unwrap();
}

/// A full rename leaves the links in a read-only domain as they are and
/// lists them under `left_behind`, like a domain the caller cannot write
/// (decision D7). The links still resolve through the alias the rename keeps.
async fn a_link_in_a_locked_domain_is_left_behind_and_listed_body(store: Arc<Mutex<dyn Store>>) {
    let f = full_machine(store, "").await;
    let m = &f.m;
    lock(m, "ops");
    let pager_before = std::fs::read_to_string(m.root.join("ops/pager.md")).unwrap();
    let engine = m.engine(true).await;
    let report = engine
        .rename_domain("eng", "platform", false, &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(report["rewritten"], serde_json::json!([]), "{report}");
    assert_eq!(
        report["left_behind"],
        serde_json::json!([{ "domain": "ops", "path": "pager.md", "references": 3 }]),
        "{report}"
    );
    assert_eq!(
        std::fs::read_to_string(m.root.join("ops/pager.md")).unwrap(),
        pager_before,
        "the locked domain is untouched"
    );
}
both_backends!(
    a_link_in_a_locked_domain_is_left_behind_and_listed,
    a_link_in_a_locked_domain_is_left_behind_and_listed_body
);

/// A `--local` rename writes nothing into the domain, so a read-only domain
/// takes it (decision D6); a full rename writes its MANIFEST and is refused.
async fn a_local_rename_of_a_locked_domain_is_allowed_body(store: Arc<Mutex<dyn Store>>) {
    let m = machine(store).await;
    lock(&m, "ops");
    let manifest_before = std::fs::read(m.root.join("ops/MANIFEST.md")).unwrap();
    let engine = m.engine(true).await;
    let full = engine
        .rename_domain("ops", "pages", false, &Scope::Unrestricted)
        .await
        .unwrap_err();
    assert!(matches!(full, EngineError::DomainReadOnly(_)), "{full:?}");
    engine
        .rename_domain("ops", "pages", true, &Scope::Unrestricted)
        .await
        .expect("a local rename writes nothing into the domain");
    assert_eq!(
        std::fs::read(m.root.join("ops/MANIFEST.md")).unwrap(),
        manifest_before
    );
}
both_backends!(
    a_local_rename_of_a_locked_domain_is_allowed,
    a_local_rename_of_a_locked_domain_is_allowed_body
);

/// A move that would rewrite a link in a domain being renamed does not wait
/// for that rename: the linker is not written, it is named in the receipt at
/// once, and the move finishes while the rename is still running. Waiting
/// would hold every other move back for as long as the rename may take
/// (moves run one at a time). No index row comes back under the old name.
async fn a_move_skips_a_linker_whose_domain_is_being_renamed_body(store: Arc<Mutex<dyn Store>>) {
    let m = machine(store).await;
    let engine = m.engine(true).await;
    // A write into ops is running when the rename starts, so the rename has
    // paused ops and waits for that write before its first step.
    let write_hold = engine.hold_next_write();
    let writer = {
        let engine = engine.clone();
        let ops = m.ops.clone();
        tokio::spawn(async move {
            engine
                .write_engram(&write_params(&ops, "Note", "written before the rename"))
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(20), write_hold.reached())
        .await
        .expect("the write into ops is counted and held");
    let renamer = {
        let engine = engine.clone();
        let ops = m.ops.clone();
        tokio::spawn(async move {
            engine
                .rename_domain_local(&ops, "runbooks", NameOrigin::Explicit, &Scope::Unrestricted)
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(20), async {
        while !engine.is_renaming(&m.ops) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the rename has paused ops");

    let mover = {
        let engine = engine.clone();
        let eng = m.eng.clone();
        tokio::spawn(async move {
            engine
                .move_engram(
                    &MoveParams {
                        identifier: "alpha".to_string(),
                        domain: eng,
                        destination: "gamma.md".to_string(),
                        destination_domain: None,
                        permalink: None,
                        update_links: None,
                    },
                    &Scope::Unrestricted,
                )
                .await
        })
    };
    let receipt = tokio::time::timeout(Duration::from_secs(10), mover)
        .await
        .expect("the move does not wait for the rename")
        .unwrap()
        .expect("the move itself lands");
    assert!(
        std::fs::read_to_string(m.root.join("ops/pager.md"))
            .unwrap()
            .contains(&format!("[[{}:alpha]]", m.eng)),
        "nothing was rewritten in the paused domain"
    );
    let failed = receipt["links_failed"].as_array().expect("{receipt}");
    assert_eq!(failed.len(), 1, "{receipt}");
    assert_eq!(failed[0]["domain"], m.ops, "{receipt}");
    assert_eq!(failed[0]["path"], "pager.md", "{receipt}");
    assert!(
        failed[0]["reason"]
            .as_str()
            .unwrap()
            .contains("is being renamed"),
        "{receipt}"
    );

    write_hold.release();
    writer.await.unwrap().unwrap();
    renamer.await.unwrap().unwrap();
    assert_eq!(
        domain_id(&m.store, &m.ops).await,
        None,
        "no index row came back under the old name"
    );
}
both_backends!(
    a_move_skips_a_linker_whose_domain_is_being_renamed,
    a_move_skips_a_linker_whose_domain_is_being_renamed_body
);

/// A move that relinks an engram in its own source domain never asks that
/// domain for a second ticket: a rename that paused the source waits for the
/// move, and a move waiting for that rename would stall both until the wait
/// ran out.
async fn a_move_relinking_in_its_own_domain_finishes_while_a_rename_waits_body(
    store: Arc<Mutex<dyn Store>>,
) {
    let m = machine(store).await;
    std::fs::write(
        m.root.join("eng/beta.md"),
        engram("Beta", "beta", "See [[alpha]] first."),
    )
    .unwrap();
    let engine = m.engine(true).await;
    engine.sync(None).await.unwrap();

    // The move is held right after its source counted it.
    let write_hold = engine.hold_next_write();
    let mover = {
        let engine = engine.clone();
        let eng = m.eng.clone();
        tokio::spawn(async move {
            engine
                .move_engram(
                    &MoveParams {
                        identifier: "alpha".to_string(),
                        domain: eng,
                        destination: "gamma.md".to_string(),
                        destination_domain: None,
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

    let renamer = {
        let engine = engine.clone();
        let (eng, platform) = (m.eng.clone(), m.platform.clone());
        tokio::spawn(async move {
            engine
                .rename_domain_local(&eng, &platform, NameOrigin::Explicit, &Scope::Unrestricted)
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(20), async {
        while !engine.is_renaming(&m.eng) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the rename has paused the source domain");

    write_hold.release();
    let receipt = tokio::time::timeout(Duration::from_secs(10), mover)
        .await
        .expect("the move finishes while the rename waits for it")
        .unwrap()
        .expect("the move lands");
    assert_eq!(receipt["links_rewritten"], 2, "{receipt}");
    assert!(receipt.get("links_failed").is_none(), "{receipt}");
    assert!(
        std::fs::read_to_string(m.root.join("eng/beta.md"))
            .unwrap()
            .contains("See [[gamma]] first."),
        "the same-domain linker is rewritten"
    );
    renamer.await.unwrap().unwrap();
}
both_backends!(
    a_move_relinking_in_its_own_domain_finishes_while_a_rename_waits,
    a_move_relinking_in_its_own_domain_finishes_while_a_rename_waits_body
);

/// A rename onto a name that differs from another domain's only in letter
/// case is refused before any step: the two would share one state folder on
/// a disk that ignores case. Nothing moves and no journal is left.
async fn a_rename_onto_another_domains_name_in_other_case_is_refused_body(
    store: Arc<Mutex<dyn Store>>,
) {
    let m = machine(store).await;
    let engine = m.engine(true).await;
    let before = std::fs::read_to_string(m.config_path()).unwrap();
    let err = engine
        .rename_domain_local("eng", "OPS", NameOrigin::Explicit, &Scope::Unrestricted)
        .await
        .expect_err("a case twin of ops is refused");
    assert!(
        matches!(err, EngineError::Conflict(_))
            && err
                .to_string()
                .contains("'OPS' differs from the domain 'ops' only in letter case"),
        "{err}"
    );
    assert_eq!(std::fs::read_to_string(m.config_path()).unwrap(), before);
    assert!(!m.journal().exists());
}
both_backends!(
    a_rename_onto_another_domains_name_in_other_case_is_refused,
    a_rename_onto_another_domains_name_in_other_case_is_refused_body
);

/// A domain renamed to its own name in another letter case keeps working,
/// and its state folders follow under the new spelling, through a
/// temporary name, so a disk that ignores case never loses them: the kept
/// texts and the draft journal are there, byte for byte, under `Eng`.
async fn a_case_only_rename_moves_the_state_folders_body(store: Arc<Mutex<dyn Store>>) {
    let m = machine(store).await;
    let engine = m.engine(true).await;
    let kept_root = m.state().join("kept");
    crystalline_core::kept::write(
        &kept_root,
        "eng",
        None,
        crystalline_core::kept::KeptKind::Engram,
        "alpha.md",
        chrono::Utc::now(),
        "typed",
    )
    .unwrap();
    let journal = m.state().join("overlays/eng/someone");
    std::fs::create_dir_all(&journal).unwrap();
    std::fs::write(journal.join("note.md"), "draft").unwrap();

    let report = engine
        .rename_domain_local("eng", "Eng", NameOrigin::Explicit, &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(report["domain"], "Eng", "{report}");
    let listed = |dir: PathBuf| -> Vec<String> {
        std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect()
    };
    assert_eq!(listed(kept_root.clone()), vec!["Eng".to_string()]);
    assert_eq!(listed(m.state().join("overlays")), vec!["Eng".to_string()]);
    let found = crystalline_core::kept::scan(&kept_root);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].domain, "Eng");
    assert_eq!(crystalline_core::kept::read(&found[0]).unwrap(), "typed");
    assert_eq!(
        std::fs::read_to_string(m.state().join("overlays/Eng/someone/note.md")).unwrap(),
        "draft"
    );
}
both_backends!(
    a_case_only_rename_moves_the_state_folders,
    a_case_only_rename_moves_the_state_folders_body
);

/// A rename a crash stopped after its texts moved, finished by a restart or
/// by sending the same rename again, keeps the renamed domain's kept texts
/// under the new name: neither the replay of the step nor the registration
/// check sets them aside, recorded step or not.
async fn a_finished_rename_keeps_its_kept_texts_under_the_new_name_body(
    store: Arc<Mutex<dyn Store>>,
) {
    for (step, unrecorded, resend) in [
        (RenameStep::KeptDir, false, false),
        (RenameStep::KeptDir, true, false),
        (RenameStep::KeptDir, true, true),
        (RenameStep::ProvisionReceipt, true, true),
    ] {
        let tag = format!("-k{step:?}{unrecorded}{resend}").to_lowercase();
        let m = machine_tagged(store.clone(), &tag).await;
        let kept_root = m.state().join("kept");
        crystalline_core::kept::write(
            &kept_root,
            &m.eng,
            None,
            crystalline_core::kept::KeptKind::Engram,
            "alpha.md",
            chrono::Utc::now(),
            "typed",
        )
        .unwrap();
        let engine = m.engine(true).await;
        engine.fail_rename_after(Some(step));
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
        if unrecorded {
            let mut journal: serde_json::Value =
                serde_json::from_slice(&std::fs::read(m.journal()).unwrap()).unwrap();
            journal["done"].as_array_mut().unwrap().pop();
            std::fs::write(m.journal(), serde_json::to_vec(&journal).unwrap()).unwrap();
        }
        let restarted = m.engine(false).await;
        if resend {
            restarted
                .rename_domain_local(
                    &m.eng,
                    &m.platform,
                    NameOrigin::Explicit,
                    &Scope::Unrestricted,
                )
                .await
                .unwrap();
        } else {
            restarted.recover_rename_journal().await.unwrap().unwrap();
        }
        let found = crystalline_core::kept::scan(&kept_root);
        assert_eq!(found.len(), 1, "{step:?} {unrecorded} {resend}: {found:?}");
        assert_eq!(
            found[0].domain, m.platform,
            "{step:?} {unrecorded} {resend}"
        );
        assert!(
            !found[0].removed,
            "{step:?} {unrecorded} {resend}: set aside"
        );
    }
}
both_backends!(
    a_finished_rename_keeps_its_kept_texts_under_the_new_name,
    a_finished_rename_keeps_its_kept_texts_under_the_new_name_body
);

// --- a MANIFEST rename: finish it or keep the links ------------------------------

/// A reflection page of `ops` that links to `eng` under its old name.
const OPS_PAGE: &str = "# B\n\n## Links\n<!-- r:bbbb2222 -->\n\nSee [[eng:alpha]].\n";

/// [`machine`], with `eng` an explicit domain whose MANIFEST was edited from
/// `eng` to `platform` and kept the alias `engineering`, the open question
/// the adoption recorded for it, `ops`'s pager linking to `eng` under the
/// old name, the alias and the new name, and a reflection page in `ops`
/// that links to it under the old name.
async fn question_machine(store: Arc<Mutex<dyn Store>>) -> Machine {
    let m = machine(store).await;
    std::fs::write(
        m.root.join("eng/MANIFEST.md"),
        manifest_named("Eng", "platform"),
    )
    .unwrap();
    std::fs::write(
        m.root.join("ops/MANIFEST.md"),
        manifest("Ops").replacen("status: current\n", "status: current\nreflection: on\n", 1),
    )
    .unwrap();
    std::fs::create_dir_all(m.root.join("ops/reflections")).unwrap();
    std::fs::write(m.root.join("ops/reflections/b.md"), OPS_PAGE).unwrap();
    std::fs::write(
        m.root.join("ops/pager.md"),
        engram(
            "Pager",
            "pager",
            "See [[eng:alpha]], then crystalline://engineering/alpha, then [[platform:alpha]].",
        ),
    )
    .unwrap();
    let mut cfg = crystalline_service::overlay::load_file(&m.config_path()).unwrap();
    let entry = cfg.domains.get_mut("eng").unwrap();
    entry.name_origin = Some(NameOrigin::Explicit);
    entry.canonical_seen = Some("platform".to_string());
    entry.aliases = vec!["engineering".to_string()];
    entry.rename_question = Some(RenameQuestion {
        from: "eng".to_string(),
        to: "platform".to_string(),
        seen_at: "2026-10-21T08:00:00Z".to_string(),
        answered: None,
    });
    crystalline_core::config::save_yaml(&m.config_path(), &cfg).unwrap();
    m.engine(true).await.sync(None).await.unwrap();
    m
}

fn saved_question(m: &Machine) -> Option<RenameQuestion> {
    crystalline_service::overlay::load_file(&m.config_path())
        .unwrap()
        .domains
        .get("eng")
        .and_then(|entry| entry.rename_question.clone())
}

/// What a finished respell leaves behind in `ops`: every link to `eng` under
/// an old name is spelled `platform`, and every one still reaches `eng`.
async fn assert_respelled(m: &Machine, engine: &Engine) {
    let pager = std::fs::read_to_string(m.root.join("ops/pager.md")).unwrap();
    assert!(
        pager.contains(
            "See [[platform:alpha]], then crystalline://platform/alpha, then [[platform:alpha]]."
        ),
        "{pager}"
    );
    let page = std::fs::read_to_string(m.root.join("ops/reflections/b.md")).unwrap();
    assert!(page.contains("See [[platform:alpha]]."), "{page}");
    assert!(!page.contains("[[eng:"), "{page}");
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

/// Finishing respells the links in engrams and in Reflection pages to the
/// new name, and nothing else: the MANIFEST, the configuration key and the
/// index row stay as they are, and the question is answered.
async fn finishing_respells_engram_and_reflection_links_body(store: Arc<Mutex<dyn Store>>) {
    let m = question_machine(store).await;
    let engine = m.engine(true).await;
    let manifest_before = std::fs::read_to_string(m.root.join("eng/MANIFEST.md")).unwrap();
    let eng_id = domain_id(&m.store, "eng").await.expect("eng indexed");

    let report = engine
        .finish_manifest_rename("eng", &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(report["domain"], "eng", "{report}");
    assert_eq!(report["from"], "eng", "{report}");
    assert_eq!(report["to"], "platform", "{report}");
    assert_eq!(
        report["rewritten"],
        serde_json::json!([{ "domain": "ops", "engrams": 2, "references": 3 }]),
        "the pager and the page: {report}"
    );
    assert_eq!(report["left_behind"], serde_json::json!([]), "{report}");

    assert_respelled(&m, &engine).await;
    assert_eq!(
        std::fs::read_to_string(m.root.join("eng/MANIFEST.md")).unwrap(),
        manifest_before,
        "the MANIFEST is not written"
    );
    let cfg = crystalline_service::overlay::load_file(&m.config_path()).unwrap();
    let entry = cfg.domains.get("eng").expect("the key stays");
    assert_eq!(entry.aliases, vec!["engineering".to_string()]);
    assert_eq!(domain_id(&m.store, "eng").await, Some(eng_id));
    assert_eq!(
        entry.rename_question.as_ref().and_then(|q| q.answered),
        Some(RenameAnswer::Finished)
    );
    assert_eq!(engine.rename_question("eng"), None);
    assert!(!m.journal().exists(), "the journal is gone");
    assert!(!engine.is_renaming("eng"));

    // Nothing is left to finish.
    let again = engine
        .finish_manifest_rename("eng", &Scope::Unrestricted)
        .await
        .expect_err("no open question");
    assert!(matches!(again, EngineError::Conflict(_)), "{again}");
}
both_backends!(
    finishing_respells_engram_and_reflection_links,
    finishing_respells_engram_and_reflection_links_body
);

/// A respell a crash stopped before its relink step was recorded is
/// finished at the next start: the links are respelled, the question is
/// answered and the journal goes.
async fn a_crash_mid_respell_finishes_on_restart_body(store: Arc<Mutex<dyn Store>>) {
    let m = question_machine(store).await;
    let engine = m.engine(true).await;
    let pager_before = std::fs::read_to_string(m.root.join("ops/pager.md")).unwrap();

    engine.fail_rename_after(Some(RenameStep::Relink));
    engine
        .finish_manifest_rename("eng", &Scope::Unrestricted)
        .await
        .expect_err("the failpoint stops the respell");
    assert!(m.journal().is_file(), "the journal stays");
    drop(engine);
    // As if the crash came before the journal recorded the step, with one
    // link not yet respelled.
    let mut journal: serde_json::Value =
        serde_json::from_slice(&std::fs::read(m.journal()).unwrap()).unwrap();
    assert_eq!(journal["respell_only"], true, "{journal}");
    journal["done"] = serde_json::json!([]);
    std::fs::write(m.journal(), serde_json::to_vec(&journal).unwrap()).unwrap();
    std::fs::write(m.root.join("ops/pager.md"), &pager_before).unwrap();
    std::fs::write(m.root.join("ops/reflections/b.md"), OPS_PAGE).unwrap();

    let restarted = m.engine(false).await;
    restarted.sync(None).await.unwrap_or_default();
    let recovered = restarted
        .recover_rename_journal()
        .await
        .unwrap()
        .expect("a journal was left to finish");
    assert_eq!(recovered["domain"], "eng", "{recovered}");
    assert_eq!(recovered["to"], "platform", "{recovered}");
    assert!(!m.journal().exists(), "the journal is gone");
    restarted.sync(None).await.unwrap();
    assert_respelled(&m, &restarted).await;
    assert_eq!(
        saved_question(&m).and_then(|q| q.answered),
        Some(RenameAnswer::Finished)
    );
    assert!(
        crystalline_service::overlay::load_file(&m.config_path())
            .unwrap()
            .domains
            .contains_key("eng"),
        "the key stays"
    );
}
both_backends!(
    a_crash_mid_respell_finishes_on_restart,
    a_crash_mid_respell_finishes_on_restart_body
);

/// A respell journal left behind is never finished by a full rename that
/// happens to name the same two names: that rename is refused, naming how
/// the respell is finished.
async fn a_rename_never_resumes_a_respell_journal_body(store: Arc<Mutex<dyn Store>>) {
    let m = question_machine(store).await;
    let engine = m.engine(true).await;
    engine.fail_rename_after(Some(RenameStep::Relink));
    engine
        .finish_manifest_rename("eng", &Scope::Unrestricted)
        .await
        .expect_err("the failpoint stops the respell");

    let err = engine
        .rename_domain_local(
            "eng",
            "platform",
            NameOrigin::Explicit,
            &Scope::Unrestricted,
        )
        .await
        .expect_err("a respell is pending");
    let text = conflict(err);
    assert!(text.contains("crystalline doctor --fix"), "{text}");
    let cfg = crystalline_service::overlay::load_file(&m.config_path()).unwrap();
    assert!(cfg.domains.contains_key("eng"), "nothing was renamed");
}
both_backends!(
    a_rename_never_resumes_a_respell_journal,
    a_rename_never_resumes_a_respell_journal_body
);

/// Keeping the links closes the question and changes nothing else: the
/// links keep their spelling and reach the domain through the alias.
async fn keep_closes_the_question_and_keeps_the_alias_body(store: Arc<Mutex<dyn Store>>) {
    let m = question_machine(store).await;
    let engine = m.engine(true).await;
    let pager_before = std::fs::read_to_string(m.root.join("ops/pager.md")).unwrap();

    let answer = engine
        .keep_manifest_rename("eng", &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(
        answer,
        serde_json::json!({ "domain": "eng", "from": "eng", "to": "platform", "answered": "keep" })
    );
    assert_eq!(
        saved_question(&m).and_then(|q| q.answered),
        Some(RenameAnswer::Keep)
    );
    assert_eq!(engine.rename_question("eng"), None);
    assert_eq!(
        std::fs::read_to_string(m.root.join("ops/pager.md")).unwrap(),
        pager_before
    );
    for spelling in ["eng", "engineering", "platform"] {
        assert_eq!(
            engine.local_domain_name(spelling).await.as_deref(),
            Some("eng"),
            "{spelling}"
        );
    }
    let err = engine
        .keep_manifest_rename("eng", &Scope::Unrestricted)
        .await
        .expect_err("no open question");
    assert!(matches!(err, EngineError::Conflict(_)), "{err}");
}
both_backends!(
    keep_closes_the_question_and_keeps_the_alias,
    keep_closes_the_question_and_keeps_the_alias_body
);

/// A rename by hand settles the question too, under the domain's new name.
async fn a_manual_rename_closes_the_question_body(store: Arc<Mutex<dyn Store>>) {
    let m = question_machine(store).await;
    let engine = m.engine(true).await;

    engine
        .rename_domain_local("eng", "core", NameOrigin::Explicit, &Scope::Unrestricted)
        .await
        .unwrap();

    let cfg = crystalline_service::overlay::load_file(&m.config_path()).unwrap();
    assert_eq!(
        cfg.domains["core"]
            .rename_question
            .as_ref()
            .and_then(|q| q.answered),
        Some(RenameAnswer::Finished)
    );
    assert_eq!(engine.rename_question("core"), None);
}
both_backends!(
    a_manual_rename_closes_the_question,
    a_manual_rename_closes_the_question_body
);

/// Only a caller who may rename the domain finishes or keeps it.
async fn a_caller_who_may_not_rename_is_refused_body(store: Arc<Mutex<dyn Store>>) {
    let m = question_machine(store).await;
    let auth = AuthStore::open(&m.state().join("web-auth.db"))
        .await
        .unwrap();
    auth.add_user("bob", "Bob", None, Role::Editor, "pw12345678")
        .await
        .unwrap();
    auth.upsert_domain_member("eng", "bob", MemberLevel::Editor, None, "ada")
        .await
        .unwrap();
    drop(auth);
    let engine = m.engine(true).await;
    let bob = Scope::User {
        account: "bob".to_string(),
        admin: false,
    };
    for err in [
        engine
            .finish_manifest_rename("eng", &bob)
            .await
            .unwrap_err(),
        engine.keep_manifest_rename("eng", &bob).await.unwrap_err(),
    ] {
        assert!(matches!(err, EngineError::Forbidden(_)), "{err}");
    }
    assert!(engine.rename_question("eng").is_some());

    let ada = Scope::User {
        account: "ada".to_string(),
        admin: false,
    };
    engine.keep_manifest_rename("eng", &ada).await.unwrap();
}
both_backends!(
    a_caller_who_may_not_rename_is_refused,
    a_caller_who_may_not_rename_is_refused_body
);

/// The stored domain of a Reflection source follows the respell too, to the
/// new name, and the source still reads current: it is checked through the
/// name table, where the new name reaches the domain, and a read names it
/// by its local name here.
async fn finishing_respells_the_domain_of_reflection_sources_body(store: Arc<Mutex<dyn Store>>) {
    let m = machine(store).await;
    // `eng` is private to ada, so a page that cites it must be read by no
    // one else either: `ops` is made private to ada too.
    AuthStore::open(&m.state().join("web-auth.db"))
        .await
        .unwrap()
        .set_domain_visibility("ops", true, "ada")
        .await
        .unwrap();
    std::fs::write(
        m.root.join("ops/MANIFEST.md"),
        manifest("Ops").replacen("status: current\n", "status: current\nreflection: on\n", 1),
    )
    .unwrap();
    let engine = m.engine(true).await;
    engine.sync(None).await.unwrap();
    let mut page = crystalline_service::params::WriteReflectionParams {
        domain: "ops".into(),
        path: "a.md".into(),
        markdown: Some("# A\n\nFor people.\n\n## Alpha\n\nRead it first.\n".into()),
        ..Default::default()
    };
    page.sources = std::collections::BTreeMap::from([(
        "Alpha".to_string(),
        vec!["crystalline://eng/alpha".to_string()],
    )]);
    let agent = crystalline_service::ReflectionCaller::agent(Scope::Unrestricted, "agent", false);
    engine.write_reflection(&page, &agent).await.unwrap();
    let text = std::fs::read_to_string(m.root.join("ops/reflections/a.md")).unwrap();
    assert!(text.contains("domain: eng"), "{text}");
    drop(engine);

    std::fs::write(
        m.root.join("eng/MANIFEST.md"),
        manifest_named("Eng", "platform"),
    )
    .unwrap();
    let mut cfg = crystalline_service::overlay::load_file(&m.config_path()).unwrap();
    let entry = cfg.domains.get_mut("eng").unwrap();
    entry.name_origin = Some(NameOrigin::Explicit);
    entry.canonical_seen = Some("platform".to_string());
    entry.rename_question = Some(RenameQuestion {
        from: "eng".to_string(),
        to: "platform".to_string(),
        seen_at: "2026-10-21T08:00:00Z".to_string(),
        answered: None,
    });
    crystalline_core::config::save_yaml(&m.config_path(), &cfg).unwrap();
    let engine = m.engine(true).await;
    engine.sync(None).await.unwrap();

    engine
        .finish_manifest_rename("eng", &Scope::Unrestricted)
        .await
        .unwrap();

    let text = std::fs::read_to_string(m.root.join("ops/reflections/a.md")).unwrap();
    assert!(text.contains("domain: platform"), "{text}");
    assert!(!text.contains("domain: eng"), "{text}");
    let doc = engine.read_reflection("ops", "a.md", &agent).await.unwrap();
    let source = &doc["sections"][0]["sources"][0];
    assert_eq!(source["domain"], "eng", "{doc}");
    assert_eq!(source["state"], "current", "{doc}");
}
both_backends!(
    finishing_respells_the_domain_of_reflection_sources,
    finishing_respells_the_domain_of_reflection_sources_body
);

/// The pager and the page as [`question_machine`] writes them, put back and
/// reindexed after a respell that stopped.
async fn restore_old_links(m: &Machine, engine: &Engine, pager: &str) {
    std::fs::write(m.root.join("ops/pager.md"), pager).unwrap();
    std::fs::write(m.root.join("ops/reflections/b.md"), OPS_PAGE).unwrap();
    engine.sync(None).await.unwrap();
}

/// A respell that stopped is never run after the question was kept: the keep
/// drops its journal, and a journal found at the next start for a question
/// nobody asks any more is dropped too, with the links left as they are.
async fn a_kept_question_drops_a_stopped_respell_body(store: Arc<Mutex<dyn Store>>) {
    let m = question_machine(store).await;
    let engine = m.engine(true).await;
    let pager_before = std::fs::read_to_string(m.root.join("ops/pager.md")).unwrap();
    engine.fail_rename_after(Some(RenameStep::Relink));
    engine
        .finish_manifest_rename("eng", &Scope::Unrestricted)
        .await
        .expect_err("the failpoint stops the respell");
    let journal = std::fs::read(m.journal()).unwrap();
    restore_old_links(&m, &engine, &pager_before).await;

    engine
        .keep_manifest_rename("eng", &Scope::Unrestricted)
        .await
        .unwrap();
    assert!(!m.journal().exists(), "the keep drops the journal");
    drop(engine);

    // As if the keep had come from a process that could not drop it.
    std::fs::write(m.journal(), &journal).unwrap();
    let restarted = m.engine(false).await;
    assert!(restarted.recover_rename_journal().await.unwrap().is_none());
    assert!(!m.journal().exists(), "the start drops it");
    assert_eq!(
        std::fs::read_to_string(m.root.join("ops/pager.md")).unwrap(),
        pager_before,
        "the links keep their spelling"
    );
    assert_eq!(
        std::fs::read_to_string(m.root.join("ops/reflections/b.md")).unwrap(),
        OPS_PAGE
    );
    assert_eq!(
        saved_question(&m).and_then(|q| q.answered),
        Some(RenameAnswer::Keep)
    );
}
both_backends!(
    a_kept_question_drops_a_stopped_respell,
    a_kept_question_drops_a_stopped_respell_body
);

/// A respell that stopped, then a newer MANIFEST edit: finishing respells to
/// the name the question asks about now, not to the one the stopped respell
/// was for.
async fn a_newer_name_after_a_stopped_respell_is_the_one_finished_body(
    store: Arc<Mutex<dyn Store>>,
) {
    let m = question_machine(store).await;
    let engine = m.engine(true).await;
    let pager_before = std::fs::read_to_string(m.root.join("ops/pager.md")).unwrap();
    engine.fail_rename_after(Some(RenameStep::Relink));
    engine
        .finish_manifest_rename("eng", &Scope::Unrestricted)
        .await
        .expect_err("the failpoint stops the respell");
    restore_old_links(&m, &engine, &pager_before).await;
    drop(engine);

    // The MANIFEST now declares `core`, and the question moved with it, as
    // the adoption leaves it: `platform` is an alias.
    std::fs::write(
        m.root.join("eng/MANIFEST.md"),
        manifest_named("Eng", "core"),
    )
    .unwrap();
    let mut cfg = crystalline_service::overlay::load_file(&m.config_path()).unwrap();
    let entry = cfg.domains.get_mut("eng").unwrap();
    entry.canonical_seen = Some("core".to_string());
    entry.aliases = vec!["engineering".to_string(), "platform".to_string()];
    entry.rename_question.as_mut().unwrap().to = "core".to_string();
    crystalline_core::config::save_yaml(&m.config_path(), &cfg).unwrap();
    let engine = m.engine(true).await;
    engine.sync(None).await.unwrap();

    let report = engine
        .finish_manifest_rename("eng", &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(report["to"], "core", "{report}");
    let pager = std::fs::read_to_string(m.root.join("ops/pager.md")).unwrap();
    assert!(
        pager.contains("See [[core:alpha]], then crystalline://core/alpha, then [[core:alpha]]."),
        "{pager}"
    );
    let page = std::fs::read_to_string(m.root.join("ops/reflections/b.md")).unwrap();
    assert!(page.contains("See [[core:alpha]]."), "{page}");
    assert_eq!(
        saved_question(&m).and_then(|q| q.answered),
        Some(RenameAnswer::Finished)
    );
    assert!(!m.journal().exists());
}
both_backends!(
    a_newer_name_after_a_stopped_respell_is_the_one_finished,
    a_newer_name_after_a_stopped_respell_is_the_one_finished_body
);

/// A question closes on its own once no link here uses an old name any
/// more after a teammate's finished rename arrives with a pull: the pull
/// answers it, and the listing shows no rename.
async fn a_question_whose_links_already_match_closes_on_its_own_body(store: Arc<Mutex<dyn Store>>) {
    let m = question_machine(store).await;
    let engine = m.engine(true).await;
    engine.adopt_domain_names().await.unwrap();
    assert!(
        engine.rename_question("eng").is_some(),
        "old spellings are left: the question stays"
    );
    // Before the pull: old spellings are still here.
    let before = engine.rename_questions_already_clean().await;
    assert!(!before.contains("eng"), "{before:?}");

    // The pull brings the teammate's respelled files.
    std::fs::write(
        m.root.join("ops/pager.md"),
        engram(
            "Pager",
            "pager",
            "See [[platform:alpha]], then crystalline://platform/alpha, then [[platform:alpha]].",
        ),
    )
    .unwrap();
    std::fs::write(
        m.root.join("ops/reflections/b.md"),
        OPS_PAGE.replace("[[eng:", "[[platform:"),
    )
    .unwrap();
    engine.sync(None).await.unwrap();
    engine.adopt_domain_names().await.unwrap();
    engine.settle_rename_questions_after_pull(&before).await;

    assert_eq!(engine.rename_question("eng"), None);
    assert_eq!(
        saved_question(&m).and_then(|q| q.answered),
        Some(RenameAnswer::Finished)
    );
    let listed = engine
        .list_domains(
            &ListDomainsParams {
                include_routing: false,
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    let row = listed["domains"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["name"] == "eng")
        .cloned()
        .unwrap();
    assert_eq!(row["rename_pending"], serde_json::Value::Null, "{row}");
}
both_backends!(
    a_question_whose_links_already_match_closes_on_its_own,
    a_question_whose_links_already_match_closes_on_its_own_body
);

/// A stopped respell, then the MANIFEST goes back to the old name: the
/// adoption drops the question, and the journal with it.
async fn changing_the_name_back_drops_a_stopped_respell_body(store: Arc<Mutex<dyn Store>>) {
    let m = question_machine(store).await;
    let engine = m.engine(true).await;
    engine.fail_rename_after(Some(RenameStep::Relink));
    engine
        .finish_manifest_rename("eng", &Scope::Unrestricted)
        .await
        .expect_err("the failpoint stops the respell");
    assert!(m.journal().is_file());

    std::fs::write(m.root.join("eng/MANIFEST.md"), manifest_named("Eng", "eng")).unwrap();
    engine.sync(None).await.unwrap();
    engine.adopt_domain_names().await.unwrap();

    assert_eq!(saved_question(&m), None, "the question is dropped");
    assert!(!m.journal().exists(), "and the journal with it");
}
both_backends!(
    changing_the_name_back_drops_a_stopped_respell,
    changing_the_name_back_drops_a_stopped_respell_body
);

/// A stopped respell recorded against another configuration than the one
/// this engine opened is neither run nor dropped here, also once nobody
/// asks for it: it belongs to that configuration.
async fn another_configs_respell_journal_is_left_alone_body(store: Arc<Mutex<dyn Store>>) {
    let m = question_machine(store).await;
    let engine = m.engine(true).await;
    engine.fail_rename_after(Some(RenameStep::Relink));
    engine
        .finish_manifest_rename("eng", &Scope::Unrestricted)
        .await
        .expect_err("the failpoint stops the respell");
    drop(engine);
    let mut journal: serde_json::Value =
        serde_json::from_slice(&std::fs::read(m.journal()).unwrap()).unwrap();
    journal["owner"]["config"] = serde_json::json!("/elsewhere/config.yaml");
    std::fs::write(m.journal(), serde_json::to_vec(&journal).unwrap()).unwrap();

    // Nobody asks for it any more: the MANIFEST went back to the old name.
    std::fs::write(m.root.join("eng/MANIFEST.md"), manifest_named("Eng", "eng")).unwrap();
    let other = m.engine(true).await;
    assert!(other.recover_rename_journal().await.unwrap().is_none());
    other.sync(None).await.unwrap();
    other.adopt_domain_names().await.unwrap();
    other.finish_rename_questions().await.unwrap();
    assert!(
        m.journal().is_file(),
        "the journal stays for its own config"
    );
}
both_backends!(
    another_configs_respell_journal_is_left_alone,
    another_configs_respell_journal_is_left_alone_body
);

/// An engram the index names but that cannot be read is no proof its links
/// match: the question stays open.
async fn an_unreadable_engram_keeps_the_question_open_body(store: Arc<Mutex<dyn Store>>) {
    let m = question_machine(store).await;
    let engine = m.engine(true).await;
    let before = engine.rename_questions_already_clean().await;
    std::fs::write(
        m.root.join("ops/reflections/b.md"),
        OPS_PAGE.replace("[[eng:", "[[platform:"),
    )
    .unwrap();
    engine.sync(None).await.unwrap();
    // Still in the index with its old links, and no longer UTF-8 on disk,
    // which a read refuses rather than answer from the row.
    std::fs::write(m.root.join("ops/pager.md"), [0xff, 0xfe, 0x00, 0x80]).unwrap();

    engine.adopt_domain_names().await.unwrap();
    engine.settle_rename_questions_after_pull(&before).await;
    assert!(engine.rename_question("eng").is_some());
}
both_backends!(
    an_unreadable_engram_keeps_the_question_open,
    an_unreadable_engram_keeps_the_question_open_body
);

/// The `V110` findings in `ops` that speak about a link spelled `eng`.
async fn v110_on_eng(engine: &Engine) -> Vec<serde_json::Value> {
    let v = engine
        .evolve_detect(
            &EvolveParams {
                domains: vec!["ops".to_string()],
                ..EvolveParams::default()
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    v["queue"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["rule"] == "V110" && r["evidence"].as_str().unwrap().contains("`eng`"))
        .cloned()
        .collect()
}

/// While the rename waits for a person, a link under the old name is that
/// person's to respell: `V110` is judgment and its fix names the two ways
/// they finish it, never an edit for an agent to apply.
async fn an_open_rename_makes_v110_for_the_old_name_judgment_body(store: Arc<Mutex<dyn Store>>) {
    let m = question_machine(store).await;
    let engine = m.engine(true).await;
    let found = v110_on_eng(&engine).await;
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0]["class"], "judgment", "{:?}", found[0]);
    let fix = found[0]["fix"].as_str().unwrap();
    assert!(fix.contains("crystalline doctor --fix"), "{fix}");
    assert!(!fix.contains("find_replace"), "{fix}");
}
both_backends!(
    an_open_rename_makes_v110_for_the_old_name_judgment,
    an_open_rename_makes_v110_for_the_old_name_judgment_body
);

/// A person who kept the links as they are kept the old name as a label:
/// `V110` no longer speaks about it at all.
async fn a_kept_rename_raises_no_v110_for_the_old_name_body(store: Arc<Mutex<dyn Store>>) {
    let m = question_machine(store).await;
    let engine = m.engine(true).await;
    engine
        .keep_manifest_rename("eng", &Scope::Unrestricted)
        .await
        .unwrap();
    let found = v110_on_eng(&engine).await;
    assert!(found.is_empty(), "{found:?}");
}
both_backends!(
    a_kept_rename_raises_no_v110_for_the_old_name,
    a_kept_rename_raises_no_v110_for_the_old_name_body
);

/// Only a person answers a rename question. Links respelled here by hand or
/// by an agent's edit, then synced and lined up again, leave it open: the
/// banner and the doctor entry stay until the person chooses.
async fn links_respelled_here_leave_the_question_open_body(store: Arc<Mutex<dyn Store>>) {
    let m = question_machine(store).await;
    let engine = m.engine(true).await;
    let pager = std::fs::read_to_string(m.root.join("ops/pager.md")).unwrap();
    engine
        .edit_engram(&EditParams {
            identifier: "pager".to_string(),
            domain: "ops".to_string(),
            operation: "find_replace".to_string(),
            content: Some("[[platform:alpha]], then crystalline://platform/alpha".to_string()),
            find_text: Some("[[eng:alpha]], then crystalline://engineering/alpha".to_string()),
            ..EditParams::default()
        })
        .await
        .unwrap_or_else(|e| panic!("{e}: {pager}"));
    std::fs::write(
        m.root.join("ops/reflections/b.md"),
        OPS_PAGE.replace("[[eng:", "[[platform:"),
    )
    .unwrap();
    engine.sync(None).await.unwrap();
    engine.adopt_domain_names().await.unwrap();

    assert!(engine.rename_question("eng").is_some());
    assert_eq!(saved_question(&m).and_then(|q| q.answered), None);

    // A later pull that brought none of it does not close it either: the
    // links already matched before the pull.
    let before = engine.rename_questions_already_clean().await;
    assert!(before.contains("eng"), "{before:?}");
    engine.settle_rename_questions_after_pull(&before).await;
    assert!(engine.rename_question("eng").is_some());
    assert_eq!(saved_question(&m).and_then(|q| q.answered), None);
}

/// An unattended evolve run asks for mechanical findings only; while the
/// rename waits for a person, none of them is a V110 that would respell the
/// old name.
async fn a_mechanical_sweep_holds_no_v110_for_an_open_rename_body(store: Arc<Mutex<dyn Store>>) {
    let m = question_machine(store).await;
    let engine = m.engine(true).await;
    let v = engine
        .evolve_detect(
            &EvolveParams {
                domains: vec!["ops".to_string()],
                class: Some("mechanical".to_string()),
                ..EvolveParams::default()
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    let v110: Vec<&serde_json::Value> = v["queue"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["rule"] == "V110" && r["evidence"].as_str().unwrap().contains("`eng`"))
        .collect();
    assert!(v110.is_empty(), "{v110:?}");
}
both_backends!(
    a_mechanical_sweep_holds_no_v110_for_an_open_rename,
    a_mechanical_sweep_holds_no_v110_for_an_open_rename_body
);
both_backends!(
    links_respelled_here_leave_the_question_open,
    links_respelled_here_leave_the_question_open_body
);

/// The usual teammate case: one pull brings the MANIFEST rename and the
/// respelled files together. The question the adoption after it records
/// was never seen clean before the pull, so that pull settles it.
async fn a_pull_with_the_rename_and_its_respelled_files_closes_its_question_body(
    store: Arc<Mutex<dyn Store>>,
) {
    let m = machine(store).await;
    let mut cfg = crystalline_service::overlay::load_file(&m.config_path()).unwrap();
    cfg.domains.get_mut("eng").unwrap().name_origin = Some(NameOrigin::Explicit);
    crystalline_core::config::save_yaml(&m.config_path(), &cfg).unwrap();
    let engine = m.engine(true).await;
    engine.sync(None).await.unwrap();
    engine.adopt_domain_names().await.unwrap();
    assert_eq!(engine.rename_question("eng"), None);

    let before = engine.rename_questions_already_clean().await;
    // The pull: the teammate's MANIFEST rename and their respelled link.
    std::fs::write(
        m.root.join("eng/MANIFEST.md"),
        manifest_named("Eng", "platform"),
    )
    .unwrap();
    std::fs::write(
        m.root.join("ops/pager.md"),
        engram("Pager", "pager", "See [[platform:alpha]] before paging."),
    )
    .unwrap();
    engine.sync(None).await.unwrap();
    engine.adopt_domain_names().await.unwrap();
    assert!(
        saved_question(&m).is_some(),
        "the adoption records the question"
    );
    engine.settle_rename_questions_after_pull(&before).await;

    assert_eq!(engine.rename_question("eng"), None);
    assert_eq!(
        saved_question(&m).and_then(|q| q.answered),
        Some(RenameAnswer::Finished)
    );
}
both_backends!(
    a_pull_with_the_rename_and_its_respelled_files_closes_its_question,
    a_pull_with_the_rename_and_its_respelled_files_closes_its_question_body
);

/// The text of `ops`'s engram `permalink` as stored.
async fn ops_content(engine: &Engine, permalink: &str) -> String {
    engine
        .read_engram(&read_params(permalink, "ops"), &Scope::Unrestricted)
        .await
        .unwrap()["content"]
        .as_str()
        .unwrap()
        .to_string()
}

/// While a rename question is open, and after "keep", a write keeps a link
/// under the old name as written: respelling it is the person's answer, not
/// the write's.
async fn a_write_keeps_the_old_name_while_asked_and_after_keep_body(store: Arc<Mutex<dyn Store>>) {
    let m = question_machine(store).await;
    let engine = m.engine(true).await;
    engine
        .write_engram(&write_params("ops", "Asked", "See [[eng:alpha]]."))
        .await
        .unwrap();
    let asked = ops_content(&engine, "asked").await;
    assert!(asked.contains("See [[eng:alpha]]."), "{asked}");

    engine
        .keep_manifest_rename("eng", &Scope::Unrestricted)
        .await
        .unwrap();
    engine
        .write_engram(&write_params("ops", "Kept", "See [[eng:alpha]]."))
        .await
        .unwrap();
    let kept = ops_content(&engine, "kept").await;
    assert!(kept.contains("See [[eng:alpha]]."), "{kept}");
}
both_backends!(
    a_write_keeps_the_old_name_while_asked_and_after_keep,
    a_write_keeps_the_old_name_while_asked_and_after_keep_body
);

/// Once the rename is finished, a write spells the domain by its new name
/// again, as it does with no question at all.
async fn a_write_respells_the_old_name_after_the_rename_is_finished_body(
    store: Arc<Mutex<dyn Store>>,
) {
    let m = question_machine(store).await;
    let engine = m.engine(true).await;
    engine
        .finish_manifest_rename("eng", &Scope::Unrestricted)
        .await
        .unwrap();
    engine
        .write_engram(&write_params("ops", "Later", "See [[eng:alpha]]."))
        .await
        .unwrap();
    let later = ops_content(&engine, "later").await;
    assert!(later.contains("See [[platform:alpha]]."), "{later}");
}
both_backends!(
    a_write_respells_the_old_name_after_the_rename_is_finished,
    a_write_respells_the_old_name_after_the_rename_is_finished_body
);

/// A scan that fails before the pull counts the question as one that may
/// have been respelled here: it stays open, whatever the pull brings.
async fn a_failed_scan_before_a_pull_keeps_the_question_open_body(store: Arc<Mutex<dyn Store>>) {
    let m = question_machine(store).await;
    let engine = m.engine(true).await;
    engine.fail_next_rename_scan();
    let before = engine.rename_questions_already_clean().await;
    assert!(before.contains("eng"), "fail closed: {before:?}");

    std::fs::write(
        m.root.join("ops/pager.md"),
        engram(
            "Pager",
            "pager",
            "See [[platform:alpha]], then crystalline://platform/alpha, then [[platform:alpha]].",
        ),
    )
    .unwrap();
    std::fs::write(
        m.root.join("ops/reflections/b.md"),
        OPS_PAGE.replace("[[eng:", "[[platform:"),
    )
    .unwrap();
    engine.sync(None).await.unwrap();
    engine.adopt_domain_names().await.unwrap();
    engine.settle_rename_questions_after_pull(&before).await;
    assert!(engine.rename_question("eng").is_some());
}
both_backends!(
    a_failed_scan_before_a_pull_keeps_the_question_open,
    a_failed_scan_before_a_pull_keeps_the_question_open_body
);

/// `doctor --fix` respells local spellings, but never the old name of a
/// domain whose rename question is open or answered "keep", and `doctor`
/// does not list those as spellings to fix.
async fn doctor_fix_leaves_a_held_old_name_alone_body(store: Arc<Mutex<dyn Store>>) {
    let m = question_machine(store).await;
    let engine = m.engine(true).await;
    let held = |report: &serde_json::Value| {
        report["local_spellings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row.to_string().contains("\"eng\""))
    };

    let report = engine.name_report().await.unwrap();
    assert!(!held(&report), "open: {report}");
    engine.fix_local_spellings().await.unwrap();
    let pager = std::fs::read_to_string(m.root.join("ops/pager.md")).unwrap();
    assert!(pager.contains("See [[eng:alpha]]"), "{pager}");

    engine
        .keep_manifest_rename("eng", &Scope::Unrestricted)
        .await
        .unwrap();
    let report = engine.name_report().await.unwrap();
    assert!(!held(&report), "kept: {report}");
    engine.fix_local_spellings().await.unwrap();
    let pager = std::fs::read_to_string(m.root.join("ops/pager.md")).unwrap();
    assert!(pager.contains("See [[eng:alpha]]"), "{pager}");
}
both_backends!(
    doctor_fix_leaves_a_held_old_name_alone,
    doctor_fix_leaves_a_held_old_name_alone_body
);

/// The time of the last sync the engine keeps in memory follows the domain:
/// a rename moves it off the old name, and a removal drops it, so a domain
/// later added under either name does not show a sync it never had.
async fn the_last_sync_kept_in_memory_follows_a_rename_and_a_removal_body(
    store: Arc<Mutex<dyn Store>>,
) {
    let m = machine(store).await;
    let engine = m.engine(true).await;
    engine.sync(None).await.unwrap();
    assert!(engine.last_synced_here("eng").is_some());

    engine
        .rename_domain_local(
            "eng",
            "platform",
            NameOrigin::Explicit,
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert_eq!(engine.last_synced_here("eng"), None, "the old name is gone");
    assert!(engine.last_synced_here("platform").is_some());

    engine
        .unregister_domain("platform", &Scope::Unrestricted, false, &[])
        .await
        .unwrap();
    assert_eq!(engine.last_synced_here("platform"), None);
}

both_backends!(
    the_last_sync_kept_in_memory_follows_a_rename_and_a_removal,
    the_last_sync_kept_in_memory_follows_a_rename_and_a_removal_body
);
