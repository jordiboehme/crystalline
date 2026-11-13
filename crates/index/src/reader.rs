//! Read connections beside the write connection.
//!
//! The engine holds one store behind a mutex for every write and every
//! transaction. Reads that need no guard go through a [`ReadStore`] beside it
//! instead, so a search does not wait for a sync apply and two searches do not
//! wait for each other:
//!
//! - A read pool keeps a few read connections of the backend (turso: more
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
use std::pin::Pin;
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
            fn inbound_page(&self, query: InboundQuery<'_>) -> Result<InboundPage>;
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

/// `impl ReadStore for $ty` where every method runs
/// `$wrap!(self, name, (args))`: the wrapper calls the same method with the
/// same arguments on the store it picks.
macro_rules! delegate_read_store {
    ([$($gen:tt)*] [$ty:ty] [$wrap:ident]
     $( fn $name:ident(&self $(, $arg:ident : $argty:ty)* $(,)?) -> $ret:ty; )*) => {
        #[async_trait::async_trait]
        impl<$($gen)*> $crate::store::ReadStore for $ty {
            $(
                async fn $name(&self $(, $arg: $argty)*) -> $ret {
                    $wrap!(self, $name, ($($arg),*))
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
    ($self:ident, $name:ident, ($($arg:ident),*)) => {{
        txn::assert_none_open();
        let guard = $self.0.lock().await;
        let store: &dyn Store = &*guard;
        store.$name($($arg),*).await
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
    /// thread. Such a connection gets a thread of its own (see [`ReadPool`]).
    fn blocking(&self) -> bool {
        false
    }
}

/// One call's work on a read connection: the store, and the call's own
/// arguments it was handed.
type Call<S, O, T> = dyn for<'s> Fn(&'s S, &'s O) -> Pin<Box<dyn Future<Output = Result<T>> + Send + 's>>
    + Send
    + Sync;

/// A job for a reader thread: run on its connection, inside its runtime.
type Job<S> = Box<dyn FnOnce(&S, &tokio::runtime::Runtime) + Send>;

/// Where the read connections live.
enum Slots<S> {
    /// In the pool, borrowed by the calling task for its call (Postgres,
    /// whose statements are real I/O the caller's task awaits).
    InTask(std::sync::Mutex<Vec<S>>),
    /// Each on a thread of its own, which runs the calls handed to it
    /// (turso, whose statements step synchronously). A call allocates on
    /// that thread, so the allocator's per-thread caches stay with a fixed
    /// set of threads; running them on the caller's worker with
    /// `block_in_place` had tokio rotate workers through ever more threads,
    /// and each kept its own cache: 344 to 397 MiB of physical footprint
    /// after a reindex beside eight searchers, against 113 MiB without
    /// readers and 160 MiB with the calls kept on fixed threads.
    Threads {
        free: Arc<std::sync::Mutex<Vec<usize>>>,
        senders: Vec<std::sync::mpsc::Sender<Job<S>>>,
    },
}

/// A fixed set of read connections, one per call.
pub(crate) struct ReadPool<S> {
    slots: Slots<S>,
    permits: Arc<Semaphore>,
    size: u32,
}

/// One read connection of an in-task pool, taken for one call.
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
        if let (Some(s), Slots::InTask(slots)) = (self.store.take(), &self.pool.slots) {
            slots.lock().unwrap_or_else(|p| p.into_inner()).push(s);
        }
    }
}

fn closed() -> IndexError {
    IndexError::Db("the read connections are closed".to_string())
}

impl<S: ReaderBackend> ReadPool<S> {
    /// A pool over `slots`. Connections whose statements step synchronously
    /// each get a thread of their own (`crystalline-reader-<n>`), which ends
    /// when the pool is dropped.
    pub(crate) fn new(slots: Vec<S>) -> std::io::Result<ReadPool<S>> {
        let size = slots.len();
        let threaded = slots.first().is_some_and(ReaderBackend::blocking);
        let slots = if threaded {
            let mut senders = Vec::with_capacity(size);
            for (n, store) in slots.into_iter().enumerate() {
                let (tx, rx) = std::sync::mpsc::channel::<Job<S>>();
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()?;
                std::thread::Builder::new()
                    .name(format!("crystalline-reader-{n}"))
                    .spawn(move || {
                        while let Ok(job) = rx.recv() {
                            job(&store, &runtime);
                            // Work through what is queued, then, idle,
                            // give the allocator's cache on this thread back.
                            while let Ok(job) = rx.try_recv() {
                                job(&store, &runtime);
                            }
                            give_back_freed_memory();
                        }
                    })?;
                senders.push(tx);
            }
            Slots::Threads {
                free: Arc::new(std::sync::Mutex::new((0..size).collect())),
                senders,
            }
        } else {
            Slots::InTask(std::sync::Mutex::new(slots))
        };
        Ok(ReadPool {
            slots,
            permits: Arc::new(Semaphore::new(size)),
            size: size as u32,
        })
    }

    /// One connection of an in-task pool, for the calling task. A threaded
    /// pool hands none out: its connections stay on their threads.
    pub(crate) async fn lease(&self) -> Result<Lease<'_, S>> {
        let Slots::InTask(slots) = &self.slots else {
            return Err(IndexError::Db(
                "a threaded read pool lends no connection".to_string(),
            ));
        };
        let permit = self.permits.acquire().await.map_err(|_| closed())?;
        let store = slots
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .pop()
            .ok_or_else(closed)?;
        Ok(Lease {
            pool: self,
            store: Some(store),
            _permit: permit,
        })
    }

    /// Every permit: no reader runs until the returned permit is dropped. On
    /// a threaded pool a permit comes back only when its call has ended on
    /// its thread, so a caller that gave up waiting does not count as idle.
    pub(crate) async fn drain(&self) -> Result<SemaphorePermit<'_>> {
        self.permits
            .acquire_many(self.size)
            .await
            .map_err(|_| closed())
    }

    /// Run `call` with `args` on one connection, in one read transaction,
    /// and once more from the start when the first run failed with an error
    /// the backend calls retryable.
    pub(crate) async fn run<T, O, F>(&self, args: O, call: F) -> Result<T>
    where
        T: Send + 'static,
        O: Send + Sync + 'static,
        F: for<'s> Fn(&'s S, &'s O) -> Pin<Box<dyn Future<Output = Result<T>> + Send + 's>>
            + Send
            + Sync
            + 'static,
    {
        match &self.slots {
            Slots::InTask(_) => {
                let lease = self.lease().await?;
                attempts(lease.store(), &args, &call).await
            }
            Slots::Threads { free, senders } => {
                let permit = self
                    .permits
                    .clone()
                    .acquire_owned()
                    .await
                    .map_err(|_| closed())?;
                let slot = free
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .pop()
                    .ok_or_else(closed)?;
                let (tx, rx) = tokio::sync::oneshot::channel();
                let free = free.clone();
                let job: Job<S> = Box::new(move |store: &S, runtime: &tokio::runtime::Runtime| {
                    let call: &Call<S, O, T> = &call;
                    let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        runtime.block_on(attempts(store, &args, call))
                    }))
                    .unwrap_or_else(|_| {
                        // Never leave a read transaction open behind a panic.
                        runtime.block_on(store.close_read(false));
                        Err(IndexError::Db("a read failed with a panic".to_string()))
                    });
                    let _ = tx.send(out);
                    // The connection is free again before its permit is.
                    free.lock().unwrap_or_else(|p| p.into_inner()).push(slot);
                    drop(permit);
                });
                senders[slot].send(job).map_err(|_| closed())?;
                rx.await.map_err(|_| closed())?
            }
        }
    }
}

