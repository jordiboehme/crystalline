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

use crystalline_core::config::{DomainEntry, GlobalConfig, ReviewMode};
use crystalline_core::parse_engram;
use crystalline_index::{DomainKind, EngramRecord, FileStamp, Store, TursoStore};
use crystalline_service::engine::ConfigureAction;
use crystalline_service::overlay_journal;
use crystalline_service::params::{DeleteParams, EditParams, ReadParams, WriteParams};
use crystalline_service::{Engine, Scope, SimilarProbe};
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
    fixture_with_state_dir(true).await
}

/// The same, with the choice of whether the engine is told where its state
/// directory is. `false` is only ever used by the test that pins what an engine
/// without one may do, which is nothing.
async fn fixture_with_state_dir(pinned: bool) -> Fixture {
    build_fixture(pinned, false).await
}

/// The same domain, in review mode: every write by every actor joins that
/// actor's own draft and the folder on disk goes on saying what the team
/// reviewed.
///
/// Review mode is written straight into the configuration here. Turning it on
/// through a verb - with its origin requirement, its dirty-tree refusal and its
/// per-actor fold on the way out - is Task 7's; this task is what the mode
/// *does* once a domain carries it, so a fixture that had to satisfy the
/// enabling gates would be testing those gates instead.
async fn review_fixture() -> Fixture {
    build_fixture(true, true).await
}

async fn build_fixture(pinned: bool, review: bool) -> Fixture {
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
    let mut entry = DomainEntry::file(dir);
    if review {
        entry.review = Some(ReviewMode::Overlay);
    }
    cfg.domains.insert("team".to_string(), entry);
    let config_path = root.join("config.yaml");
    crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();

    let state = root.join("state");
    let store: Arc<Mutex<dyn Store>> =
        Arc::new(Mutex::new(TursoStore::open_in_memory().await.unwrap()));
    let engine = Engine::new(store.clone(), cfg, None, Some(config_path));
    let engine = Arc::new(if pinned {
        engine.with_state_dir(state.clone())
    } else {
        engine
    });
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
        overlay_journal::journal_entries(&f.state, "team")
            .entries
            .is_empty(),
        "no actor's mirror is left behind"
    );
    assert!(
        !f.state.join("overlays/team").exists(),
        "and the domain's journal folder is gone"
    );
}

/// An engine that was never told where its state directory is reaches no
/// journal at all in a test build, and says which method to call.
///
/// The sweep on the removal path is `std::fs::remove_dir_all` under
/// `<state_dir>/overlays/<domain>`, so an engine falling back to the real state
/// directory is a test suite deleting a developer's own drafts by domain name -
/// silently, since the sweep is best effort, and unrecoverably, since the
/// journal is the one copy of a draft a rebuild cannot make again. An audit of
/// the fixtures is not enough (the first one missed three binaries), so the
/// resolver itself refuses under the test seam and every fixture that touches a
/// journal has to say where.
#[tokio::test]
async fn an_engine_with_no_state_dir_reaches_no_journal_in_a_test_build() {
    let f = fixture_with_state_dir(false).await;

    let err = f
        .engine
        .restore_overlays("team")
        .await
        .expect_err("a restore with nowhere to restore from is refused");
    let text = err.to_string();
    assert!(
        text.contains("with_state_dir"),
        "and the refusal names the method that fixes it: {text}"
    );

    // The removal paths do not fail - they are best effort by design - but they
    // sweep nothing and they say the count is unknown rather than zero.
    let preview = f
        .engine
        .domain_remove_preview("team", &Scope::Unrestricted, false)
        .await
        .unwrap();
    assert_eq!(preview["drafts_unknown"], serde_json::json!(true));
    let report = f
        .engine
        .unregister_domain("team", &Scope::Unrestricted, false)
        .await
        .unwrap();
    assert_eq!(report["drafts_swept"], serde_json::json!(0));
}

