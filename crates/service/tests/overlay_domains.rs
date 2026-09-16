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
use crystalline_service::{Engine, Scope, ShareActor, SimilarProbe};
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

    /// Write one actor's draft FILE: an attachment written in review mode,
    /// which lands in that actor's files overlay and never in the folder.
    ///
    /// It goes through `attachment_write_as` rather than through a
    /// `DomainView`, because `domain_view` is `pub(crate)` and an integration
    /// test cannot build one. The receipt's `draft` flag is asserted here, so a
    /// fixture can never quietly write the team's folder instead.
    async fn file(&self, domain: &str, actor: &str, path: &str, bytes: &[u8]) {
        let written = self
            .engine
            .attachment_write_as(domain, path, bytes.to_vec(), &scope_of(actor))
            .await
            .unwrap();
        assert!(written.draft, "a write in review mode is a draft: {path}");
    }

    /// One actor's deletion of a reviewed file: a marker in their files
    /// overlay, with the folder untouched.
    async fn delete_file(&self, domain: &str, actor: &str, path: &str) {
        let draft = self
            .engine
            .attachment_delete_as(domain, path, &scope_of(actor))
            .await
            .unwrap();
        assert!(draft, "a deletion in review mode is a draft: {path}");
    }

    /// What one actor's files overlay holds on disk, as `(path, tombstone)`
    /// pairs ordered by path.
    fn files_held(&self, domain: &str, actor: &str) -> Vec<(String, bool)> {
        let dir = self
            .state
            .join("overlays")
            .join(domain)
            .join(actor)
            .join("files");
        let mut out = Vec::new();
        fn walk(dir: &std::path::Path, prefix: &str, out: &mut Vec<(String, bool)>) {
            let Ok(listed) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in listed.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                let rel = if prefix.is_empty() {
                    name
                } else {
                    format!("{prefix}/{name}")
                };
                if entry.path().is_dir() {
                    walk(&entry.path(), &rel, out);
                } else if let Some(base) = rel.strip_suffix(".tombstone") {
                    out.push((base.to_string(), true));
                } else {
                    out.push((rel, false));
                }
            }
        }
        walk(&dir, "", &mut out);
        out.sort();
        out
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

/// **A wipe and rebuild leaves the files overlay exactly where it was, and
/// never reads one of its files back as a draft.**
///
/// The two substrates share an actor folder, and the restore walks that folder.
/// What keeps them apart is one fact - the walk keeps only `.md` and
/// `.md.tombstone`, and a validated attachment path can never end in `.md` -
/// and this is where that fact has to hold or a wipe turns a screenshot into an
/// engram row. Planted adversarially: the actor holds a draft, an overlay file
/// and a deletion marker at once.
#[tokio::test]
async fn an_index_wipe_leaves_the_files_overlay_alone() {
    let f = review_fixture().await;
    let alice = Scope::User {
        account: "alice".to_string(),
        admin: false,
    };
    f.draft("team", "alice", "plan.md", ALICE_DRAFT).await;
    f.engine
        .attachment_write_as("team", "assets/deck.png", b"deck bytes".to_vec(), &alice)
        .await
        .unwrap();
    // A file the team reviewed, so alice's delete of it is a marker rather than
    // a removal - the shape the walk must also pass over.
    let assets = f.domain_root("team").join("assets");
    std::fs::create_dir_all(&assets).unwrap();
    std::fs::write(assets.join("gone.png"), b"gone bytes").unwrap();
    f.engine.sync(None).await.unwrap();
    f.engine
        .attachment_delete_as("team", "assets/gone.png", &alice)
        .await
        .unwrap();

    let file = f.state.join("overlays/team/alice/files/assets/deck.png");
    let marker = f
        .state
        .join("overlays/team/alice/files/assets/gone.png.tombstone");
    assert!(
        file.is_file() && marker.is_file(),
        "both shapes are planted"
    );
    let before = std::fs::read(&file).unwrap();

    {
        let store = f.store.lock().await;
        store.wipe().await.unwrap();
    }
    f.engine.sync(None).await.unwrap();

    assert_eq!(
        f.held("team", "alice").await,
        vec![("plan.md".to_string(), ALICE_DRAFT.to_string(), false)],
        "the draft came back and neither the overlay file nor its marker became a row"
    );
    assert!(
        file.is_file() && marker.is_file(),
        "and both are still on disk, untouched by the rebuild"
    );
    assert_eq!(std::fs::read(&file).unwrap(), before, "byte for byte");

    // And they are still hers to read, which is the whole reason they survived.
    let (bytes, _) = f
        .engine
        .attachment_read_as("team", "assets/deck.png", &alice)
        .await
        .unwrap();
    assert_eq!(bytes, before);
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
        .domain_remove_preview(
            "team",
            &Scope::Unrestricted,
            false,
            &named(&["alice", "bob"]),
        )
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
        .unregister_domain(
            "team",
            &Scope::Unrestricted,
            false,
            &named(&["alice", "bob"]),
        )
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

/// The reviewed team domain with an accounts store behind it, so a scope can be
/// somebody in particular: `owner` holds the domain, `mem` is an editor in it,
/// and the domain is private, which is what makes membership decide anything.
async fn screened_origin_fixture() -> Fixture {
    let f = reviewed_origin_fixture().await;
    let auth = Arc::new(
        crystalline_service::rest::AuthStore::open(&f.root.join("web-auth.db"))
            .await
            .unwrap(),
    );
    for name in ["owner", "mem"] {
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
        "mem",
        crystalline_service::rest::MemberLevel::Editor,
        "owner",
    )
    .await
    .unwrap();
    f.engine
        .set_domain_access(Arc::new(crystalline_service::DomainAccess::new(auth)));
    f
}

/// A status tells everybody what they are holding and tells the domain's owner
/// who else is holding anything.
///
/// The two halves are one rule: a count of your own unshared work is yours to
/// know, and a count of somebody else's is the coordination view of whoever
/// holds the domain. A member gets the first and not the second - not a
/// refusal, just no key - because a member asking after their own drafts is
/// not asking about anybody else's.
#[tokio::test]
async fn a_members_status_carries_only_its_own_count() {
    let f = screened_origin_fixture().await;
    f.draft("team", "mem", "plan.md", ALICE_DRAFT).await;
    f.draft("team", "owner", "fresh.md", ALICE_NEW).await;
    f.tombstone("team", "owner", "plan.md").await;

    let mine = f
        .engine
        .origin_status(Some("team"), false, &account("mem"))
        .await
        .unwrap();
    let entry = &mine["domains"][0];
    assert_eq!(
        entry["my_drafts"],
        serde_json::json!(1),
        "a member is told what a member is holding: {entry}"
    );
    assert!(
        entry.get("drafts").is_none(),
        "and nothing about anybody else: {entry}"
    );

    let theirs = f
        .engine
        .origin_status(Some("team"), false, &account("owner"))
        .await
        .unwrap();
    let entry = &theirs["domains"][0];
    assert_eq!(
        entry["my_drafts"],
        serde_json::json!(2),
        "a deletion is unshared work like any other draft, so it counts: {entry}"
    );
    assert_eq!(
        entry["drafts"],
        serde_json::json!([
            { "actor": "mem", "entries": 1 },
            { "actor": "owner", "entries": 2 },
        ]),
        "the owner sees who is drafting here and how much: {entry}"
    );
    let text = entry.to_string();
    for secret in ["plan.md", "fresh.md", "alice would have it"] {
        assert!(
            !text.contains(secret),
            "counts are not content, and {secret} is in the report: {text}"
        );
    }
}

/// A domain that takes changes directly carries neither key.
///
/// `my_drafts: 0` on a domain nobody can draft in would say "you are holding
/// nothing here", which reads as "you could be". The key's presence is what
/// says the domain reviews at all, exactly as `out_of_band`'s is.
#[tokio::test]
async fn a_direct_domains_status_says_nothing_about_drafts() {
    let f = origin_fixture().await;
    let status = f
        .engine
        .origin_status(Some("team"), false, &Scope::Unrestricted)
        .await
        .unwrap();
    let entry = &status["domains"][0];
    assert!(
        entry.get("my_drafts").is_none() && entry.get("drafts").is_none(),
        "a domain that takes changes directly has no drafts to report: {entry}"
    );
}

/// Actor names as a removal takes them, so a test reads as the call it makes.
fn named(actors: &[&str]) -> Vec<String> {
    actors.iter().map(|a| (*a).to_string()).collect()
}

/// The question a removal puts is answered from the rows, not from their
/// mirror.
///
/// A draft row can land while its journal entry fails - `write_overlay_entry`
/// reports that as a `draft_warning` and keeps the row - and the row is the
/// draft. A preview counting the mirror would tell somebody that nobody is
/// holding work in a domain where somebody is, in front of the one call that
/// ends it for good.
#[tokio::test]
async fn the_removal_preview_counts_the_rows_not_the_mirror() {
    let f = fixture().await;
    f.draft("team", "alice", "plan.md", ALICE_DRAFT).await;
    // Bob's row, with no mirror beside it: the draft whose journal write did
    // not land.
    {
        let store = f.store.lock().await;
        let id = f.domain_id(&*store, "team").await;
        store
            .upsert_overlay(id, "bob", &record(ALICE_NEW, "fresh.md"))
            .await
            .unwrap();
    }
    assert_eq!(
        overlay_journal::journal_counts(&f.state, "team")
            .per_actor
            .into_iter()
            .collect::<Vec<_>>(),
        vec![("alice".to_string(), 1)],
        "the mirror knows about alice alone, which is the whole point"
    );

    let preview = f
        .engine
        .domain_remove_preview(
            "team",
            &Scope::Unrestricted,
            false,
            &named(&["alice", "bob"]),
        )
        .await
        .unwrap();
    assert_eq!(
        preview["drafts"],
        serde_json::json!([
            { "actor": "alice", "entries": 1 },
            { "actor": "bob", "entries": 1 },
        ]),
        "the rows are the authority, mirror or no mirror: {preview}"
    );
    assert_eq!(preview["drafts_unknown"], serde_json::json!(false));
}

/// Ending a domain ends everybody's unshared work in it, so somebody else's is
/// never ended by omission.
///
/// The same rule leaving review mode applies, with the one difference that
/// makes it simpler: a removal has no fold to offer, so the answer is not what
/// happens to each actor's drafts but that each actor's drafts are being
/// ended. Naming them is the answer. The caller's OWN drafts need no naming -
/// they are the one person in the room who already knows.
#[tokio::test]
async fn removing_a_domain_refuses_until_every_other_actors_drafts_are_named() {
    let f = fixture().await;
    f.draft("team", "alice", "plan.md", ALICE_DRAFT).await;
    f.draft("team", "alice", "fresh.md", ALICE_NEW).await;
    f.tombstone("team", "bob", "plan.md").await;

    let err = f
        .engine
        .unregister_domain("team", &Scope::Unrestricted, false, &[])
        .await
        .expect_err("somebody else's drafts are not ended by omission");
    let text = err.to_string();
    for expected in ["alice", "2 drafts", "bob", "1 draft", "end_drafts"] {
        assert!(
            text.contains(expected),
            "the refusal names who and how many, and how to answer: {text}"
        );
    }
    // Names and counts, never the work itself: a refusal is read by somebody
    // who may end these drafts and may not read them.
    for secret in [
        "plan.md",
        "fresh.md",
        "alice would have it",
        "only alice has",
    ] {
        assert!(
            !text.contains(secret),
            "a count is not a disclosure, and {secret} is in the refusal: {text}"
        );
    }
    assert!(
        !overlay_journal::journal_entries(&f.state, "team")
            .entries
            .is_empty(),
        "and nothing was swept on the way to refusing"
    );
    assert_eq!(f.held("team", "alice").await.len(), 2, "with its drafts");

    // An actor who holds nothing here is somebody meaning a different domain
    // or a different moment, exactly as it is on the way out of review mode.
    let err = f
        .engine
        .unregister_domain(
            "team",
            &Scope::Unrestricted,
            false,
            &named(&["alice", "bob", "carol"]),
        )
        .await
        .expect_err("naming a stranger is not an answer about this domain");
    assert!(
        err.to_string().contains("carol"),
        "and the refusal says who nobody is: {err}"
    );

    // Named, and it goes.
    let report = f
        .engine
        .unregister_domain(
            "team",
            &Scope::Unrestricted,
            false,
            &named(&["alice", "bob"]),
        )
        .await
        .unwrap();
    assert_eq!(report["drafts_swept"], serde_json::json!(3));
}

/// Nobody is asked to confirm the ending of their own drafts.
///
/// A local session is the machine owner and drafts as `owner`. A removal that
/// made them name themselves would be a question with one possible answer,
/// asked of the person who just asked for the removal.
#[tokio::test]
async fn a_removal_asks_nothing_about_the_callers_own_drafts() {
    let f = fixture().await;
    f.draft("team", "owner", "plan.md", ALICE_DRAFT).await;

    let preview = f
        .engine
        .domain_remove_preview("team", &Scope::Unrestricted, false, &[])
        .await
        .unwrap();
    assert_eq!(
        preview["drafts"],
        serde_json::json!([{ "actor": "owner", "entries": 1 }]),
        "the preview still says what would be lost: {preview}"
    );
    let report = f
        .engine
        .unregister_domain("team", &Scope::Unrestricted, false, &[])
        .await
        .unwrap();
    assert_eq!(report["drafts_swept"], serde_json::json!(1));
}

/// Naming yourself is an answer a removal takes, and never one it asks for.
///
/// The preview lists every actor holding drafts, the caller among them, and the
/// MCP confirmation question reads that list out. A client that answers with the
/// list it was shown has to be served, or the refusal would instruct the very
/// thing it refuses - and the verb this one mirrors, leaving review mode, wants
/// the caller named. So both answers are good: with your own name in the list,
/// and without it.
#[tokio::test]
async fn a_removal_accepts_the_callers_own_name_and_never_asks_for_it() {
    let f = fixture().await;
    f.draft("team", "owner", "plan.md", ALICE_DRAFT).await;
    f.draft("team", "alice", "fresh.md", ALICE_NEW).await;

    // The list the preview shows, answered back verbatim.
    let preview = f
        .engine
        .domain_remove_preview(
            "team",
            &Scope::Unrestricted,
            false,
            &named(&["alice", "owner"]),
        )
        .await
        .expect("the answer a client reads off the preview is an answer");
    assert_eq!(
        preview["drafts"],
        serde_json::json!([
            { "actor": "alice", "entries": 1 },
            { "actor": "owner", "entries": 1 },
        ]),
        "and the preview does list the caller, which is why: {preview}"
    );

    // And the shorter answer, which is the one the refusal asks for.
    f.engine
        .domain_remove_preview("team", &Scope::Unrestricted, false, &named(&["alice"]))
        .await
        .expect("your own drafts need no naming");

    let report = f
        .engine
        .unregister_domain(
            "team",
            &Scope::Unrestricted,
            false,
            &named(&["owner", "alice"]),
        )
        .await
        .unwrap();
    assert_eq!(report["drafts_swept"], serde_json::json!(2));
}

/// A removal that is going to refuse closes nobody's co-editing room.
///
/// The rows and the journal surviving say nothing about this: the sweep runs
/// before either of them is touched, so a gate decided after it would refuse
/// with somebody's unsaved room already closed. The claim is written into
/// `unregister_domain`'s ordering comment, and this is what holds it there.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refused_removal_closes_no_co_editing_room() {
    let f = fixture().await;
    let sessions = crystalline_service::collab::session::CollabSessions::new(f.engine.clone());
    f.engine.set_collab_sessions(&sessions);
    f.draft("team", "alice", "plan.md", ALICE_DRAFT).await;
    let _joined = sessions.join("team", "plan", None).await.unwrap();
    assert_eq!(sessions.session_count().await, 1, "the room is open");

    f.engine
        .unregister_domain("team", &Scope::Unrestricted, false, &[])
        .await
        .expect_err("alice's drafts are not ended by omission");
    assert_eq!(
        sessions.session_count().await,
        1,
        "and the room somebody is typing in is still open"
    );
}

