//! `crystalline-index` is the derived, disposable index built from Engram files
//! on disk. It holds the backend-agnostic [`Store`] trait, the Turso-backed
//! implementation, the sync engine that keeps the index in step with the files,
//! the search planner and the embedding pipeline (chunking, the provider trait,
//! local candle and remote OpenAI-compatible providers, and semantic and hybrid
//! search).
//!
//! The local candle provider lives behind the `local-embeddings` feature (on by
//! default). A `--no-default-features` build drops candle entirely and keeps the
//! chunker, the remote provider and text search; asking for a local provider on
//! such a build is an [`IndexError::Unsupported`].
//!
//! Files are the source of truth and the index is fully rebuildable, so a
//! corrupt or stale index is never a data-loss risk: [`Store::wipe`] followed by
//! a resync (the `reindex --full` path) recreates it from disk.

pub mod embed;
mod error;
mod factory;
#[cfg(feature = "postgres")]
pub mod postgres;
mod store;
mod sync;
pub mod turso;

pub use embed::{
    ChunkParams, EmbedReport, EmbeddingProvider, ModelDownload, chunk_engram, configured_model_id,
    download_local_model, provider_from_config, run_embedding_pass,
};
pub use error::{IndexError, Result};
pub use factory::open_store;
pub use store::{
    ChunkJob, ChunkModelCount, DomainHost, DomainId, DomainKind, DomainStats, EdgeKind,
    EmbeddingCoverage, EmbeddingRow, EngramDescriptor, EngramId, EngramRecord, EngramSummary,
    FileStamp, FilterOp, FtsMode, GraphEdge, GraphNode, GraphSlice, HitKind, HostClaim, InboundRef,
    MetadataFilter, NewChunk, Page, RecentFilter, SearchHit, SearchMode, SearchQuery, Store,
    StoreInfo, StoredEngram, parse_metadata_filters,
};
pub use sync::{SyncReport, sync_domain, sync_domain_with};
pub use turso::TursoStore;

#[cfg(feature = "postgres")]
pub use postgres::PostgresStore;