/// Give the memory this thread freed back to the allocator's shared pool
/// and the OS. turso installs mimalloc as the global allocator, and
/// mimalloc keeps what a thread frees in that thread's own heap: a search
/// allocates tens of MiB (its candidates, lowered), and each reader thread
/// kept that much after it went idle. Measured on 20k engrams beside eight
/// searchers, four readers held 77 to 95 MiB more physical footprint than
/// no readers; collecting when a reader thread goes idle brings that to the
/// footprint without readers (134 to 143 MiB against 138 to 155 MiB).
/// Harmless when another allocator is the global one: mimalloc then has
/// nothing of this thread's to give back.
fn give_back_freed_memory() {
    // SAFETY: `mi_collect` takes no pointers and may be called from any
    // thread at any time; it only walks this thread's own heap.
    unsafe { libmimalloc_sys::mi_collect(true) };
}

/// One read transaction around `call`, and a second one when the first
/// failed retryably.
async fn attempts<S, O, T>(store: &S, args: &O, call: &Call<S, O, T>) -> Result<T>
where
    S: ReaderBackend,
{
    let mut retried = false;
    loop {
        let out = match store.open_read().await {
            Ok(()) => call(store, args).await,
            Err(e) => Err(e),
        };
        store.close_read(out.is_ok()).await;
        match out {
            Err(e) if !retried && store.retryable(&e) => {
                retried = true;
                tracing::warn!("a read failed on a refused temp directory; running it again");
            }
            other => return other,
        }
    }
}

