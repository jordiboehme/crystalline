//! One writer per file domain on a shared database (F9): only the instance
//! that holds a file domain's host lock writes its engrams and attachments.
//! Another live host refuses a write and names itself; a lock whose holder
//! went silent is taken over by a write, never by a read.
//!
//! Postgres only: a Turso database belongs to one instance.

#![cfg(feature = "postgres")]

use std::sync::Arc;

use crystalline_core::config::{DomainEntry, GlobalConfig};
use crystalline_index::Store;
use crystalline_service::Scope;
use crystalline_service::engine::Engine;
use crystalline_service::params::*;
use serde_json::{Value, json};
use tokio::sync::Mutex;

fn manifest() -> String {
    "---\ntype: manifest\ntitle: Eng\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# Eng\n\n## Scope\n\n- covers things\n\n## When to Use\n\n- when routing\n".to_string()
}

fn engram(title: &str, permalink: &str, body: &str) -> String {
    format!(
        "---\ntype: engram\ntitle: {title}\npermalink: {permalink}\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# {title}\n\n{body}\n"
    )
}

/// Two instances, `inst-a` and `inst-b`, each with its own copy of the file
/// domain `eng` on its own disk, over one Postgres schema. `inst-a` synced
/// first and hosts `eng`.
struct Pair {
    alpha: Arc<Engine>,
    beta: Arc<Engine>,
    b_root: std::path::PathBuf,
    _tmp: tempfile::TempDir,
    scratch: crystalline_index::ScratchSchema,
}

async fn store_on(scratch: &crystalline_index::ScratchSchema) -> Arc<Mutex<dyn Store>> {
    let pg = crystalline_index::PostgresStore::open_in_schema(scratch.url(), scratch.name())
        .await
        .expect("open the postgres test schema");
    Arc::new(Mutex::new(pg))
}

async fn instance(
    scratch: &crystalline_index::ScratchSchema,
    base: &std::path::Path,
    id: &str,
) -> (Arc<Engine>, std::path::PathBuf) {
    let root = base.join(id).join("eng");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("MANIFEST.md"), manifest()).unwrap();
    std::fs::write(
        root.join("alpha.md"),
        engram("Alpha", "alpha", "the body about turbines"),
    )
    .unwrap();
    let mut cfg = GlobalConfig::default();
    cfg.domains
        .insert("eng".to_string(), DomainEntry::file(root.clone()));
    let config = base.join(id).join("config.yaml");
    crystalline_core::config::save_yaml(&config, &cfg).unwrap();
    let engine = Engine::new(store_on(scratch).await, cfg, None, Some(config))
        .with_state_dir(base.join(id).join("state"))
        .with_instance_id(id.to_string());
    (Arc::new(engine), root)
}

async fn pair() -> Option<Pair> {
    let scratch = crate::support::scratch_schema("fh")?;
    let tmp = tempfile::tempdir().unwrap();
    let (alpha, _) = instance(&scratch, tmp.path(), "inst-a").await;
    alpha.sync(None).await.unwrap();
    let (beta, b_root) = instance(&scratch, tmp.path(), "inst-b").await;
    Some(Pair {
        alpha,
        beta,
        b_root,
        _tmp: tmp,
        scratch,
    })
}

impl Pair {
    /// Who holds `eng`'s host lock now, and its last heartbeat.
    async fn holder(&self) -> Option<crystalline_index::DomainHost> {
        let store = store_on(&self.scratch).await;
        let store = store.lock().await;
        let id = store.domain_id("eng").await.unwrap().unwrap();
        store.domain_host(id).await.unwrap()
    }

    /// Make `inst-a`'s heartbeat 91 seconds old: its laptop sleeps.
    async fn alpha_goes_silent(&self) {
        let store = store_on(&self.scratch).await;
        let store = store.lock().await;
        let id = store.domain_id("eng").await.unwrap().unwrap();
        let beat = crystalline_index::claim_window(91).1;
        store
            .claim_domain_host(id, "inst-a", "inst-a", &beat, &beat, true)
            .await
            .unwrap();
    }
}

fn params<T: serde::de::DeserializeOwned>(v: Value) -> T {
    serde_json::from_value(v).unwrap()
}

/// The refusal a write on `inst-b` meets while `inst-a` hosts `eng`.
fn refused(err: impl std::fmt::Display) {
    let text = err.to_string();
    assert!(
        text.contains("domain 'eng' is written by inst-a, the instance that hosts it")
            && text.contains("crystalline sync --domain eng --take-over"),
        "{text}"
    );
}