/// An empty answer and an unanswerable one are not the same thing, and a
/// destructive confirmation is the last place to confuse them.
///
/// A journal folder that cannot be enumerated used to come back as an empty
/// list, which the preview reported as an affirmative "nobody is drafting
/// here" - in front of a removal that then deletes whatever was really in
/// there. The same blindness reached the orphan sweep, where "no drafts"
/// decides whether a domain has anything to collect at all.
#[tokio::test]
async fn an_unreadable_journal_is_never_read_as_nobody_drafting() {
    let f = fixture().await;
    // The journal folder is not a folder. Portable, deterministic, and exactly
    // as unreadable as a permission problem.
    std::fs::create_dir_all(f.state.join("overlays")).unwrap();
    std::fs::write(f.state.join("overlays/team"), "not a folder").unwrap();

    let preview = f
        .engine
        .domain_remove_preview("team", &Scope::Unrestricted, false)
        .await
        .unwrap();
    assert_eq!(preview["drafts"], serde_json::json!([]));
    assert_eq!(
        preview["drafts_unknown"],
        serde_json::json!(true),
        "a count nothing could read is not a count of zero: {preview}"
    );

    // The orphan sweep, same rule. `solo` holds one actor's draft and no base
    // row, so what the journal says is the whole of what it has to collect -
    // and with the journal unreadable it keeps the domain rather than deleting
    // rows it cannot account for.
    f.draft("solo", "alice", "fresh.md", ALICE_NEW).await;
    std::fs::remove_dir_all(f.state.join("overlays/solo")).unwrap();
    std::fs::write(f.state.join("overlays/solo"), "not a folder either").unwrap();

    let report = f
        .engine
        .collect_orphaned_domains(None, false)
        .await
        .unwrap();
    assert_eq!(
        report["collected"],
        serde_json::json!([]),
        "nothing is collected while the journal cannot be read: {report}"
    );
    let solo = report["considered"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["domain"] == "solo")
        .cloned()
        .unwrap();
    assert_eq!(solo["kept"], serde_json::json!("no_rows"));
    assert_eq!(solo["drafts_unknown"], serde_json::json!(true));
    assert!(
        solo["reason"]
            .as_str()
            .unwrap_or_default()
            .contains("could not be read"),
        "and the report says why it was kept: {solo}"
    );
    assert_eq!(
        f.held("solo", "alice").await.len(),
        1,
        "the draft row is still there, uncollected"
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
        overlay_journal::journal_entries(&f.state, "solo")
            .entries
            .is_empty(),
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

// --- Task 4: review mode routes every write into its actor's draft ----------

/// One account, as an authenticated surface resolves it.
fn account(name: &str) -> Scope {
    Scope::User {
        account: name.to_string(),
        admin: false,
    }
}

fn write_params(domain: &str, title: &str, content: &str) -> WriteParams {
    WriteParams {
        domain: domain.to_string(),
        title: title.to_string(),
        content: content.to_string(),
        folder: None,
        engram_type: None,
        tags: vec!["team".to_string()],
        status: None,
        metadata: None,
        overwrite: false,
    }
}

fn read(identifier: &str) -> ReadParams {
    ReadParams {
        identifier: identifier.to_string(),
        domain: Some("team".to_string()),
    }
}

impl Fixture {
    /// The markdown one actor sees at an identifier, or the error text when
    /// that actor sees nothing there.
    async fn reads(&self, identifier: &str, scope: &Scope) -> std::result::Result<String, String> {
        match self.engine.read_engram(&read(identifier), scope).await {
            Ok(value) => Ok(value["content"].as_str().unwrap_or_default().to_string()),
            Err(e) => Err(e.to_string()),
        }
    }

    /// Every file under the domain's folder, as (relative path, bytes), so a
    /// test can say "the tree did not move" about the whole tree rather than
    /// about the one file it remembered to check.
    fn tree(&self, domain: &str) -> Vec<(String, String)> {
        let root = self.domain_root(domain);
        let mut out = Vec::new();
        let mut stack = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    let rel = path
                        .strip_prefix(&root)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/");
                    out.push((rel, std::fs::read_to_string(&path).unwrap_or_default()));
                }
            }
        }
        out.sort();
        out
    }
}

