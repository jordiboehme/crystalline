//! The once-per-parser-change reparse, on both backends: a domain whose rows
//! an older parser derived is reparsed whole by its next sync (a file domain
//! from its files, a virtual domain from the content the database stores),
//! without a byte of either changing, and stamped with the current
//! `PARSE_GENERATION`; an interrupted run repeats, and a current domain is
//! left alone.
//!
//! The "older parser" is staged by hand: the rows of a wrapped bullet are
//! rewritten to what the parser before generation 1 derived from it (the
//! first physical line only, no tag from the continuation line, no wrapped
//! relation) and the domain's generation is set back to 0, which is exactly
//! what an upgraded index holds. Turso (in-memory) always runs; Postgres runs
//! when `CRYSTALLINE_TEST_POSTGRES_URL` is set.

use std::path::Path;
use std::sync::Arc;

use crystalline_core::{PARSE_GENERATION, parse_engram};
use crystalline_index::{
    ChunkParams, DomainId, DomainKind, EngramRecord, FileStamp, Store, TursoStore,
    reparse_stored_domain, sync_domain_with,
};
use tokio::sync::Mutex;

// --- fixtures ------------------------------------------------------------------

/// An engram with a wrapped observation whose tag sits on its continuation
/// line, and a wrapped relation whose target sits on its continuation line.
const WRAPPED: &str = "---\ntype: engram\ntitle: Wrapped\npermalink: wrapped\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# Wrapped\n\nThe pump rules, written the way people write them.\n\n- [fact] the purge runs before every mix swap,\n  which is why the swap waits for night #purge\n- relates_to\n  [[other]]\n";

const OTHER: &str = "---\ntype: engram\ntitle: Other\npermalink: other\ntags:\n  - t\nstatus: current\nrecorded_at: 2026-01-01\n---\n\n# Other\n\nThe other side of the relation, with enough text to stand.\n";

fn params() -> ChunkParams {
    ChunkParams::default()
}

/// What the parser before generation 1 derived from [`WRAPPED`]: the
/// observation is its first physical line with no tag, and the relation
/// bullet, `relates_to` alone on its line, is no relation at all.
fn fragment_record(content: &str, path: &str, stamp: FileStamp) -> EngramRecord {
    let engram = parse_engram(content).unwrap();
    let mut record = EngramRecord::from_engram(&engram, path, stamp);
    assert_eq!(record.observations.len(), 1, "the fixture holds one bullet");
    record.observations[0].content = "the purge runs before every mix swap,".to_string();
    record.observations[0].tags.clear();
    record.relations.clear();
    record
}

/// How many observations carry `#purge`, and how many `relates_to` relations
/// the domain holds: zero and zero for the fragment rows, one and one once the
/// wrapped bullets are read whole.
async fn wrapped_counts(store: &dyn Store, domain: &str) -> (i64, i64) {
    let vocab = store.vocabulary(Some(domain), None).await.unwrap();
    let purge = vocab
        .tags
        .iter()
        .find(|t| t.name == "purge")
        .map(|t| t.observations)
        .unwrap_or(0);
    let relates = vocab
        .relation_types
        .iter()
        .find(|r| r.name == "relates_to")
        .map(|r| r.count)
        .unwrap_or(0);
    (purge, relates)
}

async fn generation(store: &dyn Store, domain: DomainId) -> u32 {
    store.parse_generation(domain).await.unwrap()
}

/// A file domain synced by the current parser, then staged as an index an
/// older parser built: the wrapped engram's rows are the fragments and the
/// generation is 0. The stamp stays the one of the file on disk, so the
/// mtime-and-size prefilter sees nothing to do and only the generation can
/// make the sync look again.
async fn staged_file_domain(store: &dyn Store, root: &Path) -> DomainId {
    std::fs::create_dir_all(root).unwrap();
    std::fs::write(root.join("wrapped.md"), WRAPPED).unwrap();
    std::fs::write(root.join("other.md"), OTHER).unwrap();
    sync_domain_with(store, "d", root, &params()).await.unwrap();
    let domain = store
        .upsert_domain("d", Some(&root.to_string_lossy()), DomainKind::File)
        .await
        .unwrap();
    assert_eq!(
        generation(store, domain).await,
        PARSE_GENERATION,
        "a domain row created now starts at the current generation"
    );
    assert_eq!(wrapped_counts(store, "d").await, (1, 1));

    let stamp = store.file_stamps(domain).await.unwrap()["wrapped.md"].clone();
    store
        .upsert_engram(domain, &fragment_record(WRAPPED, "wrapped.md", stamp))
        .await
        .unwrap();
    store.set_parse_generation(domain, 0).await.unwrap();
    assert_eq!(
        wrapped_counts(store, "d").await,
        (0, 0),
        "staged as fragments"
    );
    domain
}