/// Every family of engram write on an instance that does not host the
/// domain is refused while its host is live, names the host, and changes
/// neither this disk nor the shared rows.
#[tokio::test]
async fn a_live_host_refuses_every_engram_write_on_postgres() {
    let Some(p) = pair().await else { return };
    let beta = &p.beta;
    refused(
        beta.write_engram(&params(json!({
            "domain": "eng", "title": "New", "content": "Typed on beta."
        })))
        .await
        .unwrap_err(),
    );
    assert!(!p.b_root.join("new.md").exists(), "nothing written here");
    refused(
        beta.edit_engram(&params(json!({
            "identifier": "alpha", "domain": "eng", "operation": "append",
            "content": "More from beta."
        })))
        .await
        .unwrap_err(),
    );
    refused(
        beta.delete_engram(&params(json!({ "identifier": "alpha", "domain": "eng" })))
            .await
            .unwrap_err(),
    );
    refused(
        beta.move_engram(
            &params(json!({
                "identifier": "alpha", "domain": "eng", "destination": "moved/alpha"
            })),
            &Scope::Unrestricted,
        )
        .await
        .unwrap_err(),
    );
    refused(
        beta.retire_engram(&params(json!({
            "identifier": "alpha", "domain": "eng", "status": "deprecated"
        })))
        .await
        .unwrap_err(),
    );
    refused(
        beta.attachment_write("eng", "assets/flow.png", b"\x89PNG".to_vec())
            .await
            .unwrap_err(),
    );
    let src = p._tmp.path().join("import");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("imported.md"), engram("Imported", "imported", "x")).unwrap();
    refused(
        beta.import_domain("eng", &src, false, false)
            .await
            .unwrap_err(),
    );
    assert!(
        std::fs::read_to_string(p.b_root.join("alpha.md"))
            .unwrap()
            .contains("the body about turbines")
            && !std::fs::read_to_string(p.b_root.join("alpha.md"))
                .unwrap()
                .contains("More from beta")
    );
    assert_eq!(p.holder().await.unwrap().instance_id, "inst-a");
    // A read is served from the shared rows, as before.
    let read = beta
        .read_engram(
            &params(json!({ "identifier": "alpha", "domain": "eng" })),
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert!(read.to_string().contains("turbines"), "{read}");
}

/// A lock whose holder sent no heartbeat for 90 seconds is not moved by a
/// read; the next write takes it over, as a sync takes it, and marks the
/// host move.
#[tokio::test]
async fn a_stale_host_is_taken_over_by_a_write_never_by_a_read_on_postgres() {
    let Some(p) = pair().await else { return };
    p.alpha_goes_silent().await;
    let beta = &p.beta;
    beta.read_engram(
        &params(json!({ "identifier": "alpha", "domain": "eng" })),
        &Scope::Unrestricted,
    )
    .await
    .unwrap();
    beta.list_domains(&ListDomainsParams::default(), &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(
        p.holder().await.unwrap().instance_id,
        "inst-a",
        "no read moved the lock"
    );
    beta.write_engram(&params(json!({
        "domain": "eng", "title": "Taken", "content": "Written once alpha went silent."
    })))
    .await
    .unwrap();
    assert_eq!(p.holder().await.unwrap().instance_id, "inst-b");
    assert!(p.b_root.join("taken.md").exists());
    let store = store_on(&p.scratch).await;
    let store = store.lock().await;
    let id = store.domain_id("eng").await.unwrap().unwrap();
    let mark = store.host_move(id).await.unwrap().expect("a host move");
    assert_eq!(mark.claimant.as_deref(), Some("inst-b"));
    drop(store);
    // The old host refuses from its next write, naming the new one.
    let err = p
        .alpha
        .write_engram(&params(json!({
            "domain": "eng", "title": "Late", "content": "Alpha woke up."
        })))
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("domain 'eng' is written by inst-b, the instance that hosts it"),
        "{err}"
    );
}

