//! The shared reindex driver, on both backends: a forced run re-reads every
//! file, destroys nothing it does not have to, prunes what disk no longer has,
//! and says so durably while it is in flight.
//!
//! Every body is a pure function of an `Arc<Mutex<dyn Store>>`, because the
//! driver takes the store handle rather than a locked store: the walk-and-hash
//! pass runs with no lock held, which is what lets a daemon keep answering
//! during a rebuild. Turso (in-memory) always runs; Postgres runs when
//! `CRYSTALLINE_TEST_POSTGRES_URL` is set and is skipped with a one-time note
//! when it is not.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use crystalline_index::{
    ChunkParams, DomainId, DomainKind, DomainStats, EMBED_PAGE_SIZE, EmbeddingProvider,
    EngramRecord, FileStamp, NoReindexHooks, Store, TursoStore, apply_scan, reindex_domains,
    run_embedding_pass, scan_domain,
};
use tokio::sync::Mutex;

// --- corpus helpers ----------------------------------------------------------

fn write(dir: &Path, rel: &str, content: &str) {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, content).unwrap();
}

fn engram(title: &str, permalink: &str, body: &str) -> String {
    format!(
        "---\ntype: engram\ntitle: {title}\npermalink: {permalink}\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# {title}\n\n{body}\n"
    )
}

const MODEL: &str = "fake-8";

fn params() -> ChunkParams {
    ChunkParams::for_model(MODEL)
}

// --- fake provider -----------------------------------------------------------

/// The deterministic, network-free provider the embedding suite uses: each word
/// hashes into one of eight buckets, L2-normalized. Deterministic matters here,
/// because these tests compare the exact vector a chunk carried before a rebuild
/// against the one it carries after.
struct FakeProvider;

fn embed_one(text: &str) -> Vec<f32> {
    let mut v = [0f32; 8];
    for tok in text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
    {
        let mut h: u64 = 0;
        for byte in tok.to_lowercase().bytes() {
            h = h.wrapping_mul(31).wrapping_add(byte as u64);
        }
        v[(h % 8) as usize] += 1.0;
    }
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm == 0.0 {
        let mut z = [0f32; 8];
        z[0] = 1.0;
        return z.to_vec();
    }
    v.iter().map(|x| x / norm).collect()
}

#[async_trait]
impl EmbeddingProvider for FakeProvider {
    async fn embed(&self, texts: &[String]) -> crystalline_index::Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|t| embed_one(t)).collect())
    }
    fn model_id(&self) -> &str {
        MODEL
    }
    fn dims(&self) -> usize {
        8
    }
    fn max_input_tokens(&self) -> usize {
        512
    }
}

// --- store helpers -----------------------------------------------------------

async fn embed_everything(store: &Arc<Mutex<dyn Store>>) {
    let store = store.lock().await;
    run_embedding_pass(&*store, &FakeProvider, |_, _| {})
        .await
        .unwrap();
}

async fn stats_of(store: &Arc<Mutex<dyn Store>>, name: &str) -> DomainStats {
    let store = store.lock().await;
    store
        .domain_stats()
        .await
        .unwrap()
        .into_iter()
        .find(|d| d.name == name)
        .unwrap_or_else(|| panic!("no stats for domain '{name}'"))
}

async fn domain_id(store: &Arc<Mutex<dyn Store>>, name: &str, root: &Path) -> DomainId {
    let store = store.lock().await;
    store
        .upsert_domain(name, Some(&root.to_string_lossy()), DomainKind::File)
        .await
        .unwrap()
}

/// Everything a reader of this index can see, for a before-and-after compare:
/// the named domains' engram counts, and the index-wide chunk and embedding
/// totals once, beside them rather than repeated into every row. The chunk and
/// coverage figures are not per-domain and must not read as though they were.
#[derive(Debug, PartialEq)]
struct Visible {
    /// `(domain, engrams)` for the domains asked about, in that order.
    domains: Vec<(String, i64)>,
    /// Chunk rows across the whole index.
    chunks: usize,
    /// Chunks carrying an embedding for the test model, across the whole index.
    embedded: usize,
}

