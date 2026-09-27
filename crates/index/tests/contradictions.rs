//! The contradiction tables on both backends: the pair rows that say "up to
//! date at these checksums", the line rows the sweep reads, the cascade on
//! delete, the domain and whole-index clears, and the rule that an upsert of a
//! scored engram keeps its scores until a rescore replaces them.
//!
//! The `FakeProvider` harness, the `write`/`engram`/`sync_and_embed` helpers
//! and the `parity!` backend runner are mirrored from `tests/lead_vectors.rs`
//! (Turso always, Postgres when `CRYSTALLINE_TEST_POSTGRES_URL` is set).

use std::path::Path;

use async_trait::async_trait;
use crystalline_index::{
    ChunkParams, ContradictionRow, DomainId, DomainKind, EmbeddingProvider, EngramId, Result,
    ScoredPair, Store, TursoStore, run_embedding_pass, sync_domain_with,
};

// --- fake provider (mirrored from tests/lead_vectors.rs) ----------------------

/// A deterministic, network-free provider. It hashes each word into one of eight
/// buckets and L2-normalizes, so texts that share vocabulary get similar
/// vectors: enough structure to exercise ranking.
struct FakeProvider {
    model: String,
}

impl FakeProvider {
    fn new(model: &str) -> FakeProvider {
        FakeProvider {
            model: model.to_string(),
        }
    }
}

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
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|t| embed_one(t)).collect())
    }
    fn model_id(&self) -> &str {
        &self.model
    }
    fn dims(&self) -> usize {
        8
    }
    fn max_input_tokens(&self) -> usize {
        512
    }
}

// --- helpers (mirrored from tests/lead_vectors.rs) ----------------------------

fn write(dir: &Path, rel: &str, content: &str) {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, content).unwrap();
}

fn engram(title: &str, permalink: &str, status: &str, extra_fm: &str, body: &str) -> String {
    format!(
        "---\ntype: engram\ntitle: {title}\npermalink: {permalink}\ntags:\n  - t\nstatus: {status}\nrecorded_at: 2026-01-01\n{extra_fm}---\n\n{body}\n"
    )
}

/// Sync the corpus fingerprinting for the fake model, then embed everything.
async fn sync_and_embed(store: &dyn Store, name: &str, root: &Path, provider: &FakeProvider) {
    let params = ChunkParams::for_model(provider.model_id());
    sync_domain_with(store, name, root, &params).await.unwrap();
    run_embedding_pass(store, provider, |_, _| {})
        .await
        .unwrap();
}

// --- backend runner (mirrored from tests/lead_vectors.rs) ---------------------

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

/// A distinct schema name per test invocation. The pid keeps runs apart, the
/// counter keeps tests within a run apart; both stay well under Postgres's
/// 63-byte identifier limit.
/// A hash salt keeps a recycled pid from adopting a schema a panicking run left behind.
#[cfg(feature = "postgres")]
fn unique_schema() -> String {
    use std::hash::{BuildHasher, RandomState};
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!(
        "ct_{}_{}_{:x}",
        std::process::id(),
        n,
        RandomState::new().hash_one(n)
    )
}

/// Run a parity body against Turso (always) and Postgres (when configured),
/// giving each backend a fresh, isolated store.
macro_rules! parity {
    ($name:ident, $body:path) => {
        #[tokio::test]
        async fn $name() {
            {
                let store = TursoStore::open_in_memory().await.unwrap();
                $body(&store).await;
            }
            #[cfg(feature = "postgres")]
            {
                if let Some(url) = pg_url() {
                    let schema = unique_schema();
                    let store = crystalline_index::PostgresStore::open_in_schema(&url, &schema)
                        .await
                        .expect("open the postgres test schema");
                    $body(&store).await;
                    store
                        .drop_schema()
                        .await
                        .expect("drop the postgres test schema");
                }
            }
        }
    };
}

// --- the contradiction tables ------------------------------------------------

fn pair(a: EngramId, b: EngramId, ca: &str, cb: &str) -> ScoredPair {
    ScoredPair {
        a,
        b,
        checksum_a: ca.to_string(),
        checksum_b: cb.to_string(),
    }
}