/// An archive import writes the folder the team shares, so a domain that
/// reviews changes refuses one - and the preview, which writes nothing, goes on
/// answering.
///
/// It is the same rule the cross-domain move is refused by: the folder changes
/// only by a pull, and an import is a write of the folder under an admin's
/// hand. A domain in review mode has no answer for "import this as whose
/// draft", because an import is not anybody's draft.
#[tokio::test]
async fn an_archive_import_into_a_reviewing_domain_is_refused_with_the_folder_rule() {
    let f = review_fixture().await;
    let files = vec![(
        "fresh.md".to_string(),
        ALICE_NEW.replace("permalink: fresh", "permalink: imported"),
    )];

    // The preview still answers: it writes nothing, and a question about a
    // refusal is not the refusal.
    let preview = f
        .engine
        .import_domain_files("team", &files, false, true)
        .await
        .unwrap();
    assert_eq!(preview["dry_run"], serde_json::json!(true), "{preview}");

    let refused = f
        .engine
        .import_domain_files("team", &files, false, false)
        .await
        .expect_err("an import writes the folder, and this domain's folder changes by a pull");
    let text = refused.to_string();
    assert!(
        matches!(
            &refused,
            crystalline_service::engine::EngineError::Refused(_)
        ),
        "it is a refusal, not an invalid request: {refused:?}"
    );
    assert!(
        text.contains("team") && text.contains("reviews changes"),
        "the refusal names the domain and why: {text}"
    );
    assert!(
        text.contains("Write it there"),
        "and says what to do instead: {text}"
    );
    assert!(
        !f.domain_root("team").join("fresh.md").exists(),
        "and nothing landed in the folder"
    );
}

/// A PNG stand-in: it never has to decode, only to travel unchanged, so it is
/// a short blob carrying the NUL a text-shaped path would lose.
const DECK_PNG: &[u8] = b"\x89PNG\r\n\x1a\n\x00a deck somebody drafted";
/// A file the team already has, so a draft can delete one.
const OLD_PNG: &[u8] = b"\x89PNG\r\n\x1a\n\x00the team's old deck";

/// Every attachment path the domain's own rows hold, ordered.
async fn attachment_paths(f: &Fixture) -> Vec<String> {
    let mut out: Vec<String> = f
        .engine
        .attachment_list("team")
        .await
        .unwrap()
        .into_iter()
        .map(|row| row.path)
        .collect();
    out.sort();
    out
}