/// The headline property of review mode, in one test: a write by an
/// authenticated account changes nothing on disk, lands as that account's own
/// draft, is mirrored where a wipe can bring it back, and is visible to its
/// author and to nobody else.
///
/// Both write shapes are here because they reach the store by different
/// routes: a create builds its markdown and indexes it, an edit reads the
/// current text and writes the result back.
#[tokio::test]
async fn overlay_writes_never_touch_the_tree_and_reads_shadow_per_actor() {
    let f = review_fixture().await;
    let before = f.tree("team");
    let alice = account("alice");

    let created = f
        .engine
        .write_engram_as(
            &write_params("team", "Fresh", "- [idea] a page only alice has #team"),
            Some("claude-code/2.0-for-alice"),
            &alice,
        )
        .await
        .unwrap();
    assert_eq!(created["permalink"], serde_json::json!("fresh"));

    f.engine
        .edit_engram_as(
            &EditParams {
                identifier: "plan".to_string(),
                domain: "team".to_string(),
                operation: "append".to_string(),
                content: Some("- [decision] and alice would add this #team".to_string()),
                key: None,
                value: None,
                find_text: None,
                expected_replacements: None,
                section: None,
                include_subsections: false,
                expected_checksum: None,
                ack_scope: None,
            },
            Some("claude-code/2.0-for-alice"),
            &alice,
        )
        .await
        .unwrap();

    assert_eq!(
        f.tree("team"),
        before,
        "the folder on disk still says exactly what the team reviewed"
    );

    let held = f.held("team", "alice").await;
    let paths: Vec<&str> = held.iter().map(|(p, _, _)| p.as_str()).collect();
    assert_eq!(
        paths,
        vec!["fresh.md", "plan.md"],
        "both writes landed in alice's own draft: {held:?}"
    );
    assert!(
        held[1].1.contains("alice would add this"),
        "the edit joined her draft of the base row: {held:?}"
    );
    assert!(
        held[0].1.starts_with("---"),
        "a draft row carries the whole document, frontmatter included, because \
         nothing on disk holds it: {held:?}"
    );

    // Mirrored, in the order and shape a restore reads back.
    let mirrored = overlay_journal::journal_entries(&f.state, "team");
    let names: Vec<(String, String)> = mirrored
        .entries
        .iter()
        .map(|e| (e.actor.clone(), e.path.clone()))
        .collect();
    assert_eq!(
        names,
        vec![
            ("alice".to_string(), "fresh.md".to_string()),
            ("alice".to_string(), "plan.md".to_string()),
        ],
        "every draft is mirrored where a wipe can bring it back"
    );

    // Shadowing: alice reads her own, everybody else reads the tree.
    assert!(
        f.reads("plan", &alice)
            .await
            .unwrap()
            .contains("alice would add this"),
        "alice reads her draft of the path"
    );
    let bob = f.reads("plan", &account("bob")).await.unwrap();
    assert!(
        bob.contains("as the team has it") && !bob.contains("alice would add this"),
        "bob reads what the team reviewed: {bob}"
    );
    assert!(
        f.reads("fresh", &alice).await.is_ok(),
        "alice reads the page only she has"
    );
    let miss = f.reads("fresh", &account("bob")).await.unwrap_err();
    assert!(
        miss.contains("no engram 'fresh' in domain 'team'"),
        "and for bob it is the miss an engram nobody wrote produces: {miss}"
    );
}

