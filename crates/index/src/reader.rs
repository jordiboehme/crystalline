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

use std::sync::Arc;

use tokio::sync::Mutex;

use crate::store::Store;

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
