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

mod support;

use std::path::PathBuf;
use std::sync::Arc;

use crystalline_core::config::{
    AuthConfig, DomainEntry, GitHubConfig, GlobalConfig, OriginConfig, ResponseFormat, ReviewMode,
    ServiceConfig,
};
use crystalline_core::parse_engram;
use crystalline_index::{DomainKind, EngramRecord, FileStamp, Store, TursoStore};
use crystalline_service::daemon::http_router;
use crystalline_service::engine::ConfigureAction;
use crystalline_service::overlay_journal;
use crystalline_service::params::{DeleteParams, EditParams, ReadParams, WriteParams};
use crystalline_service::rest::{AuthStore, Role};
use crystalline_service::{Engine, Scope, SimilarProbe};
use crystalline_service::{FoldChoice, ReviewModeConfirm};
use tokio::sync::Mutex;
use yrs::sync::{Message, MessageReader, SyncMessage};
use yrs::updates::decoder::{Decode, DecoderV1};
use yrs::updates::encoder::Encode;
use yrs::{Doc, GetString, ReadTxn, Text, Transact, Update};

const MANIFEST: &str = "---\ntype: manifest\ntitle: team\npermalink: manifest\ntags:\n  - manifest\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# team\n\n## Scope\n\n- The shared domain\n\n## When to Use\n\n- Route here for team work\n";
/// The base engram: what the domain's files on disk say exists.
const PLAN: &str = "---\ntype: engram\ntitle: Plan\npermalink: plan\ntags:\n  - team\nstatus: stable\nrecorded_at: 2026-01-02\n---\n\n# Plan\n\n- [decision] the plan as the team has it #team\n";
/// Alice's draft of the same path: her private rewrite of it.
const ALICE_DRAFT: &str = "---\ntype: engram\ntitle: Plan\npermalink: plan\ntags:\n  - team\nstatus: draft\nrecorded_at: 2026-01-03\n---\n\n# Plan\n\n- [decision] the plan as alice would have it #team\n";
/// Two engrams on one topic, for the capture advisory: the draft a receipt is
/// about and the base engram the team already has beside it. The marker words
/// are the ones `support::TopicEmbedder` reads, so "a neighbour appears" is a
/// deterministic fact rather than a hash collision.
const RETRY_BODY: &str = "- [decision] the retry queue doubles its backoff on every failure #team\n- [decision] a dead-letter ttl bounds how long a retry waits #team";
const RETRY_NEIGHBOUR: &str = "---\ntype: engram\ntitle: Retry backoff lesson\npermalink: retry-backoff-lesson\ntags:\n  - team\nstatus: stable\nrecorded_at: 2026-01-02\n---\n\n# Retry backoff lesson\n\n- [decision] retries wait on a backoff that doubles each time #team\n- [decision] the dead-letter ttl is the bound on a stuck retry #team\n";

/// A base engram on the drafts' own topic: what an authenticated search must
/// keep answering with whoever is asking.
const LEDGER: &str = "---\ntype: engram\ntitle: Nightly ledger\npermalink: nightly-ledger\ntags:\n  - team\nstatus: stable\nrecorded_at: 2026-01-02\n---\n\n# Nightly ledger\n\n- [fact] the ledger reconciles nightly #team\n";

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
    /// Where this engine keeps its per-domain origin state, so a test can
    /// record a base snapshot the way a first pull would have.
    origins: PathBuf,
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
    build_fixture(pinned, false, None, false, false).await
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
    build_fixture(true, true, None, false, false).await
}

/// A team domain with a GitHub origin and a base snapshot that says exactly
/// what is on disk: the state enabling review mode insists on. Not in review
/// mode yet - turning it on is what the tests built on this do.
async fn origin_fixture() -> Fixture {
    build_fixture(true, false, None, false, true).await
}

/// The same team domain, already in review mode, for the tests that ask what a
/// domain in review mode reports about its own working tree.
async fn reviewed_origin_fixture() -> Fixture {
    build_fixture(true, true, None, false, true).await
}

/// The review-mode domain with a deterministic embedding provider behind it,
/// which is what the capture advisory needs to find anything at all: with no
/// provider the probe returns early and a test asserting `similar` would be
/// asserting nothing.
async fn review_fixture_with_provider() -> Fixture {
    build_fixture(
        true,
        true,
        Some(Arc::new(support::TopicEmbedder)),
        false,
        false,
    )
    .await
}