async fn visible(store: &Arc<Mutex<dyn Store>>, names: &[&str]) -> Visible {
    let store = store.lock().await;
    let stats = store.domain_stats().await.unwrap();
    let coverage = store.embedding_coverage().await.unwrap();
    Visible {
        domains: names
            .iter()
            .map(|name| {
                let d = stats
                    .iter()
                    .find(|d| &d.name == name)
                    .unwrap_or_else(|| panic!("no stats for domain '{name}'"));
                (d.name.clone(), d.engrams)
            })
            .collect(),
        chunks: coverage.total_chunks,
        embedded: coverage.embedded_for(MODEL),
    }
}

/// The vectors of every engram's lead chunk in a domain, ordered by engram id:
/// the evidence that a rebuild carried the embeddings over rather than dropping
/// and recomputing them.
async fn lead_vectors(store: &Arc<Mutex<dyn Store>>, domain: DomainId) -> Vec<(i64, Vec<f32>)> {
    let store = store.lock().await;
    store
        .lead_vectors(domain, MODEL, None)
        .await
        .unwrap()
        .into_iter()
        .map(|v| (v.engram_id.0, v.vector))
        .collect()
}

/// Rewrite a file with new content of the same byte length, restoring its
/// modification time, so its recorded stamp still matches on disk: the one case
/// the mtime-and-size prefilter is designed to miss and a forced run must catch.
fn rewrite_behind_the_prefilter(path: &Path, content: &str) {
    let before = std::fs::metadata(path).unwrap();
    let was = before.modified().unwrap();
    assert_eq!(
        before.len() as usize,
        content.len(),
        "the replacement must be the same size, or the prefilter would catch it"
    );
    std::fs::write(path, content).unwrap();
    let f = std::fs::File::options().write(true).open(path).unwrap();
    f.set_times(std::fs::FileTimes::new().set_modified(was))
        .unwrap();
    let after = std::fs::metadata(path).unwrap();
    assert_eq!(
        after.modified().unwrap(),
        was,
        "the modification time was restored, so the prefilter still sees no change"
    );
    assert_eq!(after.len(), before.len());
}

// --- backend runner ----------------------------------------------------------

