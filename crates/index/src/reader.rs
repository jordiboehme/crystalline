//! Read connections beside the write connection.
//!
//! The engine holds one store behind a mutex for every write and every
//! transaction. Reads that need no guard go through a [`ReadStore`] beside it
//! instead, so a search does not wait for a sync apply and two searches do not
//! wait for each other:
//!
//! - [`ReadPool`] keeps a few read connections of the backend (turso: more
//!   connections of the same database, `query_only`; Postgres: a pool of its
//!   own with read-only sessions). One call takes one connection and runs in
//!   one read transaction, so a method sees one state, as it did under the
//!   lock. A caller that wrote and then reads sees its write: every write
//!   commits before the writer guard is dropped, and a read transaction that
//!   starts later sees the commit.
//! - [`LockedReader`] takes the writer lock for every call: the behaviour from
//!   before read connections, used for in-memory stores, with the kill switch
//!   ([`READ_CONNECTIONS_ENV`]) and wherever a store has no pool.
//!
//! Code between `begin()` and `commit()` keeps reading through the writer it
//! holds: a reader would not see the transaction's own rows, and under
//! [`LockedReader`] it would wait for the lock its own task holds. A debug
//! build checks that ([`txn`]).

use std::future::Future;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{Mutex, Semaphore, SemaphorePermit};

use crate::error::{IndexError, Result};
use crate::store::{ReadStore, Store};

/// The environment variable that turns read connections off: `0` (or
/// `false`, `off`, `no`) keeps every read on the writer connection, as before
/// 0.32.1. Read once, when a store opens.
pub const READ_CONNECTIONS_ENV: &str = "CRYSTALLINE_READ_CONNECTIONS";

/// How many read connections a store opens beside its writer.
pub const READ_POOL_SIZE: usize = 4;

/// Whether read connections are on: they are unless [`READ_CONNECTIONS_ENV`]
/// turns them off.
pub fn read_connections_enabled() -> bool {
    enabled_by(std::env::var(READ_CONNECTIONS_ENV).ok().as_deref())
}

/// Whether `value` of [`READ_CONNECTIONS_ENV`] leaves read connections on.
fn enabled_by(value: Option<&str>) -> bool {
    !value.is_some_and(|v| {
        matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "off" | "no"
        )
    })
}

/// Which tasks have a write transaction open, so a debug build can catch a
/// reader used inside one. `begin()` of a backend notes the task that opened
/// it, `commit()` and `rollback()` take the note away. Nothing is kept in a
/// release build.
pub mod txn {
    #[cfg(debug_assertions)]
    static OPEN: std::sync::Mutex<Vec<tokio::task::Id>> = std::sync::Mutex::new(Vec::new());

    /// The current task opened a write transaction.
    pub fn opened() {
        #[cfg(debug_assertions)]
        if let Some(id) = tokio::task::try_id() {
            OPEN.lock().unwrap_or_else(|p| p.into_inner()).push(id);
        }
    }

    /// The current task ended its write transaction.
    pub fn closed() {
        #[cfg(debug_assertions)]
        if let Some(id) = tokio::task::try_id() {
            let mut open = OPEN.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(i) = open.iter().position(|o| *o == id) {
                open.swap_remove(i);
            }
        }
    }

    /// Whether the current task has a write transaction open.
    pub fn open_here() -> bool {
        #[cfg(debug_assertions)]
        if let Some(id) = tokio::task::try_id() {
            return OPEN.lock().unwrap_or_else(|p| p.into_inner()).contains(&id);
        }
        false
    }

    /// Panic in a debug build when the current task has a write transaction
    /// open: a read inside it belongs on the writer it holds.
    #[track_caller]
    pub fn assert_none_open() {
        debug_assert!(
            !open_here(),
            "a reader was used inside an open write transaction; read through the writer instead"
        );
    }
}