/// The MCP tools meet the same rule: each write tool on the instance that
/// does not host the domain answers the refusal naming the host.
#[tokio::test]
async fn the_write_tools_refuse_on_an_instance_that_does_not_host_on_postgres() {
    let Some(p) = pair().await else { return };
    for (tool, args) in [
        (
            "write_engram",
            json!({ "domain": "eng", "title": "New", "content": "x" }),
        ),
        (
            "edit_engram",
            json!({ "identifier": "alpha", "domain": "eng", "operation": "append", "content": "x" }),
        ),
        (
            "delete_engram",
            json!({ "identifier": "alpha", "domain": "eng" }),
        ),
        (
            "move_engram",
            json!({ "identifier": "alpha", "domain": "eng", "destination": "moved/alpha" }),
        ),
    ] {
        let err = crystalline_service::route::run_tool_routed(
            &p.beta,
            tool,
            args,
            &crystalline_remote::ForwardedAgent::default(),
            None,
        )
        .await
        .unwrap_err();
        refused(format!("{tool}: {err:#}"));
    }
}

/// Run `write` on alpha, and let beta take `eng` over while the write sits
/// between its files and its rows (a host that paused there).
async fn while_beta_takes_over<T>(
    p: &Pair,
    write: impl std::future::Future<Output = Result<T, crystalline_service::engine::EngineError>>,
) -> Result<T, crystalline_service::engine::EngineError> {
    let (entered, release) = p.alpha.hold_next_row_fence_until();
    tokio::pin!(write);
    tokio::select! {
        written = &mut write => panic!("the write never reached its rows: {:?}", written.err()),
        () = entered.notified() => {}
    }
    p.beta.sync_take_over(Some("eng"), true).await.unwrap();
    release.notify_one();
    write.await
}

/// The refusal of a write whose text the fence kept, naming inst-b.
fn kept_refusal(err: impl std::fmt::Display) {
    let text = err.to_string();
    assert!(
        text.contains("domain 'eng' is written by inst-b, the instance that hosts it")
            && text.ends_with(crystalline_service::engine::KEPT_HERE),
        "{text}"
    );
}

/// What alpha keeps for `eng`, as (kind, path, text, sender account).
async fn kept_on(engine: &Engine) -> Vec<(String, String, String, Option<String>)> {
    engine
        .kept_texts("eng", &Scope::Unrestricted)
        .await
        .unwrap()
        .into_iter()
        .map(|k| {
            (
                k.kind.as_str().to_string(),
                k.path.clone(),
                std::fs::read_to_string(&k.file).unwrap(),
                k.sender.as_ref().map(|s| s.account.clone()),
            )
        })
        .collect()
}

impl Pair {
    fn a_root(&self) -> std::path::PathBuf {
        self._tmp.path().join("inst-a").join("eng")
    }

    /// The shared row of `rel`, through beta's store.
    async fn row_of(&self, rel: &str) -> Option<String> {
        let store = store_on(&self.scratch).await;
        let store = store.lock().await;
        let id = store.domain_id("eng").await.unwrap().unwrap();
        store
            .file_stamps(id)
            .await
            .unwrap()
            .get(rel)
            .map(|s| s.sha256.clone())
    }
}

/// A host that lost its lock between its gate and its rows writes no row:
/// a new engram, an edit, a delete, a move and an attachment upload are
/// each refused naming the new host; a text sent is kept with its sender;
/// the old host's disk is back; the shared rows are as they were.
#[tokio::test]
async fn writes_after_the_lock_moved_land_nothing_on_postgres() {
    let Some(p) = pair().await else { return };
    let a_root = p.a_root();
    let alpha_md = std::fs::read(a_root.join("alpha.md")).unwrap();
    let alpha_row = p.row_of("alpha.md").await.expect("alpha's row");

    // A new engram.
    let err = while_beta_takes_over(
        &p,
        p.alpha.write_engram(&params(json!({
            "domain": "eng", "title": "Paused", "content": "Typed while the laptop slept."
        }))),
    )
    .await
    .unwrap_err();
    kept_refusal(&err);
    assert!(!a_root.join("paused.md").exists(), "the new file is gone");
    assert!(p.row_of("paused.md").await.is_none(), "no row");
    let kept = kept_on(&p.alpha).await;
    assert!(
        kept.iter()
            .any(|(kind, path, text, sender)| kind == "engram"
                && path == "paused.md"
                && text.contains("Typed while the laptop slept.")
                && sender.as_deref() == Some("owner")),
        "{kept:?}"
    );

    // An edit.
    p.alpha.sync_take_over(Some("eng"), true).await.unwrap();
    let err = while_beta_takes_over(
        &p,
        p.alpha.edit_engram(&params(json!({
            "identifier": "alpha", "domain": "eng", "operation": "append",
            "content": "An edit that waited."
        }))),
    )
    .await
    .unwrap_err();
    kept_refusal(&err);
    assert_eq!(std::fs::read(a_root.join("alpha.md")).unwrap(), alpha_md);
    assert_eq!(
        p.row_of("alpha.md").await.as_deref(),
        Some(alpha_row.as_str())
    );

    // A delete.
    p.alpha.sync_take_over(Some("eng"), true).await.unwrap();
    let err = while_beta_takes_over(
        &p,
        p.alpha
            .delete_engram(&params(json!({ "identifier": "alpha", "domain": "eng" }))),
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("is written by inst-b"), "{err}");
    assert_eq!(std::fs::read(a_root.join("alpha.md")).unwrap(), alpha_md);
    assert!(p.row_of("alpha.md").await.is_some(), "the row stays");

    // A move.
    p.alpha.sync_take_over(Some("eng"), true).await.unwrap();
    let err = while_beta_takes_over(
        &p,
        p.alpha.move_engram(
            &params(json!({
                "identifier": "alpha", "domain": "eng", "destination": "moved/alpha"
            })),
            &Scope::Unrestricted,
        ),
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("is written by inst-b"), "{err}");
    assert_eq!(std::fs::read(a_root.join("alpha.md")).unwrap(), alpha_md);
    assert!(!a_root.join("moved/alpha.md").exists());
    assert!(p.row_of("alpha.md").await.is_some() && p.row_of("moved/alpha.md").await.is_none());

    // An attachment upload.
    p.alpha.sync_take_over(Some("eng"), true).await.unwrap();
    let err = while_beta_takes_over(
        &p,
        p.alpha
            .attachment_write("eng", "assets/flow.png", b"\x89PNG".to_vec()),
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("is written by inst-b"), "{err}");
    assert!(!a_root.join("assets/flow.png").exists());
}

