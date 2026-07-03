//! Error type for the storage and index layer.

/// The result type used across `crystalline-index`.
pub type Result<T> = std::result::Result<T, IndexError>;

/// An error from the store, the sync engine or the search planner.
#[derive(Debug, thiserror::Error)]
pub enum IndexError {
    /// A database error surfaced by the backend.
    #[error("database error: {0}")]
    Db(String),
    /// A schema migration failed.
    #[error("migration error: {0}")]
    Migration(String),
    /// A constraint was violated, for example a duplicate permalink.
    #[error("constraint violation: {0}")]
    Constraint(String),
    /// A compare-and-swap write found the stored engram already changed since
    /// the caller last read it. Raised only by [`crate::Store::upsert_engram_checked`]
    /// when an `expected_sha` is supplied and differs from the stored one; the
    /// engine surfaces it as a conflict so a stale virtual edit is refused rather
    /// than silently clobbering a concurrent change.
    #[error("stale edit: engram changed since it was read (expected {expected}, found {found})")]
    StaleEdit {
        /// The sha256 the caller expected the stored row to still have.
        expected: String,
        /// The sha256 actually stored now.
        found: String,
    },
    /// A referenced entity was not found.
    #[error("not found: {0}")]
    NotFound(String),
    /// A JSON (de)serialization error.
    #[error("json error: {0}")]
    Json(String),
    /// A filesystem error during sync.
    #[error("io error at {path}: {source}")]
    Io {
        /// The path involved.
        path: String,
        /// The underlying error.
        source: std::io::Error,
    },
    /// A feature that is planned for a later milestone was requested.
    #[error("unsupported: {0}")]
    Unsupported(String),
    /// An input was malformed.
    #[error("invalid input: {0}")]
    Invalid(String),
    /// The embedding model or its inference failed.
    #[error("embedding error: {0}")]
    Embedding(String),
    /// A remote embedding endpoint returned an error or was unreachable.
    #[error("remote embedding error: {0}")]
    Remote(String),
    /// Semantic search was asked to compare against embeddings from a different
    /// model or dimensionality than the active provider. The index is being
    /// re-embedded; callers surface this as "reindex in progress" rather than
    /// returning results from the wrong vector space. Text search is unaffected.
    #[error(
        "stale embeddings: {embedded} of {total} chunks embedded with '{stored_model}', active model is '{active_model}' (reindex in progress)"
    )]
    StaleEmbeddings {
        /// The model that produced the stored embeddings.
        stored_model: String,
        /// The active provider's model.
        active_model: String,
        /// Chunks already embedded for the active model.
        embedded: usize,
        /// Total chunks in the index.
        total: usize,
    },
}

impl From<turso::Error> for IndexError {
    fn from(e: turso::Error) -> Self {
        match e {
            turso::Error::Constraint(m) => IndexError::Constraint(m),
            turso::Error::Corrupt(m) => IndexError::Db(format!("corrupt: {m}")),
            turso::Error::NotAdb(m) => IndexError::Db(format!("not a database: {m}")),
            other => IndexError::Db(other.to_string()),
        }
    }
}

impl From<serde_json::Error> for IndexError {
    fn from(e: serde_json::Error) -> Self {
        IndexError::Json(e.to_string())
    }
}

#[cfg(feature = "postgres")]
impl From<sqlx::Error> for IndexError {
    fn from(e: sqlx::Error) -> Self {
        // A unique-violation (SQLSTATE 23505) is a constraint the sync engine
        // collects into `SyncReport.failed` rather than aborting the batch, so
        // it maps to `Constraint` the same way Turso's constraint error does.
        // Everything else is a plain database error.
        if let sqlx::Error::Database(db) = &e
            && db.code().as_deref() == Some("23505")
        {
            return IndexError::Constraint(db.message().to_string());
        }
        IndexError::Db(e.to_string())
    }
}

impl From<reqwest::Error> for IndexError {
    fn from(e: reqwest::Error) -> Self {
        IndexError::Remote(e.to_string())
    }
}