/// A store method's argument, carried to a reader thread as a value of its
/// own and handed to the method from there.
pub(crate) trait Arg<'a>: Sized {
    type Owned: Send + Sync + 'static;
    fn own(&self) -> Self::Owned;
    fn view(owned: &'a Self::Owned) -> Self;
}

macro_rules! copy_args {
    ($($t:ty),*) => {$(
        impl<'a> Arg<'a> for $t {
            type Owned = $t;
            fn own(&self) -> $t {
                *self
            }
            fn view(owned: &'a $t) -> $t {
                *owned
            }
        }
    )*};
}

copy_args!(
    crate::store::DomainId,
    crate::store::EngramId,
    crate::store::ChunkTable,
    crate::store::AssetTable,
    usize,
    u8,
    f32,
    Option<(i64, i64)>
);

impl<'a> Arg<'a> for &'a str {
    type Owned = String;
    fn own(&self) -> String {
        self.to_string()
    }
    fn view(owned: &'a String) -> &'a str {
        owned
    }
}

impl<'a> Arg<'a> for Option<&'a str> {
    type Owned = Option<String>;
    fn own(&self) -> Option<String> {
        self.map(str::to_string)
    }
    fn view(owned: &'a Option<String>) -> Option<&'a str> {
        owned.as_deref()
    }
}

