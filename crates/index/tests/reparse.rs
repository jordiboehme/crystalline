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
    ChunkParams, DomainId, DomainKind, EngramRecord, FileStamp, Store, TursoStore, apply_scan,
    reparse_stored_domain, scan_paths, sync_domain_with,
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

    let reparsed = reparse_stored_domain(&*store, "v", domain, &params())
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
        reparse_stored_domain(&*store, "v", domain, &params())
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

    reparse_stored_domain(&*store, "v", domain, &params())
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
        reparse_stored_domain(&*store, "v", domain, &params())
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

// --- fix round 1 -------------------------------------------------------------------

/// A forced walk that could not read one file has not reparsed the whole
/// domain: the unreadable file keeps the rows an older parser derived, so the
/// domain stays behind and the next sync reads everything again. Without
/// this, a file unreadable during the upgrade sync would keep its old rows for
/// good, since a permission fix moves neither its mtime nor its size.
async fn an_unreadable_file_leaves_the_domain_behind(store: Arc<Mutex<dyn Store>>) {
    use std::os::unix::fs::PermissionsExt;
    let store = store.lock().await;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("d");
    let domain = staged_file_domain(&*store, &root).await;
    let other = root.join("other.md");
    std::fs::set_permissions(&other, std::fs::Permissions::from_mode(0o000)).unwrap();

    let report = sync_domain_with(&*store, "d", &root, &params())
        .await
        .unwrap();
    assert!(!report.failed.is_empty(), "{report:?}");
    assert_eq!(
        generation(&*store, domain).await,
        0,
        "a walk that missed a file does not stamp"
    );

    std::fs::set_permissions(&other, std::fs::Permissions::from_mode(0o644)).unwrap();
    let report = sync_domain_with(&*store, "d", &root, &params())
        .await
        .unwrap();
    assert!(report.failed.is_empty(), "{report:?}");
    assert_eq!(report.updated, 2, "read whole again: {report:?}");
    assert_eq!(generation(&*store, domain).await, PARSE_GENERATION);
}

/// A targeted pass (the watcher's) parses only the paths it was handed, so it
/// never stamps the domain, even when it reparses a file of a domain behind.
async fn a_targeted_pass_never_stamps(store: Arc<Mutex<dyn Store>>) {
    let store = store.lock().await;
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("d");
    let domain = staged_file_domain(&*store, &root).await;
    std::fs::write(
        root.join("wrapped.md"),
        format!("{WRAPPED}- [fact] one more line\n"),
    )
    .unwrap();

    let snapshot = store.file_stamps(domain).await.unwrap();
    let scan = scan_paths(
        "d",
        &root,
        snapshot,
        vec!["wrapped.md".to_string()],
        &params(),
    )
    .await;
    let report = apply_scan(&*store, domain, scan).await.unwrap();
    assert_eq!(report.updated, 1, "{report:?}");
    assert_eq!(
        wrapped_counts(&*store, "d").await,
        (1, 1),
        "the file it read"
    );
    assert_eq!(
        generation(&*store, domain).await,
        0,
        "one file is not the domain"
    );
}