/// Every [`ReadStore`] method, handed to `$callback` with the tokens in
/// front, so a wrapper is written once per kind and not once per method.
macro_rules! read_store_methods {
    ($callback:ident ! { $($front:tt)* }) => {
        $callback! { $($front)*
            fn domain_id(&self, name: &str) -> Result<Option<DomainId>>;
            fn domain_spellings(&self) -> Result<Vec<(String, DomainId)>>;
            fn meta_values(&self) -> Result<Vec<(EngramId, String, String)>>;
            fn engrams_referencing_domains(&self, spellings: &[String]) -> Result<Vec<(String, String)>>;
            fn spelled_references(&self, domain: DomainId, spellings: &[String]) -> Result<Vec<SpelledRef>>;
            fn file_stamps(&self, domain: DomainId) -> Result<HashMap<String, FileStamp>>;
            fn engram_rows_under_reflections(&self, domain: DomainId, limit: usize) -> Result<Vec<String>>;
            fn has_engram_rows(&self, domain: DomainId) -> Result<bool>;
            fn has_reflection_rows(&self, domain: DomainId) -> Result<bool>;
            fn base_paths(&self, domain: DomainId, paths: &[String]) -> Result<HashSet<String>>;
            fn cited_engrams(&self, domain: DomainId, permalinks: &[String]) -> Result<Vec<CitedEngram>>;
            fn engram_content(&self, domain: DomainId, path: &str) -> Result<Option<String>>;
            fn all_engram_contents(&self, domain: DomainId) -> Result<Vec<StoredEngram>>;
            fn lookup_id(&self, domain: &str, permalink: &str) -> Result<Option<EngramId>>;
            fn find_engram(&self, domain: &str, key: &str) -> Result<Option<EngramDescriptor>>;
            fn find_engram_any(&self, key: &str) -> Result<Vec<EngramDescriptor>>;
            fn list_engrams(&self, domain: &str, path_prefix: Option<&str>, engram_type: Option<&str>) -> Result<Vec<EngramDescriptor>>;
            fn browse_level(&self, domain: &str, path_prefix: Option<&str>, depth: usize, limit: usize) -> Result<BrowseLevel>;
            fn folder_holds_other(&self, domain: DomainId, prefix: &str, except: &str) -> Result<bool>;
            fn engrams_with_tag(&self, tag: &str, domain: Option<&str>) -> Result<Vec<EngramDescriptor>>;
            fn tag_aliases(&self, domains: Option<&[String]>) -> Result<Vec<(String, String)>>;
            fn inbound_refs(&self, engram_id: EngramId, domain_id: DomainId, permalink: &str, title: &str) -> Result<Vec<InboundRef>>;
            fn engrams_mentioning(&self, needle: &str) -> Result<Vec<ContentMention>>;
            fn inbound_page(&self, query: &InboundQuery<'_>) -> Result<InboundPage>;
            fn outbound_refs(&self, engram_id: EngramId, actor: Option<&str>) -> Result<Vec<OutboundRef>>;
            fn unresolved_refs(&self, domain: DomainId, actor: Option<&str>) -> Result<Vec<UnresolvedRef>>;
            fn search(&self, query: &SearchQuery) -> Result<Page<SearchHit>>;
            fn search_with_candidate_cap(&self, query: &SearchQuery, candidate_cap: usize) -> Result<Page<SearchHit>>;
            fn neighbors(&self, ids: &[EngramId], depth: u8, actor: Option<&str>) -> Result<GraphSlice>;
            fn recent(&self, filter: &RecentFilter) -> Result<Vec<EngramSummary>>;
            fn chunks_needing_embedding_in(&self, table: ChunkTable, model: &str, domains: Option<&[DomainId]>, limit: usize, after: Option<(i64, i64)>) -> Result<Vec<ChunkJob>>;
            fn embedding_coverage_in(&self, table: ChunkTable, domains: Option<&[DomainId]>) -> Result<EmbeddingCoverage>;
            fn embedding_coverage_of_names(&self, names: &[String]) -> Result<EmbeddingCoverage>;
            fn embedding_coverage_of(&self, table: ChunkTable) -> Result<EmbeddingCoverage>;
            fn chunks_needing_embedding(&self, model: &str, domains: Option<&[DomainId]>, limit: usize, after: Option<(i64, i64)>) -> Result<Vec<ChunkJob>>;
            fn embedding_coverage(&self) -> Result<EmbeddingCoverage>;
            fn lead_vectors(&self, domain: DomainId, model: &str, actor: Option<&str>) -> Result<Vec<LeadVector>>;
            fn contradiction_pairs_scored(&self, domain: DomainId, model: &str) -> Result<Vec<ScoredPair>>;
            fn contradictions(&self, domain: DomainId, model: &str, min_score: f32) -> Result<Vec<ContradictionRow>>;
            fn scored_pair_count(&self, model: &str) -> Result<u64>;
            fn observation_vectors_present(&self, model: &str, hashes: &[String]) -> Result<HashSet<String>>;
            fn observation_vectors(&self, model: &str, hashes: &[String]) -> Result<HashMap<String, Vec<f32>>>;
            fn observation_vector_hashes(&self, model: &str) -> Result<Vec<String>>;
            fn parse_generation(&self, domain: DomainId) -> Result<u32>;
            fn overlay_entry(&self, domain: DomainId, actor: &str, path: &str) -> Result<Option<StoredEngram>>;
            fn overlay_entries(&self, domain: DomainId, actor: &str) -> Result<Vec<StoredEngram>>;
            fn overlay_paths(&self, domain: DomainId, actor: &str) -> Result<Vec<(String, bool)>>;
            fn overlay_counts(&self, domain: DomainId) -> Result<Vec<(String, u64)>>;
            fn store_info(&self) -> Result<StoreInfo>;
            fn domain_stats(&self) -> Result<Vec<DomainStats>>;
            fn domain_names(&self) -> Result<Vec<String>>;
            fn vocabulary(&self, domain: Option<&str>, actor: Option<&str>) -> Result<Vocabulary>;
            fn get_asset(&self, table: AssetTable, domain: DomainId, path: &str) -> Result<Option<AttachmentRow>>;
            fn list_assets(&self, table: AssetTable, domain: DomainId) -> Result<Vec<AttachmentRow>>;
            fn get_attachment(&self, domain: DomainId, path: &str) -> Result<Option<AttachmentRow>>;
            fn list_attachments(&self, domain: DomainId) -> Result<Vec<AttachmentRow>>;
            fn read_attachment_blob(&self, domain: DomainId, path: &str) -> Result<Option<Vec<u8>>>;
            fn get_reflection(&self, domain: DomainId, path: &str) -> Result<Option<ReflectionRow>>;
            fn list_reflections(&self, domain: DomainId) -> Result<Vec<ReflectionRow>>;
            fn list_reflection_stamps(&self, domain: DomainId) -> Result<Vec<ReflectionStamp>>;
            fn reflection_coverage(&self, domain: DomainId) -> Result<EmbeddingCoverage>;
            fn reflection_vector_hits(&self, domain: DomainId, query: &[f32], model: &str) -> Result<Vec<SectionVector>>;
            fn own_labels(&self, instance_id: &str) -> Result<Vec<String>>;
            fn live_label_holders(&self, label: &str, stale_before: &str) -> Result<Vec<String>>;
            fn domain_host(&self, domain: DomainId) -> Result<Option<DomainHost>>;
            fn domain_host_held(&self, domain: DomainId) -> Result<Option<DomainHost>>;
            fn reflection_disclosures(&self, domain: DomainId) -> Result<Vec<(String, String)>>;
            fn automation_lease(&self, domain: DomainId, loop_kind: &str) -> Result<Option<AutomationLease>>;
            fn automation_runs(&self, domain: DomainId, loop_kind: &str, limit: usize) -> Result<Vec<AutomationRun>>;
            fn host_move_text(&self, domain: DomainId) -> Result<Option<String>>;
            fn last_domain_host(&self, domain: DomainId) -> Result<Option<(String, String)>>;
            fn label_taken(&self, label: &str, instance_id: &str) -> Result<bool>;
            fn host_move(&self, domain: DomainId) -> Result<Option<HostMove>>;
        }
    };
}

/// `impl ReadStore for $ty` where every method runs `$wrap!(self, s, call)`,
/// `call` being the same method on `s`.
macro_rules! delegate_read_store {
    ([$($gen:tt)*] [$ty:ty] [$wrap:ident]
     $( fn $name:ident(&self $(, $arg:ident : $argty:ty)* $(,)?) -> $ret:ty; )*) => {
        #[async_trait::async_trait]
        impl<$($gen)*> $crate::store::ReadStore for $ty {
            $(
                async fn $name(&self $(, $arg: $argty)*) -> $ret {
                    $wrap!(self, s, s.$name($($arg),*))
                }
            )*
        }
    };
}

/// Reads through the writer, under its lock: the behaviour before read
/// connections.
pub struct LockedReader(Arc<Mutex<dyn Store>>);

impl LockedReader {
    pub fn new(store: Arc<Mutex<dyn Store>>) -> LockedReader {
        LockedReader(store)
    }
}

macro_rules! locked_call {
    ($self:ident, $s:ident, $call:expr) => {{
        txn::assert_none_open();
        let guard = $self.0.lock().await;
        let $s: &dyn Store = &*guard;
        $call.await
    }};
}

mod locked {
    use super::*;
    use crate::error::Result;
    use crate::store::*;
    use crate::sweep::{SpelledRef, UnresolvedRef};
    use std::collections::{HashMap, HashSet};

    read_store_methods!(delegate_read_store! { [] [LockedReader] [locked_call] });
}

/// What a backend's read connection does around one call.
#[async_trait]
pub(crate) trait ReaderBackend: ReadStore + Send + Sync + 'static {
    /// Open the call's read transaction.
    async fn open_read(&self) -> Result<()>;
    /// End it: commit after a successful call, roll back otherwise. Never
    /// fails the call; a failure here is logged.
    async fn close_read(&self, ok: bool);
    /// Whether a failed call may run once more from the start (the turso
    /// temp directory refusal).
    fn retryable(&self, _err: &IndexError) -> bool {
        false
    }
    /// Whether the backend's statements step synchronously on the polling
    /// thread, so a call should hand its worker off with `block_in_place`.
    fn blocking(&self) -> bool {
        false
    }
}