fn row(
    a: EngramId,
    b: EngramId,
    la: usize,
    lb: usize,
    ab: f32,
    ba: f32,
    period: bool,
) -> ContradictionRow {
    ContradictionRow {
        a,
        b,
        line_a: la,
        line_b: lb,
        hash_a: format!("h{la}"),
        hash_b: format!("h{lb}"),
        score_ab: ab,
        score_ba: ba,
        period,
    }
}

/// Two engrams synced and embedded; their ids in key order and the
/// checksums the store recorded for them.
async fn two(store: &dyn Store, root: &Path) -> (DomainId, EngramId, String, EngramId, String) {
    write(
        root,
        "alpha.md",
        &engram(
            "Alpha",
            "alpha",
            "stable",
            "",
            "The build runs nightly.\n\n- [fact] The build uses Node 18 #t\n",
        ),
    );
    write(
        root,
        "beta.md",
        &engram(
            "Beta",
            "beta",
            "stable",
            "",
            "The build runs nightly.\n\n- [fact] The build uses Node 20 #t\n",
        ),
    );
    let provider = FakeProvider::new("fake");
    sync_and_embed(store, "notes", root, &provider).await;
    let domain = store
        .upsert_domain("notes", Some(&root.to_string_lossy()), DomainKind::File)
        .await
        .unwrap();
    let stamps = store.file_stamps(domain).await.unwrap();
    let alpha = store.find_engram("notes", "alpha").await.unwrap().unwrap();
    let beta = store.find_engram("notes", "beta").await.unwrap().unwrap();
    let (first, second) = if alpha.id.0 < beta.id.0 {
        (alpha, beta)
    } else {
        (beta, alpha)
    };
    (
        domain,
        first.id,
        stamps[&first.path].sha256.clone(),
        second.id,
        stamps[&second.path].sha256.clone(),
    )
}