/// A delete in review mode is that actor's draft deletion: the file stays, the
/// base row stays, and the path reads as absent for its author alone.
#[tokio::test]
async fn a_delete_is_a_tombstone_only_its_author_sees() {
    let f = review_fixture().await;
    let before = f.tree("team");
    let alice = account("alice");

    let receipt = f
        .engine
        .delete_engram_as(
            &DeleteParams {
                identifier: "plan".to_string(),
                domain: "team".to_string(),
                expected_checksum: None,
            },
            Some("claude-code/2.0-for-alice"),
            &alice,
        )
        .await
        .unwrap();
    assert_eq!(receipt["deleted"], serde_json::json!(true));
    assert_eq!(
        receipt["draft"],
        serde_json::json!(true),
        "the receipt says the deletion is a draft: {receipt}"
    );

    assert_eq!(f.tree("team"), before, "the file is still on disk");
    let held = f.held("team", "alice").await;
    assert_eq!(held.len(), 1, "alice holds one row: {held:?}");
    assert_eq!(held[0].0, "plan.md");
    assert!(held[0].2, "and it is a tombstone");

    // A tombstone is chunkless, exactly as the journal restore writes one, so
    // the two shapes are one shape and a deleted draft never sits in the
    // embedding backlog.
    let backlog = {
        let store = f.store.lock().await;
        store
            .chunks_needing_embedding("test-model", None, 100, None)
            .await
            .unwrap()
    };
    assert_eq!(
        backlog
            .iter()
            .filter(|job| job.text.contains("as the team has it"))
            .count(),
        1,
        "only the base row's chunks are in the backlog: {:?}",
        backlog.iter().map(|j| &j.text).collect::<Vec<_>>()
    );

    let miss = f.reads("plan", &alice).await.unwrap_err();
    assert!(
        miss.contains("no engram 'plan' in domain 'team'"),
        "the path reads as absent for alice, in the words an engram nobody wrote \
         produces: {miss}"
    );
    assert!(
        f.reads("plan", &account("bob"))
            .await
            .unwrap()
            .contains("as the team has it"),
        "and it is still there for everybody else"
    );
    // The mirror carries the deletion, so a wipe brings it back as one.
    let mirrored = overlay_journal::journal_entries(&f.state, "team");
    assert_eq!(mirrored.entries.len(), 1);
    assert_eq!(
        mirrored.entries[0].content, None,
        "a tombstone mirrors no content"
    );
}

/// No identity, no draft: a write on a domain that reviews changes has nowhere
/// to land, and the refusal teaches how to connect rather than failing blankly
/// or - far worse - falling through onto the base row.
#[tokio::test]
async fn no_identity_no_overlay_writes_refuse_with_teaching_text() {
    let f = review_fixture().await;
    let before = f.tree("team");

    let err = f
        .engine
        .write_engram_as(
            &write_params("team", "Fresh", "- [idea] nobody in particular #team"),
            None,
            &Scope::Anonymous,
        )
        .await
        .expect_err("a write with no identity is refused");
    assert_eq!(
        err.to_string(),
        crystalline_service::OVERLAY_NEEDS_IDENTITY,
        "the refusal is the teaching sentence, verbatim"
    );

    let edit_err = f
        .engine
        .edit_engram_as(
            &EditParams {
                identifier: "plan".to_string(),
                domain: "team".to_string(),
                operation: "append".to_string(),
                content: Some("- [decision] nobody in particular #team".to_string()),
                key: None,
                value: None,
                find_text: None,
                expected_replacements: None,
                section: None,
                include_subsections: false,
                expected_checksum: None,
                ack_scope: None,
            },
            None,
            &Scope::Anonymous,
        )
        .await
        .expect_err("and so is an edit");
    assert_eq!(
        edit_err.to_string(),
        crystalline_service::OVERLAY_NEEDS_IDENTITY
    );

    assert_eq!(f.tree("team"), before, "nothing reached the tree");
    assert!(
        f.held("team", "").await.is_empty() && f.held("team", "anonymous").await.is_empty(),
        "and nothing reached a draft either"
    );
    assert!(
        overlay_journal::journal_entries(&f.state, "team")
            .entries
            .is_empty(),
        "and nothing was mirrored"
    );
    let base = f.reads("plan", &Scope::Unrestricted).await.unwrap();
    assert!(
        !base.contains("nobody in particular"),
        "the base row is untouched: {base}"
    );
}

