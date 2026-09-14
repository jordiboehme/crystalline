//! What survives, and what must not, when the index a draft lives in is
//! destroyed or its domain ends.
//!
//! An overlay entry is one actor's private draft of a path in a shared domain.
//! It is a full `engram` row carrying that actor's key and it is primary data:
//! nothing on disk says it exists, so a rebuild from the files - the answer to
//! every other index problem - cannot bring it back. The overlay journal under
//! the state directory is the copy that can, and these tests are what say it
//! does.
//!
//! The write verbs do not journal yet (that is Task 4), so every test here
//! writes the row and its mirror by hand, exactly as the verbs will: one
//! `upsert_overlay` beside one `overlay_journal::journal_write`.

use std::path::PathBuf;
use std::sync::Arc;

use crystalline_core::config::{DomainEntry, GlobalConfig};
use crystalline_core::parse_engram;
use crystalline_index::{DomainKind, EngramRecord, FileStamp, Store, TursoStore};
use crystalline_service::overlay_journal;
use crystalline_service::params::ReadParams;
use crystalline_service::{Engine, Scope};
use tokio::sync::Mutex;

const MANIFEST: &str = "---\ntype: manifest\ntitle: team\npermalink: manifest\ntags:\n  - manifest\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# team\n\n## Scope\n\n- The shared domain\n\n## When to Use\n\n- Route here for team work\n";
/// The base engram: what the domain's files on disk say exists.
const PLAN: &str = "---\ntype: engram\ntitle: Plan\npermalink: plan\ntags:\n  - team\nstatus: stable\nrecorded_at: 2026-01-02\n---\n\n# Plan\n\n- [decision] the plan as the team has it #team\n";
/// Alice's draft of the same path: her private rewrite of it.
const ALICE_DRAFT: &str = "---\ntype: engram\ntitle: Plan\npermalink: plan\ntags:\n  - team\nstatus: draft\nrecorded_at: 2026-01-03\n---\n\n# Plan\n\n- [decision] the plan as alice would have it #team\n";
/// A draft of a path no file holds: the sharp case, since nothing on disk
/// could ever bring it back.
const ALICE_NEW: &str = "---\ntype: engram\ntitle: Fresh\npermalink: fresh\ntags:\n  - team\nstatus: draft\nrecorded_at: 2026-01-03\n---\n\n# Fresh\n\n- [idea] a page only alice has #team\n- relates_to [[Plan]]\n";

/// One engine over one temp directory, with everything a test here needs to
/// act as the write verbs will.
struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    engine: Arc<Engine>,
    store: Arc<Mutex<dyn Store>>,
    state: PathBuf,
}

/// A file domain `team` (MANIFEST + plan.md), synced, with the state directory
/// inside the temp dir so no journal write or sweep can reach the real one.
async fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let dir = root.join("team");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("MANIFEST.md"), MANIFEST).unwrap();
    std::fs::write(dir.join("plan.md"), PLAN).unwrap();

    let mut cfg = GlobalConfig {
        domains_root: Some(root.join("domains-root")),
        ..GlobalConfig::default()
    };
    cfg.domains
        .insert("team".to_string(), DomainEntry::file(dir));
    let config_path = root.join("config.yaml");
    crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();

    let state = root.join("state");
    let store: Arc<Mutex<dyn Store>> =
        Arc::new(Mutex::new(TursoStore::open_in_memory().await.unwrap()));
    let engine = Arc::new(
        Engine::new(store.clone(), cfg, None, Some(config_path)).with_state_dir(state.clone()),
    );
    engine.sync(None).await.unwrap();
    Fixture {
        _tmp: tmp,
        root,
        engine,
        store,
        state,
    }
}

/// A record the way a write verb builds one for a draft that is on nobody's
/// disk: the full markdown in the `content` column, the way a virtual domain's
/// rows carry it (`Engine::index_markdown`'s `store_full`), because the row is
/// the only place a draft's document lives.
fn record(text: &str, path: &str) -> EngramRecord {
    let mut record = EngramRecord::from_engram(
        &parse_engram(text).unwrap(),
        path,
        FileStamp {
            mtime: 0,
            size: text.len() as u64,
            sha256: "0".repeat(64),
        },
    );
    record.content = text.to_string();
    record
}

impl Fixture {
    /// The domain's own folder, which is also the path its index row carries.
    fn domain_root(&self, domain: &str) -> PathBuf {
        self.root.join(domain)
    }

    /// Resolve a domain id the way every writer here does: with the row's own
    /// path and kind, so the upsert updates nothing.
    async fn domain_id(&self, store: &dyn Store, domain: &str) -> crystalline_index::DomainId {
        store
            .upsert_domain(
                domain,
                Some(&self.domain_root(domain).to_string_lossy()),
                DomainKind::File,
            )
            .await
            .unwrap()
    }