// --- file domain -----------------------------------------------------------------

/// The upgrade, on a file domain: the next plain sync reparses every file
/// once, the wrapped bullet becomes one observation and one relation, the
/// files keep every byte and the domain is stamped current.
async fn a_file_domain_behind_is_reparsed_by_its_next_sync(store: Arc<Mutex<dyn Store>>) {
    let store = store.lock().await;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("d");
    let domain = staged_file_domain(&*store, &root).await;

    let report = sync_domain_with(&*store, "d", &root, &params())
        .await
        .unwrap();
    assert_eq!(report.updated, 2, "every file was reparsed: {report:?}");
    assert_eq!(wrapped_counts(&*store, "d").await, (1, 1));
    assert_eq!(generation(&*store, domain).await, PARSE_GENERATION);
    assert_eq!(
        std::fs::read_to_string(root.join("wrapped.md")).unwrap(),
        WRAPPED,
        "the file is byte for byte what it was"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("other.md")).unwrap(),
        OTHER
    );

    // Done once: the next sync is the ordinary no-op again.
    let again = sync_domain_with(&*store, "d", &root, &params())
        .await
        .unwrap();
    assert_eq!(again.updated, 0, "{again:?}");
    assert_eq!(again.unchanged, 2, "{again:?}");
}

/// A run that dies before its commit leaves the old generation, so the next
/// sync does the whole reparse again. The interruption is the one the reindex
/// suite injects: the domain's root cannot be walked.
async fn an_interrupted_file_reparse_repeats_on_the_next_sync(store: Arc<Mutex<dyn Store>>) {
    let store = store.lock().await;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("d");
    let domain = staged_file_domain(&*store, &root).await;

    let aside = tmp.path().join("aside");
    std::fs::rename(&root, &aside).unwrap();
    sync_domain_with(&*store, "d", &root, &params())
        .await
        .expect_err("the walk fails");
    assert_eq!(
        generation(&*store, domain).await,
        0,
        "nothing completed, so nothing was stamped"
    );
    assert_eq!(wrapped_counts(&*store, "d").await, (0, 0));

    std::fs::rename(&aside, &root).unwrap();
    sync_domain_with(&*store, "d", &root, &params())
        .await
        .unwrap();
    assert_eq!(wrapped_counts(&*store, "d").await, (1, 1));
    assert_eq!(generation(&*store, domain).await, PARSE_GENERATION);
}

/// A domain at the current generation is never walked whole: the sync is the
/// ordinary prefiltered one, so rows nothing changed stay as they are, even
/// the staged fragments here.
async fn a_current_file_domain_is_not_reparsed(store: Arc<Mutex<dyn Store>>) {
    let store = store.lock().await;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("d");
    let domain = staged_file_domain(&*store, &root).await;
    store
        .set_parse_generation(domain, PARSE_GENERATION)
        .await
        .unwrap();

    let report = sync_domain_with(&*store, "d", &root, &params())
        .await
        .unwrap();
    assert_eq!(report.updated, 0, "{report:?}");
    assert_eq!(report.unchanged, 2, "{report:?}");
    assert_eq!(
        wrapped_counts(&*store, "d").await,
        (0, 0),
        "nothing was reparsed"
    );
}

// --- virtual domain ----------------------------------------------------------------

/// A virtual domain as an older parser left it: both engrams stored with their
/// full markdown, the wrapped one with the fragment rows, generation 0.
async fn staged_virtual_domain(store: &dyn Store) -> DomainId {
    let domain = store
        .upsert_domain("v", None, DomainKind::Virtual)
        .await
        .unwrap();
    assert_eq!(generation(store, domain).await, PARSE_GENERATION);
    for (path, text, fragment) in [("wrapped.md", WRAPPED, true), ("other.md", OTHER, false)] {
        let stamp = FileStamp {
            mtime: 1_700_000_000,
            size: text.len() as u64,
            sha256: sha256(text),
        };
        let mut record = if fragment {
            fragment_record(text, path, stamp)
        } else {
            EngramRecord::from_engram(&parse_engram(text).unwrap(), path, stamp)
        };
        record.content = text.to_string();
        store.upsert_engram(domain, &record).await.unwrap();
    }
    store.resolve_pending_relations(domain).await.unwrap();
    store.set_parse_generation(domain, 0).await.unwrap();
    assert_eq!(
        wrapped_counts(store, "v").await,
        (0, 0),
        "staged as fragments"
    );
    domain
}