/// The machine owner is an actor like any other. Whoever runs the CLI, the
/// control socket or a local stdio session drafts under the owner key rather
/// than writing straight through the review the domain asked for.
#[tokio::test]
async fn the_machine_owner_writes_the_owner_overlay() {
    let f = review_fixture().await;
    let before = f.tree("team");

    f.engine
        .write_engram(&write_params(
            "team",
            "Fresh",
            "- [idea] the owner's own #team",
        ))
        .await
        .unwrap();

    assert_eq!(f.tree("team"), before, "not even the owner writes the tree");
    let held = f.held("team", "owner").await;
    assert_eq!(held.len(), 1, "the owner holds one draft: {held:?}");
    assert_eq!(held[0].0, "fresh.md");
    assert!(
        f.reads("fresh", &account("alice")).await.is_err(),
        "and it is the owner's alone"
    );
}

/// In review mode a draft belongs to the account that wrote it, so the
/// provenance is the composed acting actor the surface resolved - the harness
/// and the human behind it - and a configured `identity.actor` no longer
/// outranks it. On a direct domain the setting still wins, which is what makes
/// this a property of the mode rather than a change to provenance everywhere.
#[tokio::test]
async fn an_overlay_write_records_the_composed_actor_as_provenance() {
    let f = review_fixture().await;
    f.engine
        .configure(&ConfigureAction::Set {
            key: "identity.actor".to_string(),
            value: "the-house-style".to_string(),
        })
        .await
        .unwrap();

    f.engine
        .write_engram_as(
            &write_params("team", "Fresh", "- [idea] a page only alice has #team"),
            Some("claude-code/2.0-for-alice"),
            &account("alice"),
        )
        .await
        .unwrap();
    let held = f.held("team", "alice").await;
    assert!(
        held[0].1.contains("by: claude-code/2.0-for-alice"),
        "the draft is the account's, so the composed actor is what it records: {}",
        held[0].1
    );

    // The same configuration, on a domain that takes changes directly.
    let direct = fixture().await;
    direct
        .engine
        .configure(&ConfigureAction::Set {
            key: "identity.actor".to_string(),
            value: "the-house-style".to_string(),
        })
        .await
        .unwrap();
    direct
        .engine
        .write_engram_as(
            &write_params("team", "Fresh", "- [idea] written straight through #team"),
            Some("claude-code/2.0-for-alice"),
            &account("alice"),
        )
        .await
        .unwrap();
    let written = std::fs::read_to_string(direct.domain_root("team").join("fresh.md")).unwrap();
    assert!(
        written.contains("by: the-house-style"),
        "a direct domain still honours the configured actor: {written}"
    );
}

/// A draft is a write like any other from the caller's side: the receipt says
/// it is a draft, and the capture advisory that rides on a write receipt still
/// finds the neighbours it would have found.
#[tokio::test]
async fn a_write_receipt_says_draft_and_still_carries_similar() {
    let f = review_fixture().await;
    let alice = account("alice");

    let mut created = f
        .engine
        .write_engram_as(
            &write_params(
                "team",
                "Fresh",
                "- [decision] the plan as alice would have it #team",
            ),
            Some("claude-code/2.0-for-alice"),
            &alice,
        )
        .await
        .unwrap();
    assert_eq!(
        created["draft"],
        serde_json::json!(true),
        "the receipt says where the write landed: {created}"
    );

    // `attach_similar` is what the MCP and REST surfaces hang on a write
    // receipt; the engine verb hands the receipt back and the surface decorates
    // it, so the probe runs here exactly as it does there.
    f.engine
        .attach_similar(
            &mut created,
            SimilarProbe::Edit {
                new_text: "the plan as alice would have it",
            },
            &alice,
        )
        .await;
    assert_eq!(
        created["draft"],
        serde_json::json!(true),
        "and the advisory leaves it saying so: {created}"
    );

    // An edit receipt says it too.
    let edited = f
        .engine
        .edit_engram_as(
            &EditParams {
                identifier: "plan".to_string(),
                domain: "team".to_string(),
                operation: "append".to_string(),
                content: Some("- [decision] and alice would add this #team".to_string()),
                key: None,
                value: None,
                find_text: None,
                expected_replacements: None,
                section: None,
                include_subsections: false,
                expected_checksum: None,
                ack_scope: None,
            },
            Some("claude-code/2.0-for-alice"),
            &alice,
        )
        .await
        .unwrap();
    assert_eq!(edited["draft"], serde_json::json!(true), "{edited}");
}

