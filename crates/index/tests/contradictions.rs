//! The contradiction tables on both backends: the pair rows that say "up to
//! date at these checksums", the line rows the sweep reads, the cascade on
//! delete, the domain and whole-index clears, and the rule that an upsert of a
//! scored engram keeps its scores until a rescore replaces them.
//!
//! The `FakeProvider` harness, the `write`/`engram`/`sync_and_embed` helpers
//! and the `parity!` backend runner are mirrored from `tests/lead_vectors.rs`
//! (Turso always, Postgres when `CRYSTALLINE_TEST_POSTGRES_URL` is set).

use std::collections::HashSet;
use std::path::Path;

use async_trait::async_trait;
use crystalline_index::{
    ChunkParams, ContradictionRow, DomainId, DomainKind, EmbeddingProvider, EngramId,
    ObservationVector, Result, ScoredPair, Store, TursoStore, run_embedding_pass, sync_domain_with,
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
        similarity: 0.0,
        period,
    }
}

/// Two engrams synced and embedded; their ids in key order and the
/// checksums the store recorded for them.
async fn two(store: &dyn Store, root: &Path) -> (DomainId, EngramId, String, EngramId, String) {
    let t = two_in(store, root, "notes").await;
    (t.domain, t.a, t.ca, t.b, t.cb)
}

/// What [`two_in`] synced: the domain, both engrams in key order with the
/// checksums the store recorded and the paths they live at.
struct Two {
    domain: DomainId,
    a: EngramId,
    ca: String,
    path_a: String,
    b: EngramId,
    cb: String,
    path_b: String,
}

/// [`two`] in the domain `name`, with the paths kept, so a test can delete
/// either side of the pair and keep a second domain beside the first.
async fn two_in(store: &dyn Store, root: &Path, name: &str) -> Two {
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
    sync_and_embed(store, name, root, &provider).await;
    let domain = store
        .upsert_domain(name, Some(&root.to_string_lossy()), DomainKind::File)
        .await
        .unwrap();
    let stamps = store.file_stamps(domain).await.unwrap();
    let alpha = store.find_engram(name, "alpha").await.unwrap().unwrap();
    let beta = store.find_engram(name, "beta").await.unwrap().unwrap();
    let (first, second) = if alpha.id.0 < beta.id.0 {
        (alpha, beta)
    } else {
        (beta, alpha)
    };
    Two {
        domain,
        a: first.id,
        ca: stamps[&first.path].sha256.clone(),
        path_a: first.path.clone(),
        b: second.id,
        cb: stamps[&second.path].sha256.clone(),
        path_b: second.path.clone(),
    }
}

/// Score `t`'s pair with one line row.
async fn score(store: &dyn Store, t: &Two) {
    store
        .replace_contradictions(
            t.domain,
            &pair(t.a, t.b, &t.ca, &t.cb),
            0.9,
            "nli-x",
            "2026-09-27T10:00:00Z",
            &[row(t.a, t.b, 5, 5, 0.9, 0.9, false)],
        )
        .await
        .unwrap();
}