/// The compare-and-swap the virtual reparse writes through: a row whose
/// stored content moved on since the caller read it is refused, and the newer
/// content stays. On Turso the store is the only writer, so this is the
/// sequential case; the Postgres race below is the concurrent one.
async fn a_stale_compare_keeps_the_newer_content(store: Arc<Mutex<dyn Store>>) {
    let store = store.lock().await;
    let domain = staged_virtual_domain(&*store).await;
    let old = store
        .all_engram_contents(domain)
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.path == "wrapped.md")
        .unwrap();
    let newer = WRAPPED.replace("written the way", "now written the way");
    let mut record = EngramRecord::from_engram(
        &parse_engram(&newer).unwrap(),
        "wrapped.md",
        FileStamp {
            mtime: 1_700_000_100,
            size: newer.len() as u64,
            sha256: sha256(&newer),
        },
    );
    record.content = newer.clone();
    store.upsert_engram(domain, &record).await.unwrap();

    // What a reparse that read the row before that write would write back.
    let mut stale = EngramRecord::from_engram(
        &parse_engram(&old.content).unwrap(),
        "wrapped.md",
        FileStamp {
            mtime: 1_700_000_000,
            size: old.content.len() as u64,
            sha256: old.sha256.clone(),
        },
    );
    stale.content = old.content.clone();
    store.begin().await.unwrap();
    let err = store
        .upsert_engram_checked(domain, &stale, Some(&old.sha256))
        .await
        .expect_err("the row moved on");
    store.rollback().await.unwrap();
    assert!(
        matches!(err, crystalline_index::IndexError::StaleEdit { .. }),
        "{err}"
    );
    assert_eq!(
        store
            .engram_content(domain, "wrapped.md")
            .await
            .unwrap()
            .as_deref(),
        Some(newer.as_str())
    );
}

parity!(
    unreadable_file_leaves_the_domain_behind,
    an_unreadable_file_leaves_the_domain_behind
);
parity!(targeted_pass_never_stamps, a_targeted_pass_never_stamps);
parity!(
    stale_compare_keeps_the_newer_content,
    a_stale_compare_keeps_the_newer_content
);

/// After a sync whose stamp was refused: the domain is still behind and its
/// rows are the ones from before the run, because the stamp and the rows it
/// vouches for commit together.
async fn assert_rolled_back(store: &dyn Store, domain: DomainId) {
    assert_eq!(generation(store, domain).await, 0, "nothing was stamped");
    assert_eq!(
        wrapped_counts(store, "d").await,
        (0, 0),
        "the rows went back with the stamp"
    );
}

/// The next sync with the stamp allowed again repeats the whole reparse.
async fn assert_repeats(store: &dyn Store, root: &Path, domain: DomainId) {
    let report = sync_domain_with(store, "d", root, &params()).await.unwrap();
    assert_eq!(report.updated, 2, "the whole domain again: {report:?}");
    assert_eq!(wrapped_counts(store, "d").await, (1, 1));
    assert_eq!(generation(store, domain).await, PARSE_GENERATION);
}

/// A reparse whose stamp is refused, a trigger standing in for a crash
/// between the rows and the stamp, rolls its rows back too and is repeated
/// by the next sync. Turso: the trigger is set up through a connection of its
/// own while no store holds the file.
#[tokio::test]
async fn a_refused_stamp_rolls_the_reparse_back_and_it_repeats_on_turso() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("index.db");
    let root = tmp.path().join("d");
    let domain = {
        let store = TursoStore::open(&db).await.unwrap();
        staged_file_domain(&store, &root).await
    };
    let raw = async |sql: &str| {
        let database = turso::Builder::new_local(db.to_str().unwrap())
            .build()
            .await
            .unwrap();
        database.connect().unwrap().execute(sql, ()).await.unwrap();
    };
    raw(
        "CREATE TRIGGER refuse_stamp BEFORE UPDATE OF parse_generation ON domain \
         WHEN NEW.parse_generation > 0 BEGIN SELECT RAISE(ABORT, 'stamp refused'); END",
    )
    .await;
    {
        let store = TursoStore::open(&db).await.unwrap();
        sync_domain_with(&store, "d", &root, &params())
            .await
            .expect_err("the stamp is refused");
        assert_rolled_back(&store, domain).await;
    }
    raw("DROP TRIGGER refuse_stamp").await;
    let store = TursoStore::open(&db).await.unwrap();
    assert_repeats(&store, &root, domain).await;
}