/// The other four write verbs, in one test, because the failure they share is
/// the one this whole mode exists to prevent: a verb nobody routed writes
/// straight through the review the domain asked for, and nothing else in the
/// wave would notice.
///
/// Save, retire and restore each join the actor's draft of the path they name;
/// a move is a tombstone at the source and an entry at the destination, which
/// is the shape a rename in review mode has to take - the reviewed file stays
/// where the team put it, and this actor sees the engram at its new address.
#[tokio::test]
async fn every_write_verb_lands_in_the_draft_and_none_of_them_touches_the_tree() {
    let f = review_fixture().await;
    let before = f.tree("team");
    let alice = account("alice");
    let who = Some("claude-code/2.0-for-alice");

    // -- save: the whole document, verbatim, against the checksum alice read --
    let read = f.engine.read_engram(&read("plan"), &alice).await.unwrap();
    let saved = f
        .engine
        .save_engram(
            &crystalline_service::params::SaveParams {
                domain: "team".to_string(),
                identifier: "plan".to_string(),
                content: PLAN.replace("as the team has it", "as alice saved it"),
                expected_checksum: read["checksum"].as_str().unwrap().to_string(),
            },
            &alice,
        )
        .await
        .unwrap();
    assert_eq!(saved["draft"], serde_json::json!(true), "{saved}");
    assert!(
        f.reads("plan", &alice)
            .await
            .unwrap()
            .contains("as alice saved it"),
        "the save joined alice's draft"
    );

    // -- retire: the guided status edit, on the draft she now holds --
    let retired = f
        .engine
        .retire_engram_as(
            &crystalline_service::params::RetireParams {
                domain: "team".to_string(),
                identifier: "plan".to_string(),
                status: "archived".to_string(),
                successor: None,
                valid_to: None,
            },
            who,
            &alice,
        )
        .await
        .unwrap();
    assert_eq!(retired["draft"], serde_json::json!(true), "{retired}");
    assert!(
        f.reads("plan", &alice)
            .await
            .unwrap()
            .contains("status: archived"),
        "the retirement joined the same draft"
    );

    // -- move: a tombstone where the team's file is, an entry where alice
    //    now looks for it --
    let moved = f
        .engine
        .move_engram(
            &crystalline_service::params::MoveParams {
                identifier: "plan".to_string(),
                domain: "team".to_string(),
                destination: "archive/plan.md".to_string(),
                destination_domain: None,
                update_links: None,
            },
            &alice,
        )
        .await
        .unwrap();
    assert_eq!(moved["draft"], serde_json::json!(true), "{moved}");
    let held = f.held("team", "alice").await;
    let shape: Vec<(&str, bool)> = held
        .iter()
        .map(|(path, _, tomb)| (path.as_str(), *tomb))
        .collect();
    assert_eq!(
        shape,
        vec![("archive/plan.md", false), ("plan.md", true)],
        "a move is a tombstone at the source and an entry at the destination: {held:?}"
    );
    assert!(
        f.reads("plan", &alice).await.is_err(),
        "alice no longer sees it where the team's file is"
    );
    assert!(
        f.reads("plan", &account("bob"))
            .await
            .unwrap()
            .contains("as the team has it"),
        "and bob still does"
    );

    // -- restore: the room-recovery verb, into the draft rather than the tree --
    f.engine
        .restore_engram("team", "recovered.md", ALICE_NEW, &alice)
        .await
        .unwrap();
    assert!(
        f.reads("fresh", &alice).await.is_ok(),
        "the restored document is alice's draft"
    );

    assert_eq!(
        f.tree("team"),
        before,
        "and not one of the four moved a single byte on disk"
    );
    // Everything alice holds is mirrored, so a wipe brings all of it back.
    let mirrored: Vec<(String, Option<bool>)> = overlay_journal::journal_entries(&f.state, "team")
        .entries
        .into_iter()
        .map(|e| (e.path, e.content.map(|_| true)))
        .collect();
    assert_eq!(
        mirrored,
        vec![
            ("archive/plan.md".to_string(), Some(true)),
            ("plan.md".to_string(), None),
            ("recovered.md".to_string(), Some(true)),
        ],
        "every draft and the one deletion are mirrored"
    );
}

