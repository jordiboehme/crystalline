//! `crystalline-index` is the derived, disposable index built from Engram files
//! on disk. It holds the backend-agnostic [`Store`] trait, the Turso-backed
//! implementation, the sync engine that keeps the index in step with the files
//! and the search planner. The embedding pipeline lands in M4; the `chunk` table
//! and the [`Store::chunks_needing_embedding`] and [`Store::store_embeddings`]
//! hooks are in place but unpopulated.
//!
//! Files are the source of truth and the index is fully rebuildable, so a
//! corrupt or stale index is never a data-loss risk: [`Store::wipe`] followed by
//! a resync (the `reindex --full` path) recreates it from disk.

mod error;
mod store;
mod sync;
pub mod turso;

pub use error::{IndexError, Result};
pub use store::{
    ChunkJob, DomainId, DomainStats, EdgeKind, EmbeddingRow, EngramId, EngramRecord, EngramSummary,
    FileStamp, FilterOp, FtsMode, GraphEdge, GraphNode, GraphSlice, HitKind, MetadataFilter, Page,
    RecentFilter, SearchHit, SearchMode, SearchQuery, Store, StoreInfo, parse_metadata_filters,
};
pub use sync::{SyncReport, sync_domain};
pub use turso::TursoStore;
