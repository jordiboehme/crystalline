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