// --- the registered-set screen composes ahead of the actor dimension --------

/// The review-mode domain again, made private to `owner` with `alice` invited
/// and `out` a signed-in stranger, plus a `ghost` domain the index holds rows
/// for and nobody registered.
async fn screened_fixture() -> Fixture {
    let f = review_fixture().await;
    let auth = Arc::new(
        crystalline_service::rest::AuthStore::open(&f.root.join("web-auth.db"))
            .await
            .unwrap(),
    );
    for name in ["owner", "alice", "out"] {
        auth.add_user(
            name,
            name,
            None,
            crystalline_service::rest::Role::Editor,
            "pw12345678",
        )
        .await
        .unwrap();
    }
    auth.set_domain_visibility("team", true, "owner")
        .await
        .unwrap();
    auth.upsert_domain_member(
        "team",
        "alice",
        crystalline_service::rest::MemberLevel::Editor,
        "owner",
    )
    .await
    .unwrap();
    f.engine
        .set_domain_access(Arc::new(crystalline_service::DomainAccess::new(auth)));
    f
}

/// A draft is not a way around the domain screen. Alice's draft in a private
/// domain reads for her and is the same nothing a stranger gets about every
/// other engram in there - and, crucially, about the domain itself.
#[tokio::test]
async fn a_draft_in_a_hidden_domain_is_invisible_to_a_reader_who_cannot_see_the_domain() {
    let f = screened_fixture().await;
    let alice = account("alice");
    f.engine
        .write_engram_as(
            &write_params("team", "Fresh", "- [idea] a page only alice has #team"),
            Some("claude-code/2.0-for-alice"),
            &alice,
        )
        .await
        .unwrap();
    assert!(
        f.reads("fresh", &alice).await.is_ok(),
        "alice, who is a member, reads her own draft"
    );

    // A draft the stranger holds themselves, written straight into the store
    // the way one left behind by an account whose membership was later
    // withdrawn would be. This is what makes the test sharp: with nothing of
    // their own in there, a stranger asking the overlay question first would
    // still find nothing, and the composition order would be untested.
    let stranger = account("out");
    f.draft("team", "out", "secret.md", ALICE_NEW).await;
    let mine = f
        .reads("fresh", &stranger)
        .await
        .expect_err("a draft of their own is no way back into a domain they may not see");
    assert!(
        mine.contains("no engram 'fresh' in domain 'team'"),
        "and it is the ordinary miss: {mine}"
    );

    // And the two shapes the domain itself holds, because they fail
    // differently: a draft over a base row, and one at a path the domain's
    // files never held.
    for identifier in ["fresh", "plan"] {
        let miss = f
            .reads(identifier, &stranger)
            .await
            .expect_err("a stranger reads nothing in a domain they may not see");
        assert!(
            miss.contains(&format!("no engram '{identifier}' in domain 'team'")),
            "and it is the miss an engram nobody wrote produces: {miss}"
        );
    }
}