/// The same on Postgres, with the trigger set up over a plain connection to
/// the store's schema.
#[cfg(feature = "postgres")]
#[tokio::test]
async fn a_refused_stamp_rolls_the_reparse_back_and_it_repeats_on_postgres() {
    use sqlx::Connection;
    let Some(url) = pg_url() else {
        return;
    };
    let schema = unique_schema();
    let store = crystalline_index::PostgresStore::open_in_schema(&url, &schema)
        .await
        .unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("d");
    let domain = staged_file_domain(&store, &root).await;
    let mut raw = sqlx::PgConnection::connect(&url).await.unwrap();
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
        "SET search_path TO {schema}, public; \
         CREATE FUNCTION refuse_stamp() RETURNS trigger AS $$ BEGIN \
           IF NEW.parse_generation > 0 THEN RAISE EXCEPTION 'stamp refused'; END IF; \
           RETURN NEW; END $$ LANGUAGE plpgsql; \
         CREATE TRIGGER refuse_stamp BEFORE UPDATE OF parse_generation ON domain \
           FOR EACH ROW EXECUTE FUNCTION refuse_stamp();"
    )))
    .execute(&mut raw)
    .await
    .unwrap();

    sync_domain_with(&store, "d", &root, &params())
        .await
        .expect_err("the stamp is refused");
    assert_rolled_back(&store, domain).await;

    sqlx::raw_sql("DROP TRIGGER refuse_stamp ON domain")
        .execute(&mut raw)
        .await
        .unwrap();
    assert_repeats(&store, &root, domain).await;
    store.drop_schema().await.unwrap();
}

/// Two instances on one Postgres schema: B holds an uncommitted write of a
/// virtual engram while A, a reparse that read the row before it, writes the
/// old text back through the compare-and-swap. A waits for B's row lock, then
/// finds the row moved on and is refused, and B's text is what stays. Under
/// READ COMMITTED a compare that only read the row would have passed and
/// overwritten B's write once it committed.
#[cfg(feature = "postgres")]
#[tokio::test]
async fn a_concurrent_write_survives_the_reparse_on_postgres() {
    let Some(url) = pg_url() else {
        return;
    };
    let schema = unique_schema();
    let a = Arc::new(
        crystalline_index::PostgresStore::open_in_schema(&url, &schema)
            .await
            .unwrap(),
    );
    let b = crystalline_index::PostgresStore::open_in_schema(&url, &schema)
        .await
        .unwrap();
    let domain = staged_virtual_domain(&*a).await;
    let old = a
        .all_engram_contents(domain)
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.path == "wrapped.md")
        .unwrap();
    let record_of = |text: &str, mtime: i64| {
        let mut record = EngramRecord::from_engram(
            &parse_engram(text).unwrap(),
            "wrapped.md",
            FileStamp {
                mtime,
                size: text.len() as u64,
                sha256: sha256(text),
            },
        );
        record.content = text.to_string();
        record
    };

    // B's write, held open.
    let newer = WRAPPED.replace("written the way", "now written the way");
    b.begin().await.unwrap();
    b.upsert_engram(domain, &record_of(&newer, 1_700_000_100))
        .await
        .unwrap();

    // A's reparse of the row it read before B's write.
    let stale = record_of(&old.content, 1_700_000_000);
    let expected = old.sha256.clone();
    let writer = Arc::clone(&a);
    let reparse = tokio::spawn(async move {
        writer.begin().await.unwrap();
        let outcome = writer
            .upsert_engram_checked(domain, &stale, Some(&expected))
            .await;
        match &outcome {
            Ok(_) => writer.commit().await.unwrap(),
            Err(_) => writer.rollback().await.unwrap(),
        }
        outcome
    });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(!reparse.is_finished(), "A waits for B's row lock");
    b.commit().await.unwrap();

    let outcome = reparse.await.unwrap();
    assert!(
        matches!(
            outcome,
            Err(crystalline_index::IndexError::StaleEdit { .. })
        ),
        "{outcome:?}"
    );
    assert_eq!(
        a.engram_content(domain, "wrapped.md")
            .await
            .unwrap()
            .as_deref(),
        Some(newer.as_str()),
        "B's write is what stays"
    );
    a.drop_schema().await.unwrap();
}
