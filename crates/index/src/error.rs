//! Error type for the storage and index layer.

impl IndexError {
    /// Whether this is another process holding the database file, rather than
    /// a damaged file or any other failure.
    ///
    /// turso raises the lock failure through its catch-all string variant
    /// rather than a typed one, so the `Locking error` text its own message
    /// carries is the only handle there is. Read conservatively: a message this
    /// does not recognize answers `false`, which every caller treats as the
    /// ordinary failure it already handled.
    ///
    /// Public because two layers act on it differently. The store's recovery
    /// path must never set a held file aside as if it were damaged, and the
    /// daemon's startup waits a moment for a predecessor that has given up
    /// ownership but not yet let go of the file.
    pub fn is_locked_by_another_process(&self) -> bool {
        self.to_string().contains("Locking error")
    }
}

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
    #[error("io error at {path}: {source}{}", crystalline_core::config::io_hint_suffix(.path, .source))]
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
    /// A multi-domain run failed while working on one named domain.
    ///
    /// The name is otherwise lost: a driver rebuilding five domains that
    /// reports only "constraint violation" does not say which domain to look
    /// at. The underlying error is kept as the source, so a caller matching on
    /// a specific failure still can.
    #[error("{operation} of '{domain}' failed: {source}")]
    InDomain {
        /// The verb the caller was running, for example `reindex`.
        operation: String,
        /// The domain the failure happened in.
        domain: String,
        /// The failure itself.
        #[source]
        source: Box<IndexError>,
    },
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
    /// The database's recorded schema version is above the newest migration
    /// this binary knows: a later Crystalline already raised the schema past
    /// what this copy ships. Applying nothing and refusing is the only safe
    /// move - a migration list this binary does not have cannot be replayed
    /// backwards, and running the known migrations against a newer schema
    /// would either no-op past the gap or, worse, misapply a step whose
    /// preconditions the newer schema no longer meets.
    ///
    /// This is exactly the 2026-09-23 incident: an older bundled binary
    /// (Claude Desktop's extension, still on a previous release) opened an
    /// index a newer install had already migrated, found no guard, and ran
    /// anyway - failing partway through sync and re-downloading a model the
    /// newer binary had already pruned. The message names both versions and
    /// the two remedies (the two ways an out-of-date binary reaches this
    /// index) so a person fixes the actual cause rather than the symptom.
    ///
    /// Deliberately not `is_locked_by_another_process`: this is a version
    /// mismatch, not a held file, so `open_store_as_owner`'s retry must never
    /// treat it as a predecessor still letting go and wait it out.
    #[error(
        "this index was upgraded by a newer Crystalline (schema v{found}, this binary knows v{known}). \
         This copy is out of date. If Claude Desktop runs Crystalline as an extension, install the \
         current .mcpb over it; otherwise upgrade this binary."
    )]
    SchemaTooNew {
        /// The schema version recorded in the database.
        found: i64,
        /// The newest migration version this binary ships.
        known: i64,
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

// The whole module is macos-gated, not just the test: on other platforms a
// gated-out lone test would leave `use super::*` dangling and fail clippy.
#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn io_display_carries_the_privacy_hint_for_eperm_under_documents() {
        let path = crystalline_core::config::expand_tilde("~/Documents/x")
            .to_string_lossy()
            .to_string();
        let e = IndexError::Io {
            path,
            source: std::io::Error::from_raw_os_error(1),
        };
        assert!(e.to_string().contains("Files and Folders"), "{e}");
    }
}