/// The review-mode domain served over the production HTTP router with the MCP
/// gate on, which is the only way to ask a search question as somebody in
/// particular: the account is resolved at the door, and everything below it -
/// the scope, the actor, the rows a search is entitled to - follows from that
/// one resolution.
///
/// Hands back the fixture (whose temp directory has to outlive the server), the
/// address and the auth store, so a test can mint a personal token against the
/// very store the gate reads.
async fn served_review_instance() -> (Fixture, std::net::SocketAddr, Arc<AuthStore>) {
    let f = build_fixture(true, true, None, true, false).await;
    let auth = Arc::new(AuthStore::open(&f.root.join("web-auth.db")).await.unwrap());
    let router = http_router(
        f.engine.clone(),
        Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        &[],
        auth.clone(),
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
    (f, addr, auth)
}

async fn build_fixture(
    pinned: bool,
    review: bool,
    provider: Option<Arc<dyn crystalline_index::EmbeddingProvider>>,
    auth: bool,
    origin: bool,
) -> Fixture {
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
    if origin {
        // A team domain: enabling review mode needs one, because review with
        // no proposal flow behind it is a gate with no door. `github.enabled`
        // rides along so `origin_status` answers rather than refusing.
        entry.origin = Some(OriginConfig {
            repo: "acme/team".to_string(),
            path: None,
            branch: Some("main".to_string()),
            poll_secs: None,
        });
        cfg.github = Some(GitHubConfig {
            enabled: Some(true),
            ..GitHubConfig::default()
        });
    }
    cfg.domains.insert("team".to_string(), entry);
    if auth {
        // The MCP gate on, OAuth explicitly off (it would otherwise derive back
        // on), and plain JSON out so an assertion reads the hits rather than
        // the framing.
        cfg.auth = Some(AuthConfig {
            mcp: Some(true),
            oauth: Some(false),
            ..AuthConfig::default()
        });
        cfg.service = Some(ServiceConfig {
            response_format: Some(ResponseFormat::Json),
            ..ServiceConfig::default()
        });
    }
    let config_path = root.join("config.yaml");
    crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();

    let state = root.join("state");
    let store: Arc<Mutex<dyn Store>> =
        Arc::new(Mutex::new(TursoStore::open_in_memory().await.unwrap()));
    let engine = Engine::new(store.clone(), cfg, provider, Some(config_path));
    let engine = if pinned {
        engine.with_state_dir(state.clone())
    } else {
        engine
    };
    let origins = root.join("origins");
    let mut engine = engine.with_origins_dir(origins.clone());
    // The forge is injected wherever this fixture carries an origin, so no
    // status read here ever reaches this machine's own keychain or the network.
    let mut commit = String::new();
    if origin {
        let mock = Arc::new(support::MockProvider::new());
        commit = mock.add_commit(std::collections::BTreeMap::from([(
            "MANIFEST.md".to_string(),
            MANIFEST.as_bytes().to_vec(),
        )]));
        mock.set_branch("main", &commit);
        engine = engine.with_origin_provider(mock);
    }
    let engine = Arc::new(engine);
    engine.sync(None).await.unwrap();
    let f = Fixture {
        _tmp: tmp,
        root,
        engine,
        store,
        state,
        origins,
    };
    if origin {
        f.snapshot_origin_at("team", &commit);
    }
    f
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
        // A tombstone answers to no address, so its permalink column carries
        // the row's own identity in this actor's dimension: its path. That is
        // what `overlay_journal::tombstone_record` and `write_overlay_tombstone`
        // both write, and a helper that wrote anything else would let a test
        // pass against a shape no verb produces (Task 4's deviation 3).
        rec.permalink = path.to_string();
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

    /// Record the domain's working tree as its origin base snapshot, the way a
    /// first pull would have: every file it holds, at the bytes it holds them
    /// at. A domain whose snapshot says exactly what is on disk has nothing
    /// unshared, which is the state enabling review mode insists on.
    fn snapshot_origin(&self, domain: &str) {
        let commit = std::fs::read_to_string(self.origins.join(domain).join("state.json"))
            .ok()
            .and_then(|text| {
                serde_json::from_str::<serde_json::Value>(&text).ok()?["base_commit"]
                    .as_str()
                    .map(str::to_string)
            })
            .unwrap_or_default();
        self.snapshot_origin_at(domain, &commit);
    }

    /// The same, at an explicit base commit - what the first snapshot uses,
    /// since there is no earlier state to read the commit back out of.
    fn snapshot_origin_at(&self, domain: &str, commit: &str) {
        let root = self.domain_root(domain);
        let mut state = crystalline_remote::state::OriginState::new("acme/team", "main");
        state.base_commit = commit.to_string();
        for entry in walkdir::WalkDir::new(&root).into_iter().flatten() {
            if !entry.file_type().is_file() {
                continue;
            }
            let bytes = std::fs::read(entry.path()).unwrap();
            let rel = entry
                .path()
                .strip_prefix(&root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            state.files.insert(
                rel,
                crystalline_remote::state::BaseStamp {
                    sha256: support::sha256_hex(&bytes),
                    size: bytes.len() as u64,
                },
            );
        }
        state.save(&self.origins.join(domain)).unwrap();
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
/// it is a draft, and the capture advisory that rides on a write receipt finds
/// the neighbours it would have found.
///
/// The provider is the whole reason this test can assert anything: without one
/// `attach_similar` returns before it probes, and a test that only re-checked
/// `draft` after the call would pass against a probe that never ran.
///
/// Two shapes, because they reach the title differently and one of them was
/// broken: a create carries its own title in the probe, while an edit looks the
/// title up - and for an engram that exists only as this actor's draft there is
/// no base row to look it up in.
#[tokio::test]
async fn a_write_receipt_says_draft_and_still_carries_similar() {
    let f = review_fixture_with_provider().await;
    let alice = account("alice");
    let who = Some("claude-code/2.0-for-alice");
    // The neighbour the advisory should find: a base engram the team has, on
    // the same topic. Base rather than a second draft because search is the
    // base dimension until Task 5 threads the actor through it, which is
    // exactly what makes this the honest shape to assert today.
    std::fs::write(
        f.domain_root("team").join("retry-backoff.md"),
        RETRY_NEIGHBOUR,
    )
    .unwrap();
    f.engine.sync(None).await.unwrap();

    let mut created = f
        .engine
        .write_engram_as(
            &write_params("team", "Retry queue gotcha", RETRY_BODY),
            who,
            &alice,
        )
        .await
        .unwrap();
    assert_eq!(
        created["draft"],
        serde_json::json!(true),
        "the receipt says where the write landed: {created}"
    );

    // Drained here rather than waited on: with no embed worker wired there is
    // nothing listening for the write's nudge, so the pass is run directly.
    f.engine.embed_pending().await.unwrap();

    // `attach_similar` is what the MCP and REST surfaces hang on a write
    // receipt; the engine verb hands the receipt back and the surface decorates
    // it, so the probe runs here exactly as it does there.
    f.engine
        .attach_similar(
            &mut created,
            SimilarProbe::Write {
                title: "Retry queue gotcha",
                description: None,
                body: RETRY_BODY,
            },
            &alice,
        )
        .await;
    let neighbours: Vec<&str> = created["similar"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .map(|r| r["permalink"].as_str().unwrap_or_default())
                .collect()
        })
        .unwrap_or_default();
    assert!(
        neighbours.contains(&"retry-backoff-lesson"),
        "the advisory found the neighbour the team already has: {created}"
    );
    assert_eq!(
        created["draft"],
        serde_json::json!(true),
        "and it left the receipt saying the write is a draft: {created}"
    );

    // An EDIT of an engram that exists only as alice's draft. The probe has to
    // read its title, and a base-row lookup finds nothing there - which used to
    // skip the advisory without a word.
    let mut edited = f
        .engine
        .edit_engram_as(
            &EditParams {
                identifier: "retry-queue-gotcha".to_string(),
                domain: "team".to_string(),
                operation: "append".to_string(),
                content: Some("- [decision] raising the ttl drained the queue #team".to_string()),
                key: None,
                value: None,
                find_text: None,
                expected_replacements: None,
                section: None,
                include_subsections: false,
                expected_checksum: None,
                ack_scope: None,
            },
            who,
            &alice,
        )
        .await
        .unwrap();
    assert_eq!(edited["draft"], serde_json::json!(true), "{edited}");
    f.engine.embed_pending().await.unwrap();
    f.engine
        .attach_similar(
            &mut edited,
            SimilarProbe::Edit {
                // Over the probe's own eighty-character floor with the title in
                // front of it, or the advisory would be skipped for a reason
                // that has nothing to do with the draft.
                new_text: "raising the dead-letter ttl drained the retry queue and the backoff stopped doubling past its cap",
            },
            &alice,
        )
        .await;
    let after: Vec<&str> = edited["similar"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .map(|r| r["permalink"].as_str().unwrap_or_default())
                .collect()
        })
        .unwrap_or_default();
    assert!(
        after.contains(&"retry-backoff-lesson"),
        "an edit of a draft-only engram probes like any other edit: {edited}"
    );

    // And the other half, which needed the actor threaded into search: a draft
    // finding another draft. Both of these exist only in alice's overlay - no
    // file on disk is about docking clamps at all - so a base-only advisory has
    // nothing to answer with, which is exactly what a writer in review mode
    // would have been told about every neighbour they had.
    let probe = |session: &Scope, title: &str, body: &str| {
        let engine = f.engine.clone();
        let session = session.clone();
        let title = title.to_string();
        let body = body.to_string();
        async move {
            let mut receipt = engine
                .write_engram_as(&write_params("team", &title, &body), who, &session)
                .await
                .unwrap();
            engine.embed_pending().await.unwrap();
            engine
                .attach_similar(
                    &mut receipt,
                    SimilarProbe::Write {
                        title: &title,
                        description: None,
                        body: &body,
                    },
                    &session,
                )
                .await;
            receipt
        }
    };
    probe(
        &alice,
        "Docking clamps",
        "- [decision] the docking clamp holds the bay through the thrust burn #team",
    )
    .await;
    let second = probe(
        &alice,
        "Clamp seating",
        "- [decision] the clamps seat before thrust and the bay reports it #team",
    )
    .await;
    let alices: Vec<&str> = second["similar"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .map(|r| r["permalink"].as_str().unwrap_or_default())
                .collect()
        })
        .unwrap_or_default();
    assert!(
        alices.contains(&"docking-clamps"),
        "one draft is a neighbour of the next, for their own author: {second}"
    );

    // And for nobody else. Bob writes on the same topic and is told nothing
    // about what alice is drafting.
    let bobs_receipt = probe(
        &account("bob"),
        "Clamp inspection",
        "- [decision] the bay clamps are inspected after every thrust test #team",
    )
    .await;
    let bobs: Vec<&str> = bobs_receipt["similar"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .map(|r| r["permalink"].as_str().unwrap_or_default())
                .collect()
        })
        .unwrap_or_default();
    assert!(
        !bobs
            .iter()
            .any(|p| p.starts_with("docking-clamps") || p.starts_with("clamp-seating")),
        "and another actor's drafts are no part of his advisory: {bobs_receipt}"
    );
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

    // The same question through the two graph verbs, anchored at the
    // stranger's OWN draft, because that is the anchor the screen has to
    // survive: the base lookup finds nothing there whoever asks, so a verb
    // that asked whose draft it is before asking whether the domain is
    // readable would resolve the anchor out of the stranger's own overlay and
    // answer with a slice. Visibility is a property of the reader on every
    // read surface, not on the ones that happen to have a base row to miss on.
    let anchor = "crystalline://team/fresh";
    let context = async |scope: Scope| {
        f.engine
            .build_context(
                &crystalline_service::params::ContextParams {
                    anchor: anchor.to_string(),
                    depth: Some(1),
                    domains: Vec::new(),
                    timeframe: None,
                    max_related: None,
                },
                &scope,
            )
            .await
    };
    let graph = async |scope: Scope| f.engine.graph_neighborhood(anchor, 1, 50, &scope).await;

    // Reachable for the member whose draft it is, so the misses below are the
    // screen answering rather than an anchor that was never there.
    assert!(
        context(alice.clone()).await.is_ok(),
        "alice, who is a member, anchors on her own draft"
    );
    assert!(graph(alice).await.is_ok(), "and draws it as a graph");

    for (verb, answer) in [
        ("build_context", context(stranger.clone()).await),
        ("graph_neighborhood", graph(stranger).await),
    ] {
        let miss = answer.err().unwrap_or_else(|| {
            panic!("{verb} answered a stranger about a domain they may not see")
        });
        assert!(
            miss.to_string()
                .contains("no engram 'fresh' in domain 'team'"),
            "{verb} gives the miss an engram nobody wrote produces, never an \
             empty slice: {miss}"
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

/// The tree an agent navigates by is this reader's tree: their drafts describe
/// the rows they are drafting, their deletions leave it, and a draft at a path
/// the domain's files never held is in it - folder and all.
#[tokio::test]
async fn a_browse_level_is_drawn_from_the_readers_own_drafts() {
    let f = review_fixture().await;
    let alice = account("alice");
    let who = Some("claude-code/2.0-for-alice");

    // A draft over the base row, retitled, and a draft in a folder nobody has.
    f.engine
        .save_engram(
            &crystalline_service::params::SaveParams {
                domain: "team".to_string(),
                identifier: "plan".to_string(),
                content: PLAN.replace("title: Plan", "title: Alice's Plan"),
                expected_checksum:
                    f.engine.read_engram(&read("plan"), &alice).await.unwrap()["checksum"]
                        .as_str()
                        .unwrap()
                        .to_string(),
            },
            &alice,
        )
        .await
        .unwrap();
    f.engine
        .write_engram_as(
            &WriteParams {
                folder: Some("notes".to_string()),
                ..write_params("team", "Fresh", "- [idea] a page only alice has #team")
            },
            who,
            &alice,
        )
        .await
        .unwrap();

    let browse = |scope: Scope| {
        let engine = f.engine.clone();
        async move {
            engine
                .browse_domain(
                    &crystalline_service::params::BrowseParams {
                        domain: "team".to_string(),
                        path: None,
                        depth: None,
                        glob: None,
                    },
                    &scope,
                )
                .await
                .unwrap()
        }
    };

    let mine = browse(alice.clone()).await;
    let titles: Vec<&str> = mine["engrams"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["title"].as_str().unwrap())
        .collect();
    assert!(
        titles.contains(&"Alice's Plan"),
        "her draft describes the row she is drafting: {mine}"
    );
    assert_eq!(
        mine["folders"],
        serde_json::json!(["notes"]),
        "and the folder her new draft sits in is in her tree: {mine}"
    );

    let theirs = browse(account("bob")).await;
    let their_titles: Vec<&str> = theirs["engrams"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["title"].as_str().unwrap())
        .collect();
    assert!(
        their_titles.contains(&"Plan") && !their_titles.contains(&"Alice's Plan"),
        "bob's tree is the one the team reviewed: {theirs}"
    );
    assert_eq!(
        theirs["folders"],
        serde_json::json!([]),
        "and it has no folder alice invented: {theirs}"
    );

    // A deletion leaves the level, for its author alone.
    f.engine
        .delete_engram_as(
            &DeleteParams {
                identifier: "plan".to_string(),
                domain: "team".to_string(),
                expected_checksum: None,
            },
            who,
            &alice,
        )
        .await
        .unwrap();
    let after = browse(alice).await;
    let paths: Vec<&str> = after["engrams"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["path"].as_str().unwrap())
        .collect();
    assert!(
        !paths.contains(&"plan.md"),
        "the path she deleted is not in her tree: {after}"
    );
    assert_eq!(
        after["total"],
        serde_json::json!(paths.len()),
        "and the level's own count says the same: {after}"
    );
    assert!(
        browse(account("bob")).await["engrams"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["path"] == "plan.md"),
        "while it is still in everybody else's"
    );
}

// --- a mirror that fails never unsays a row that landed ---------------------

/// A second base engram, so a test can move one path and delete another
/// without the two getting in each other's way.
const NOTES: &str = "---\ntype: engram\ntitle: Notes\npermalink: notes\ntags:\n  - team\nstatus: stable\nrecorded_at: 2026-01-02\n---\n\n# Notes\n\n- [decision] the notes as the team has them #team\n";

/// An engram with enough observations to split one out of.
const RICH_BODY: &str = "- [decision] the first thing #team\n- [decision] the second thing #team\n- [idea] the third thing #team\n- [idea] the fourth thing #team";

/// A file where the domain's journal folder belongs, so every mirror write
/// under it fails. Deterministic, and portable in a way a `chmod` is not: root
/// ignores a mode and Windows has no such mode, while nothing anywhere can
/// create a folder inside a file.
fn break_the_mirror(state: &std::path::Path, domain: &str) {
    std::fs::create_dir_all(state.join("overlays")).unwrap();
    std::fs::write(state.join("overlays").join(domain), b"not a folder").unwrap();
}

/// The invariant a routed write cannot be trusted without: **a row that landed
/// is never reported as unwritten.**
///
/// The journal is written after the row, so a mirror that fails leaves a draft
/// in the index with no copy the next `reindex --wipe` could bring back. That
/// is worth saying out loud and it is not worth unsaying the write for: a verb
/// that reported failure would have three callers act on a lie - a split would
/// delete the engram holding the moved observations, a move's rollback would
/// undo a move that happened, and a delete would be unretryable because the
/// tombstone it claims it did not write is there.
///
/// So the row's success is the answer and the mirror's failure rides along as a
/// warning, on the receipt and in the log. Four verbs here, one each for the
/// two row shapes a draft has: create and edit write a draft, move writes a
/// tombstone and an entry, delete writes a tombstone.
#[tokio::test]
async fn a_mirror_that_fails_never_unsays_a_draft_that_landed() {
    let f = review_fixture().await;
    std::fs::write(f.domain_root("team").join("notes.md"), NOTES).unwrap();
    f.engine.sync(None).await.unwrap();
    let alice = account("alice");
    let who = Some("claude-code/2.0-for-alice");
    break_the_mirror(&f.state, "team");

    let warns = |receipt: &serde_json::Value, path: &str| {
        let text = receipt["draft_warning"]
            .as_str()
            .unwrap_or_else(|| panic!("the receipt carries the mirror's failure: {receipt}"))
            .to_string();
        assert!(
            text.contains(path) && text.contains("reindex --wipe"),
            "and the warning names the draft and what it costs: {text}"
        );
    };

    // -- create --
    let created = f
        .engine
        .write_engram_as(
            &write_params("team", "Fresh", "- [idea] a page only alice has #team"),
            who,
            &alice,
        )
        .await
        .expect("a draft whose mirror failed still landed");
    assert_eq!(created["draft"], serde_json::json!(true));
    warns(&created, "fresh.md");

    // -- edit --
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
            who,
            &alice,
        )
        .await
        .expect("an edit whose mirror failed still landed");
    warns(&edited, "plan.md");

    // -- move: a tombstone at the source and an entry at the destination --
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
        .expect("a move whose mirror failed still landed");
    warns(&moved, "archive/plan.md");

    // -- delete --
    let deleted = f
        .engine
        .delete_engram_as(
            &DeleteParams {
                identifier: "notes".to_string(),
                domain: "team".to_string(),
                expected_checksum: None,
            },
            who,
            &alice,
        )
        .await
        .expect("a deletion whose mirror failed still landed");
    assert_eq!(deleted["deleted"], serde_json::json!(true));
    warns(&deleted, "notes.md");

    // Every row is where the receipts said it is.
    let held = f.held("team", "alice").await;
    let shape: Vec<(&str, bool)> = held
        .iter()
        .map(|(path, _, tomb)| (path.as_str(), *tomb))
        .collect();
    assert_eq!(
        shape,
        vec![
            ("archive/plan.md", false),
            ("fresh.md", false),
            ("notes.md", true),
            ("plan.md", true),
        ],
        "four writes, four rows: {held:?}"
    );
    assert!(
        f.reads("notes", &alice).await.is_err() && f.reads("plan", &alice).await.is_err(),
        "and the two deletions are deletions for her"
    );

    // And nothing at all was mirrored, which the journal says rather than
    // reporting an empty one as nobody drafting.
    let mirrored = overlay_journal::journal_entries(&f.state, "team");
    assert!(mirrored.entries.is_empty());
    assert!(
        mirrored.unreadable,
        "the journal says it could not be read rather than that nobody is drafting"
    );
}

/// The one path in review mode that can lose a user's content, pinned: a split
/// whose source edit reports failure makes the split take the new engram back,
/// and the moved observations are then in neither engram.
///
/// The mirror is broken for the SOURCE's own entry alone, so the new engram's
/// write succeeds and the rollback is reachable - which a mirror broken for the
/// whole domain would not be, since the split would fail at its first write.
#[tokio::test]
async fn a_split_whose_mirror_fails_never_takes_the_new_engram_back() {
    let f = review_fixture().await;
    let alice = account("alice");
    let who = Some("claude-code/2.0-for-alice");

    f.engine
        .write_engram_as(&write_params("team", "Rich", RICH_BODY), who, &alice)
        .await
        .unwrap();
    // A folder where the source draft's mirror file belongs: this one entry
    // cannot be written and every other one can.
    let mirror = f.state.join("overlays/team/alice/rich.md");
    std::fs::remove_file(&mirror).unwrap();
    std::fs::create_dir_all(&mirror).unwrap();

    let source = f.engine.read_engram(&read("rich"), &alice).await.unwrap();
    let lines: Vec<usize> = source["observations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["line"].as_u64().unwrap() as usize)
        .take(1)
        .collect();
    let checksum = source["checksum"].as_str().unwrap().to_string();

    let split = f
        .engine
        .split_engram_as(
            &crystalline_service::params::SplitParams {
                domain: "team".to_string(),
                identifier: "rich".to_string(),
                title: "The Split Out Part".to_string(),
                folder: None,
                observations: lines,
                sections: Vec::new(),
                expected_checksum: Some(checksum),
            },
            who,
            &alice,
        )
        .await
        .expect("a split whose source mirror failed still landed both engrams");
    assert_eq!(
        split["draft"],
        serde_json::json!(true),
        "and its receipt says the split landed in a draft: {split}"
    );

    // Both engrams are alice's, and the moved observations are in exactly one
    // of them - which is the whole point: taking the new one back would have
    // left them in neither.
    let new = f
        .reads("the-split-out-part", &alice)
        .await
        .expect("the new engram is still there");
    assert!(
        new.contains("the first thing"),
        "the moved observation is in the new engram: {new}"
    );
    let rest = f.reads("rich", &alice).await.unwrap();
    assert!(
        !rest.contains("the first thing") && rest.contains("the second thing"),
        "and out of the source, which kept the rest: {rest}"
    );
}

/// Split is the fifth write verb, and in review mode both of its writes are
/// drafts: the engram it creates and the source it edits. Its receipt says so,
/// the tree does not move, and both rows are the splitter's alone.
#[tokio::test]
async fn a_split_in_review_mode_lands_both_engrams_as_drafts() {
    let f = review_fixture().await;
    let before = f.tree("team");
    let alice = account("alice");
    let who = Some("claude-code/2.0-for-alice");

    f.engine
        .write_engram_as(&write_params("team", "Rich", RICH_BODY), who, &alice)
        .await
        .unwrap();
    let source = f.engine.read_engram(&read("rich"), &alice).await.unwrap();
    let lines: Vec<usize> = source["observations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["line"].as_u64().unwrap() as usize)
        .take(1)
        .collect();

    let split = f
        .engine
        .split_engram_as(
            &crystalline_service::params::SplitParams {
                domain: "team".to_string(),
                identifier: "rich".to_string(),
                title: "The Split Out Part".to_string(),
                folder: None,
                observations: lines,
                sections: Vec::new(),
                expected_checksum: Some(source["checksum"].as_str().unwrap().to_string()),
            },
            who,
            &alice,
        )
        .await
        .unwrap();
    assert_eq!(
        split["draft"],
        serde_json::json!(true),
        "the receipt says the split landed in a draft: {split}"
    );
    assert!(
        split.get("draft_warning").is_none(),
        "and says nothing about a mirror, because both were mirrored: {split}"
    );

    assert_eq!(f.tree("team"), before, "neither write reached the tree");
    let held = f.held("team", "alice").await;
    let paths: Vec<&str> = held.iter().map(|(p, _, _)| p.as_str()).collect();
    assert_eq!(
        paths,
        vec!["rich.md", "the-split-out-part.md"],
        "both engrams are alice's own drafts: {held:?}"
    );
    assert!(
        f.reads("the-split-out-part", &account("bob"))
            .await
            .is_err(),
        "and nobody else has either of them"
    );
    let mirrored: Vec<String> = overlay_journal::journal_entries(&f.state, "team")
        .entries
        .into_iter()
        .map(|e| e.path)
        .collect();
    assert_eq!(
        mirrored,
        vec!["rich.md".to_string(), "the-split-out-part.md".to_string()],
        "and both are mirrored"
    );
}

/// Catching up is catching up on YOUR work. An agent that has just written a
/// draft asks `recent_activity` and has to see it there; the row it shadows is
/// the team's answer, not this reader's, and a path this reader deleted is not
/// recent activity for them at all.
#[tokio::test]
async fn recent_activity_lists_the_readers_own_drafts_and_not_the_rows_they_shadow() {
    let f = review_fixture().await;
    std::fs::write(f.domain_root("team").join("notes.md"), NOTES).unwrap();
    f.engine.sync(None).await.unwrap();
    let alice = account("alice");
    let who = Some("claude-code/2.0-for-alice");

    // A draft over a base row, retitled; a draft at a path no file holds; and
    // a deletion of a second base row.
    let read_plan = f.engine.read_engram(&read("plan"), &alice).await.unwrap();
    f.engine
        .save_engram(
            &crystalline_service::params::SaveParams {
                domain: "team".to_string(),
                identifier: "plan".to_string(),
                content: PLAN.replace("title: Plan", "title: Alice's Plan"),
                expected_checksum: read_plan["checksum"].as_str().unwrap().to_string(),
            },
            &alice,
        )
        .await
        .unwrap();
    f.engine
        .write_engram_as(
            &write_params("team", "Fresh", "- [idea] a page only alice has #team"),
            who,
            &alice,
        )
        .await
        .unwrap();
    f.engine
        .delete_engram_as(
            &DeleteParams {
                identifier: "notes".to_string(),
                domain: "team".to_string(),
                expected_checksum: None,
            },
            who,
            &alice,
        )
        .await
        .unwrap();

    let recent = |scope: Scope| {
        let engine = f.engine.clone();
        async move {
            let answer = engine
                .recent_activity(
                    &crystalline_service::params::RecentParams {
                        domains: vec!["team".to_string()],
                        // Wide enough to reach the base rows the fixture's
                        // files carry, which are dated rather than written now.
                        timeframe: Some("10y".to_string()),
                        types: Vec::new(),
                    },
                    &scope,
                )
                .await
                .unwrap();
            let titles: Vec<String> = answer["engrams"]
                .as_array()
                .unwrap()
                .iter()
                .map(|e| e["title"].as_str().unwrap_or_default().to_string())
                .collect();
            (answer, titles)
        }
    };

    let (mine, my_titles) = recent(alice.clone()).await;
    assert!(
        my_titles.contains(&"Alice's Plan".to_string()) && !my_titles.contains(&"Plan".to_string()),
        "her draft is the row, and the one it shadows is not beside it: {mine}"
    );
    assert!(
        my_titles.contains(&"Fresh".to_string()),
        "the page only she has is in her catch-up: {mine}"
    );
    assert!(
        !my_titles.contains(&"Notes".to_string()),
        "and the one she deleted is not: {mine}"
    );
    assert_eq!(
        mine["count"],
        serde_json::json!(my_titles.len()),
        "the count is the count of what came back: {mine}"
    );

    let (theirs, their_titles) = recent(account("bob")).await;
    assert!(
        their_titles.contains(&"Plan".to_string())
            && their_titles.contains(&"Notes".to_string())
            && !their_titles.contains(&"Alice's Plan".to_string())
            && !their_titles.contains(&"Fresh".to_string()),
        "everybody else catches up on what the team reviewed: {theirs}"
    );
}

/// A deletion is a deletion however the engram was addressed. An identifier
/// that names no domain resolves across base rows, and once one has resolved
/// the domain is known - so the reader's own tombstone over it is one lookup
/// away and is honoured.
///
/// The draft half stays conditional on naming a domain, and the test says so
/// rather than leaving a reader to find out: an engram that exists only as a
/// draft is reached by naming its domain.
#[tokio::test]
async fn a_tombstone_is_honoured_for_an_identifier_that_names_no_domain() {
    let f = review_fixture().await;
    let alice = account("alice");
    let bare = |identifier: &str| ReadParams {
        identifier: identifier.to_string(),
        domain: None,
    };

    assert!(
        f.engine.read_engram(&bare("plan"), &alice).await.is_ok(),
        "before the deletion she reads it unnamed like anybody else"
    );
    f.engine
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

    let miss = f
        .engine
        .read_engram(&bare("plan"), &alice)
        .await
        .expect_err("her deletion holds for an identifier that names no domain");
    assert!(
        miss.to_string().contains("no engram matches 'plan'"),
        "in the words a bare identifier nobody wrote produces: {miss}"
    );
    assert!(
        f.engine
            .read_engram(&bare("plan"), &account("bob"))
            .await
            .is_ok(),
        "and it is still there for everybody else"
    );
}

/// **An authenticated search answers out of the caller's own drafts, and out of
/// nobody else's.**
///
/// Driven through the real transport rather than at the engine seam, because
/// the whole claim is about an identity: the account is resolved once at the
/// door, `overlay_actor` reads it off the scope that resolution produced, and
/// the store's actor screen reads that. Two accounts on one domain in review
/// mode is the shape that fails if any link in that chain answers with the
/// machine owner instead of the caller.
///
/// Three things are asserted for each of the two accounts, because a predicate
/// that got one of them wrong would still look right from the other two: the
/// caller's own draft is in the answer, the other caller's draft is not, and
/// the engram the team actually reviewed is in both.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_authenticated_search_finds_the_callers_draft_and_nobody_elses() {
    let (f, addr, auth) = served_review_instance().await;
    // A base engram the team has, on the same topic as the drafts, so the
    // answer has something in it that is nobody's draft.
    std::fs::write(f.domain_root("team").join("ledger.md"), LEDGER).unwrap();
    f.engine.sync(None).await.unwrap();

    for who in ["alice", "bob"] {
        auth.add_user(who, who, None, Role::Editor, "pw12345678")
            .await
            .unwrap();
    }
    let alice = support::McpTestSession::open(
        &addr,
        Some(&auth.issue_mcp_token("alice", "t").await.unwrap().token),
    )
    .await;
    let bob = support::McpTestSession::open(
        &addr,
        Some(&auth.issue_mcp_token("bob", "t").await.unwrap().token),
    )
    .await;

    // Each writes into the domain, and review mode routes each write into its
    // own author's draft.
    for (session, title, body) in [
        (
            &alice,
            "Ledger rota",
            "- [decision] alice takes the ledger rota next quarter #team",
        ),
        (
            &bob,
            "Ledger freeze",
            "- [decision] bob freezes the ledger before the audit #team",
        ),
    ] {
        let receipt = session
            .call_tool(
                "write_engram",
                serde_json::json!({
                    "domain": "team",
                    "title": title,
                    "content": body,
                }),
            )
            .await;
        // The receipt rides inside an SSE frame, so the JSON arrives escaped.
        assert!(
            receipt.contains(r#"\"draft\":true"#),
            "the write landed in a draft: {receipt}"
        );
    }

    let search = serde_json::json!({ "query": "ledger", "domains": ["team"] });
    let hers = alice.call_tool("search_engrams", search.clone()).await;
    assert!(
        hers.contains("ledger-rota"),
        "alice's search finds alice's draft: {hers}"
    );
    assert!(!hers.contains("ledger-freeze"), "and never bob's: {hers}");
    assert!(
        hers.contains("nightly-ledger"),
        "and the engram the team reviewed is still in it: {hers}"
    );

    let his = bob.call_tool("search_engrams", search).await;
    assert!(
        his.contains("ledger-freeze"),
        "bob's search finds bob's draft: {his}"
    );
    assert!(!his.contains("ledger-rota"), "and never alice's: {his}");
    assert!(
        his.contains("nightly-ledger"),
        "and the engram the team reviewed is in his answer too: {his}"
    );
}

/// **A draft never takes a permalink another path already holds.**
///
/// The overlay dimension relaxes what the index enforces: the unique index is
/// `(domain, permalink, actor)`, so a draft and a base row at a *different*
/// path can share one address and the database says nothing. Nothing
/// downstream can carry that state. A search merges its hits by permalink and
/// would drop one of the two without saying so, and a draft holding an address
/// the reviewed folder already spends could never be folded back into it,
/// since the base rows do refuse it there. So the write path keeps the rule
/// the import path keeps, in the words the import path uses.
///
/// Three verbs, because a permalink reaches a draft by three different routes:
/// a save takes it from the document verbatim, a create derives it from where
/// the engram lands, and a move carries the one the draft already has to a new
/// path. The move is the sharp one - it writes twice, so a refusal that came
/// too late would have vacated the source already, and a draft lives in its
/// row and nowhere else.
#[tokio::test]
async fn a_draft_never_takes_a_permalink_another_path_holds() {
    let f = review_fixture().await;
    let alice = account("alice");
    let who = Some("claude-code/2.0-for-alice");
    // Two more engrams the team reviewed: one whose path and permalink agree,
    // and one whose path and permalink do not, which is the shape a create can
    // collide with (its own address is always its path).
    std::fs::write(f.domain_root("team").join("notes.md"), NOTES).unwrap();
    std::fs::create_dir_all(f.domain_root("team").join("docs")).unwrap();
    std::fs::write(f.domain_root("team").join("docs").join("ledger.md"), LEDGER).unwrap();
    f.engine.sync(None).await.unwrap();

    // -- save: the document is written verbatim, so the permalink line in it is
    //    the address the row takes, and the path does not move --
    let read = f.engine.read_engram(&read("notes"), &alice).await.unwrap();
    let stolen = f
        .engine
        .save_engram(
            &crystalline_service::params::SaveParams {
                domain: "team".to_string(),
                identifier: "notes".to_string(),
                content: NOTES.replace("permalink: notes", "permalink: plan"),
                expected_checksum: read["checksum"].as_str().unwrap().to_string(),
            },
            &alice,
        )
        .await
        .expect_err("a save may not point her draft of one path at another path's address");
    assert!(
        stolen
            .to_string()
            .contains("permalink 'plan' already exists at another path")
            && stolen.to_string().contains("plan.md"),
        "and the refusal names the path that holds it: {stolen}"
    );
    assert!(
        f.held("team", "alice").await.is_empty(),
        "a refused save leaves her holding no draft at all"
    );

    // -- create: the address is where the engram lands, and an engram the team
    //    keeps somewhere else can already answer to it. `overwrite` is a
    //    same-path decision, so it is no way past this --
    let taken = f
        .engine
        .write_engram_as(
            &WriteParams {
                overwrite: true,
                ..write_params("team", "Nightly ledger", "- [idea] a second ledger #team")
            },
            who,
            &alice,
        )
        .await
        .expect_err("a create may not land on an address the team's folder already spends");
    assert!(
        taken
            .to_string()
            .contains("permalink 'nightly-ledger' already exists at another path")
            && taken.to_string().contains("docs/ledger.md"),
        "naming where it is held, not just that it is: {taken}"
    );

    // -- and the addresses stayed unique, which is what the refusals are for:
    //    a search answers with each one exactly once --
    let hits = f
        .engine
        .search_engrams(
            &crystalline_service::params::SearchParams {
                domains: vec!["team".to_string()],
                ..Default::default()
            },
            &alice,
        )
        .await
        .unwrap();
    let mut addresses: Vec<String> = hits["hits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|hit| hit["permalink"].as_str().unwrap_or_default().to_string())
        .collect();
    addresses.sort();
    let unique = {
        let mut seen = addresses.clone();
        seen.dedup();
        seen
    };
    assert_eq!(
        addresses, unique,
        "every address in her own search answers once: {addresses:?}"
    );
    assert!(
        addresses.contains(&"plan".to_string())
            && addresses.contains(&"nightly-ledger".to_string()),
        "both contested addresses are in it, each held by the engram the team reviewed: {addresses:?}"
    );

    // -- move: the draft carries its address to the new path, and the team can
    //    have taken that address in the meantime --
    f.engine
        .write_engram_as(
            &write_params("team", "Rota", "- [decision] alice takes the rota #team"),
            who,
            &alice,
        )
        .await
        .expect("nothing holds that address yet, so her draft may have it");
    std::fs::write(
        f.domain_root("team").join("other.md"),
        NOTES
            .replace("permalink: notes", "permalink: rota")
            .replace("Notes", "Rota"),
    )
    .unwrap();
    f.engine.sync(None).await.unwrap();

    let moved = f
        .engine
        .move_engram(
            &crystalline_service::params::MoveParams {
                // By path: the address itself now resolves to the engram the
                // team put at it, which is the collision seen from the other
                // side, and her draft answers to its own path whoever else
                // holds the address.
                identifier: "rota.md".to_string(),
                domain: "team".to_string(),
                destination: "archive/rota.md".to_string(),
                destination_domain: None,
                update_links: None,
            },
            &alice,
        )
        .await
        .expect_err(
            "the team took that address while she was drafting, so the move has nowhere to land",
        );
    assert!(
        moved
            .to_string()
            .contains("permalink 'rota' already exists at another path")
            && moved.to_string().contains("other.md"),
        "naming the engram that now holds it: {moved}"
    );
    let held = f.held("team", "alice").await;
    let shape: Vec<(&str, bool)> = held
        .iter()
        .map(|(path, _, tomb)| (path.as_str(), *tomb))
        .collect();
    assert_eq!(
        shape,
        vec![("rota.md", false)],
        "and a move that cannot land writes nothing: her draft is where it was, not a tombstone \
         over a draft that is gone: {held:?}"
    );
}

// --- Task 6: the actor reaches the graph ------------------------------------

/// A draft's relation is in its author's own neighbourhood and in nobody
/// else's, seeded from either end.
///
/// The graph is where a leak is hardest to see and hardest to undo: an edge is
/// a fact about the engram at each of its ends, so a frontier that walked
/// somebody else's draft would tell a reader that a private page exists AND
/// pull the team's engrams into a neighbourhood through it. Seeded from both
/// ends because a screen applied at one endpoint only passes the test from the
/// side it screens.
#[tokio::test]
async fn a_drafts_relation_reaches_its_authors_context_and_nobody_elses() {
    let f = review_fixture().await;
    let alice = account("alice");
    let bob = account("bob");

    for (scope, title, body) in [
        (
            &alice,
            "Fresh",
            "- [idea] a page only alice has #team\n\n- relates_to [[Plan]]",
        ),
        (
            &bob,
            "Bobs idea",
            "- [idea] a page only bob has #team\n\n- relates_to [[Plan]]",
        ),
    ] {
        let receipt = f
            .engine
            .write_engram_as(&write_params("team", title, body), None, scope)
            .await
            .unwrap();
        assert_eq!(receipt["draft"], serde_json::json!(true), "{receipt}");
    }

    let context = async |anchor: &str, scope: &Scope| {
        f.engine
            .build_context(
                &crystalline_service::params::ContextParams {
                    anchor: anchor.to_string(),
                    depth: Some(1),
                    domains: Vec::new(),
                    timeframe: None,
                    max_related: None,
                },
                scope,
            )
            .await
    };
    /// The permalinks of a slice's nodes, sorted.
    fn nodes(value: &serde_json::Value) -> Vec<String> {
        let mut out: Vec<String> = value["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n["permalink"].as_str().unwrap().to_string())
            .collect();
        out.sort();
        out
    }

    // Seeded at the base engram both drafts point at.
    assert_eq!(
        nodes(&context("crystalline://team/plan", &alice).await.unwrap()),
        vec!["fresh".to_string(), "plan".to_string()],
        "her own draft is in her neighbourhood of the plan, and his is not"
    );
    assert_eq!(
        nodes(&context("crystalline://team/plan", &bob).await.unwrap()),
        vec!["bobs-idea".to_string(), "plan".to_string()],
        "and his in his"
    );
    assert_eq!(
        nodes(
            &context("crystalline://team/plan", &account("carol"))
                .await
                .unwrap()
        ),
        vec!["plan".to_string()],
        "an account drafting nothing here sees the graph the team's files draw"
    );

    // Seeded at the draft itself, which only its author can anchor on at all.
    assert_eq!(
        nodes(&context("crystalline://team/fresh", &alice).await.unwrap()),
        vec!["fresh".to_string(), "plan".to_string()],
        "her draft is an anchor of her own"
    );
    assert!(
        context("crystalline://team/fresh", &bob).await.is_err(),
        "and for anybody else it is the miss an engram nobody wrote produces"
    );

    // A path its author deleted anchors nothing for them and is out of their
    // graph, while the team's neighbourhood is untouched.
    f.engine
        .delete_engram_as(
            &DeleteParams {
                identifier: "plan".to_string(),
                domain: "team".to_string(),
                expected_checksum: None,
            },
            None,
            &alice,
        )
        .await
        .unwrap();
    assert!(
        context("crystalline://team/plan", &alice).await.is_err(),
        "she deleted it, so there is nothing there to anchor on"
    );
    assert_eq!(
        nodes(&context("crystalline://team/fresh", &alice).await.unwrap()),
        vec!["fresh".to_string()],
        "and her draft stands alone, its relation reaching a path she took away"
    );
    assert_eq!(
        nodes(&context("crystalline://team/plan", &bob).await.unwrap()),
        vec!["bobs-idea".to_string(), "plan".to_string()],
        "her deletion is hers: the team's plan is where it was for everybody else"
    );
}

/// A draft's own relations are the ones its author reads back.
///
/// `read_engram` resolves a draft over a base row to the BASE descriptor, so
/// one engram keeps one address however it is being rewritten. The edges are
/// the other half of that answer and they are not the base row's: a relation
/// the draft added would read as unresolved and one it removed would still be
/// reported, which is the index contradicting the document in the same
/// response. Who points IN stays the base row's, because that is a fact about
/// the address the team shares rather than about the private rewrite.
///
/// The relation points at a base row, which is the only kind of target a
/// reference ever resolves to: a resolved edge is a fact about the domain, so
/// nothing lands on one reader's private draft.
#[tokio::test]
async fn a_drafts_own_relations_are_what_its_author_reads() {
    let f = review_fixture().await;
    let alice = account("alice");

    f.engine
        .edit_engram_as(
            &EditParams {
                identifier: "plan".to_string(),
                domain: "team".to_string(),
                operation: "append".to_string(),
                content: Some("- relates_to [[manifest]]".to_string()),
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
            &alice,
        )
        .await
        .unwrap();

    let hers = f.engine.read_engram(&read("plan"), &alice).await.unwrap();
    assert_eq!(
        hers["permalink"],
        serde_json::json!("plan"),
        "one engram, one address: her read is still of the team's plan"
    );
    let relations = hers["relations"].as_array().unwrap();
    assert_eq!(
        relations.len(),
        1,
        "the relation she wrote is in the document she reads: {hers}"
    );
    assert_eq!(
        relations[0]["resolved"],
        serde_json::json!(true),
        "and it resolves, because her own row is where her edges hang: {hers}"
    );

    let theirs = f
        .engine
        .read_engram(&read("plan"), &account("bob"))
        .await
        .unwrap();
    assert!(
        theirs["relations"].as_array().unwrap().is_empty(),
        "and the engram the team reviewed has no relation at all: {theirs}"
    );
}

// --- Task 7: turning review mode on, and folding it off ---------------------

/// The confirm a test means when it names one actor's choice.
fn folds(pairs: &[(&str, FoldChoice)]) -> ReviewModeConfirm {
    ReviewModeConfirm::Confirmed {
        folds: pairs
            .iter()
            .map(|(actor, choice)| ((*actor).to_string(), *choice))
            .collect(),
    }
}

/// The permalinks one actor's plan entry names, with the conflict each carries.
fn plan_entries(plan: &serde_json::Value, actor: &str) -> Vec<(String, bool, bool)> {
    plan["actors"]
        .as_array()
        .expect("the plan lists actors")
        .iter()
        .find(|row| row["actor"] == serde_json::json!(actor))
        .map(|row| {
            row["drafts"]
                .as_array()
                .unwrap()
                .iter()
                .map(|d| {
                    (
                        d["path"].as_str().unwrap().to_string(),
                        d["tombstone"].as_bool().unwrap(),
                        !d["conflict"].is_null(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The `review` key the config file on disk carries for a domain, as a string.
fn review_key(f: &Fixture, domain: &str) -> Option<String> {
    let text = std::fs::read_to_string(f.root.join("config.yaml")).unwrap();
    let cfg: GlobalConfig = serde_yaml_ng::from_str(&text).unwrap();
    cfg.domains
        .get(domain)
        .and_then(|e| e.review.as_ref())
        .map(|_| "overlay".to_string())
}

/// Review mode is a promise that every change is reviewed before it lands, and
/// a domain that already has unshared work in its folder cannot make it: the
/// work in the tree went round no review at all, and turning the mode on would
/// bless it silently. So the refusal names the paths and says what to do with
/// them.
#[tokio::test]
async fn enabling_review_on_a_dirty_domain_refuses_naming_the_paths() {
    let f = origin_fixture().await;
    std::fs::write(
        f.domain_root("team").join("notes.md"),
        PLAN.replace("permalink: plan", "permalink: notes")
            .replace("Plan", "Notes"),
    )
    .unwrap();

    let refused = f
        .engine
        .set_review_mode(
            "team",
            Some(ReviewMode::Overlay),
            ReviewModeConfirm::Confirmed { folds: Vec::new() },
            &Scope::Unrestricted,
        )
        .await
        .expect_err("a folder with unshared work in it cannot start reviewing");
    let words = refused.to_string();
    assert!(
        words.contains("share or revert these first, then enable review"),
        "the refusal says what to do with them: {words}"
    );
    assert!(
        words.contains("notes.md"),
        "and names the path that is in the way: {words}"
    );
    assert_eq!(
        review_key(&f, "team"),
        None,
        "and the configuration was not written on the way to refusing"
    );
}

/// Two shapes a domain can be in that review mode has no answer for: a domain
/// with no GitHub origin, where a reviewed change has nowhere to be proposed,
/// and a virtual domain, whose engrams live in the database with no folder for
/// a fold to land in.
#[tokio::test]
async fn enabling_review_needs_an_origin_and_refuses_a_virtual_domain() {
    let f = fixture().await;

    let refused = f
        .engine
        .set_review_mode(
            "team",
            Some(ReviewMode::Overlay),
            ReviewModeConfirm::Confirmed { folds: Vec::new() },
            &Scope::Unrestricted,
        )
        .await
        .expect_err("a domain with nowhere to propose a change reviews nothing");
    assert!(
        refused
            .to_string()
            .contains("connect it to a GitHub repository"),
        "the refusal names the missing half: {refused}"
    );

    f.engine.domain_add_virtual("scratch").await.unwrap();
    let refused = f
        .engine
        .set_review_mode(
            "scratch",
            Some(ReviewMode::Overlay),
            ReviewModeConfirm::Confirmed { folds: Vec::new() },
            &Scope::Unrestricted,
        )
        .await
        .expect_err("a virtual domain has no folder a fold could land in");
    assert!(
        refused.to_string().contains("virtual domain"),
        "the refusal says which kind of domain this is: {refused}"
    );
    assert_eq!(review_key(&f, "team"), None);
    assert_eq!(review_key(&f, "scratch"), None);
}

/// The clean case: the key lands in the config file AND the mode is live in
/// this engine straight away, which is the half a config write alone would not
/// prove.
#[tokio::test]
async fn enabling_review_on_a_clean_domain_writes_the_config() {
    let f = origin_fixture().await;
    let alice = account("alice");

    // Before: the domain takes changes directly, and alice's write is in the
    // folder the team shares.
    f.engine
        .write_engram_as(
            &write_params("team", "Direct", "- [fact] written before review #team"),
            None,
            &alice,
        )
        .await
        .unwrap();
    assert!(
        f.domain_root("team").join("direct.md").exists(),
        "a direct domain takes a write into its own folder"
    );
    f.snapshot_origin("team");

    let preview = f
        .engine
        .set_review_mode(
            "team",
            Some(ReviewMode::Overlay),
            ReviewModeConfirm::Preview,
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert_eq!(preview["applied"], serde_json::json!(false));
    assert_eq!(
        review_key(&f, "team"),
        None,
        "a preview answers the question and writes nothing"
    );

    let receipt = f
        .engine
        .set_review_mode(
            "team",
            Some(ReviewMode::Overlay),
            ReviewModeConfirm::Confirmed { folds: Vec::new() },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert_eq!(receipt["applied"], serde_json::json!(true));
    assert_eq!(receipt["review"], serde_json::json!("overlay"));
    assert_eq!(
        review_key(&f, "team"),
        Some("overlay".to_string()),
        "the configuration on disk says the domain reviews changes"
    );

    let now = f
        .engine
        .write_engram_as(
            &write_params("team", "Reviewed", "- [fact] written after review #team"),
            None,
            &alice,
        )
        .await
        .unwrap();
    assert_eq!(
        now["draft"],
        serde_json::json!(true),
        "and the very next write joins her draft rather than the folder: {now}"
    );
    assert!(
        !f.domain_root("team").join("reviewed.md").exists(),
        "the folder still says what the team reviewed"
    );
}

/// Leaving review mode ends every actor's private drafts one way or the other,
/// so the plan is put before anybody answers: who holds what, which of their
/// drafts are deletions, and which paths more than one of them is drafting.
#[tokio::test]
async fn disable_preview_lists_every_actors_entries() {
    let f = review_fixture().await;
    f.draft("team", "alice", "plan.md", ALICE_DRAFT).await;
    f.draft("team", "alice", "fresh.md", ALICE_NEW).await;
    f.tombstone("team", "bob", "plan.md").await;

    let plan = f
        .engine
        .set_review_mode(
            "team",
            None,
            ReviewModeConfirm::Preview,
            &Scope::Unrestricted,
        )
        .await
        .unwrap();

    assert_eq!(plan["applied"], serde_json::json!(false));
    assert_eq!(plan["domain"], serde_json::json!("team"));
    assert_eq!(plan["mode"], serde_json::json!("direct"));
    assert_eq!(
        plan["actors"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| (
                row["actor"].as_str().unwrap().to_string(),
                row["entries"].as_u64().unwrap()
            ))
            .collect::<Vec<_>>(),
        vec![("alice".to_string(), 2), ("bob".to_string(), 1)],
        "every actor holding anything is named, in order: {plan}"
    );
    assert_eq!(
        plan_entries(&plan, "alice"),
        vec![
            ("fresh.md".to_string(), false, false),
            ("plan.md".to_string(), false, false),
        ],
        "her drafts, by path, neither of them a deletion: {plan}"
    );
    assert_eq!(
        plan_entries(&plan, "bob"),
        vec![("plan.md".to_string(), true, false)],
        "and his one deletion says it is one: {plan}"
    );
    assert_eq!(
        plan["contested_paths"],
        serde_json::json!([{ "path": "plan.md", "actors": ["alice", "bob"] }]),
        "a path two of them are drafting is named before either is folded: {plan}"
    );

    assert_eq!(
        f.held("team", "alice").await.len(),
        2,
        "a preview changes nothing at all"
    );
    assert_eq!(review_key(&f, "team"), Some("overlay".to_string()));
}

/// A fold is the drafts landing in the folder the team shares: a rewrite
/// becomes the file, a new page becomes a new file, a deletion takes the file
/// away - and the overlay is empty afterwards, mirror included, so the next
/// sync has nothing to bring back.
#[tokio::test]
async fn a_confirmed_fold_lands_on_disk_and_empties_the_overlay() {
    let f = review_fixture().await;
    std::fs::write(
        f.domain_root("team").join("notes.md"),
        PLAN.replace("permalink: plan", "permalink: notes")
            .replace("Plan", "Notes"),
    )
    .unwrap();
    f.engine.sync(None).await.unwrap();

    f.draft("team", "alice", "plan.md", ALICE_DRAFT).await;
    f.draft("team", "alice", "fresh.md", ALICE_NEW).await;
    f.engine
        .delete_engram_as(
            &DeleteParams {
                identifier: "notes".to_string(),
                domain: "team".to_string(),
                expected_checksum: None,
            },
            None,
            &account("alice"),
        )
        .await
        .unwrap();

    let receipt = f
        .engine
        .set_review_mode(
            "team",
            None,
            folds(&[("alice", FoldChoice::Fold)]),
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert_eq!(receipt["applied"], serde_json::json!(true));
    assert_eq!(
        receipt["folded"],
        serde_json::json!([{ "actor": "alice", "written": 2, "deleted": 1 }]),
        "the receipt says what landed and what went: {receipt}"
    );

    assert_eq!(
        std::fs::read_to_string(f.domain_root("team").join("plan.md")).unwrap(),
        ALICE_DRAFT,
        "her rewrite is the file now"
    );
    assert_eq!(
        std::fs::read_to_string(f.domain_root("team").join("fresh.md")).unwrap(),
        ALICE_NEW,
        "and the page only she had is a file the team has"
    );
    assert!(
        !f.domain_root("team").join("notes.md").exists(),
        "and the page she deleted is gone from the folder"
    );

    assert!(
        f.held("team", "alice").await.is_empty(),
        "nothing of hers is left in the overlay"
    );
    assert_eq!(
        review_key(&f, "team"),
        None,
        "and the domain takes changes directly again"
    );

    // The index followed the tree: her rewrite is what everybody reads, and the
    // engram she deleted is not there for anybody.
    let read = f.reads("plan", &account("bob")).await.unwrap();
    assert!(
        read.contains("alice would have it"),
        "the folded text is the team's plan now: {read}"
    );
    assert!(
        f.reads("notes", &account("bob")).await.is_err(),
        "and the engram she deleted is gone for everybody"
    );

    // The journal went with the rows. A second sync is what proves it: the
    // restore runs in every sync pass, so a mirror left behind would put the
    // drafts straight back.
    f.engine.sync(None).await.unwrap();
    assert!(
        f.held("team", "alice").await.is_empty(),
        "a sync after the fold brings nothing back"
    );
}

/// A discard is the other answer: the drafts end and the folder never hears
/// about them.
#[tokio::test]
async fn a_discard_drops_without_touching_disk() {
    let f = review_fixture().await;
    f.draft("team", "alice", "plan.md", ALICE_DRAFT).await;
    f.draft("team", "alice", "fresh.md", ALICE_NEW).await;
    let before = f.tree("team");

    let receipt = f
        .engine
        .set_review_mode(
            "team",
            None,
            folds(&[("alice", FoldChoice::Discard)]),
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert_eq!(
        receipt["discarded"],
        serde_json::json!([{ "actor": "alice", "entries": 2 }]),
        "the receipt says how much was dropped: {receipt}"
    );

    assert_eq!(
        f.tree("team"),
        before,
        "the folder is byte for byte as it was"
    );
    assert!(
        f.held("team", "alice").await.is_empty(),
        "and her rows are gone"
    );
    f.engine.sync(None).await.unwrap();
    assert!(
        f.held("team", "alice").await.is_empty(),
        "a sync after the discard brings nothing back either"
    );
}

/// Leaving review mode decides the fate of somebody's unshared work, so it is
/// never decided by omission: a confirm that does not say what to do with an
/// actor's drafts refuses and names them.
#[tokio::test]
async fn a_confirm_missing_an_actor_refuses() {
    let f = review_fixture().await;
    f.draft("team", "alice", "plan.md", ALICE_DRAFT).await;
    f.draft("team", "bob", "fresh.md", ALICE_NEW).await;
    let before = f.tree("team");

    let refused = f
        .engine
        .set_review_mode(
            "team",
            None,
            folds(&[("alice", FoldChoice::Fold)]),
            &Scope::Unrestricted,
        )
        .await
        .expect_err("bob's drafts were not spoken for");
    assert!(
        refused.to_string().contains("bob"),
        "the refusal names who is unaccounted for: {refused}"
    );

    let refused = f
        .engine
        .set_review_mode(
            "team",
            None,
            folds(&[
                ("alice", FoldChoice::Fold),
                ("bob", FoldChoice::Discard),
                ("carol", FoldChoice::Fold),
            ]),
            &Scope::Unrestricted,
        )
        .await
        .expect_err("carol holds nothing here, so naming her is a mistake worth saying");
    assert!(
        refused.to_string().contains("carol"),
        "the refusal names the actor who holds nothing: {refused}"
    );

    assert_eq!(f.tree("team"), before, "neither refusal touched the folder");
    assert_eq!(f.held("team", "alice").await.len(), 1);
    assert_eq!(f.held("team", "bob").await.len(), 1);
    assert_eq!(review_key(&f, "team"), Some("overlay".to_string()));
}

/// One engram answers to one address, and a fold is where two of them can meet:
/// two actors folding the same path, and a draft whose address the team took
/// while it was being drafted. Both refuse before anything is written, and the
/// preview flags the second one ahead of the confirm.
#[tokio::test]
async fn a_fold_that_would_collide_refuses_and_the_preview_flags_it() {
    let f = review_fixture().await;
    f.draft("team", "alice", "plan.md", ALICE_DRAFT).await;
    f.draft("team", "bob", "plan.md", ALICE_DRAFT).await;
    let before = f.tree("team");

    let refused = f
        .engine
        .set_review_mode(
            "team",
            None,
            folds(&[("alice", FoldChoice::Fold), ("bob", FoldChoice::Fold)]),
            &Scope::Unrestricted,
        )
        .await
        .expect_err("two folds cannot both be the file at one path");
    assert!(
        refused.to_string().contains("plan.md")
            && refused.to_string().contains("alice")
            && refused.to_string().contains("bob"),
        "the refusal names the path and both actors: {refused}"
    );
    assert_eq!(f.tree("team"), before, "and nothing was written on the way");

    // One of them folding and the other discarding is not a collision at all.
    f.engine
        .set_review_mode(
            "team",
            None,
            folds(&[("alice", FoldChoice::Fold), ("bob", FoldChoice::Discard)]),
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(f.domain_root("team").join("plan.md")).unwrap(),
        ALICE_DRAFT
    );

    // The second shape: the team takes an address while somebody is drafting
    // under it. Nothing converges that today, so the fold is where it is caught.
    let g = review_fixture().await;
    let alice = account("alice");
    g.engine
        .write_engram_as(
            &write_params("team", "Rota", "- [decision] alice takes the rota #team"),
            None,
            &alice,
        )
        .await
        .unwrap();
    std::fs::write(
        g.domain_root("team").join("other.md"),
        PLAN.replace("permalink: plan", "permalink: rota")
            .replace("Plan", "Rota"),
    )
    .unwrap();
    g.engine.sync(None).await.unwrap();

    let plan = g
        .engine
        .set_review_mode(
            "team",
            None,
            ReviewModeConfirm::Preview,
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert_eq!(
        plan_entries(&plan, "alice"),
        vec![("rota.md".to_string(), false, true)],
        "the preview flags the draft that cannot land: {plan}"
    );

    let refused = g
        .engine
        .set_review_mode(
            "team",
            None,
            folds(&[("alice", FoldChoice::Fold)]),
            &Scope::Unrestricted,
        )
        .await
        .expect_err("the address is spoken for, so her draft has nowhere to land");
    assert!(
        refused.to_string().contains("rota.md")
            && refused.to_string().contains("alice")
            && refused.to_string().contains("other.md"),
        "the refusal names her path, her name and the engram that holds the address: {refused}"
    );
    assert_eq!(
        g.held("team", "alice").await.len(),
        1,
        "and her draft is still hers to fix"
    );
}

/// A fold writes the files of a domain somebody may be co-editing right now, so
/// the rooms are closed first: a room closed afterwards would land its own
/// stale text over the fold and the drafts would be gone with nothing to redo
/// them from.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fold_closes_open_rooms_first() {
    let f = review_fixture().await;
    let sessions = crystalline_service::collab::session::CollabSessions::new(f.engine.clone());
    f.engine.set_collab_sessions(&sessions);
    f.draft("team", "alice", "fresh.md", ALICE_NEW).await;

    // A room over a path in the same domain, with text in it that has never
    // been saved. Not the path being folded: there the fold and the room's
    // final save both write the same file and the last one wins whichever
    // order they run in, which says nothing about the order. Here the room's
    // save is its own change to the tree, and the question the assertion asks
    // is whether the sync at the end of the fold saw it.
    let joined = sessions.join("team", "plan").await.unwrap();
    let doc = Doc::with_options(yrs::Options {
        offset_kind: yrs::OffsetKind::Utf16,
        ..yrs::Options::default()
    });
    let replies = joined
        .session
        .handle_frame(
            joined.conn,
            &Message::Sync(SyncMessage::SyncStep1(doc.transact().state_vector())).encode_v1(),
        )
        .await;
    for reply in replies {
        let mut decoder = DecoderV1::from(reply.as_slice());
        for message in MessageReader::new(&mut decoder).flatten() {
            if let Message::Sync(SyncMessage::SyncStep2(update)) = message {
                doc.transact_mut()
                    .apply_update(Update::decode_v1(&update).unwrap())
                    .unwrap();
            }
        }
    }
    let update = {
        let text = doc.get_or_insert_text("content");
        let mut txn = doc.transact_mut();
        let end = text.len(&txn);
        text.insert(&mut txn, end, "typed but never flushed\n");
        txn.encode_update_v1()
    };
    joined
        .session
        .handle_frame(
            joined.conn,
            &Message::Sync(SyncMessage::Update(update)).encode_v1(),
        )
        .await;
    assert_eq!(sessions.session_count().await, 1, "the room is open");

    let receipt = f
        .engine
        .set_review_mode(
            "team",
            None,
            folds(&[("alice", FoldChoice::Fold)]),
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert_eq!(
        receipt["rooms_closed"],
        serde_json::json!(1),
        "the room was swept: {receipt}"
    );
    assert_eq!(sessions.session_count().await, 0, "and it is not there now");
    assert!(
        std::fs::read_to_string(f.domain_root("team").join("plan.md"))
            .unwrap()
            .contains("typed but never flushed"),
        "the room's final save landed in the folder the domain keeps"
    );
    assert_eq!(
        std::fs::read_to_string(f.domain_root("team").join("fresh.md")).unwrap(),
        ALICE_NEW,
        "and the fold landed beside it"
    );
    let read = f.reads("plan", &account("bob")).await.unwrap();
    assert!(
        read.contains("typed but never flushed"),
        "and the sync at the end of the fold saw both, because the room was \
         closed before it ran rather than after: {read}"
    );
    assert!(
        f.held("team", "owner").await.is_empty(),
        "and the room's save is in the file rather than in a draft nobody \
         planned for: a room saves as the machine owner, so a sweep one step \
         earlier would have left one behind"
    );
}

/// In review mode every legitimate write joins a draft, so anything the working
/// tree has that the origin does not got there some other way. It is reported
/// rather than blocked - the folder is the operator's - and a domain that is
/// not reviewing says nothing at all, because for it a local change is
/// ordinary unshared work.
#[tokio::test]
async fn a_review_domain_reports_its_out_of_band_tree_edits() {
    let f = reviewed_origin_fixture().await;

    // A reviewing domain says so with an empty list before it has anything to
    // name: the key's presence is what tells a client which mode this domain is
    // in, so an absent key on a clean folder would read as "takes changes
    // directly" rather than as "nothing has gone round review".
    let status = f
        .engine
        .origin_status(Some("team"), false, &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(
        status["domains"][0]["out_of_band"],
        serde_json::json!([]),
        "a reviewing domain with a clean folder says so rather than saying nothing: {status}"
    );

    std::fs::write(
        f.domain_root("team").join("smuggled.md"),
        PLAN.replace("permalink: plan", "permalink: smuggled")
            .replace("Plan", "Smuggled"),
    )
    .unwrap();

    let status = f
        .engine
        .origin_status(Some("team"), false, &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(
        status["domains"][0]["out_of_band"],
        serde_json::json!(["smuggled.md"]),
        "a reviewed domain names what appeared in its folder without review: {status}"
    );

    let g = origin_fixture().await;
    std::fs::write(
        g.domain_root("team").join("ordinary.md"),
        PLAN.replace("permalink: plan", "permalink: ordinary")
            .replace("Plan", "Ordinary"),
    )
    .unwrap();
    let status = g
        .engine
        .origin_status(Some("team"), false, &Scope::Unrestricted)
        .await
        .unwrap();
    assert!(
        status["domains"][0].get("out_of_band").is_none(),
        "and a domain taking changes directly says nothing about out-of-band work: {status}"
    );
}

/// The plan and the fold answer one question, so the ordinary rename previews
/// the way it folds.
///
/// A rename inside an overlay is a PAIR: a tombstone at the old path and an
/// entry at the new one carrying the same address (`move_within_overlay`, and
/// Task 4's deviation 3 says why it has to be that shape). A preview that asked
/// only "does a base row hold this address" would call every rename a
/// collision, in red, on the one flow an author is most likely to have used -
/// and then fold it without complaint.
#[tokio::test]
async fn a_rename_inside_one_overlay_previews_clean_and_folds_clean() {
    let f = review_fixture().await;
    let alice = account("alice");

    f.engine
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

    let plan = f
        .engine
        .set_review_mode(
            "team",
            None,
            ReviewModeConfirm::Preview,
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert_eq!(
        plan_entries(&plan, "alice"),
        vec![
            ("archive/plan.md".to_string(), false, false),
            ("plan.md".to_string(), true, false),
        ],
        "her own deletion of the old path is what frees the address for the new one: {plan}"
    );

    f.engine
        .set_review_mode(
            "team",
            None,
            folds(&[("alice", FoldChoice::Fold)]),
            &Scope::Unrestricted,
        )
        .await
        .expect("and the fold agrees with the plan that showed no conflict");
    assert!(
        !f.domain_root("team").join("plan.md").exists(),
        "the engram moved: the old path is gone"
    );
    assert!(
        f.domain_root("team").join("archive/plan.md").exists(),
        "and the new one is the file"
    );
}

/// One address claimed at two paths by two people is a collision the plan has
/// to name, because neither draft is in the folder for the other one's
/// `conflict` to find and neither path is contested.
#[tokio::test]
async fn one_address_claimed_at_two_paths_is_flagged_in_the_plan_and_refused() {
    let f = review_fixture().await;
    // Written straight into the store, because the write path lets this state
    // happen and no verb produces it in one call: the permalink rule screens a
    // draft against the base rows and against its OWN author's drafts, never
    // against somebody else's, so alice may point alpha.md at 'shared' and bob
    // may point beta.md at the same address a moment later.
    let alpha = ALICE_NEW
        .replace("permalink: fresh", "permalink: shared")
        .replace("Fresh", "Alpha");
    let beta = ALICE_NEW
        .replace("permalink: fresh", "permalink: shared")
        .replace("Fresh", "Beta");
    f.draft("team", "alice", "alpha.md", &alpha).await;
    f.draft("team", "bob", "beta.md", &beta).await;
    let before = f.tree("team");

    let plan = f
        .engine
        .set_review_mode(
            "team",
            None,
            ReviewModeConfirm::Preview,
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert_eq!(
        plan["contested_addresses"],
        serde_json::json!([{
            "permalink": "shared",
            "paths": ["alpha.md", "beta.md"],
            "actors": ["alice", "bob"],
        }]),
        "the plan names the address before either of them answers: {plan}"
    );

    let refused = f
        .engine
        .set_review_mode(
            "team",
            None,
            folds(&[("alice", FoldChoice::Fold), ("bob", FoldChoice::Fold)]),
            &Scope::Unrestricted,
        )
        .await
        .expect_err("one engram answers to one address, so both cannot land");
    let words = refused.to_string();
    assert!(
        words.contains("shared") && words.contains("alpha.md") && words.contains("beta.md"),
        "the refusal names the address and both paths: {words}"
    );
    assert_eq!(f.tree("team"), before, "and nothing was written on the way");
    assert_eq!(review_key(&f, "team"), Some("overlay".to_string()));

    // One of them folding is no collision at all.
    f.engine
        .set_review_mode(
            "team",
            None,
            folds(&[("alice", FoldChoice::Fold), ("bob", FoldChoice::Discard)]),
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert!(f.domain_root("team").join("alpha.md").exists());
    assert!(!f.domain_root("team").join("beta.md").exists());
}

/// The rooms are swept before the folds, so what they save is part of the
/// folder the folds have to fit into - and the check that decides whether they
/// fit has to be asked of the folder as it stands THEN.
///
/// A room whose author edited the frontmatter's permalink line moves a base
/// engram's address. If the fold validated against the folder as it was before
/// the sweep, it would write a second engram at that address and the first
/// thing to notice would be the sync at the end - by which time the key is off,
/// the files are written and every actor's rows have been dropped, with the
/// domain's sync failing the same way for ever after.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_room_that_takes_an_address_while_it_closes_refuses_with_nothing_folded() {
    let f = review_fixture().await;
    let sessions = crystalline_service::collab::session::CollabSessions::new(f.engine.clone());
    f.engine.set_collab_sessions(&sessions);
    // Her draft answers to 'fresh', which nothing in the folder holds yet.
    f.draft("team", "alice", "fresh.md", ALICE_NEW).await;

    let joined = sessions.join("team", "plan").await.unwrap();
    let doc = Doc::with_options(yrs::Options {
        offset_kind: yrs::OffsetKind::Utf16,
        ..yrs::Options::default()
    });
    let replies = joined
        .session
        .handle_frame(
            joined.conn,
            &Message::Sync(SyncMessage::SyncStep1(doc.transact().state_vector())).encode_v1(),
        )
        .await;
    for reply in replies {
        let mut decoder = DecoderV1::from(reply.as_slice());
        for message in MessageReader::new(&mut decoder).flatten() {
            if let Message::Sync(SyncMessage::SyncStep2(update)) = message {
                doc.transact_mut()
                    .apply_update(Update::decode_v1(&update).unwrap())
                    .unwrap();
            }
        }
    }
    // The one edit that moves an address: the frontmatter's permalink line,
    // retyped onto the address her draft already answers to.
    let update = {
        let text = doc.get_or_insert_text("content");
        let full = {
            let txn = doc.transact();
            text.get_string(&txn)
        };
        let at = full
            .find("permalink: plan")
            .expect("the base engram's own line")
            + "permalink: ".len();
        let mut txn = doc.transact_mut();
        text.remove_range(&mut txn, at as u32, 4);
        text.insert(&mut txn, at as u32, "fresh");
        txn.encode_update_v1()
    };
    joined
        .session
        .handle_frame(
            joined.conn,
            &Message::Sync(SyncMessage::Update(update)).encode_v1(),
        )
        .await;

    let refused = f
        .engine
        .set_review_mode(
            "team",
            None,
            folds(&[("alice", FoldChoice::Fold)]),
            &Scope::Unrestricted,
        )
        .await
        .expect_err("the folder took her address while its rooms were closing");
    let words = refused.to_string();
    assert!(
        words.contains("fresh.md") && words.contains("alice"),
        "the refusal names her path and her name: {words}"
    );

    // Nothing was folded, and the domain is reviewing again: the same call
    // works once somebody has given one of the two engrams an address of its
    // own.
    assert_eq!(review_key(&f, "team"), Some("overlay".to_string()));
    assert_eq!(
        f.held("team", "alice")
            .await
            .into_iter()
            .map(|(path, _, tomb)| (path, tomb))
            .collect::<Vec<_>>(),
        vec![("fresh.md".to_string(), false)],
        "her draft is where it was"
    );
    assert!(
        !f.domain_root("team").join("fresh.md").exists(),
        "and no fold reached the folder"
    );
    // The room's own save DID land - it is an out-of-band edit of the tree now,
    // which `origin status` names for a reviewing domain - and the index takes
    // it without complaint, which is the wedge this refusal exists to prevent.
    assert!(
        std::fs::read_to_string(f.domain_root("team").join("plan.md"))
            .unwrap()
            .contains("permalink: fresh"),
        "the room's save is what the file says"
    );
    f.engine
        .sync(None)
        .await
        .expect("and the domain still syncs, which a wedged fold would have ended");
}

/// A `direct` on a domain that already takes changes directly is a statement of
/// what holds, not a verb: it closes nobody's co-editing room and syncs
/// nothing.
///
/// The conjunction behind it is what keeps the mid-fold recovery alive - a
/// domain whose key has already come off but whose overlay is not empty still
/// has to fold - so both halves are asserted here.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn leaving_a_domain_that_never_reviewed_changes_nothing() {
    let f = fixture().await;
    let sessions = crystalline_service::collab::session::CollabSessions::new(f.engine.clone());
    f.engine.set_collab_sessions(&sessions);
    let _joined = sessions.join("team", "plan").await.unwrap();

    let receipt = f
        .engine
        .set_review_mode(
            "team",
            None,
            ReviewModeConfirm::Confirmed { folds: Vec::new() },
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert_eq!(receipt["applied"], serde_json::json!(true));
    assert_eq!(
        receipt["rooms_closed"],
        serde_json::json!(0),
        "nobody's editor was closed to state what already held: {receipt}"
    );
    assert_eq!(
        sessions.session_count().await,
        1,
        "and the room is still open"
    );

    // The other half of the conjunction: a domain the key has already come off
    // but whose overlay is not empty is the recovery path, and it runs.
    f.draft("team", "alice", "fresh.md", ALICE_NEW).await;
    let receipt = f
        .engine
        .set_review_mode(
            "team",
            None,
            folds(&[("alice", FoldChoice::Fold)]),
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert_eq!(
        receipt["folded"],
        serde_json::json!([{ "actor": "alice", "written": 1, "deleted": 0 }]),
        "the fold a half-finished call left behind still lands: {receipt}"
    );
    assert!(f.domain_root("team").join("fresh.md").exists());
}