    /// Write one actor's draft the way Task 4's write verbs will: the row and
    /// its mirror, in that order.
    async fn draft(&self, domain: &str, actor: &str, path: &str, text: &str) {
        let store = self.store.lock().await;
        let id = self.domain_id(&*store, domain).await;
        store
            .upsert_overlay(id, actor, &record(text, path))
            .await
            .unwrap();
        overlay_journal::journal_write(&self.state, domain, actor, path, text).unwrap();
    }

    /// Write one actor's deletion of a base path the same way.
    async fn tombstone(&self, domain: &str, actor: &str, path: &str) {
        let store = self.store.lock().await;
        let id = self.domain_id(&*store, domain).await;
        let mut rec = record(PLAN, path);
        rec.tombstone = true;
        store.upsert_overlay(id, actor, &rec).await.unwrap();
        overlay_journal::journal_tombstone(&self.state, domain, actor, path).unwrap();
    }

    /// What one actor holds, as `(path, content, tombstone)` triples ordered by
    /// path.
    async fn held(&self, domain: &str, actor: &str) -> Vec<(String, String, bool)> {
        let store = self.store.lock().await;
        let id = self.domain_id(&*store, domain).await;
        store
            .overlay_entries(id, actor)
            .await
            .unwrap()
            .into_iter()
            .map(|e| (e.path, e.content, e.tombstone))
            .collect()
    }
}