/// A file somebody drafted is unshared work exactly as a drafted page is, so
/// the removal counts it and asks about it by name - and a files overlay that
/// cannot be read is not an empty one.
///
/// The files overlay is primary data rather than a mirror of a row, which is
/// what makes this different from the journal beside it: a journal nobody can
/// read is a copy of rows that are readable, while a files overlay nobody can
/// read is work nobody can account for. So it refuses, exactly as an index that
/// cannot be asked does, and for the same reason - the branch that decides
/// whether somebody's only copy of their work is deleted must never read a
/// failure as "there was nothing there".
#[tokio::test]
async fn the_removal_gate_counts_files_as_drafts_and_an_unreadable_files_folder_refuses() {
    let f = review_fixture().await;
    f.file("team", "bob", "assets/deck.png", DECK_PNG).await;

    // Named, because the preview raises every refusal the removal would: an
    // unnamed actor holding anything is exactly the refusal asserted below.
    let preview = f
        .engine
        .domain_remove_preview("team", &Scope::Unrestricted, false, &["bob".to_string()])
        .await
        .unwrap();
    assert_eq!(
        preview["drafts"],
        serde_json::json!([{ "actor": "bob", "entries": 1 }]),
        "his one file is one draft: {preview}"
    );
    assert_eq!(preview["drafts_unknown"], serde_json::json!(false));

    let refused = f
        .engine
        .unregister_domain("team", &Scope::Unrestricted, false, &[])
        .await
        .expect_err("a file somebody drafted is not ended by omission");
    assert!(
        refused.to_string().contains("bob (1 draft)"),
        "the refusal names him and what he is holding: {refused}"
    );

    // The folder is not a folder any more. Portable, deterministic, and
    // exactly as unreadable as a permission problem.
    let files = f.state.join("overlays/team/bob/files");
    std::fs::remove_dir_all(&files).unwrap();
    std::fs::write(&files, "not a folder").unwrap();

    // The preview raises the removal's own refusals before it shapes an
    // answer, so an unreadable count is a refusal here rather than a
    // `drafts_unknown: true` body: the question is never put about a removal
    // that would refuse anyway.
    let refused = f
        .engine
        .domain_remove_preview("team", &Scope::Unrestricted, false, &["bob".to_string()])
        .await
        .expect_err("a count nothing could read is asked about by nobody");
    assert!(
        refused.to_string().contains("could not be read"),
        "and the preview says why: {refused}"
    );
    let refused = f
        .engine
        .unregister_domain("team", &Scope::Unrestricted, false, &["bob".to_string()])
        .await
        .expect_err("a count nothing could read refuses however it was answered");
    assert!(
        refused.to_string().contains("could not be read"),
        "and it says why: {refused}"
    );

    // Readable again, and the removal that follows says how much it took: the
    // journal's sweep carries the files away with the drafts by construction,
    // so a count that left them out would be a number smaller than the loss.
    std::fs::remove_file(&files).unwrap();
    f.file("team", "bob", "assets/deck.png", DECK_PNG).await;
    let report = f
        .engine
        .unregister_domain("team", &Scope::Unrestricted, false, &["bob".to_string()])
        .await
        .unwrap();
    assert_eq!(
        report["drafts_swept"],
        serde_json::json!(1),
        "the one file went with the domain, and is counted: {report}"
    );
}