/// A fixed set of read connections, one per call.
pub(crate) struct ReadPool<S> {
    slots: std::sync::Mutex<Vec<S>>,
    permits: Semaphore,
    size: u32,
}

/// One read connection, taken from the pool for one call.
pub(crate) struct Lease<'a, S> {
    pool: &'a ReadPool<S>,
    store: Option<S>,
    _permit: SemaphorePermit<'a>,
}

impl<S> Lease<'_, S> {
    pub(crate) fn store(&self) -> &S {
        self.store.as_ref().expect("a lease holds its connection")
    }
}

impl<S> Drop for Lease<'_, S> {
    fn drop(&mut self) {
        if let Some(s) = self.store.take() {
            self.pool
                .slots
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(s);
        }
    }
}

impl<S> ReadPool<S> {
    pub(crate) fn new(slots: Vec<S>) -> ReadPool<S> {
        let size = slots.len();
        ReadPool {
            slots: std::sync::Mutex::new(slots),
            permits: Semaphore::new(size),
            size: size as u32,
        }
    }

    pub(crate) async fn lease(&self) -> Result<Lease<'_, S>> {
        let permit = self
            .permits
            .acquire()
            .await
            .map_err(|_| IndexError::Db("the read connections are closed".to_string()))?;
        let store = self
            .slots
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .pop()
            .expect("a permit always finds a free connection");
        Ok(Lease {
            pool: self,
            store: Some(store),
            _permit: permit,
        })
    }

    /// Every permit: no reader runs until the returned permit is dropped.
    pub(crate) async fn drain(&self) -> Result<SemaphorePermit<'_>> {
        self.permits
            .acquire_many(self.size)
            .await
            .map_err(|_| IndexError::Db("the read connections are closed".to_string()))
    }
}