/// The whole point of the journal: an index destroyed and rebuilt from the
/// files on disk has nothing to rebuild a draft from, so the drafts come back
/// from the mirror instead.
///
/// The wipe is reproduced as `Store::wipe` plus a rebuilding sync, which is
/// what `crystalline reindex --wipe` does to the database - the CLI verb
/// itself needs the index file to itself and cannot run against this in-memory
/// store, so its own wiring is pinned by `crates/cli/tests/data.rs`.
///
/// Three shapes, because they fail differently: a draft over a file that is
/// still there, a draft at a path no file holds, and one actor's tombstone of
/// a base row, which carries no content of its own and is rebuilt from the
/// base.
#[tokio::test]
async fn an_index_wipe_keeps_drafts_through_the_journal() {
    let f = fixture().await;
    f.draft("team", "alice", "plan.md", ALICE_DRAFT).await;
    f.draft("team", "alice", "fresh.md", ALICE_NEW).await;
    f.tombstone("team", "bob", "plan.md").await;

    {
        let store = f.store.lock().await;
        store.wipe().await.unwrap();
    }
    assert!(
        f.held("team", "alice").await.is_empty(),
        "the wipe really took the rows"
    );

    // The rebuild: the base comes back from the files, the drafts from the
    // journal.
    f.engine.sync(None).await.unwrap();

    assert_eq!(
        f.held("team", "alice").await,
        vec![
            ("fresh.md".to_string(), ALICE_NEW.to_string(), false),
            ("plan.md".to_string(), ALICE_DRAFT.to_string(), false),
        ],
        "both of alice's drafts came back, content and all"
    );
    let bob = f.held("team", "bob").await;
    assert_eq!(bob.len(), 1, "bob holds his one deletion: {bob:?}");
    assert_eq!(bob[0].0, "plan.md");
    assert!(bob[0].2, "and it is still a tombstone");
    // A tombstone's mirror carries no content of its own, so the row comes back
    // from the base row it deletes - the base's own content, not a guess.
    let base_content = {
        let store = f.store.lock().await;
        let id = f.domain_id(&*store, "team").await;
        store.engram_content(id, "plan.md").await.unwrap().unwrap()
    };
    assert_eq!(
        bob[0].1, base_content,
        "the restored tombstone stands over the base row's own content"
    );

    // A restored draft is a whole row, chunks included. Task 1 kept
    // `chunks_needing_embedding` unscoped precisely so a draft's chunks reach
    // the same embedding backlog a base row's do; a restore that wrote the row
    // and no chunks would make every draft that has been through a wipe
    // permanently un-embeddable, and nothing else in the wave would notice.
    let backlog = {
        let store = f.store.lock().await;
        store
            .chunks_needing_embedding("test-model", None, 100, None)
            .await
            .unwrap()
    };
    assert!(
        backlog
            .iter()
            .any(|job| job.text.contains("a page only alice has")),
        "the restored draft is in the embedding backlog like any other row: {:?}",
        backlog.iter().map(|j| &j.text).collect::<Vec<_>>()
    );
    // And its edges resolve. The draft relates to the base engram, and asking
    // the store to resolve what is still pending is the only probe that can see
    // a draft's edges at all today: every reading surface is screened to the
    // base until Task 5 threads the actor through them. A restore that left the
    // rows pending would have work for this call to do.
    let pending = {
        let store = f.store.lock().await;
        let id = f.domain_id(&*store, "team").await;
        (
            store.resolve_pending_relations(id).await.unwrap(),
            store.resolve_pending_links(id).await.unwrap(),
        )
    };
    assert_eq!(
        pending,
        (0, 0),
        "the restore left no relation or link of its own pointing forward"
    );

    // The base is untouched by any of it: a reader who is nobody in particular
    // still sees what the files say.
    let base = f
        .engine
        .read_engram(
            &ReadParams {
                identifier: "plan".to_string(),
                domain: Some("team".to_string()),
            },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert!(
        base["content"]
            .as_str()
            .unwrap_or_default()
            .contains("as the team has it"),
        "the base row is what the files say: {base}"
    );

    // Store rows win. An actor's live row is the truth and the journal is the
    // mirror, so a later sync fills gaps and overwrites nothing.
    {
        let store = f.store.lock().await;
        let id = f.domain_id(&*store, "team").await;
        let moved_on = ALICE_DRAFT.replace("alice would have it", "alice has since moved on to");
        store
            .upsert_overlay(id, "alice", &record(&moved_on, "plan.md"))
            .await
            .unwrap();
    }
    f.engine.sync(None).await.unwrap();
    let alice = f.held("team", "alice").await;
    assert!(
        alice[1].1.contains("since moved on"),
        "the row the store holds is never overwritten by the mirror: {alice:?}"
    );
}

/// A domain's removal takes every actor's drafts with it - the rows and the
/// mirror that would bring them back, together - and the preview says how many
/// are at stake before anybody answers the question.
#[tokio::test]
async fn removing_a_domain_sweeps_every_actors_journal_and_names_the_counts() {
    let f = fixture().await;
    f.draft("team", "alice", "plan.md", ALICE_DRAFT).await;
    f.draft("team", "alice", "fresh.md", ALICE_NEW).await;
    f.tombstone("team", "bob", "plan.md").await;

    let preview = f
        .engine
        .domain_remove_preview("team", &Scope::Unrestricted, false)
        .await
        .unwrap();
    assert_eq!(
        preview["drafts"],
        serde_json::json!([
            { "actor": "alice", "entries": 2 },
            { "actor": "bob", "entries": 1 },
        ]),
        "the preview names every actor's count before the question is put: {preview}"
    );
    assert_eq!(preview["drafts_unknown"], serde_json::json!(false));

    let report = f
        .engine
        .unregister_domain("team", &Scope::Unrestricted, false)
        .await
        .unwrap();
    assert_eq!(
        report["drafts_swept"],
        serde_json::json!(3),
        "the receipt says how many mirrored drafts went with it: {report}"
    );
    assert!(
        overlay_journal::journal_entries(&f.state, "team").is_empty(),
        "no actor's mirror is left behind"
    );
    assert!(
        !f.state.join("overlays/team").exists(),
        "and the domain's journal folder is gone"
    );
}

/// The second removal path. The daemon's own sweep never goes through
/// `unregister_domain`: it calls `clear_domain` on a domain nobody registers.
/// A journal left behind there is worse than a leak - the next sync would
/// restore the drafts, and the rows would come back for a domain that does not
/// exist.
///
/// The orphan holds drafts and nothing else, which is the case the collector
/// cannot see on its own: `domain_stats` counts base rows, so a drafts-only
/// domain reads as empty to it and would be kept as having nothing to collect.
/// The registered domain beside it is the control: its drafts and its mirror
/// are none of this sweep's business.
#[tokio::test]
async fn an_orphan_collection_sweeps_the_journal_and_nothing_is_resurrected() {
    let f = fixture().await;
    f.draft("team", "alice", "fresh.md", ALICE_NEW).await;
    // A domain nobody registers, holding one actor's draft and no base row at
    // all - an index carried in from elsewhere, or a registration dropped by
    // hand.
    f.draft("solo", "alice", "fresh.md", ALICE_NEW).await;

    // A person asking, so the grace period is not in the way.
    let report = f
        .engine
        .collect_orphaned_domains(None, false)
        .await
        .unwrap();
    assert_eq!(
        report["collected"],
        serde_json::json!(["solo"]),
        "a domain holding only drafts is still a domain with something to collect: {report}"
    );
    assert_eq!(
        report["drafts_swept"],
        serde_json::json!(1),
        "the sweep says what it took: {report}"
    );
    assert!(
        f.held("solo", "alice").await.is_empty(),
        "the rows went with the domain"
    );
    assert!(
        overlay_journal::journal_entries(&f.state, "solo").is_empty(),
        "and so did the mirror"
    );
    assert_eq!(
        f.held("team", "alice").await.len(),
        1,
        "the registered domain's drafts are none of the sweep's business"
    );

    // And nothing brings them back: a restore into a domain nobody registers
    // is refused, so even a sync that reached this name would write no row.
    assert!(
        f.engine.restore_overlays("solo").await.is_err(),
        "restoring into an unregistered domain is refused"
    );
    f.engine.sync(None).await.unwrap();
    assert!(
        f.held("solo", "alice").await.is_empty(),
        "nothing was resurrected"
    );
}