async fn a_replace_reads_back_by_domain_model_and_floor(store: &dyn Store) {
    let tmp = tempfile::tempdir().unwrap();
    let (domain, a, ca, b, cb) = two(store, tmp.path()).await;
    let p = pair(a, b, &ca, &cb);
    store
        .replace_contradictions(
            domain,
            &p,
            0.93,
            "nli-x",
            "2026-09-27T10:00:00Z",
            &[
                row(a, b, 5, 5, 0.91, 0.40, false),
                row(a, b, 5, 6, 0.30, 0.55, true),
            ],
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .contradiction_pairs_scored(domain, "nli-x")
            .await
            .unwrap(),
        vec![p.clone()]
    );
    // The floor applies to the higher of the two orders.
    let all = store.contradictions(domain, "nli-x", 0.5).await.unwrap();
    assert_eq!(
        all,
        vec![
            row(a, b, 5, 5, 0.91, 0.40, false),
            row(a, b, 5, 6, 0.30, 0.55, true)
        ]
    );
    assert_eq!(
        store.contradictions(domain, "nli-x", 0.6).await.unwrap(),
        vec![row(a, b, 5, 5, 0.91, 0.40, false)]
    );
    assert!(
        store
            .contradictions(domain, "other-nli", 0.0)
            .await
            .unwrap()
            .is_empty(),
        "rows are per model"
    );
    assert!(
        store
            .contradiction_pairs_scored(domain, "other-nli")
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(store.scored_pair_count("nli-x").await.unwrap(), 1);
    assert_eq!(store.scored_pair_count("other-nli").await.unwrap(), 0);

    // A rescore replaces the pair row and its line rows, nothing survives.
    let p2 = pair(a, b, "new-a", &cb);
    store
        .replace_contradictions(
            domain,
            &p2,
            0.93,
            "nli-x",
            "2026-09-27T11:00:00Z",
            &[row(a, b, 7, 5, 0.6, 0.6, false)],
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .contradiction_pairs_scored(domain, "nli-x")
            .await
            .unwrap(),
        vec![p2]
    );
    assert_eq!(
        store.contradictions(domain, "nli-x", 0.0).await.unwrap(),
        vec![row(a, b, 7, 5, 0.6, 0.6, false)]
    );

    // A pair scored with nothing to say is still a scored pair, and a second
    // model keeps its own rows beside the first.
    store
        .replace_contradictions(
            domain,
            &pair(a, b, &ca, &cb),
            0.93,
            "nli-y",
            "2026-09-27T12:00:00Z",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .contradiction_pairs_scored(domain, "nli-y")
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        store
            .contradictions(domain, "nli-x", 0.0)
            .await
            .unwrap()
            .len(),
        1
    );
}
parity!(
    replace_contradictions_parity,
    a_replace_reads_back_by_domain_model_and_floor
);

async fn deleting_an_engram_takes_its_pairs_and_rows(store: &dyn Store) {
    let tmp = tempfile::tempdir().unwrap();
    let (domain, a, ca, b, cb) = two(store, tmp.path()).await;
    store
        .replace_contradictions(
            domain,
            &pair(a, b, &ca, &cb),
            0.9,
            "nli-x",
            "2026-09-27T10:00:00Z",
            &[row(a, b, 5, 5, 0.9, 0.9, false)],
        )
        .await
        .unwrap();
    store.delete_engram(domain, "alpha.md").await.unwrap();
    assert!(
        store
            .contradiction_pairs_scored(domain, "nli-x")
            .await
            .unwrap()
            .is_empty(),
        "the pair went with the engram"
    );
    assert!(
        store
            .contradictions(domain, "nli-x", 0.0)
            .await
            .unwrap()
            .is_empty(),
        "and so did its rows"
    );
}
parity!(
    delete_cascades_contradictions_parity,
    deleting_an_engram_takes_its_pairs_and_rows
);

async fn clear_domain_and_wipe_clear_both_tables(store: &dyn Store) {
    let tmp = tempfile::tempdir().unwrap();
    let (domain, a, ca, b, cb) = two(store, tmp.path()).await;
    store
        .replace_contradictions(
            domain,
            &pair(a, b, &ca, &cb),
            0.9,
            "nli-x",
            "2026-09-27T10:00:00Z",
            &[row(a, b, 5, 5, 0.9, 0.9, false)],
        )
        .await
        .unwrap();
    store.clear_domain(domain).await.unwrap();
    assert!(
        store
            .contradiction_pairs_scored(domain, "nli-x")
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .contradictions(domain, "nli-x", 0.0)
            .await
            .unwrap()
            .is_empty()
    );

    let tmp = tempfile::tempdir().unwrap();
    let (domain, a, ca, b, cb) = two(store, tmp.path()).await;
    store
        .replace_contradictions(
            domain,
            &pair(a, b, &ca, &cb),
            0.9,
            "nli-x",
            "2026-09-27T10:00:00Z",
            &[row(a, b, 5, 5, 0.9, 0.9, false)],
        )
        .await
        .unwrap();
    store.wipe().await.unwrap();
    assert_eq!(store.scored_pair_count("nli-x").await.unwrap(), 0);
    assert!(
        store
            .contradictions(domain, "nli-x", 0.0)
            .await
            .unwrap()
            .is_empty()
    );
}
parity!(
    clear_and_wipe_contradictions_parity,
    clear_domain_and_wipe_clear_both_tables
);

/// Review focus 3. Every upsert runs the child-row delete, and `reindex
/// --full` upserts every engram, so a contradiction delete in that path would
/// erase every score in the index. An edited engram keeps its old rows (the
/// checksum mismatch is what re-queues it) until a rescore replaces them.
async fn an_upsert_keeps_the_scores_until_a_rescore_replaces_them(store: &dyn Store) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let (domain, a, ca, b, cb) = two(store, root).await;
    store
        .replace_contradictions(
            domain,
            &pair(a, b, &ca, &cb),
            0.9,
            "nli-x",
            "2026-09-27T10:00:00Z",
            &[row(a, b, 5, 5, 0.9, 0.9, false)],
        )
        .await
        .unwrap();
    write(
        root,
        "alpha.md",
        &engram(
            "Alpha",
            "alpha",
            "stable",
            "",
            "The build runs nightly and on tags.\n\n- [fact] The build uses Node 18 #t\n",
        ),
    );
    sync_and_embed(store, "notes", root, &FakeProvider::new("fake")).await;
    assert_eq!(
        store
            .contradiction_pairs_scored(domain, "nli-x")
            .await
            .unwrap(),
        vec![pair(a, b, &ca, &cb)],
        "the pair row keeps the checksums it was scored at"
    );
    assert_eq!(
        store
            .contradictions(domain, "nli-x", 0.0)
            .await
            .unwrap()
            .len(),
        1,
        "and its line rows"
    );
}
parity!(
    upsert_keeps_contradictions_parity,
    an_upsert_keeps_the_scores_until_a_rescore_replaces_them
);