/// Run `fut` to the end. When `blocking` and the runtime has several workers,
/// the worker is handed off (`block_in_place`) first, so statements that step
/// on this thread do not hold up the other tasks queued on it.
pub(crate) async fn drive<F: Future>(blocking: bool, fut: F) -> F::Output {
    use tokio::runtime::{Handle, RuntimeFlavor};
    match Handle::try_current() {
        Ok(handle) if blocking && handle.runtime_flavor() == RuntimeFlavor::MultiThread => {
            tokio::task::block_in_place(|| handle.block_on(fut))
        }
        _ => fut.await,
    }
}

macro_rules! pooled_call {
    ($self:ident, $s:ident, $call:expr) => {{
        $crate::reader::txn::assert_none_open();
        let lease = $self.lease().await?;
        let $s = lease.store();
        $crate::reader::drive($crate::reader::ReaderBackend::blocking($s), async {
            let mut retried = false;
            loop {
                let out = match $crate::reader::ReaderBackend::open_read($s).await {
                    Ok(()) => $call.await,
                    Err(e) => Err(e),
                };
                $crate::reader::ReaderBackend::close_read($s, out.is_ok()).await;
                match out {
                    Err(e) if !retried && $crate::reader::ReaderBackend::retryable($s, &e) => {
                        retried = true;
                        tracing::warn!(
                            "a read failed on a refused temp directory; running it again"
                        );
                    }
                    other => break other,
                }
            }
        })
        .await
    }};
}