/// An instance whose configuration registers `domains`, each a file domain
/// with one engram tagged `old-tag`, on its own disk.
async fn instance_with(
    scratch: &crystalline_index::ScratchSchema,
    base: &std::path::Path,
    id: &str,
    domains: &[&str],
    tweak: impl FnOnce(&mut GlobalConfig),
) -> Arc<Engine> {
    let mut cfg = GlobalConfig::default();
    for name in domains {
        let root = base.join(id).join(name);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("MANIFEST.md"), manifest()).unwrap();
        std::fs::write(
            root.join("tagged.md"),
            "---\ntype: engram\ntitle: Tagged\npermalink: tagged\ntags:\n  - old-tag\n  - keep\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# Tagged\n\nbody\n",
        )
        .unwrap();
        cfg.domains
            .insert(name.to_string(), DomainEntry::file(root.clone()));
    }
    tweak(&mut cfg);
    let config = base.join(id).join("config.yaml");
    crystalline_core::config::save_yaml(&config, &cfg).unwrap();
    Arc::new(
        Engine::new(store_on(scratch).await, cfg, None, Some(config))
            .with_state_dir(base.join(id).join("state"))
            .with_instance_id(id.to_string()),
    )
}

/// A tag rename across every domain on an instance retags the domains it
/// writes, names the file domain another live instance hosts, and leaves
/// that domain's files on this disk and its rows as they were; its dry
/// run claims nothing.
#[tokio::test]
async fn a_retag_across_domains_skips_and_names_a_domain_hosted_elsewhere_on_postgres() {
    let Some(scratch) = crate::support::scratch_schema("fhr") else {
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let alpha = instance_with(&scratch, tmp.path(), "inst-a", &["eng"], |_| {}).await;
    alpha.sync(None).await.unwrap();
    let beta = instance_with(&scratch, tmp.path(), "inst-b", &["eng", "ops"], |_| {}).await;
    beta.sync(None).await.unwrap();
    let eng_on_beta = tmp.path().join("inst-b/eng/tagged.md");
    let before = std::fs::read(&eng_on_beta).unwrap();
    let eng_row = || async {
        let store = store_on(&scratch).await;
        let store = store.lock().await;
        let id = store.domain_id("eng").await.unwrap().unwrap();
        store.file_stamps(id).await.unwrap()["tagged.md"]
            .sha256
            .clone()
    };
    let row_before = eng_row().await;

    let dry = beta
        .retag("old-tag", "new-tag", None, false, true, false)
        .await
        .unwrap();
    assert_eq!(dry["hosted_elsewhere_skipped"], json!(["eng"]), "{dry}");
    let done = beta
        .retag("old-tag", "new-tag", None, false, false, false)
        .await
        .unwrap();
    assert_eq!(done["hosted_elsewhere_skipped"], json!(["eng"]), "{done}");
    assert_eq!(done["rewritten"], 1, "{done}");
    assert!(
        std::fs::read_to_string(tmp.path().join("inst-b/ops/tagged.md"))
            .unwrap()
            .contains("new-tag")
    );
    assert_eq!(
        std::fs::read(&eng_on_beta).unwrap(),
        before,
        "eng's file here is untouched"
    );
    assert_eq!(eng_row().await, row_before, "eng's row is untouched");
    let store = store_on(&scratch).await;
    let store = store.lock().await;
    let id = store.domain_id("eng").await.unwrap().unwrap();
    assert_eq!(
        store.domain_host(id).await.unwrap().unwrap().instance_id,
        "inst-a"
    );
}

/// A team update of a domain another instance hosts is refused with the
/// hosted sentence before anything is pulled or written.
#[tokio::test]
async fn a_team_update_on_an_instance_that_does_not_host_is_refused_on_postgres() {
    let Some(scratch) = crate::support::scratch_schema("fho") else {
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let with_origin = |cfg: &mut GlobalConfig| {
        let entry = cfg.domains.get_mut("eng").unwrap();
        entry.origin = Some(params(json!({ "repo": "team/eng" })));
        cfg.github = Some(params(json!({ "enabled": true })));
    };
    let alpha = instance_with(&scratch, tmp.path(), "inst-a", &["eng"], with_origin).await;
    alpha.sync(None).await.unwrap();
    let beta = instance_with(&scratch, tmp.path(), "inst-b", &["eng"], with_origin).await;
    let listing = || {
        let mut names: Vec<String> = std::fs::read_dir(tmp.path().join("inst-b/eng"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    };
    let before = listing();
    let answer = beta
        .origin_update(Some("eng"), &Scope::Unrestricted)
        .await
        .unwrap();
    let error = answer["errors"][0]["error"].as_str().unwrap_or_default();
    assert!(
        error.contains("domain 'eng' is written by inst-a, the instance that hosts it"),
        "{answer}"
    );
    assert_eq!(listing(), before, "no file landed");
}

/// A discard carries on past a domain this instance does not host and says where
/// to run it. The rows on the shared store still count, so the name stays refused.
#[tokio::test]
async fn a_discard_reports_a_domain_it_does_not_host_on_postgres() {
    let Some(p) = pair().await else { return };
    let actor = "ada";
    // A draft this instance's journal holds, in a domain the other one hosts.
    let state = p._tmp.path().join("inst-b/state");
    crystalline_service::overlay_journal::journal_write(&state, "eng", actor, "plan.md", "# Plan")
        .unwrap();
    let report = p.beta.discard_account_drafts(actor, "root").await.unwrap();
    let row = &report["domains"][0];
    assert_eq!(row["not_hosted"], true, "{report}");
    assert!(
        row["where"].as_str().unwrap().contains("Run this there"),
        "{report}"
    );
    assert!(
        !p.beta.account_drafts(actor).await.unwrap().is_empty(),
        "the name stays refused"
    );
}

/// When nobody holds the lock any more by the time the rows commit (its
/// holder released it), the write says so in its own sentence.
#[tokio::test]
async fn a_write_with_no_host_left_says_so_on_postgres() {
    let Some(p) = pair().await else { return };
    let (entered, release) = p.alpha.hold_next_row_fence_until();
    let orphan: WriteParams = params(json!({
        "domain": "eng", "title": "Orphan", "content": "Nobody hosts this now."
    }));
    let write = p.alpha.write_engram(&orphan);
    tokio::pin!(write);
    tokio::select! {
        written = &mut write => panic!("the write never reached its rows: {:?}", written.err()),
        () = entered.notified() => {}
    }
    {
        let store = store_on(&p.scratch).await;
        let store = store.lock().await;
        let id = store.domain_id("eng").await.unwrap().unwrap();
        let (now, stale) = crystalline_index::claim_window(90);
        let token = match store
            .claim_domain_host(id, "zeta", "zeta", &now, &stale, true)
            .await
            .unwrap()
        {
            crystalline_index::HostClaim::Acquired { token } => token,
            other => panic!("{other:?}"),
        };
        assert!(store.release_domain_host(id, "zeta", &token).await.unwrap());
    }
    release.notify_one();
    let err = write.await.unwrap_err().to_string();
    assert_eq!(
        err,
        format!(
            "domain 'eng' has no host right now; try again{}",
            crystalline_service::engine::KEPT_HERE
        )
    );
    assert!(!p.a_root().join("orphan.md").exists());
}