/// Every count of drafts is one count: the listing's own `my_drafts`, the
/// domain's sync status and the owner-only `drafts` view all answer rows plus
/// files, because all three are derived in one place.
#[tokio::test]
async fn my_drafts_and_the_drafts_route_count_files_beside_rows() {
    let f = review_fixture().await;
    f.draft("team", "alice", "plan.md", ALICE_DRAFT).await;
    f.file("team", "alice", "assets/deck.png", DECK_PNG).await;
    f.file("team", "alice", "assets/notes.png", DECK_PNG).await;

    let listing = f
        .engine
        .list_domains(
            &crystalline_service::params::ListDomainsParams::default(),
            &account("alice"),
        )
        .await
        .unwrap();
    let team = listing["domains"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == "team")
        .cloned()
        .unwrap();
    assert_eq!(
        team["my_drafts"],
        serde_json::json!(3),
        "her page and her two files: {team}"
    );

    let drafts = f
        .engine
        .domain_drafts("team", &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(
        drafts,
        serde_json::json!({ "actors": [{ "actor": "alice", "entries": 3 }] }),
        "and whoever owns the domain sees the same three: {drafts}"
    );
}

/// A member is told what a member is holding, on the one read every member
/// already makes.
///
/// The domain's sync status carries the same count, and a plain member cannot
/// reach that route: it is gated with the share verbs. So the listing carries
/// it too - a count of your own unshared work is a fact about you, and the
/// screen that says "you have work waiting here" must not need permission to
/// share in order to say it.
#[tokio::test]
async fn a_members_listing_carries_its_own_draft_count() {
    let f = screened_fixture().await;
    f.draft("team", "alice", "plan.md", ALICE_DRAFT).await;
    f.tombstone("team", "alice", "gone.md").await;
    f.draft("team", "owner", "fresh.md", ALICE_NEW).await;

    let listing = f
        .engine
        .list_domains(
            &crystalline_service::params::ListDomainsParams::default(),
            &account("alice"),
        )
        .await
        .unwrap();
    let team = listing["domains"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == "team")
        .cloned()
        .unwrap();
    assert_eq!(
        team["my_drafts"],
        serde_json::json!(2),
        "her own two, a deletion among them, and nothing of the owner's: {team}"
    );
    assert_eq!(team["review"], serde_json::json!("overlay"));
    let text = team.to_string();
    for secret in ["plan.md", "gone.md", "fresh.md", "owner"] {
        assert!(
            !text.contains(secret),
            "a listing row says how much, never whose or what, and {secret} is in it: {text}"
        );
    }
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

    // The removal paths do not fail - they are best effort by design - and they
    // sweep nothing. The count is another matter since Task 8: it is read from
    // the index rows rather than from the journal, so a journal nothing can
    // reach is not a count nothing can read, and the preview answers honestly
    // that nobody is drafting here.
    let preview = f
        .engine
        .domain_remove_preview("team", &Scope::Unrestricted, false, &[])
        .await
        .unwrap();
    assert_eq!(preview["drafts"], serde_json::json!([]));
    assert_eq!(
        preview["drafts_unknown"],
        serde_json::json!(false),
        "the rows answered, whatever the journal could not do: {preview}"
    );
    let report = f
        .engine
        .unregister_domain("team", &Scope::Unrestricted, false, &[])
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

    // The preview counts the rows, so an unreadable journal is not an
    // unreadable count: nobody is drafting in `team`, and that is what it says.
    // `drafts_unknown` still exists for the case that IS unknown - an index
    // that could not be asked - which is the only thing left that can set it.
    let preview = f
        .engine
        .domain_remove_preview("team", &Scope::Unrestricted, false, &[])
        .await
        .unwrap();
    assert_eq!(preview["drafts"], serde_json::json!([]));
    assert_eq!(
        preview["drafts_unknown"],
        serde_json::json!(false),
        "the rows are readable, so the count is: {preview}"
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

/// The scope one actor name means. The machine owner drafts as `owner`, which
/// is [`Scope::Unrestricted`]'s own overlay key, and anybody else drafts under
/// their account.
fn scope_of(actor: &str) -> Scope {
    if actor == "owner" {
        Scope::Unrestricted
    } else {
        account(actor)
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
    // The tombstone is about the PATH: nothing of the team's shows through at
    // `plan.md` for her any more. The address is a different question, and it
    // travelled with the document she moved, which is what the read below says.
    assert!(
        f.reads("plan.md", &alice).await.is_err(),
        "alice no longer sees it where the team's file is"
    );
    assert!(
        f.reads("plan", &alice)
            .await
            .expect("while the address finds her draft where she moved it")
            .contains("status: archived"),
        "which is her own copy rather than the team's"
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
        f.reads("notes", &alice).await.is_err(),
        "the deletion is a deletion for her"
    );
    // The move is not a deletion, and the tombstone it left at the source does
    // not make it one: a document travels verbatim, so the address travelled to
    // `archive/plan.md` with it, and her own view is the one view that has to
    // find her own work where she put it.
    assert!(
        f.reads("plan", &alice)
            .await
            .expect("the address follows her draft")
            .contains("and alice would add this"),
        "and the move moved it rather than ending it"
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

/// The file rows one actor's plan entry names, as `(path, tombstone)` pairs.
/// A file row is told apart from an engram row by its `kind`, which is the one
/// key an engram row does not carry.
fn plan_file_rows(plan: &serde_json::Value, actor: &str) -> Vec<(String, bool)> {
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
                .filter(|d| d["kind"] == serde_json::json!("file"))
                .map(|d| {
                    assert!(
                        d.get("permalink").is_none(),
                        "a file answers to no address, so it carries no permalink: {d}"
                    );
                    assert!(
                        d["conflict"].is_null(),
                        "and an address cannot be in its way: {d}"
                    );
                    (
                        d["path"].as_str().unwrap().to_string(),
                        d["tombstone"].as_bool().unwrap(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A file somebody drafted is a draft change like any other, so the plan counts
/// it and names it - and says which of the rows is a file, because what happens
/// to a file when it folds is not what happens to a page.
#[tokio::test]
async fn the_fold_plan_counts_a_file_as_an_entry_and_lists_it_as_kind_file() {
    let f = review_fixture().await;
    f.draft("team", "alice", "plan.md", ALICE_DRAFT).await;
    f.draft("team", "alice", "fresh.md", ALICE_NEW).await;
    f.file("team", "alice", "assets/deck.png", DECK_PNG).await;

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
        plan["actors"][0]["entries"],
        serde_json::json!(3),
        "two pages and one file are three draft changes: {plan}"
    );
    assert_eq!(
        plan_entries(&plan, "alice"),
        vec![
            ("fresh.md".to_string(), false, false),
            ("plan.md".to_string(), false, false),
            ("assets/deck.png".to_string(), false, false),
        ],
        "the engram rows are exactly what they were, and the file row is after \
         them: {plan}"
    );
    assert_eq!(
        plan_file_rows(&plan, "alice"),
        vec![("assets/deck.png".to_string(), false)],
        "and the file is the one row marked as one: {plan}"
    );
    for row in plan["actors"][0]["drafts"]
        .as_array()
        .unwrap()
        .iter()
        .take(2)
    {
        assert!(
            row.get("kind").is_none(),
            "an engram row keeps its exact shape, `kind` included by absence: {row}"
        );
        assert!(row["permalink"].is_string(), "{row}");
    }
}

/// A fold takes the files with the pages: a file somebody drafted becomes a
/// file the team has, a file they deleted goes, and both the bytes and the row
/// that describes them move together.
#[tokio::test]
async fn a_confirmed_fold_lands_the_files_and_applies_the_sidecars() {
    let f = review_fixture().await;
    std::fs::create_dir_all(f.domain_root("team").join("assets")).unwrap();
    std::fs::write(f.domain_root("team").join("assets/old.png"), OLD_PNG).unwrap();
    f.engine.sync(None).await.unwrap();
    assert_eq!(
        attachment_paths(&f).await,
        vec!["assets/old.png".to_string()],
        "the team's own file is the one the domain holds to start with"
    );

    f.draft("team", "alice", "plan.md", ALICE_DRAFT).await;
    f.file("team", "alice", "assets/deck.png", DECK_PNG).await;
    f.delete_file("team", "alice", "assets/old.png").await;

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
        serde_json::json!([{ "actor": "alice", "written": 2, "deleted": 1 }]),
        "the page and the file both landed, and the deletion counts as one: {receipt}"
    );

    assert_eq!(
        std::fs::read(f.domain_root("team").join("assets/deck.png")).unwrap(),
        DECK_PNG,
        "her file is the team's file now, byte for byte"
    );
    assert!(
        !f.domain_root("team").join("assets/old.png").exists(),
        "and the one she deleted is gone from the folder"
    );
    assert_eq!(
        attachment_paths(&f).await,
        vec!["assets/deck.png".to_string()],
        "the rows followed the files, both ways"
    );
    assert!(
        f.files_held("team", "alice").is_empty(),
        "and nothing of hers is left in the files overlay"
    );
    assert!(
        !f.state.join("overlays/team/alice/files").exists(),
        "the folder itself went with them"
    );
}

/// **Leaving review mode with any actor's files unreadable refuses outright**,
/// whatever anybody answered, and nothing of anybody's is touched.
///
/// The sweep at the end of the fold is a walk of the tree, not of the plan. So
/// an unreadable listing that merely dropped an actor out of the plan left the
/// sweep removing every actor it could still count - which with no folding
/// actor at all is somebody's only copy of their work deleted after the plan
/// said there was nothing to decide. The decision is keyed off the listing
/// itself for that reason, and the plan reports the actor it could not read
/// rather than omitting them.
///
/// The unreadable state is a plain file where a directory belongs, not a
/// permission bit: it is portable, deterministic, and it binds for root too.
#[tokio::test]
async fn leaving_review_mode_with_an_unreadable_files_folder_refuses_and_keeps_every_actors_files()
{
    let f = review_fixture().await;
    f.file("team", "alice", "assets/deck.png", DECK_PNG).await;
    // Bob's files folder is not a folder. He holds no rows either, so nothing
    // but this listing could ever have named him.
    let bob = f.state.join("overlays/team/bob");
    std::fs::create_dir_all(&bob).unwrap();
    std::fs::write(bob.join("files"), "not a folder").unwrap();

    // The plan reports him rather than leaving him out, because an actor a plan
    // does not name is an actor nobody can answer for.
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
    let bobs = plan["actors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["actor"] == serde_json::json!("bob"))
        .cloned()
        .expect("the actor whose files could not be read is in the plan");
    assert_eq!(
        bobs["files_unreadable"],
        serde_json::json!(true),
        "and the plan says why there is nothing to decide about him: {plan}"
    );

    // Both shapes of answer refuse, and the one with no folding actor at all is
    // the one that used to destroy alice's work.
    for answer in [
        folds(&[]),
        folds(&[("alice", FoldChoice::Fold)]),
        folds(&[("alice", FoldChoice::Discard)]),
    ] {
        let refused = f
            .engine
            .set_review_mode("team", None, answer, &Scope::Unrestricted)
            .await
            .expect_err("a files overlay nobody can list is not one anybody can answer for");
        let text = refused.to_string();
        assert!(
            text.contains("bob") && text.contains("could not be read"),
            "the refusal names the actor and why: {text}"
        );
        assert_eq!(
            f.files_held("team", "alice"),
            vec![("assets/deck.png".to_string(), false)],
            "and her only copy of her work is where she put it"
        );
        assert!(
            bob.join("files").is_file(),
            "as is whatever is standing in his way"
        );
        assert_eq!(
            review_key(&f, "team"),
            Some("overlay".to_string()),
            "the domain reviews changes still, so the same call works later"
        );
    }

    // Readable again - he holds nothing, so nothing names him - and the same
    // call goes through.
    std::fs::remove_file(bob.join("files")).unwrap();
    f.engine
        .set_review_mode(
            "team",
            None,
            folds(&[("alice", FoldChoice::Fold)]),
            &Scope::Unrestricted,
        )
        .await
        .expect("once the tree can be read, the answer can be carried out");
    assert_eq!(
        std::fs::read(f.domain_root("team").join("assets/deck.png")).unwrap(),
        DECK_PNG,
        "her file is the team's file now"
    );
}

/// An empty files folder is nothing to decide, so nobody is asked about it -
/// and it is still swept.
///
/// A folder with no entries in it can be left behind by a write that failed
/// after creating its parents. Listing the actor is what lets the sweep end it;
/// asking the person leaving review mode to fold or discard an actor who holds
/// nothing is a question with no content, and refusing a confirm that does not
/// answer it would make a domain that holds nothing impossible to take out of
/// review mode.
#[tokio::test]
async fn an_actor_with_an_empty_files_folder_is_asked_about_by_nobody_and_swept_anyway() {
    let f = review_fixture().await;
    let empty = f.state.join("overlays/team/alice/files");
    std::fs::create_dir_all(&empty).unwrap();

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
        plan["actors"],
        serde_json::json!([]),
        "an actor holding nothing is nobody the plan has to name: {plan}"
    );

    let receipt = f
        .engine
        .set_review_mode("team", None, folds(&[]), &Scope::Unrestricted)
        .await
        .expect("an answer that names nobody is the whole answer here");
    assert_eq!(receipt["applied"], serde_json::json!(true), "{receipt}");
    assert!(
        !empty.exists(),
        "and the folder went with the mode, as an empty one should"
    );
}

/// A statement that a direct domain takes changes directly stays a statement,
/// whatever state the overlay tree is in.
///
/// `PUT {"mode":"direct"}` on a domain that already takes changes directly and
/// holds no drafts writes nothing, closes no room and syncs nothing - which is
/// the whole point of that branch. A damaged overlay tree is a fact about a
/// domain nobody can draft in, so it has no bearing on the answer: the refusal
/// that protects a fold applies where there is a fold to protect.
#[tokio::test]
async fn a_no_op_statement_on_a_direct_domain_succeeds_over_a_damaged_overlay_tree() {
    let f = fixture().await;
    std::fs::create_dir_all(f.state.join("overlays")).unwrap();
    std::fs::write(f.state.join("overlays/team"), "not a folder").unwrap();

    let receipt = f
        .engine
        .set_review_mode("team", None, folds(&[]), &Scope::Unrestricted)
        .await
        .expect("a domain that takes changes directly already is told so, not refused");
    assert_eq!(receipt["applied"], serde_json::json!(true), "{receipt}");
    assert_eq!(receipt["folded"], serde_json::json!([]), "{receipt}");
    assert_eq!(receipt["discarded"], serde_json::json!([]), "{receipt}");
}

/// **A fold that failed part way folds the leftover file on the next call.**
///
/// The review key comes off in the middle of the verb, so everything after it
/// has to work with the key already off: the second call reads the overlay
/// whatever the key says, folds what is left and only then sweeps. If that read
/// ever asked the key again, the fold would find no files, and the sweep - a
/// walk of the tree - would delete exactly the ones it had just been unable to
/// see.
#[tokio::test]
async fn a_fold_that_failed_part_way_folds_the_leftover_file_on_the_next_call() {
    let f = review_fixture().await;
    f.draft("team", "alice", "plan.md", ALICE_DRAFT).await;
    f.file("team", "alice", "assets/deck.png", DECK_PNG).await;
    // The folder's `assets` is not a folder, so the file half of the fold fails
    // where the page half has already landed. Deterministic and portable: no
    // permission bit is involved.
    std::fs::write(f.domain_root("team").join("assets"), "not a folder").unwrap();

    let failed = f
        .engine
        .set_review_mode(
            "team",
            None,
            folds(&[("alice", FoldChoice::Fold)]),
            &Scope::Unrestricted,
        )
        .await
        .expect_err("the file could not be written into the folder");
    assert!(
        failed.to_string().contains("assets"),
        "the failure names the path it could not write: {failed}"
    );
    // The half-state the recovery starts from: the key is off, her page landed,
    // her rows are still there and her file is still hers.
    assert_eq!(review_key(&f, "team"), None);
    assert_eq!(
        std::fs::read_to_string(f.domain_root("team").join("plan.md")).unwrap(),
        ALICE_DRAFT,
        "the page half of the fold landed before the file half failed"
    );
    assert_eq!(
        f.files_held("team", "alice"),
        vec![("assets/deck.png".to_string(), false)],
        "and her file is untouched in the overlay"
    );

    // The same call again, once the way is clear.
    std::fs::remove_file(f.domain_root("team").join("assets")).unwrap();
    f.engine
        .set_review_mode(
            "team",
            None,
            folds(&[("alice", FoldChoice::Fold)]),
            &Scope::Unrestricted,
        )
        .await
        .expect("leaving review mode does not require the domain to be in it");

    assert_eq!(
        std::fs::read(f.domain_root("team").join("assets/deck.png")).unwrap(),
        DECK_PNG,
        "the leftover file folded on the repeat"
    );
    assert_eq!(
        attachment_paths(&f).await,
        vec!["assets/deck.png".to_string()],
        "with its row beside it"
    );
    assert!(
        f.files_held("team", "alice").is_empty(),
        "and the overlay is empty"
    );
}

/// One actor's fold is one actor's files, and nobody else's travel with them.
#[tokio::test]
async fn a_fold_carries_only_the_folding_actors_files() {
    let f = review_fixture().await;
    f.file("team", "alice", "assets/hers.png", DECK_PNG).await;
    f.file("team", "bob", "assets/his.png", OLD_PNG).await;

    let receipt = f
        .engine
        .set_review_mode(
            "team",
            None,
            folds(&[("alice", FoldChoice::Fold), ("bob", FoldChoice::Discard)]),
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert_eq!(
        receipt["folded"],
        serde_json::json!([{ "actor": "alice", "written": 1, "deleted": 0 }]),
        "one file folded, and it is hers: {receipt}"
    );
    assert_eq!(
        receipt["discarded"],
        serde_json::json!([{ "actor": "bob", "entries": 1 }]),
        "and his ended where it stood: {receipt}"
    );

    assert_eq!(
        attachment_paths(&f).await,
        vec!["assets/hers.png".to_string()],
        "the folder gained hers and nothing of his"
    );
    assert!(
        !f.domain_root("team").join("assets/his.png").exists(),
        "his bytes never reached the folder"
    );
    assert!(
        f.files_held("team", "alice").is_empty() && f.files_held("team", "bob").is_empty(),
        "and both overlays are empty, whichever answer each of them gave"
    );
}

/// A discard is the other answer for a file too: the bytes end with the
/// directory and the folder never hears about any of it.
#[tokio::test]
async fn a_discard_removes_the_files_folder_and_leaves_the_tree_alone() {
    let f = review_fixture().await;
    std::fs::create_dir_all(f.domain_root("team").join("assets")).unwrap();
    std::fs::write(f.domain_root("team").join("assets/old.png"), OLD_PNG).unwrap();
    f.engine.sync(None).await.unwrap();
    let before = f.tree("team");

    f.file("team", "alice", "assets/deck.png", DECK_PNG).await;
    f.delete_file("team", "alice", "assets/old.png").await;

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
        "her file and her deletion are the two things that ended: {receipt}"
    );

    assert_eq!(
        f.tree("team"),
        before,
        "the folder is byte for byte as it was"
    );
    assert!(
        f.domain_root("team").join("assets/old.png").exists(),
        "the file she deleted in her draft is still the team's"
    );
    assert!(
        !f.domain_root("team").join("assets/deck.png").exists(),
        "and the one she added never became anybody's"
    );
    assert_eq!(
        attachment_paths(&f).await,
        vec!["assets/old.png".to_string()],
        "the rows say the same"
    );
    assert!(
        !f.state.join("overlays/team/alice/files").exists(),
        "her files overlay went with her drafts"
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
    let joined = sessions.join("team", "plan", None).await.unwrap();
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

    let joined = sessions.join("team", "plan", None).await.unwrap();
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
    // And it says what happened, because the collision is not one the operator
    // could have seen when they answered the plan: the folder moved under them
    // while this call was closing its rooms.
    assert!(
        words.contains("The folder changed while this domain's co-editing rooms were being closed")
            && words.contains("the domain reviews changes again")
            && words.contains("out-of-band work"),
        "the refusal says why it could not have been foreseen, and where the change went: {words}"
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
    let _joined = sessions.join("team", "plan", None).await.unwrap();

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

// --- one view of what is real to an actor -----------------------------------

/// A base engram this reader deletes, so a tombstone is one of the four shapes.
const OLD: &str = "---\ntype: engram\ntitle: Old\npermalink: old\ntags:\n  - team\nstatus: stable\nrecorded_at: 2026-01-02\n---\n\n# Old\n\n- [decision] on its way out #team\n";
/// A base engram this reader renames, which is the shape a projection gets
/// wrong: a tombstone at the old path and an entry at the new one carrying the
/// same address.
const MOVING: &str = "---\ntype: engram\ntitle: Moving\npermalink: moving\ntags:\n  - team\nstatus: stable\nrecorded_at: 2026-01-02\n---\n\n# Moving\n\n- [decision] about to be renamed #team\n";

/// The paths a surface says stand, sorted, as one comparable list.
fn sorted(mut paths: Vec<String>) -> Vec<String> {
    paths.sort();
    paths
}

/// Four surfaces that each used to derive their own projection of an overlay -
/// the fold plan, the staged share tree, the sweep's listing and a browse level
/// - asked about one actor holding all four shapes at once, and made to agree.
///
/// The fourth shape is the one the Task 7 review actually caught drifting: a
/// rename inside one overlay is a PAIR, a tombstone at the old path and an
/// entry at the new one carrying the same address, and a projection that does
/// not subtract the tombstone calls it a collision on one surface and folds it
/// without complaint on another.
#[tokio::test]
async fn one_view_answers_the_fold_the_share_the_sweep_and_the_browse() {
    let f = reviewed_origin_fixture().await;
    let root = f.domain_root("team");
    std::fs::write(root.join("old.md"), OLD).unwrap();
    std::fs::write(root.join("moving.md"), MOVING).unwrap();
    f.engine.sync(None).await.unwrap();
    // The folder is exactly what the team reviewed, so nothing here is
    // out-of-band work and the share below detects the overlay alone.
    f.snapshot_origin("team");
    // ...and the copies a pull records beside the stamps, which are the side a
    // share of a reviewing domain stages its tree from.
    for entry in walkdir::WalkDir::new(&root).into_iter().flatten() {
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        let bytes = std::fs::read(entry.path()).unwrap();
        crystalline_remote::state::write_base_file(&f.origins.join("team"), &rel, &bytes).unwrap();
    }

    let owner = Scope::Unrestricted;
    let who = Some("claude-code/2.0");

    // 1. a draft over a base file.
    let checksum = f.engine.read_engram(&read("plan"), &owner).await.unwrap()["checksum"]
        .as_str()
        .unwrap()
        .to_string();
    f.engine
        .save_engram(
            &crystalline_service::params::SaveParams {
                domain: "team".to_string(),
                identifier: "plan".to_string(),
                content: PLAN.replace("title: Plan", "title: The owner's Plan"),
                expected_checksum: checksum,
            },
            &owner,
        )
        .await
        .unwrap();
    // 2. a draft at a path no file holds.
    f.engine
        .write_engram_as(
            &WriteParams {
                folder: Some("notes".to_string()),
                ..write_params("team", "Fresh", "- [idea] a page only the owner has #team")
            },
            who,
            &owner,
        )
        .await
        .unwrap();
    // 3. a tombstone.
    f.engine
        .delete_engram_as(
            &DeleteParams {
                identifier: "old".to_string(),
                domain: "team".to_string(),
                expected_checksum: None,
            },
            who,
            &owner,
        )
        .await
        .unwrap();
    // 4. a rename: a tombstone at the old path plus an entry at the new one.
    f.engine
        .move_engram(
            &crystalline_service::params::MoveParams {
                identifier: "moving".to_string(),
                domain: "team".to_string(),
                destination: "archive/moving.md".to_string(),
                destination_domain: None,
                update_links: None,
            },
            &owner,
        )
        .await
        .unwrap();

    // What this actor's view of the folder holds, which every surface below has
    // to answer with.
    let standing = [
        "MANIFEST.md".to_string(),
        "archive/moving.md".to_string(),
        "notes/fresh.md".to_string(),
        "plan.md".to_string(),
    ];
    let gone = vec!["moving.md".to_string(), "old.md".to_string()];

    // The browse level: the folders a draft invented are in the tree and the
    // paths this actor deleted are not.
    let browse = f
        .engine
        .browse_domain(
            &crystalline_service::params::BrowseParams {
                domain: "team".to_string(),
                path: None,
                depth: None,
                glob: None,
            },
            &owner,
        )
        .await
        .unwrap();
    let level: Vec<String> = browse["engrams"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["path"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        sorted(level.clone()),
        vec!["MANIFEST.md".to_string(), "plan.md".to_string()],
        "the root level is the base minus this actor's deletions: {browse}"
    );
    assert_eq!(
        browse["folders"],
        serde_json::json!(["archive", "notes"]),
        "and both folders a draft invented are in the tree: {browse}"
    );

    // The sweep: the same listing, counted.
    let sweep = f
        .engine
        .evolve_engrams(
            &crystalline_service::params::EvolveParams {
                domains: vec!["team".to_string()],
                ..Default::default()
            },
            &owner,
        )
        .await
        .unwrap();
    assert_eq!(
        sweep["engrams_scanned"],
        serde_json::json!(standing.len()),
        "the sweep scans exactly what this actor's view holds: {sweep}"
    );

    // The fold plan: every entry, and not one of them a conflict.
    let plan = f
        .engine
        .set_review_mode("team", None, ReviewModeConfirm::Preview, &owner)
        .await
        .unwrap();
    let entries = plan_entries(&plan, "owner");
    let planned_gone: Vec<String> = entries
        .iter()
        .filter(|(_, tombstone, _)| *tombstone)
        .map(|(path, _, _)| path.clone())
        .collect();
    let planned_standing: Vec<String> = entries
        .iter()
        .filter(|(_, tombstone, _)| !*tombstone)
        .map(|(path, _, _)| path.clone())
        .collect();
    assert_eq!(sorted(planned_gone), gone, "the plan's deletions: {plan}");
    assert_eq!(
        sorted(planned_standing),
        vec![
            "archive/moving.md".to_string(),
            "notes/fresh.md".to_string(),
            "plan.md".to_string()
        ],
        "the plan's drafts: {plan}"
    );
    assert!(
        entries.iter().all(|(_, _, conflict)| !*conflict),
        "and the rename is not a collision on this surface either: {plan}"
    );

    // The staged share tree: the same paths, as a delta against the folder.
    let share = f
        .engine
        .origin_share("team", None, None, None, None, ShareActor::Owner)
        .await
        .unwrap();
    assert_eq!(share["outcome"], "proposed", "{share}");
    let added: Vec<String> = share["added"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p.as_str().unwrap().to_string())
        .collect();
    let updated: Vec<String> = share["updated"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p.as_str().unwrap().to_string())
        .collect();
    let deleted: Vec<String> = share["deleted"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p.as_str().unwrap().to_string())
        .collect();
    let mut shared_standing = added;
    shared_standing.extend(updated);
    assert_eq!(
        sorted(shared_standing),
        vec![
            "archive/moving.md".to_string(),
            "notes/fresh.md".to_string(),
            "plan.md".to_string()
        ],
        "the share carries exactly the paths that stand in this actor's view: {share}"
    );
    assert_eq!(
        sorted(deleted),
        gone,
        "and takes away exactly the ones they deleted: {share}"
    );
}

/// A domain that takes changes directly is byte for byte what it was before the
/// dimension existed: the write is the file, the read is the file, and nobody
/// holds a draft of anything.
#[tokio::test]
async fn a_direct_domain_reads_and_writes_byte_for_byte_through_the_view() {
    let f = fixture().await;
    let owner = Scope::Unrestricted;

    f.engine
        .write_engram_as(
            &write_params("team", "Fresh", "- [idea] straight into the folder #team"),
            Some("claude-code/2.0"),
            &owner,
        )
        .await
        .unwrap();

    let on_disk = std::fs::read_to_string(f.domain_root("team").join("fresh.md"))
        .expect("a direct write is the file");
    assert!(on_disk.contains("straight into the folder"));
    assert_eq!(
        f.reads("fresh", &owner).await.unwrap(),
        on_disk,
        "and the read is that same file, byte for byte"
    );
    assert_eq!(
        f.reads("plan", &account("alice")).await.unwrap(),
        std::fs::read_to_string(f.domain_root("team").join("plan.md")).unwrap(),
        "for everybody, whoever they are"
    );
    for actor in ["", "owner", "alice"] {
        assert!(
            f.held("team", actor).await.is_empty(),
            "and no view of a direct domain holds a draft of anything"
        );
    }
}

/// The registered-set screen composes ahead of the actor dimension, never
/// behind it: a draft of alice's in a domain a reader may not see is absent
/// from every surface, and so is the domain.
#[tokio::test]
async fn a_draft_in_a_hidden_domain_is_still_invisible_through_the_view() {
    let f = screened_fixture().await;
    let alice = account("alice");
    let out = account("out");
    f.engine
        .write_engram_as(
            &write_params("team", "Fresh", "- [idea] a page only alice has #team"),
            Some("claude-code/2.0-for-alice"),
            &alice,
        )
        .await
        .unwrap();

    let browse = f
        .engine
        .browse_domain(
            &crystalline_service::params::BrowseParams {
                domain: "team".to_string(),
                path: None,
                depth: None,
                glob: None,
            },
            &out,
        )
        .await
        .expect_err("a browse of a domain this reader may not see is an unknown domain");
    assert!(
        browse.to_string().contains("team") && !browse.to_string().contains("Fresh"),
        "and it says nothing about what is in there: {browse}"
    );

    let sweep = f
        .engine
        .evolve_engrams(
            &crystalline_service::params::EvolveParams {
                domains: vec!["team".to_string()],
                ..Default::default()
            },
            &out,
        )
        .await
        .expect_err("and so is a sweep of it");
    assert!(!sweep.to_string().contains("Fresh"), "{sweep}");

    let read = f.reads("fresh", &out).await.expect_err("and so is a read");
    assert!(!read.contains("only alice has"), "{read}");
    assert!(
        f.reads("fresh", &alice)
            .await
            .unwrap()
            .contains("only alice has"),
        "while her own draft still reads for her"
    );
}

/// A reader with no identity is not a refusal on the read side and is one on
/// the write side, and the asymmetry is the whole of it: they read the folder
/// the team reviewed, which is a complete answer, and they cannot write,
/// because there is no draft for the write to join.
#[tokio::test]
async fn a_write_with_no_identity_still_refuses_and_a_read_still_answers_the_base() {
    let f = review_fixture().await;
    f.draft("team", "alice", "plan.md", ALICE_DRAFT).await;

    let err = f
        .engine
        .write_engram_as(
            &write_params("team", "Fresh", "- [idea] nobody in particular #team"),
            None,
            &Scope::Anonymous,
        )
        .await
        .expect_err("a write with no identity is refused");
    assert_eq!(err.to_string(), crystalline_service::OVERLAY_NEEDS_IDENTITY);

    let base = f
        .reads("plan", &Scope::Anonymous)
        .await
        .expect("and the read answers rather than refusing");
    assert_eq!(
        base, PLAN,
        "with the folder the team reviewed, and nobody's draft of it"
    );
}

/// Every function in this crate's `src` that calls `needle`, as
/// `(file, enclosing fn)`, sorted and deduplicated.
///
/// The mechanism both allow-list guards below share, rather than two copies of
/// it: they pin two different seams onto another actor's rows and they have to
/// pin them the same way, or the newer one drifts into being weaker than the
/// older.
///
/// A source scan rather than a behavioural assertion, for the reason the index
/// census guard is one: the failure these pin is a call site added later in the
/// wrong place, which no request can be written to provoke in advance. And they
/// name the ENCLOSING FUNCTION rather than the file, because every read verb
/// lives in `engine.rs` too: a file-level allow-list would let `read_engram`
/// reach another actor's drafts and stay green.
fn call_sites(needle: &str) -> Vec<(String, String)> {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    /// The name a line declares a function under, if it declares one.
    fn declared_fn(line: &str) -> Option<&str> {
        let rest = line.trim_start();
        let rest = rest
            .strip_prefix("pub(crate) ")
            .or_else(|| rest.strip_prefix("pub "))
            .unwrap_or(rest);
        let rest = rest.strip_prefix("async ").unwrap_or(rest);
        let rest = rest.strip_prefix("fn ")?;
        let end = rest.find(|c: char| !c.is_alphanumeric() && c != '_')?;
        Some(&rest[..end])
    }
    let mut found: Vec<(String, String)> = Vec::new();
    for entry in walkdir::WalkDir::new(&src).into_iter().flatten() {
        if entry.path().extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let file = entry
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();
        let text = std::fs::read_to_string(entry.path()).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            if !line.contains(needle) {
                continue;
            }
            // The nearest `fn` at or above the call site: whose code this is.
            let owner = (0..=i)
                .rev()
                .find_map(|j| declared_fn(lines[j]))
                .unwrap_or("<file scope>")
                .to_string();
            found.push((file.clone(), owner));
        }
    }
    found.sort();
    found.dedup();
    found
}

/// Assert that `needle` is called from exactly `allowed`, and say what strayed.
fn only_these_reach(needle: &str, allowed: &[(&str, &str)], complaint: &str) {
    let mut expected: Vec<(String, String)> = allowed
        .iter()
        .map(|(f, n)| ((*f).to_string(), (*n).to_string()))
        .collect();
    expected.sort();
    assert_eq!(call_sites(needle), expected, "{complaint}");
}

/// Another actor's view is buildable, because the fold, a withdrawal, a
/// conflict resolution, a share and the convergence a pull runs all
/// legitimately name somebody who is not the caller - and it is reachable from
/// those surfaces alone.
///
/// A source scan rather than a behavioural assertion, for the reason the index
/// census guard is one: the failure this pins is a call site added later in the
/// wrong place, which no read request can be written to provoke in advance. And
/// it names the ENCLOSING FUNCTION rather than the file, because every read
/// verb lives in `engine.rs` too: a file-level allow-list would let
/// `read_engram` reach another actor's drafts and stay green.
#[test]
fn another_actors_view_is_reached_only_by_the_owner_gated_surfaces() {
    let allowed = [
        // The constructor itself.
        ("domain_view.rs", "for_actor"),
        // The convergence pass a pull runs, which walks every actor's entries.
        ("engine.rs", "converge_pulled_overlays"),
        // ...and the rename it performs when the base carried the draft along.
        ("engine.rs", "move_draft_with_the_base"),
        // The fold, which ends every actor's drafts on the way out.
        ("engine.rs", "leave_review_mode"),
        // A withdrawal, which takes back what one actor proposed.
        ("engine.rs", "revert_into_overlay"),
        // A conflict resolution inside one actor's own draft.
        ("engine.rs", "resolve_in_overlay"),
        // A share resolved through `ShareActor`.
        ("engine.rs", "stage_overlay_share"),
        // A write made INSIDE somebody else's draft: the one write-side
        // caller, and the only one there will be. A join is not something a
        // request asserts - it is a record this process minted, for an account
        // that presented a share-link the draft's own author minted on that
        // draft - so the owner it names is the store's word rather than the
        // caller's. See `DomainView::for_write_joined`, which spells out the
        // three checks that still stand between a join and a write.
        ("domain_view.rs", "for_write_joined"),
        // The co-editing saver: a room is a room over ONE overlay document,
        // and this is the view it reads and writes that document through. The
        // owner never comes from the socket - it comes from the key the room
        // was opened under, and the collab upgrade route is what decided that
        // key: your own document needs nothing, and somebody else's needs a
        // live share-link of that author's naming you plus a live join this
        // session opened on it. See `room_view` in
        // crates/service/src/collab/session.rs.
        ("session.rs", "room_view"),
    ];
    only_these_reach(
        "for_actor(",
        &allowed,
        "somebody else's view is reachable from a function that is not one of the owner-gated \
         surfaces; a read verb that reaches it answers one reader with another reader's drafts",
    );
}

/// The OTHER seam onto another actor's rows, held to the same rule by the same
/// mechanism.
///
/// `Engine::overlay_draft_at` reads one named actor's draft at one path without
/// building a view at all, so the `for_actor` guard above says nothing about
/// it - and it is the door the share-link surface actually goes through. Left
/// to a doc comment it would be one call site away from answering a reader with
/// another reader's unshared work, which is the single worst failure this mode
/// can have.
///
/// What makes each caller safe is the same thing in two forms: the actor it
/// names comes from the caller's OWN identity (the author surfaces) or from a
/// grant row this instance minted (the grantee surfaces). No caller takes an
/// actor from a request, and a caller that did would have to be added here.
#[test]
fn another_actors_draft_is_read_only_by_the_grant_surface() {
    let allowed = [
        // The seam itself.
        ("engine.rs", "overlay_draft_at"),
        // Minting a link and listing the links on a draft, both of which name
        // the CALLER's own account - so these read nobody else's rows at all,
        // and are here because the scan cannot tell that from the outside.
        ("draft_links.rs", "mint"),
        ("draft_links.rs", "list"),
        // Redeeming a link and joining the draft it opens: the actor is the
        // grant row's `owner`, written when its author minted the link.
        ("draft_links.rs", "open_link"),
        // The read a grant widens, at the one path the grant names.
        ("engine.rs", "granted_read"),
        // Which granted draft a name opens, for the two surfaces that have a
        // name rather than a path: the teaching refusal for a write that named
        // a draft the caller's own view cannot resolve, and the collab upgrade
        // deciding whose document a room is over. Both take the owner from a
        // grant row this instance minted, never from the request - the caller
        // may SAY whose draft they mean, and this is what checks it.
        ("engine.rs", "granted_draft_named"),
        // The freshness check in front of that refusal: a link whose draft has
        // gone refuses nothing.
        ("engine.rs", "screen_granted_path"),
        // Which of the owner's files a join carries: the references are read
        // off the granted draft itself.
        ("engine.rs", "screen_joined_attachment"),
    ];
    only_these_reach(
        "overlay_draft_at(",
        &allowed,
        "another actor's draft is read from a function that is not one of the share-link \
         surfaces; a read that reaches it answers one reader with another reader's unshared work",
    );
}

/// The THIRD seam, and the one the other two say nothing about: where an
/// overlay row goes away, which is where its share-links and its joins have to
/// go away with it.
///
/// A grant lasts exactly as long as the thing it grants. A row that is taken
/// away while a link on it stands leaves that link merely dormant, to spring
/// back onto whatever its author drafts at the path next - a different text,
/// written after they took the first one back, handed to somebody neither of
/// them would have told - and leaves whoever was inside it writing into an
/// entry that is not there. So the ending is not a thing each verb remembers:
/// it lives at the two places a row can stop being a draft, and this pins that
/// there are still only two.
///
/// * **Removed** - the row goes, which only `DomainView::clear_row` does. The
///   discard, the fold, a withdrawal, a conflict resolution, a settled
///   convergence and both of a rename's undo paths all reach it through
///   `DomainView::drop`, which ends the links beside it, so each inherits the
///   ending rather than repeating it. Its one other caller is
///   `DomainView::drop_mid_move`, the second guard below: a move's source is
///   taken away before the move is finished, so its ending waits for the
///   destination rather than firing on the way to a move that may not happen.
/// * **Replaced** - the row stays and stops being a draft of the page,
///   standing as this actor's deletion of what the team holds instead. Two
///   verbs do that, the delete and the move's source half, and each ends the
///   grants itself beside its call to the drop.
///
/// The WRITER is deliberately no such seam: a draft being saved is the same
/// draft, and ending its links on every save would mean a grant that survived
/// only until its author next typed.
///
/// A source scan for the reason the two guards above are one - the failure it
/// pins is a call site added later in the wrong place, which no request can be
/// written to provoke in advance. It walks `crates/service/src` alone, which is
/// the frame that matters: the index crate holds the two backend
/// implementations of the clearing statement, and a caller that took a row away
/// without ending its grants would be added here, above them.
#[test]
fn an_overlay_row_goes_away_only_where_its_grants_and_joins_end() {
    only_these_reach(
        "clear_overlay_entry(",
        // The one remover, private to the view and reached by its two
        // wrappers: `drop`, which ends the links and joins after its
        // transaction commits, and `drop_mid_move`, whose caller ends them
        // once the move it is half of has happened.
        &[("domain_view.rs", "clear_row")],
        "an overlay row is cleared from a function that is not the removal seam; a draft taken away          there keeps its share-links, which spring back onto whatever its author drafts at that          path next, and keeps the sessions that were writing inside it",
    );
    only_these_reach(
        "write_overlay_tombstone(",
        &[
            // The seam itself.
            ("engine.rs", "write_overlay_tombstone"),
            // This actor's deletion of the page the team holds, standing where
            // their draft of it stood. Ends the grants beside it.
            ("engine.rs", "delete_engram_as"),
            // The source half of a rename, which is the same replacement at
            // the path the draft left. Ends the grants beside it too.
            ("domain_view.rs", "move_within"),
        ],
        "a draft is replaced by a tombstone from a function that does not end its grants and          joins; the row is no longer a draft of that page, so a link on it opens something its          author never shared and a session inside it is inside somebody's deletion",
    );
}

/// And the removal that defers its ending is the move's source alone.
///
/// A move is two writes, and until the second lands the move has not happened:
/// the source goes back to exactly what its author held. So the source's
/// removal must not end their share-links on the way to a move that may fail -
/// and nothing else may borrow that deferral, because a verb that took a row
/// away and ended nothing would leave a link standing on a draft that is not
/// there, ready to spring back onto whatever its author drafts at that path
/// next.
///
/// Both movers end the links themselves once the destination has landed, which
/// no scan can see; what this pins is that there are only the two of them.
#[test]
fn the_move_source_is_the_only_deferred_removal() {
    only_these_reach(
        "drop_mid_move(",
        &[
            // The wrapper itself.
            ("domain_view.rs", "drop_mid_move"),
            // The author's own rename: its source half, and the rollback that
            // puts the source back when the destination would not take it.
            ("domain_view.rs", "move_within"),
            // The rename a pull performs when the base carried the draft
            // along, which never touches the move verb.
            ("engine.rs", "move_draft_with_the_base"),
        ],
        "a removal defers the ending of a draft's share-links and joins from a function that is \
         not one half of a move; whatever it takes away, nothing ends the links on it, and the \
         next draft at that path inherits them",
    );
}

// --- Task 11c: links resolve onto the author's own drafts -------------------

/// A base engram that points at the plan, for the tests about what a team link
/// means to a reader who has redrafted or deleted what it points at.
const CHARTER: &str = "---\ntype: engram\ntitle: Charter\npermalink: charter\ntags:\n  - team\nstatus: stable\nrecorded_at: 2026-01-02\n---\n\n# Charter\n\n- relates_to [[Plan]]\n- cites [[Nobody Wrote This]]\n";

impl Fixture {
    /// One base file written into the folder the team shares, and indexed.
    async fn base_file(&self, domain: &str, path: &str, text: &str) {
        std::fs::write(self.domain_root(domain).join(path), text).unwrap();
        self.engine.sync(None).await.unwrap();
    }

    /// One reader's whole `read_engram` answer.
    async fn reads_json(&self, identifier: &str, scope: &Scope) -> serde_json::Value {
        self.engine
            .read_engram(&read(identifier), scope)
            .await
            .unwrap()
    }

    /// One reader's context slice around an anchor.
    async fn context(
        &self,
        anchor: &str,
        scope: &Scope,
    ) -> crystalline_service::engine::Result<serde_json::Value> {
        self.engine
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
    }
}

/// The permalinks of a slice's nodes, sorted, each marked when the payload
/// calls it the reader's own draft.
fn slice_nodes(value: &serde_json::Value) -> Vec<String> {
    let mut out: Vec<String> = value["nodes"]
        .as_array()
        .expect("the slice lists nodes")
        .iter()
        .map(|n| {
            let mark = if n["draft"] == serde_json::json!(true) {
                "*"
            } else {
                ""
            };
            format!("{}{mark}", n["permalink"].as_str().unwrap())
        })
        .collect();
    out.sort();
    out
}

/// Whether each relation and prose link of a read answers to something, in the
/// order the document wrote them.
fn ref_verdicts(value: &serde_json::Value) -> Vec<bool> {
    value["relations"]
        .as_array()
        .unwrap()
        .iter()
        .chain(value["links"].as_array().unwrap())
        .map(|r| r["resolved"] == serde_json::json!(true))
        .collect()
}

/// Two drafts that point at each other are an edge, for the one person who can
/// read both of them.
///
/// The case the whole task exists for: somebody drafting several pages at once
/// works the way they work in a direct domain, so the link between two of their
/// own drafts has to be a link. It is theirs alone, because a draft is one
/// person's private reading of a page and an edge onto one would tell everybody
/// else that it exists.
#[tokio::test]
async fn two_drafts_linking_each_other_form_an_edge_for_their_author_only() {
    let f = review_fixture().await;
    let alice = account("alice");
    let bob = account("bob");

    for (title, body) in [
        (
            "Fresh",
            "- [idea] the first half #team\n\n- relates_to [[Plan Two]]",
        ),
        (
            "Plan Two",
            "- [idea] the second half #team\n\n- relates_to [[Fresh]]",
        ),
    ] {
        f.engine
            .write_engram_as(&write_params("team", title, body), None, &alice)
            .await
            .unwrap();
    }

    assert_eq!(
        slice_nodes(&f.context("crystalline://team/fresh", &alice).await.unwrap()),
        vec!["fresh*".to_string(), "plan-two*".to_string()],
        "from one of her drafts she reaches the other, and both say they are hers"
    );
    assert_eq!(
        slice_nodes(
            &f.context("crystalline://team/plan-two", &alice)
                .await
                .unwrap()
        ),
        vec!["fresh*".to_string(), "plan-two*".to_string()],
        "and the same from the other end"
    );

    for stranger in [&bob, &Scope::Anonymous] {
        assert!(
            f.context("crystalline://team/fresh", stranger)
                .await
                .is_err(),
            "her draft is no anchor of theirs"
        );
        assert_eq!(
            slice_nodes(
                &f.context("crystalline://team/plan", stranger)
                    .await
                    .unwrap()
            ),
            vec!["plan".to_string()],
            "and neither draft is anywhere in the graph the team's files draw"
        );
    }
}

/// A team link into a page one reader has deleted is broken for that reader and
/// sound for everybody else.
///
/// The link is written in a file nobody has touched. What differs is the view it
/// is read in: her deletion is a deletion, so the page it names is not there for
/// her, and saying otherwise would be the deletion undone by a link.
#[tokio::test]
async fn a_base_link_into_a_path_the_reader_tombstoned_reads_unresolved_for_them_and_resolved_for_a_stranger()
 {
    let f = review_fixture().await;
    let alice = account("alice");
    let bob = account("bob");
    f.base_file("team", "charter.md", CHARTER).await;

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

    assert_eq!(
        ref_verdicts(&f.reads_json("charter", &alice).await),
        vec![false, false],
        "the charter's link to the plan dangles for her, and so does the one \
         nobody ever wrote"
    );
    assert_eq!(
        ref_verdicts(&f.reads_json("charter", &bob).await),
        vec![true, false],
        "and for him the plan is where it always was"
    );

    let graph = async |scope: &Scope| {
        f.engine
            .graph_neighborhood("crystalline://team/charter", 1, 50, scope)
            .await
            .unwrap()
    };
    assert_eq!(
        slice_nodes(&graph(&alice).await),
        vec!["charter".to_string()],
        "her graph draws no arrow into a page she has deleted"
    );
    assert_eq!(
        slice_nodes(&graph(&bob).await),
        vec!["charter".to_string(), "plan".to_string()],
        "his draws the one the file describes"
    );
}

/// A team link nobody could answer is answered by the reader's own draft.
///
/// The other end of the same sentence. The charter names a page that does not
/// exist; alice writes it, as a draft; for her the link lands, and the sweep
/// and the graph follow it there.
#[tokio::test]
async fn an_unresolved_base_link_that_only_the_readers_draft_answers_reads_resolved_for_them() {
    let f = review_fixture().await;
    let alice = account("alice");
    let bob = account("bob");
    f.base_file("team", "charter.md", CHARTER).await;

    f.engine
        .write_engram_as(
            &write_params(
                "team",
                "Nobody Wrote This",
                "- [idea] somebody did after all #team",
            ),
            None,
            &alice,
        )
        .await
        .unwrap();

    assert_eq!(
        ref_verdicts(&f.reads_json("charter", &alice).await),
        vec![true, true],
        "both of the charter's links land for her now"
    );
    assert_eq!(
        ref_verdicts(&f.reads_json("charter", &bob).await),
        vec![true, false],
        "and the second still dangles for everybody else"
    );
    assert_eq!(
        slice_nodes(
            &f.context("crystalline://team/charter", &alice)
                .await
                .unwrap()
        ),
        vec![
            "charter".to_string(),
            "nobody-wrote-this*".to_string(),
            "plan".to_string()
        ],
        "and her slice walks the team's link onto the page she wrote"
    );
}

/// A draft reached by following a link says it is the reader's own, and a base
/// row beside it says nothing at all.
///
/// One line in the payload, and the silence beside it is the other half: a
/// domain that takes changes directly answers the JSON it always answered.
#[tokio::test]
async fn a_draft_reached_through_build_context_is_marked_the_readers_own() {
    let f = review_fixture().await;
    let alice = account("alice");

    f.engine
        .write_engram_as(
            &write_params(
                "team",
                "Fresh",
                "- [idea] a page only alice has #team\n\n- relates_to [[Plan]]",
            ),
            None,
            &alice,
        )
        .await
        .unwrap();

    assert_eq!(
        slice_nodes(&f.context("crystalline://team/plan", &alice).await.unwrap()),
        vec!["fresh*".to_string(), "plan".to_string()],
        "the base anchor reaches her draft through the draft's own link back, \
         and only the draft is marked"
    );

    let hers = f.reads_json("fresh", &alice).await;
    assert_eq!(
        hers["draft"],
        serde_json::json!(true),
        "reading the draft says the same thing: {hers}"
    );
    let theirs = f.reads_json("plan", &alice).await;
    assert!(
        theirs.get("draft").is_none(),
        "and reading the team's own page says nothing about drafts: {theirs}"
    );
}

/// A reference never lands on somebody else's draft, in any verb.
///
/// The rule that keeps a reviewing domain honest: a resolved edge onto a page
/// only its author can read would tell everybody else the page exists. Task 12's
/// share-link grants are the one thing that will ever widen this, and they widen
/// the candidate set rather than adding a second rule.
#[tokio::test]
async fn another_actors_draft_is_never_a_link_target() {
    let f = review_fixture().await;
    let alice = account("alice");
    let bob = account("bob");

    f.engine
        .write_engram_as(
            &write_params("team", "Fresh", "- [idea] a page only alice has #team"),
            None,
            &alice,
        )
        .await
        .unwrap();
    f.engine
        .write_engram_as(
            &write_params(
                "team",
                "Bobs idea",
                "- [idea] a page only bob has #team\n\n- relates_to [[Fresh]]",
            ),
            None,
            &bob,
        )
        .await
        .unwrap();

    assert_eq!(
        ref_verdicts(&f.reads_json("bobs-idea", &bob).await),
        vec![false],
        "his link names a page he cannot read, so it names nothing"
    );
    assert_eq!(
        slice_nodes(
            &f.context("crystalline://team/bobs-idea", &bob)
                .await
                .unwrap()
        ),
        vec!["bobs-idea*".to_string()],
        "and his graph has no edge to draw"
    );
    assert!(
        f.engine
            .read_engram(&read("bobs-idea"), &alice)
            .await
            .is_err(),
        "and his page is not hers to read at all, which is the same rule said \
         from the other side"
    );
}

/// A draft that goes away takes the links that named it with it, back onto
/// whatever stands at that address now.
///
/// Her own page answered to the title `Plan` while it stood, because her rows
/// come first in her own view. Deleting it drops the row - there is no file
/// underneath to tombstone - and the link she wrote falls back onto the team's
/// page of that name in the same transaction, rather than being left naming a
/// row nobody holds.
#[tokio::test]
async fn a_discarded_draft_releases_the_links_that_pointed_at_it() {
    let f = review_fixture().await;
    let alice = account("alice");

    // A page of her own answering to the team's own title, at a path no file
    // holds. Written as a row directly, because the address gate is what stops
    // a verb from spending an address the folder already holds.
    f.draft(
        "team",
        "alice",
        "her-plan.md",
        "---\ntype: engram\ntitle: Plan\npermalink: her-plan\ntags:\n  - team\nstatus: draft\nrecorded_at: 2026-01-03\n---\n\n# Plan\n\n- [idea] her own plan #team\n",
    )
    .await;
    f.engine
        .write_engram_as(
            &write_params(
                "team",
                "Fresh",
                "- [idea] a page only alice has #team\n\n- relates_to [[Plan]]",
            ),
            None,
            &alice,
        )
        .await
        .unwrap();
    assert_eq!(
        slice_nodes(&f.context("crystalline://team/fresh", &alice).await.unwrap()),
        vec!["fresh*".to_string(), "her-plan*".to_string()]
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>(),
        "while her page stands, her link means her page"
    );

    f.engine
        .delete_engram_as(
            &DeleteParams {
                identifier: "her-plan".to_string(),
                domain: "team".to_string(),
                expected_checksum: None,
            },
            None,
            &alice,
        )
        .await
        .unwrap();

    assert_eq!(
        ref_verdicts(&f.reads_json("fresh", &alice).await),
        vec![true],
        "her link still lands"
    );
    assert_eq!(
        slice_nodes(&f.context("crystalline://team/fresh", &alice).await.unwrap()),
        vec!["fresh*".to_string(), "plan".to_string()],
        "on the team's page of that name, which is what she reads there now"
    );
}

/// A fold leaves every link bound, against the folder the team shares.
///
/// Two drafts that pointed at each other become two files that point at each
/// other, and the answer is the same answer read in a different view: nobody's
/// drafts, nobody's marker, and both links still landing.
#[tokio::test]
async fn a_fold_leaves_every_link_bound_against_the_folder() {
    let f = review_fixture().await;
    let alice = account("alice");

    for (title, body) in [
        (
            "Fresh",
            "- [idea] the first half #team\n\n- relates_to [[Plan Two]]",
        ),
        (
            "Plan Two",
            "- [idea] the second half #team\n\n- relates_to [[Fresh]]",
        ),
    ] {
        f.engine
            .write_engram_as(&write_params("team", title, body), None, &alice)
            .await
            .unwrap();
    }

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
    assert_eq!(receipt["applied"], serde_json::json!(true), "{receipt}");
    f.engine.sync(None).await.unwrap();

    for permalink in ["fresh", "plan-two"] {
        let value = f.reads_json(permalink, &alice).await;
        assert_eq!(
            ref_verdicts(&value),
            vec![true],
            "{permalink} still points at something the team holds: {value}"
        );
        assert!(
            value.get("draft").is_none(),
            "and nobody is drafting it any more: {value}"
        );
    }
    assert_eq!(
        slice_nodes(&f.context("crystalline://team/fresh", &alice).await.unwrap()),
        vec!["fresh".to_string(), "plan-two".to_string()],
        "the pair is the team's graph now"
    );
}