mod pooled {
    use super::*;
    use crate::error::Result;
    use crate::store::*;
    use crate::sweep::{SpelledRef, UnresolvedRef};
    use std::collections::{HashMap, HashSet};

    read_store_methods!(delegate_read_store! { [S: ReaderBackend] [ReadPool<S>] [pooled_call] });
}

/// The embedding coverage snapshot per chunk table, shared by a store's writer
/// and its read connections.
///
/// A reader fills it from the state its read transaction saw, and a writer
/// may commit a change in the meantime, so a fill is only kept when nothing
/// was invalidated since the reader looked at [`CoverageCache::generation`]
/// before it opened its transaction. The writer invalidates when a write
/// starts (`invalidate`), again when the write has committed (when the
/// returned guard is dropped outside a transaction, or at `commit`), and
/// never fills the cache while a change of its own is not committed yet.
#[derive(Default)]
pub(crate) struct CoverageCache {
    state: std::sync::Mutex<CoverageState>,
    /// The writer has a transaction open (`begin` until `commit`/`rollback`).
    txn_open: std::sync::atomic::AtomicBool,
    /// The writer changed embedding state that is not committed yet.
    dirty: std::sync::atomic::AtomicBool,
}

#[derive(Default)]
struct CoverageState {
    generation: u64,
    tables: std::collections::HashMap<crate::store::ChunkTable, crate::store::EmbeddingCoverage>,
}

/// Invalidates the cache once more when the write it was taken for is over,
/// unless that write runs inside a transaction: then `commit` does it.
#[must_use = "the cache must be invalidated again after the write"]
pub(crate) struct CoverageGuard<'a>(&'a CoverageCache);

impl Drop for CoverageGuard<'_> {
    fn drop(&mut self) {
        use std::sync::atomic::Ordering;
        if !self.0.txn_open.load(Ordering::Acquire) {
            self.0.clear();
            self.0.dirty.store(false, Ordering::Release);
        }
    }
}