/// How many pair rows and line rows `domain` holds for `nli-x`.
async fn held(store: &dyn Store, domain: DomainId) -> (usize, usize) {
    (
        store
            .contradiction_pairs_scored(domain, "nli-x")
            .await
            .unwrap()
            .len(),
        store
            .contradictions(domain, "nli-x", 0.0)
            .await
            .unwrap()
            .len(),
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

/// Either side of a pair takes the pair with it: `engram_a` through the
/// primary key and `engram_b` through its own index. Turso enforces no
/// foreign keys, so the `OR engram_b` in the delete is the only guard for the
/// higher id. Another domain's pair survives both deletes.
async fn deleting_an_engram_takes_its_pairs_and_rows(store: &dyn Store) {
    let (lower, higher, other) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    let kept = two_in(store, other.path(), "other").await;
    score(store, &kept).await;

    let t = two_in(store, lower.path(), "notes").await;
    score(store, &t).await;
    assert_eq!(held(store, t.domain).await, (1, 1));
    store.delete_engram(t.domain, &t.path_a).await.unwrap();
    assert_eq!(
        held(store, t.domain).await,
        (0, 0),
        "the pair and its rows went with the lower id"
    );

    let t = two_in(store, higher.path(), "third").await;
    score(store, &t).await;
    assert_eq!(held(store, t.domain).await, (1, 1));
    store.delete_engram(t.domain, &t.path_b).await.unwrap();
    assert_eq!(
        held(store, t.domain).await,
        (0, 0),
        "the pair and its rows went with the higher id"
    );

    assert_eq!(
        held(store, kept.domain).await,
        (1, 1),
        "another domain's pair is untouched"
    );
    assert_eq!(store.scored_pair_count("nli-x").await.unwrap(), 1);
}
parity!(
    delete_cascades_contradictions_parity,
    deleting_an_engram_takes_its_pairs_and_rows
);

/// A clear takes its own domain's rows and no other's; a wipe takes all.
async fn clear_domain_and_wipe_clear_both_tables(store: &dyn Store) {
    let (tmp, other) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let t = two_in(store, tmp.path(), "notes").await;
    let kept = two_in(store, other.path(), "other").await;
    score(store, &t).await;
    score(store, &kept).await;
    store.clear_domain(t.domain).await.unwrap();
    assert_eq!(held(store, t.domain).await, (0, 0));
    assert_eq!(
        held(store, kept.domain).await,
        (1, 1),
        "the other domain keeps its pair and rows"
    );
    assert_eq!(store.scored_pair_count("nli-x").await.unwrap(), 1);

    let tmp = tempfile::tempdir().unwrap();
    let t = two_in(store, tmp.path(), "notes").await;
    score(store, &t).await;
    store.wipe().await.unwrap();
    assert_eq!(store.scored_pair_count("nli-x").await.unwrap(), 0);
    assert_eq!(held(store, t.domain).await, (0, 0));
    assert_eq!(held(store, kept.domain).await, (0, 0));
}
parity!(
    clear_and_wipe_contradictions_parity,
    clear_domain_and_wipe_clear_both_tables
);

/// A delete can land while a batch scores the pair. The write that follows
/// stores nothing for an engram that is gone (turso has no foreign key to
/// refuse it, postgres would refuse it with one), and it is no error: the
/// pair is done because it no longer exists.
async fn a_replace_after_either_engram_is_gone_stores_nothing(store: &dyn Store) {
    let (lower, higher) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    for (root, name, lower_side) in [(&lower, "notes", true), (&higher, "other", false)] {
        let t = two_in(store, root.path(), name).await;
        let gone = if lower_side { &t.path_a } else { &t.path_b };
        store.delete_engram(t.domain, gone).await.unwrap();
        store
            .replace_contradictions(
                t.domain,
                &pair(t.a, t.b, &t.ca, &t.cb),
                0.9,
                "nli-x",
                "2026-09-27T10:00:00Z",
                &[row(t.a, t.b, 5, 5, 0.9, 0.9, false)],
            )
            .await
            .expect("a vanished engram is not an error");
        assert_eq!(held(store, t.domain).await, (0, 0), "{name}");
    }
    assert_eq!(store.scored_pair_count("nli-x").await.unwrap(), 0);
}
parity!(
    replace_after_delete_parity,
    a_replace_after_either_engram_is_gone_stores_nothing
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

// --- the line vectors --------------------------------------------------------

fn vector(hash: &str, x: f32) -> ObservationVector {
    ObservationVector {
        hash: hash.to_string(),
        vector: vec![x, 1.0 - x, 0.5, 0.25],
    }
}

fn hashes(list: &[&str]) -> Vec<String> {
    list.iter().map(|h| h.to_string()).collect()
}

async fn line_vectors_round_trip_by_model_and_hash(store: &dyn Store) {
    store
        .store_observation_vectors("m1", &[vector("ha", 0.1), vector("hb", 0.2)])
        .await
        .unwrap();
    store
        .store_observation_vectors("m2", &[vector("ha", 0.9)])
        .await
        .unwrap();
    let present = store
        .observation_vectors_present("m1", &hashes(&["ha", "hb", "hc"]))
        .await
        .unwrap();
    assert_eq!(present, HashSet::from(["ha".to_string(), "hb".to_string()]));
    let got = store
        .observation_vectors("m1", &hashes(&["ha", "hc"]))
        .await
        .unwrap();
    assert_eq!(got.len(), 1, "an unknown hash is simply absent");
    // Compared against the vector the helper built, bit for bit: `1.0 - x`
    // in f32 is not the decimal literal.
    assert_eq!(got["ha"], vector("ha", 0.1).vector);
    let other = store
        .observation_vectors("m2", &hashes(&["ha"]))
        .await
        .unwrap();
    assert_eq!(other["ha"], vector("ha", 0.9).vector, "keyed by model too");
    // A second store of a known (model, hash) keeps the first: the text is
    // the same, so the vector is.
    store
        .store_observation_vectors("m1", &[vector("ha", 0.7)])
        .await
        .unwrap();
    let again = store
        .observation_vectors("m1", &hashes(&["ha"]))
        .await
        .unwrap();
    assert_eq!(again["ha"], vector("ha", 0.1).vector);
    let mut all = store.observation_vector_hashes("m1").await.unwrap();
    all.sort();
    assert_eq!(all, hashes(&["ha", "hb"]));
    // Empty input asks nothing and changes nothing.
    assert!(
        store
            .observation_vectors_present("m1", &[])
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .observation_vectors("m1", &[])
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store.delete_observation_vectors("m1", &[]).await.unwrap(),
        0
    );
    store.store_observation_vectors("m1", &[]).await.unwrap();
    assert_eq!(
        store.observation_vector_hashes("m1").await.unwrap().len(),
        2
    );
}
parity!(
    line_vectors_round_trip_parity,
    line_vectors_round_trip_by_model_and_hash
);

async fn a_long_hash_list_is_read_in_runs(store: &dyn Store) {
    let rows: Vec<ObservationVector> = (0..600).map(|i| vector(&format!("h{i}"), 0.5)).collect();
    store.store_observation_vectors("m", &rows).await.unwrap();
    let asked: Vec<String> = (0..700).map(|i| format!("h{i}")).collect();
    assert_eq!(
        store
            .observation_vectors_present("m", &asked)
            .await
            .unwrap()
            .len(),
        600,
        "more hashes than one statement names, every present one found"
    );
    assert_eq!(
        store.observation_vectors("m", &asked).await.unwrap().len(),
        600
    );
    assert_eq!(
        store
            .delete_observation_vectors("m", &asked[..300])
            .await
            .unwrap(),
        300
    );
    assert_eq!(
        store.observation_vector_hashes("m").await.unwrap().len(),
        300
    );
}
parity!(long_hash_list_parity, a_long_hash_list_is_read_in_runs);

async fn pruning_and_clearing_take_only_what_they_name(store: &dyn Store) {
    store
        .store_observation_vectors("keep", &[vector("a", 0.1), vector("b", 0.2)])
        .await
        .unwrap();
    store
        .store_observation_vectors("old", &[vector("a", 0.3)])
        .await
        .unwrap();
    assert_eq!(
        store
            .delete_observation_vectors("keep", &hashes(&["b", "zz"]))
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        store
            .delete_observation_vectors_except("keep")
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        store.observation_vector_hashes("keep").await.unwrap(),
        hashes(&["a"])
    );
    assert!(
        store
            .observation_vector_hashes("old")
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(store.clear_observation_vectors().await.unwrap(), 1);
    assert!(
        store
            .observation_vector_hashes("keep")
            .await
            .unwrap()
            .is_empty()
    );
}
parity!(
    pruning_and_clearing_parity,
    pruning_and_clearing_take_only_what_they_name
);

async fn clear_contradictions_by_domain_or_all(store: &dyn Store) {
    let tmp = tempfile::tempdir().unwrap();
    let one = two_in(store, &tmp.path().join("one"), "one").await;
    let other = two_in(store, &tmp.path().join("other"), "other").await;
    // `score` writes one pair row and one line row.
    score(store, &one).await;
    score(store, &other).await;
    store
        .clear_contradictions(Some(&[one.domain]))
        .await
        .unwrap();
    assert_eq!(held(store, one.domain).await, (0, 0));
    assert_eq!(
        held(store, other.domain).await,
        (1, 1),
        "the other domain stays"
    );
    store.clear_contradictions(None).await.unwrap();
    assert_eq!(held(store, other.domain).await, (0, 0));
}
parity!(
    clear_contradictions_parity,
    clear_contradictions_by_domain_or_all
);

async fn the_similarity_round_trips_and_wipe_clears_line_vectors(store: &dyn Store) {
    let tmp = tempfile::tempdir().unwrap();
    let (domain, a, ca, b, cb) = two(store, tmp.path()).await;
    let mut r = row(a, b, 5, 5, 0.91, 0.80, false);
    r.similarity = 0.875;
    store
        .replace_contradictions(
            domain,
            &pair(a, b, &ca, &cb),
            0.9,
            "nli-x",
            "t",
            &[r.clone()],
        )
        .await
        .unwrap();
    let back = store.contradictions(domain, "nli-x", 0.5).await.unwrap();
    assert_eq!(back, vec![r]);
    store
        .store_observation_vectors("m", &[vector("x", 0.5)])
        .await
        .unwrap();
    store.wipe().await.unwrap();
    assert!(
        store
            .observation_vector_hashes("m")
            .await
            .unwrap()
            .is_empty()
    );
}
parity!(
    similarity_and_wipe_parity,
    the_similarity_round_trips_and_wipe_clears_line_vectors
);

// --- the shared-database flag ------------------------------------------------

/// A turso index is one instance's own file: per-instance housekeeping may
/// delete its derived rows.
#[tokio::test]
async fn a_turso_store_does_not_share_its_database() {
    let store = TursoStore::open_in_memory().await.unwrap();
    assert!(!store.shares_database());
}

/// A postgres database may serve several instances at once, so per-instance
/// housekeeping must leave the shared rows alone.
#[cfg(feature = "postgres")]
#[tokio::test]
async fn a_postgres_store_shares_its_database() {
    let Some(url) = pg_url() else {
        return;
    };
    let schema = unique_schema();
    let store = crystalline_index::PostgresStore::open_in_schema(&url, &schema)
        .await
        .expect("open the postgres test schema");
    assert!(store.shares_database());
    store
        .drop_schema()
        .await
        .expect("drop the postgres test schema");
}