fn sha256(text: &str) -> String {
    use sha2::{Digest, Sha256};
    crystalline_index::hex_lower(&Sha256::digest(text.as_bytes()))
}

/// The upgrade, on a virtual domain: every engram is reparsed from its stored
/// content, which stays byte for byte what it was, checksum and modification
/// time included, and the domain is stamped current. A second call is a no-op.
async fn a_virtual_domain_behind_is_reparsed_from_its_stored_content(store: Arc<Mutex<dyn Store>>) {
    let store = store.lock().await;
    let domain = staged_virtual_domain(&*store).await;
    let before = store.all_engram_contents(domain).await.unwrap();
    let stamps_before = store.file_stamps(domain).await.unwrap();

    let reparsed = reparse_stored_domain(&*store, domain, &params())
        .await
        .unwrap();
    assert_eq!(reparsed, 2);
    assert_eq!(wrapped_counts(&*store, "v").await, (1, 1));
    assert_eq!(generation(&*store, domain).await, PARSE_GENERATION);

    let after = store.all_engram_contents(domain).await.unwrap();
    let pairs = |rows: &[crystalline_index::StoredEngram]| {
        rows.iter()
            .map(|r| (r.path.clone(), r.content.clone(), r.sha256.clone(), r.id))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        pairs(&after),
        pairs(&before),
        "every stored document, checksum and id is what it was"
    );
    assert_eq!(after[1].content, WRAPPED);
    assert_eq!(
        store.file_stamps(domain).await.unwrap(),
        stamps_before,
        "no modification time moved"
    );

    assert_eq!(
        reparse_stored_domain(&*store, domain, &params())
            .await
            .unwrap(),
        0,
        "a current domain is not reparsed again"
    );
}

/// A virtual reparse is one transaction: a failure anywhere in it rolls back
/// every row it rewrote and leaves the old generation, so the next sync
/// repeats it. The failure injected is a permalink collision: the wrapped
/// engram's stored document claims the other engram's permalink, which its
/// stored row (written by hand) did not.
async fn an_interrupted_virtual_reparse_repeats(store: Arc<Mutex<dyn Store>>) {
    let store = store.lock().await;
    let domain = store
        .upsert_domain("v", None, DomainKind::Virtual)
        .await
        .unwrap();
    let clash = WRAPPED.replace("permalink: wrapped", "permalink: other");
    for (path, text, permalink) in [
        ("a-first.md", OTHER, None),
        ("b-wrapped.md", clash.as_str(), Some("wrapped")),
    ] {
        let stamp = FileStamp {
            mtime: 1_700_000_000,
            size: text.len() as u64,
            sha256: sha256(text),
        };
        let mut record = if permalink.is_some() {
            fragment_record(text, path, stamp)
        } else {
            EngramRecord::from_engram(&parse_engram(text).unwrap(), path, stamp)
        };
        if let Some(p) = permalink {
            record.permalink = p.to_string();
        }
        record.content = text.to_string();
        store.upsert_engram(domain, &record).await.unwrap();
    }
    store.set_parse_generation(domain, 0).await.unwrap();

    reparse_stored_domain(&*store, domain, &params())
        .await
        .expect_err("the second row's reparse collides");
    assert_eq!(generation(&*store, domain).await, 0, "nothing was stamped");
    assert_eq!(
        wrapped_counts(&*store, "v").await,
        (0, 0),
        "the rows are the ones from before the run"
    );

    // The collision is gone (the first engram retired), so the next run
    // finishes what the interrupted one started.
    store.delete_engram(domain, "a-first.md").await.unwrap();
    assert_eq!(
        reparse_stored_domain(&*store, domain, &params())
            .await
            .unwrap(),
        1
    );
    assert_eq!(wrapped_counts(&*store, "v").await.0, 1);
    assert_eq!(generation(&*store, domain).await, PARSE_GENERATION);
}

// --- backend runner ------------------------------------------------------------

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
        "rp_{}_{}_{:x}",
        std::process::id(),
        n,
        RandomState::new().hash_one(n)
    )
}

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

parity!(
    file_domain_behind_is_reparsed_by_its_next_sync,
    a_file_domain_behind_is_reparsed_by_its_next_sync
);
parity!(
    interrupted_file_reparse_repeats_on_the_next_sync,
    an_interrupted_file_reparse_repeats_on_the_next_sync
);
parity!(
    current_file_domain_is_not_reparsed,
    a_current_file_domain_is_not_reparsed
);
parity!(
    virtual_domain_behind_is_reparsed_from_its_stored_content,
    a_virtual_domain_behind_is_reparsed_from_its_stored_content
);
parity!(
    interrupted_virtual_reparse_repeats,
    an_interrupted_virtual_reparse_repeats
);