/// The other half of the same screen. A domain this instance has no
/// registration for is not an answer, and a draft sitting in one is not an
/// answer either - the rows are left in the index, they simply stop being
/// something a read can reach.
#[tokio::test]
async fn a_draft_in_an_unregistered_domain_is_not_an_answer() {
    let f = review_fixture().await;
    // A domain the index holds and the configuration does not: rows written
    // straight into the store, the way a domain removed from the config leaves
    // its rows behind.
    {
        let store = f.store.lock().await;
        let id = store
            .upsert_domain("ghost", None, DomainKind::Virtual)
            .await
            .unwrap();
        store
            .upsert_engram(id, &record(ALICE_NEW, "fresh.md"))
            .await
            .unwrap();
        store
            .upsert_overlay(id, "alice", &record(ALICE_DRAFT, "plan.md"))
            .await
            .unwrap();
    }

    let alice = account("alice");
    for scope in [&alice, &Scope::Unrestricted] {
        let miss = f
            .engine
            .read_engram(
                &ReadParams {
                    identifier: "plan".to_string(),
                    domain: Some("ghost".to_string()),
                },
                scope,
            )
            .await
            .expect_err("a domain nobody registered answers nothing, drafts included");
        assert!(
            miss.to_string()
                .contains("no engram 'plan' in domain 'ghost'"),
            "and it is the ordinary miss: {miss}"
        );
    }
}

/// An overlay write on a domain the caller may not see is refused as an
/// unregistered one, not with the teaching sentence about review mode.
///
/// The surface gates are in front of the engine and would refuse this first;
/// the point is what the engine says when it is reached anyway, because
/// "this domain reviews changes before they land" is a fact about a domain the
/// caller must not learn exists.
#[tokio::test]
async fn an_overlay_write_on_a_hidden_domain_refuses_as_an_unknown_one() {
    let f = screened_fixture().await;
    let before = f.tree("team");

    let err = f
        .engine
        .write_engram_as(
            &write_params("team", "Fresh", "- [idea] a stranger's page #team"),
            Some("claude-code/2.0-for-out"),
            &account("out"),
        )
        .await
        .expect_err("a stranger's write is refused");
    let text = err.to_string();
    assert!(
        text.contains("domain 'team' not registered"),
        "refused as an unregistered domain: {text}"
    );
    assert!(
        !text.contains("reviews changes before they land"),
        "and never with the sentence that says what kind of domain it is: {text}"
    );
    assert!(
        f.held("team", "out").await.is_empty(),
        "and no draft was written"
    );
    assert_eq!(f.tree("team"), before, "and nothing reached the tree");
}

/// Review mode is a key on a registration, so a domain in review mode is
/// registered by construction and the unregistered half of the screen can
/// never hide one.
///
/// The converse is the half worth pinning: a domain nobody registered reviews
/// nothing, so an unregistered name can never route a write into a draft of a
/// domain that does not exist.
#[tokio::test]
async fn review_mode_implies_a_registration_so_the_collector_never_hides_it() {
    let f = review_fixture().await;
    {
        let store = f.store.lock().await;
        store
            .upsert_domain("ghost", None, DomainKind::Virtual)
            .await
            .unwrap();
    }

    // The registered review-mode domain answers, so it is in neither half of
    // the screen.
    assert!(f.reads("plan", &account("alice")).await.is_ok());

    // The unregistered one reviews nothing: a write there is the unregistered
    // refusal every other verb gives it, never the review-mode sentence and
    // never a draft.
    let err = f
        .engine
        .write_engram_as(
            &write_params("ghost", "Fresh", "- [idea] into a domain nobody has #team"),
            Some("claude-code/2.0-for-alice"),
            &account("alice"),
        )
        .await
        .expect_err("a domain nobody registered takes no write at all");
    let text = err.to_string();
    assert!(
        text.contains("domain 'ghost' not registered"),
        "the ordinary unregistered refusal: {text}"
    );
    assert!(
        !text.contains("reviews changes before they land"),
        "and nothing about review mode: {text}"
    );
}