impl<'a, T: Clone + Send + Sync + 'static> Arg<'a> for &'a [T] {
    type Owned = Vec<T>;
    fn own(&self) -> Vec<T> {
        self.to_vec()
    }
    fn view(owned: &'a Vec<T>) -> &'a [T] {
        owned
    }
}

impl<'a, T: Clone + Send + Sync + 'static> Arg<'a> for Option<&'a [T]> {
    type Owned = Option<Vec<T>>;
    fn own(&self) -> Option<Vec<T>> {
        self.map(<[T]>::to_vec)
    }
    fn view(owned: &'a Option<Vec<T>>) -> Option<&'a [T]> {
        owned.as_deref()
    }
}

macro_rules! clone_args {
    ($($t:ty),*) => {$(
        impl<'a> Arg<'a> for &'a $t {
            type Owned = $t;
            fn own(&self) -> $t {
                (*self).clone()
            }
            fn view(owned: &'a $t) -> &'a $t {
                owned
            }
        }
    )*};
}

clone_args!(crate::store::SearchQuery, crate::store::RecentFilter);

/// [`crate::store::InboundQuery`]'s fields, owned.
pub(crate) struct OwnedInboundQuery {
    engram_id: crate::store::EngramId,
    domain_id: crate::store::DomainId,
    permalink: String,
    title: String,
    q: Option<String>,
    rel: Option<String>,
    exclude_domains: Vec<String>,
    page: usize,
    limit: usize,
}

impl<'a> Arg<'a> for crate::store::InboundQuery<'a> {
    type Owned = OwnedInboundQuery;
    fn own(&self) -> OwnedInboundQuery {
        OwnedInboundQuery {
            engram_id: self.engram_id,
            domain_id: self.domain_id,
            permalink: self.permalink.to_string(),
            title: self.title.to_string(),
            q: self.q.map(str::to_string),
            rel: self.rel.map(str::to_string),
            exclude_domains: self.exclude_domains.to_vec(),
            page: self.page,
            limit: self.limit,
        }
    }
    fn view(owned: &'a OwnedInboundQuery) -> crate::store::InboundQuery<'a> {
        crate::store::InboundQuery {
            engram_id: owned.engram_id,
            domain_id: owned.domain_id,
            permalink: &owned.permalink,
            title: &owned.title,
            q: owned.q.as_deref(),
            rel: owned.rel.as_deref(),
            exclude_domains: &owned.exclude_domains,
            page: owned.page,
            limit: owned.limit,
        }
    }
}

macro_rules! pooled_call {
    ($self:ident, $name:ident, ($($arg:ident),*)) => {{
        $crate::reader::txn::assert_none_open();
        let args = ($($crate::reader::Arg::own(&$arg),)*);
        $self
            .run(args, |store, args| {
                let ($($arg,)*) = args;
                store.$name($($crate::reader::Arg::view($arg)),*)
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

    /// What a [`Flaky`] saw, kept outside it: a threaded pool keeps its
    /// connections on their threads.
    #[derive(Default)]
    struct Seen {
        calls: AtomicUsize,
        opened: AtomicUsize,
        closed: AtomicUsize,
    }

    impl Seen {
        fn counts(&self) -> [usize; 3] {
            [
                self.calls.load(Ordering::SeqCst),
                self.opened.load(Ordering::SeqCst),
                self.closed.load(Ordering::SeqCst),
            ]
        }
    }

    /// A backend whose first `fail` calls are refused ("refused", retryable
    /// when `retryable`), and whose later calls are "answered": an error too,
    /// so one stand-in serves every method's return type. `threaded` puts
    /// it on threads of its own, as a turso connection is; `panics` makes
    /// its first call panic.
    struct Flaky {
        seen: Arc<Seen>,
        fail: usize,
        retryable: bool,
        threaded: bool,
        panics: bool,
    }

    impl Flaky {
        fn new(fail: usize, retryable: bool) -> Flaky {
            Flaky {
                seen: Arc::default(),
                fail,
                retryable,
                threaded: false,
                panics: false,
            }
        }

        fn threaded(mut self) -> Flaky {
            self.threaded = true;
            self
        }

        fn answer(&self) -> IndexError {
            let n = self.seen.calls.fetch_add(1, Ordering::SeqCst);
            if self.panics && n == 0 {
                panic!("a read that panics");
            }
            IndexError::Db(if n < self.fail { "refused" } else { "answered" }.to_string())
        }
    }

    #[async_trait]
    impl ReaderBackend for Flaky {
        async fn open_read(&self) -> Result<()> {
            self.seen.opened.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        async fn close_read(&self, _ok: bool) {
            self.seen.closed.fetch_add(1, Ordering::SeqCst);
        }
        fn retryable(&self, err: &IndexError) -> bool {
            self.retryable && err.to_string().contains("refused")
        }
        fn blocking(&self) -> bool {
            self.threaded
        }
    }

    macro_rules! flaky_call {
        ($self:ident, $name:ident, ($($arg:ident),*)) => {{ Err($self.answer()) }};
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
        let seen = backend.seen.clone();
        let pool = ReadPool::new(vec![backend]).unwrap();
        let err = pool.domain_names().await.unwrap_err().to_string();
        (err, seen.counts())
    }

    #[tokio::test]
    async fn a_retryable_refusal_runs_the_whole_read_once_more_and_only_once() {
        for threaded in [false, true] {
            let flaky = |fail, retryable| {
                let f = Flaky::new(fail, retryable);
                if threaded { f.threaded() } else { f }
            };
            let (err, counts) = through_pool(flaky(1, true)).await;
            assert!(err.contains("answered"), "{err}");
            assert_eq!(
                counts,
                [2, 2, 2],
                "two calls, each in a read transaction of its own"
            );

            let (err, counts) = through_pool(flaky(2, true)).await;
            assert!(
                err.contains("refused"),
                "a second refusal is returned: {err}"
            );
            assert_eq!(counts, [2, 2, 2]);

            let (err, counts) = through_pool(flaky(1, false)).await;
            assert!(
                err.contains("refused"),
                "any other error is returned at once: {err}"
            );
            assert_eq!(counts, [1, 1, 1]);
        }
    }

    #[tokio::test]
    async fn a_threaded_read_runs_on_a_reader_thread_and_a_panic_there_is_an_error() {
        let mut flaky = Flaky::new(0, false).threaded();
        flaky.panics = true;
        let seen = flaky.seen.clone();
        let pool = ReadPool::new(vec![flaky]).unwrap();
        let err = pool.domain_names().await.unwrap_err().to_string();
        assert!(err.contains("panic"), "{err}");
        // The connection's transaction was ended, and the connection serves
        // the next call.
        assert_eq!(seen.counts(), [1, 1, 1]);
        let err = pool.domain_names().await.unwrap_err().to_string();
        assert!(err.contains("answered"), "{err}");
        assert!(pool.drain().await.is_ok(), "every permit came back");
    }

    #[tokio::test]
    async fn a_drain_waits_for_every_reader_and_holds_them_off() {
        let pool =
            Arc::new(ReadPool::new(vec![Flaky::new(0, false), Flaky::new(0, false)]).unwrap());
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