#[cfg(feature = "postgres")]
fn pg_url() -> Option<String> {
    use std::sync::Once;
    static NOTE: Once = Once::new();
    match std::env::var("CRYSTALLINE_TEST_POSTGRES_URL") {
        Ok(u) if !u.is_empty() => Some(u),
        _ => {
            NOTE.call_once(|| {
                eprintln!(
                    "note: skipping the postgres parity leg (CRYSTALLINE_TEST_POSTGRES_URL is unset); turso only"
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
        "rx_{}_{}_{:x}",
        std::process::id(),
        n,
        RandomState::new().hash_one(n)
    )
}

/// Run a parity body against both backends, each behind the same
/// `Arc<Mutex<dyn Store>>` handle the driver takes in production.
macro_rules! parity {
    ($name:ident, $body:path) => {
        #[tokio::test]
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

// --- 1: an interrupted rebuild leaves the old index serving ------------------

/// The incident, as a test. A forced rebuild that dies partway through must
/// leave every domain holding either its previous complete rows or its new
/// complete rows - never an empty or half-embedded set - and the domain it died
/// in must still say a rebuild was in flight.
///
/// The interruption is injected where a SIGTERM actually landed: after the
/// second domain's rebuild marker is stamped and before its apply commits
/// anything. Removing its root on disk makes the scan fail there, which is the
/// same window with none of the machinery a failing store decorator would need.
async fn interrupted_rebuild_leaves_the_old_index_serving(store: Arc<Mutex<dyn Store>>) {
    let tmp = tempfile::tempdir().unwrap();
    let a_root = tmp.path().join("a");
    let b_root = tmp.path().join("b");
    write(&a_root, "a1.md", &engram("A One", "a1", "alpha payload"));
    write(&a_root, "a2.md", &engram("A Two", "a2", "beta payload"));
    write(&b_root, "b1.md", &engram("B One", "b1", "gamma payload"));
    write(&b_root, "b2.md", &engram("B Two", "b2", "delta payload"));

    let targets = vec![
        ("a".to_string(), a_root.clone()),
        ("b".to_string(), b_root.clone()),
    ];
    reindex_domains(&*store, &targets, &params(), false, &NoReindexHooks)
        .await
        .unwrap();
    embed_everything(&store).await;

    let before = visible(&store, &["a", "b"]).await;
    assert_eq!(
        before.embedded, before.chunks,
        "the corpus starts fully embedded"
    );

    // The interruption: domain b cannot be scanned any more.
    std::fs::remove_dir_all(&b_root).unwrap();
    let err = reindex_domains(&*store, &targets, &params(), true, &NoReindexHooks)
        .await
        .expect_err("the forced run fails in domain b");
    assert!(
        err.to_string().contains("reindex of 'b' failed"),
        "the failure names the domain it happened in: {err}"
    );

    assert_eq!(
        visible(&store, &["a", "b"]).await,
        before,
        "every engram, chunk and embedding is exactly as it was: nothing was cleared"
    );
    assert!(
        stats_of(&store, "b").await.rebuild_started.is_some(),
        "the domain the run died in still says a rebuild was in flight"
    );
    assert!(
        stats_of(&store, "a").await.rebuild_started.is_none(),
        "the domain that finished its rebuild cleared its marker"
    );
}
parity!(
    an_interrupted_rebuild_leaves_the_old_index_serving,
    interrupted_rebuild_leaves_the_old_index_serving
);

// --- 2: a completed rebuild replaces the index in one step -------------------

/// What `--full` is for once it stops clearing: re-reading a file whose content
/// changed without its modification time or size moving, which a plain sync
/// skips by design. The engrams that did not change keep the exact embedding
/// they had, and the marker is gone when the run finishes.
async fn a_completed_rebuild_rereads_what_a_sync_skips(store: Arc<Mutex<dyn Store>>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("d");
    write(&root, "kept.md", &engram("Kept", "kept", "kept payload"));
    // Same byte length as its replacement below, so only the content differs.
    write(&root, "drifted.md", &engram("Drift", "drift", "aaaa bbbb"));

    let targets = vec![("d".to_string(), root.clone())];
    reindex_domains(&*store, &targets, &params(), false, &NoReindexHooks)
        .await
        .unwrap();
    embed_everything(&store).await;
    let domain = domain_id(&store, "d", &root).await;
    let kept_id = {
        let store = store.lock().await;
        store.lookup_id("d", "kept").await.unwrap().unwrap().0
    };
    let vectors_before = lead_vectors(&store, domain).await;

    rewrite_behind_the_prefilter(
        &root.join("drifted.md"),
        &engram("Drift", "drift", "cccc dddd"),
    );

    // A plain sync cannot see it: same mtime, same size, so the prefilter never
    // hashes the file at all.
    reindex_domains(&*store, &targets, &params(), false, &NoReindexHooks)
        .await
        .unwrap();
    let stale = body_of(&store, domain, "drifted.md").await;
    assert!(
        stale.contains("aaaa bbbb"),
        "a plain sync leaves the stale row: {stale}"
    );

    // The forced run re-reads every file, so it catches it.
    reindex_domains(&*store, &targets, &params(), true, &NoReindexHooks)
        .await
        .unwrap();
    let fresh = body_of(&store, domain, "drifted.md").await;
    assert!(
        fresh.contains("cccc dddd") && !fresh.contains("aaaa bbbb"),
        "the forced reindex rewrote it: {fresh}"
    );
    assert!(
        stats_of(&store, "d").await.rebuild_started.is_none(),
        "a completed rebuild clears its marker"
    );

    let vectors_after = lead_vectors(&store, domain).await;
    let kept_before = vectors_before
        .iter()
        .find(|(id, _)| *id == kept_id)
        .expect("the kept engram had a lead vector");
    let kept_after = vectors_after
        .iter()
        .find(|(id, _)| *id == kept_id)
        .expect("and still has one");
    assert_eq!(
        kept_before, kept_after,
        "an unchanged engram keeps its engram id and the exact embedding bytes it had"
    );
}
parity!(
    a_completed_rebuild_replaces_the_index_in_one_step,
    a_completed_rebuild_rereads_what_a_sync_skips
);

/// A forced resync keeps every overlay row, and proposes no deletion.
///
/// `--full` re-reads and re-upserts every file and prunes the rows disk no
/// longer has, and it derives that prune list by subtracting the walk from
/// `file_stamps`. A draft is on nobody's disk, so it survives this run for
/// exactly one reason: that snapshot carries the base predicate. Take the
/// predicate away and every draft in the index is proposed as a deletion on
/// every forced run - and on every ordinary sync too. The `deleted == 0` on the
/// report is the assertion that would have caught it: a draft silently dropped
/// leaves the same empty overlay as a draft that was never written.
///
/// A tombstone is in here beside an ordinary draft because it is the row with
/// no file behind it in the most literal sense - its base path is on disk and
/// its own reading of that path says "gone" - so if any shape of overlay row
/// were going to be mistaken for a stale row, it is this one.
async fn a_forced_resync_keeps_overlay_rows(store: Arc<Mutex<dyn Store>>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("d");
    write(&root, "a.md", &engram("A", "a", "base a"));
    write(&root, "b.md", &engram("B", "b", "base b"));
    let targets = vec![("d".to_string(), root.clone())];
    reindex_domains(&*store, &targets, &params(), false, &NoReindexHooks)
        .await
        .unwrap();
    let domain = domain_id(&store, "d", &root).await;

    // One draft and one tombstone, in two different actors' dimensions.
    {
        let store = store.lock().await;
        let base = store
            .all_engram_contents(domain)
            .await
            .unwrap()
            .into_iter()
            .find(|e| e.path == "a.md")
            .unwrap();
        let mut draft = EngramRecord::from_engram(
            &crystalline_core::parse_engram(&engram("A", "a", "alice's draft")).unwrap(),
            "a.md",
            FileStamp {
                mtime: 0,
                size: 0,
                sha256: "draft".to_string(),
            },
        );
        draft.permalink = base.permalink.clone();
        store.upsert_overlay(domain, "alice", &draft).await.unwrap();

        let mut stone = EngramRecord::from_engram(
            &crystalline_core::parse_engram(&engram("B", "b", "gone for bob")).unwrap(),
            "b.md",
            FileStamp {
                mtime: 0,
                size: 0,
                sha256: "stone".to_string(),
            },
        );
        stone.tombstone = true;
        store.upsert_overlay(domain, "bob", &stone).await.unwrap();

        // The sharpest one: a wholly new engram that exists only as a draft, so
        // its path is on nobody's disk at all. This is the row the delete
        // detection would propose for deletion on every run - the other two sit
        // at paths the walk does find, which hides the leak.
        let fresh = EngramRecord::from_engram(
            &crystalline_core::parse_engram(&engram("C", "c", "alice's new one")).unwrap(),
            "c.md",
            FileStamp {
                mtime: 0,
                size: 0,
                sha256: "fresh".to_string(),
            },
        );
        store.upsert_overlay(domain, "alice", &fresh).await.unwrap();

        assert_eq!(
            store.overlay_counts(domain).await.unwrap(),
            vec![("alice".to_string(), 2), ("bob".to_string(), 1)],
        );
    }

    let reports = reindex_domains(&*store, &targets, &params(), true, &NoReindexHooks)
        .await
        .unwrap();
    assert_eq!(
        reports.iter().map(|r| r.deleted).sum::<usize>(),
        0,
        "a forced resync proposes no deletion: no overlay row is on disk, and \
         none of them is a stale row either"
    );

    let store_guard = store.lock().await;
    let alice = store_guard
        .overlay_entry(domain, "alice", "a.md")
        .await
        .unwrap()
        .expect("alice's draft survived the rebuild");
    assert!(alice.content.contains("alice's draft"));
    assert!(!alice.tombstone);
    let bob = store_guard
        .overlay_entry(domain, "bob", "b.md")
        .await
        .unwrap()
        .expect("bob's tombstone survived it too");
    assert!(bob.tombstone, "and is still a tombstone");
    assert!(
        store_guard
            .overlay_entry(domain, "alice", "c.md")
            .await
            .unwrap()
            .is_some(),
        "and the draft with no file behind it survived, which is the one the \
         delete detection would have taken"
    );
    assert_eq!(
        store_guard.overlay_counts(domain).await.unwrap(),
        vec![("alice".to_string(), 2), ("bob".to_string(), 1)],
        "both actors still hold exactly what they held"
    );
    assert_eq!(
        store_guard.domain_stats().await.unwrap()[0].engrams,
        2,
        "and the base is the two files on disk, as it was"
    );
}
parity!(
    a_forced_resync_keeps_every_overlay_row,
    a_forced_resync_keeps_overlay_rows
);

/// The stored body of one path in a domain.
async fn body_of(store: &Arc<Mutex<dyn Store>>, domain: DomainId, path: &str) -> String {
    let store = store.lock().await;
    store
        .all_engram_contents(domain)
        .await
        .unwrap()
        .into_iter()
        .find(|e| e.path == path)
        .unwrap_or_else(|| panic!("no engram at '{path}'"))
        .content
}

// --- 3: embeddings are not destroyed -----------------------------------------

/// The whole point of the option, and the test that must fail loudly if anyone
/// reintroduces a clear before the rebuild: a forced reindex over an unchanged
/// corpus leaves nothing to embed, because every chunk's text is unchanged and
/// its embedding is carried over.
async fn a_forced_reindex_leaves_nothing_to_embed(store: Arc<Mutex<dyn Store>>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("d");
    for i in 0..6 {
        write(
            &root,
            &format!("e{i}.md"),
            &engram(&format!("E {i}"), &format!("e{i}"), "payload words here"),
        );
    }
    let targets = vec![("d".to_string(), root.clone())];
    reindex_domains(&*store, &targets, &params(), false, &NoReindexHooks)
        .await
        .unwrap();
    embed_everything(&store).await;

    let coverage_before = {
        let store = store.lock().await;
        store.embedding_coverage().await.unwrap()
    };
    assert!(coverage_before.total_chunks > 0);
    assert_eq!(
        coverage_before.embedded_for(MODEL),
        coverage_before.total_chunks,
        "the corpus starts fully embedded"
    );

    reindex_domains(&*store, &targets, &params(), true, &NoReindexHooks)
        .await
        .unwrap();

    let store = store.lock().await;
    let pending = store
        .chunks_needing_embedding(MODEL, None, EMBED_PAGE_SIZE, None)
        .await
        .unwrap();
    assert!(
        pending.is_empty(),
        "a forced reindex of an unchanged corpus re-embeds nothing, {} chunk(s) pending",
        pending.len()
    );
    let coverage_after = store.embedding_coverage().await.unwrap();
    assert_eq!(
        (
            coverage_after.total_chunks,
            coverage_after.embedded_for(MODEL)
        ),
        (
            coverage_before.total_chunks,
            coverage_before.embedded_for(MODEL)
        ),
        "coverage cannot go down across a rebuild"
    );
}
parity!(
    embeddings_survive_a_forced_reindex,
    a_forced_reindex_leaves_nothing_to_embed
);

// --- 5: deletes are still pruned ---------------------------------------------

/// What the empty-snapshot clear used to achieve and a forced resync must not
/// regress: a file removed from disk takes its row and its children with it.
async fn a_forced_reindex_still_prunes_deletes(store: Arc<Mutex<dyn Store>>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("d");
    write(
        &root,
        "stays.md",
        &engram("Stays", "stays", "- [note] kept\n"),
    );
    write(
        &root,
        "goes.md",
        &engram("Goes", "goes", "- [note] doomed observation\n"),
    );
    let targets = vec![("d".to_string(), root.clone())];
    reindex_domains(&*store, &targets, &params(), false, &NoReindexHooks)
        .await
        .unwrap();
    let before = stats_of(&store, "d").await;
    assert_eq!(before.engrams, 2);
    assert_eq!(before.observations, 2);

    std::fs::remove_file(root.join("goes.md")).unwrap();
    let reports = reindex_domains(&*store, &targets, &params(), true, &NoReindexHooks)
        .await
        .unwrap();
    assert_eq!(reports[0].deleted, 1, "the run reports the prune");

    let after = stats_of(&store, "d").await;
    assert_eq!(after.engrams, 1, "the removed engram's row is gone");
    assert_eq!(after.observations, 1, "and its children went with it");
    let store = store.lock().await;
    assert!(
        store.lookup_id("d", "goes").await.unwrap().is_none(),
        "nothing resolves the removed permalink any more"
    );
}
parity!(
    deletes_are_pruned_by_a_forced_reindex,
    a_forced_reindex_still_prunes_deletes
);

// --- the marker is cleared by the apply's own transaction --------------------

/// Where `end_rebuild` runs is the whole design, and this is what holds it: an
/// apply that fails must leave the marker standing, because the rebuild it was
/// stamped for did not land.
///
/// The failure is injected inside `apply_scan` rather than in the scan before
/// it, which is what the interruption tests above do: a transaction is already
/// open when the apply starts, so its own `begin` fails and it returns without
/// writing. Move `store.end_rebuild(domain)` above `store.begin()` - out of the
/// transaction on the near side - and this goes red, because the marker would
/// be cleared by a rebuild that never committed a row.
///
/// The test *commits* its own transaction afterwards rather than rolling it
/// back, and that is the whole point of the shape: a rollback would undo a
/// misplaced `end_rebuild` along with everything else and hide exactly the
/// defect under test. Committing keeps whatever the apply wrote before it
/// failed, which is what a reader would have seen.
///
/// What no injection here can distinguish: moving the call to *after*
/// `store.commit()` behaves identically to every observer, because the only
/// thing separating the two is a commit that fails, and nothing a test can do
/// to a `Store` through its public surface makes one fail. That direction is
/// held by `end_rebuild_is_called_inside_the_applys_transaction` below.
async fn a_failed_apply_leaves_the_marker_standing(store: Arc<Mutex<dyn Store>>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("d");
    write(&root, "a.md", &engram("A", "a", "alpha payload"));
    write(&root, "b.md", &engram("B", "b", "beta payload"));

    let targets = vec![("d".to_string(), root.clone())];
    reindex_domains(&*store, &targets, &params(), false, &NoReindexHooks)
        .await
        .unwrap();
    embed_everything(&store).await;
    let domain = domain_id(&store, "d", &root).await;
    let before = visible(&store, &["d"]).await;

    // The first lock window of a forced rebuild: stamp the marker, snapshot the
    // stamps, scan with no lock held. Exactly what the driver does.
    let now = "2026-09-14T10:00:00Z";
    let snapshot = {
        let store = store.lock().await;
        store.begin_rebuild(domain, now).await.unwrap();
        store.file_stamps(domain).await.unwrap()
    };
    let scan = scan_domain("d", &root, snapshot, &params(), true)
        .await
        .unwrap();
    assert_eq!(
        stats_of(&store, "d").await.rebuild_started.as_deref(),
        Some(now),
        "the rebuild is stamped before its apply runs"
    );

    // The injection: the apply cannot open its transaction.
    {
        let store = store.lock().await;
        store.begin().await.unwrap();
    }
    let err = {
        let store = store.lock().await;
        apply_scan(&*store, domain, scan)
            .await
            .expect_err("the apply fails with a transaction already open")
    };
    assert!(!err.to_string().is_empty());
    {
        // Commit, not roll back: a rollback would revert a misplaced
        // `end_rebuild` too and the assertion below could never fail.
        let store = store.lock().await;
        store.commit().await.unwrap();
    }

    assert_eq!(
        stats_of(&store, "d").await.rebuild_started.as_deref(),
        Some(now),
        "an apply that did not commit leaves the marker standing"
    );
    assert_eq!(
        visible(&store, &["d"]).await,
        before,
        "and leaves every row and embedding exactly as it was"
    );

    // The rebuild that does land clears it, in the transaction that commits it.
    reindex_domains(&*store, &targets, &params(), true, &NoReindexHooks)
        .await
        .unwrap();
    assert!(
        stats_of(&store, "d").await.rebuild_started.is_none(),
        "a committed rebuild clears the marker"
    );
    assert_eq!(
        visible(&store, &["d"]).await,
        before,
        "and it re-read everything without destroying any of it"
    );
}
parity!(
    a_failed_apply_leaves_the_rebuild_marker_standing,
    a_failed_apply_leaves_the_marker_standing
);

/// The other half of the placement, which no runtime injection can reach: the
/// clear must also sit *before* the commit, and the only thing that would tell
/// the two apart at runtime is a commit that fails.
///
/// So this reads the source, the way the backend-collation guard in this
/// workspace does. It is a guard against a future edit moving a call whose
/// whole meaning is where it sits, in a function where nothing else would
/// complain.
#[test]
fn end_rebuild_is_called_inside_the_applys_transaction() {
    const SYNC_SRC: &str = include_str!("../src/sync.rs");
    let start = SYNC_SRC
        .find("pub async fn apply_scan_with_slab")
        .expect("apply_scan_with_slab is where the rebuild marker is cleared");
    let body = &SYNC_SRC[start..];
    let end = body.find("\n}\n").expect("the function ends");
    let body = &body[..end];

    let begin = body
        .find("store.begin().await?")
        .expect("the apply opens its transaction");
    let clear = body
        .find("store.end_rebuild(domain)")
        .expect("the apply clears the rebuild marker");
    let commit = body
        .find("store.commit().await?")
        .expect("the apply commits its transaction");

    assert!(
        begin < clear,
        "end_rebuild must run after the transaction is open, or a rebuild that never commits still clears its marker"
    );
    assert!(
        clear < commit,
        "end_rebuild must run before the commit, or the clear is a separate autocommitted write and a crash between the two leaves a finished rebuild marked forever"
    );
}