impl CoverageCache {
    fn lock(&self) -> std::sync::MutexGuard<'_, CoverageState> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn clear(&self) {
        let mut state = self.lock();
        state.generation += 1;
        state.tables.clear();
    }

    pub(crate) fn generation(&self) -> u64 {
        self.lock().generation
    }

    pub(crate) fn get(
        &self,
        table: crate::store::ChunkTable,
    ) -> Option<crate::store::EmbeddingCoverage> {
        self.lock().tables.get(&table).cloned()
    }

    /// The writer is about to change embedding state.
    pub(crate) fn invalidate(&self) -> CoverageGuard<'_> {
        self.dirty.store(true, std::sync::atomic::Ordering::Release);
        self.clear();
        CoverageGuard(self)
    }

    /// The writer opened a transaction.
    pub(crate) fn began(&self) {
        self.txn_open
            .store(true, std::sync::atomic::Ordering::Release);
    }

    /// The writer's transaction ended: committed, or rolled back.
    pub(crate) fn ended(&self, rolled_back: bool) {
        use std::sync::atomic::Ordering;
        self.txn_open.store(false, Ordering::Release);
        if self.dirty.swap(false, Ordering::AcqRel) || rolled_back {
            self.clear();
        }
    }

    /// A snapshot the writer computed: kept unless the writer holds an
    /// uncommitted change to embedding state.
    pub(crate) fn fill_from_writer(
        &self,
        table: crate::store::ChunkTable,
        coverage: crate::store::EmbeddingCoverage,
    ) {
        if !self.dirty.load(std::sync::atomic::Ordering::Acquire) {
            self.lock().tables.insert(table, coverage);
        }
    }

    /// A snapshot a reader computed in a transaction it opened at
    /// `generation`: kept only when nothing was invalidated since.
    pub(crate) fn fill_from_reader(
        &self,
        table: crate::store::ChunkTable,
        coverage: crate::store::EmbeddingCoverage,
        generation: u64,
    ) {
        let mut state = self.lock();
        if state.generation == generation {
            state.tables.insert(table, coverage);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{ChunkTable, EmbeddingCoverage};
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn the_kill_switch_is_zero_and_its_spellings() {
        assert!(enabled_by(None));
        assert!(enabled_by(Some("1")));
        assert!(enabled_by(Some("")));
        for off in ["0", " 0 ", "false", "OFF", "no"] {
            assert!(!enabled_by(Some(off)), "{off}");
        }
    }

    #[tokio::test]
    async fn a_task_with_an_open_write_transaction_is_noted_until_it_ends() {
        tokio::spawn(async {
            assert!(!txn::open_here());
            txn::opened();
            assert_eq!(txn::open_here(), cfg!(debug_assertions));
            txn::closed();
            assert!(!txn::open_here());
        })
        .await
        .unwrap();
    }

    #[cfg(debug_assertions)]
    #[tokio::test]
    #[should_panic(expected = "a reader was used inside an open write transaction")]
    async fn a_reader_inside_an_open_write_transaction_panics_in_a_debug_build() {
        let outcome = tokio::spawn(async {
            txn::opened();
            txn::assert_none_open();
        })
        .await;
        if let Err(e) = outcome {
            std::panic::resume_unwind(e.into_panic());
        }
    }

    #[test]
    fn a_reader_fill_from_before_an_invalidation_is_not_kept() {
        let cache = CoverageCache::default();
        let seen = cache.generation();
        // The writer changes embedding state and commits while the reader
        // computes from its older snapshot.
        drop(cache.invalidate());
        cache.fill_from_reader(ChunkTable::Engram, EmbeddingCoverage::default(), seen);
        assert!(cache.get(ChunkTable::Engram).is_none());
        // A reader that began after the commit keeps its fill.
        let seen = cache.generation();
        cache.fill_from_reader(ChunkTable::Engram, EmbeddingCoverage::default(), seen);
        assert!(cache.get(ChunkTable::Engram).is_some());
    }

    #[test]
    fn a_change_inside_a_transaction_is_not_cached_until_it_commits() {
        let cache = CoverageCache::default();
        cache.began();
        let guard = cache.invalidate();
        drop(guard);
        // The writer's own snapshot holds its uncommitted change: not kept.
        cache.fill_from_writer(ChunkTable::Engram, EmbeddingCoverage::default());
        assert!(cache.get(ChunkTable::Engram).is_none());
        // A reader that began before the commit is dropped by the commit.
        let seen = cache.generation();
        cache.ended(false);
        cache.fill_from_reader(ChunkTable::Engram, EmbeddingCoverage::default(), seen);
        assert!(cache.get(ChunkTable::Engram).is_none());
        cache.fill_from_writer(ChunkTable::Engram, EmbeddingCoverage::default());
        assert!(cache.get(ChunkTable::Engram).is_some());
    }

    /// A backend whose first `fail` calls are refused ("refused", retryable
    /// when `retryable`), and whose later calls are "answered": an error too,
    /// so one stand-in serves every method's return type.
    struct Flaky {
        calls: AtomicUsize,
        opened: AtomicUsize,
        closed: AtomicUsize,
        fail: usize,
        retryable: bool,
    }

    impl Flaky {
        fn new(fail: usize, retryable: bool) -> Flaky {
            Flaky {
                calls: AtomicUsize::new(0),
                opened: AtomicUsize::new(0),
                closed: AtomicUsize::new(0),
                fail,
                retryable,
            }
        }

        fn answer(&self) -> IndexError {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            IndexError::Db(if n < self.fail { "refused" } else { "answered" }.to_string())
        }
    }

    #[async_trait]
    impl ReaderBackend for Flaky {
        async fn open_read(&self) -> Result<()> {
            self.opened.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        async fn close_read(&self, _ok: bool) {
            self.closed.fetch_add(1, Ordering::SeqCst);
        }
        fn retryable(&self, err: &IndexError) -> bool {
            self.retryable && err.to_string().contains("refused")
        }
    }

    macro_rules! flaky_call {
        ($self:ident, $s:ident, $call:expr) => {{ Err($self.answer()) }};
    }

    // The stand-in answers without looking at the arguments.
    #[allow(unused_variables)]
    mod flaky {
        use super::*;
        use crate::store::*;
        use crate::sweep::{SpelledRef, UnresolvedRef};
        use std::collections::{HashMap, HashSet};

        read_store_methods!(delegate_read_store! { [] [Flaky] [flaky_call] });
    }

    /// `domain_names` through a one-connection pool, and the counts after.
    async fn through_pool(backend: Flaky) -> (String, [usize; 3]) {
        let pool = ReadPool::new(vec![backend]);
        let err = pool.domain_names().await.unwrap_err().to_string();
        let lease = pool.lease().await.unwrap();
        let s = lease.store();
        (
            err,
            [
                s.calls.load(Ordering::SeqCst),
                s.opened.load(Ordering::SeqCst),
                s.closed.load(Ordering::SeqCst),
            ],
        )
    }

    #[tokio::test]
    async fn a_retryable_refusal_runs_the_whole_read_once_more_and_only_once() {
        let (err, counts) = through_pool(Flaky::new(1, true)).await;
        assert!(err.contains("answered"), "{err}");
        assert_eq!(
            counts,
            [2, 2, 2],
            "two calls, each in a read transaction of its own"
        );

        let (err, counts) = through_pool(Flaky::new(2, true)).await;
        assert!(
            err.contains("refused"),
            "a second refusal is returned: {err}"
        );
        assert_eq!(counts, [2, 2, 2]);

        let (err, counts) = through_pool(Flaky::new(1, false)).await;
        assert!(
            err.contains("refused"),
            "any other error is returned at once: {err}"
        );
        assert_eq!(counts, [1, 1, 1]);
    }

    #[tokio::test]
    async fn a_drain_waits_for_every_reader_and_holds_them_off() {
        let pool = Arc::new(ReadPool::new(vec![
            Flaky::new(0, false),
            Flaky::new(0, false),
        ]));
        let lease = pool.lease().await.unwrap();
        let drained = {
            let pool = pool.clone();
            tokio::spawn(async move {
                let _all = pool.drain().await.unwrap();
            })
        };
        tokio::task::yield_now().await;
        assert!(!drained.is_finished(), "the drain waits for the lease");
        drop(lease);
        drained.await.unwrap();
        assert!(pool.lease().await.is_ok());
    }
}
