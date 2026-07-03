//! Versioned, idempotent schema migrations.
//!
//! A `schema_migration` table records applied versions; on open we apply every
//! migration whose version is above the recorded maximum. Using a table rather
//! than `PRAGMA user_version` keeps the mechanism portable to the PostgreSQL
//! backend. The FTS5 virtual table is attempted as a probe inside its own
//! migration step and its failure is tolerated (see [`crate::turso`]).

use turso::Connection;

use crate::error::{IndexError, Result};

/// One migration: a version number and the DDL that raises the schema to it.
pub struct Migration {
    /// The monotonically increasing version.
    pub version: i64,
    /// A human label for diagnostics.
    pub label: &'static str,
    /// The DDL, one or more `;`-separated statements.
    pub sql: &'static str,
}

/// The ordered list of migrations. Append-only: never edit a shipped migration.
pub const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        label: "initial schema",
        sql: SCHEMA_V1,
    },
    Migration {
        version: 2,
        label: "vector chunk storage",
        sql: SCHEMA_V2,
    },
    Migration {
        version: 3,
        label: "domain kind",
        sql: SCHEMA_V3,
    },
    Migration {
        version: 4,
        label: "domain host lock",
        sql: SCHEMA_V4,
    },
];

const SCHEMA_V1: &str = r#"
CREATE TABLE domain (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL UNIQUE,
    path TEXT NOT NULL,
    last_sync TEXT
);

CREATE TABLE engram (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    domain_id INTEGER NOT NULL REFERENCES domain(id),
    path TEXT NOT NULL,
    permalink TEXT NOT NULL,
    title TEXT NOT NULL DEFAULT '',
    engram_type TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT '',
    recorded_at TEXT,
    valid_from TEXT,
    valid_to TEXT,
    timestamp TEXT,
    description TEXT,
    content TEXT NOT NULL DEFAULT '',
    metadata TEXT NOT NULL DEFAULT '{}',
    mtime INTEGER NOT NULL DEFAULT 0,
    size INTEGER NOT NULL DEFAULT 0,
    sha256 TEXT NOT NULL DEFAULT '',
    UNIQUE(domain_id, permalink)
);

CREATE UNIQUE INDEX idx_engram_path ON engram(domain_id, path);
CREATE INDEX idx_engram_current ON engram(status, valid_from, valid_to);
CREATE INDEX idx_engram_type ON engram(engram_type);
CREATE INDEX idx_engram_recorded ON engram(recorded_at);
CREATE INDEX idx_engram_domain ON engram(domain_id);

CREATE TABLE observation (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    engram_id INTEGER NOT NULL REFERENCES engram(id),
    line INTEGER NOT NULL DEFAULT 0,
    category TEXT NOT NULL DEFAULT '',
    content TEXT NOT NULL DEFAULT '',
    context TEXT
);
CREATE INDEX idx_observation_engram ON observation(engram_id);

CREATE TABLE relation (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    engram_id INTEGER NOT NULL REFERENCES engram(id),
    domain_id INTEGER NOT NULL,
    line INTEGER NOT NULL DEFAULT 0,
    rel_type TEXT NOT NULL DEFAULT '',
    to_target TEXT NOT NULL DEFAULT '',
    to_domain TEXT,
    to_id INTEGER
);
CREATE INDEX idx_relation_engram ON relation(engram_id);
CREATE INDEX idx_relation_unresolved ON relation(domain_id, to_target) WHERE to_id IS NULL;
CREATE INDEX idx_relation_to ON relation(to_id);

CREATE TABLE link (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    engram_id INTEGER NOT NULL REFERENCES engram(id),
    domain_id INTEGER NOT NULL,
    line INTEGER NOT NULL DEFAULT 0,
    to_target TEXT NOT NULL DEFAULT '',
    to_domain TEXT,
    to_id INTEGER
);
CREATE INDEX idx_link_engram ON link(engram_id);
CREATE INDEX idx_link_to ON link(to_id);

CREATE TABLE tag (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL UNIQUE
);

CREATE TABLE engram_tag (
    engram_id INTEGER NOT NULL REFERENCES engram(id),
    tag_id INTEGER NOT NULL REFERENCES tag(id),
    PRIMARY KEY (engram_id, tag_id)
);
CREATE INDEX idx_engram_tag_tag ON engram_tag(tag_id);

CREATE TABLE observation_tag (
    observation_id INTEGER NOT NULL REFERENCES observation(id),
    tag_id INTEGER NOT NULL REFERENCES tag(id),
    PRIMARY KEY (observation_id, tag_id)
);
CREATE INDEX idx_observation_tag_tag ON observation_tag(tag_id);

CREATE TABLE chunk (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    engram_id INTEGER NOT NULL REFERENCES engram(id),
    seq INTEGER NOT NULL DEFAULT 0,
    text TEXT NOT NULL DEFAULT '',
    text_hash TEXT NOT NULL DEFAULT '',
    model TEXT,
    dims INTEGER,
    embedding BLOB
);
CREATE INDEX idx_chunk_engram ON chunk(engram_id);
CREATE INDEX idx_chunk_hash ON chunk(text_hash);
"#;

// M4 gives the chunk table a native vector embedding column. The v1 table used
// a placeholder `BLOB`; v2 recreates it with `F32_BLOB(384)` so `vector_distance_cos`
// runs over it. The 384 matches the local bge default; turso 0.6.1 does not
// enforce the declared width, so other providers (whose dims are recorded in the
// `dims` column and validated in Rust) store their vectors here too. The chunk
// table is a derived, rebuildable cache, so recreating it loses nothing that a
// resync plus embed pass does not restore.
const SCHEMA_V2: &str = r#"
DROP TABLE IF EXISTS chunk;

CREATE TABLE chunk (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    engram_id INTEGER NOT NULL REFERENCES engram(id),
    seq INTEGER NOT NULL DEFAULT 0,
    text TEXT NOT NULL DEFAULT '',
    text_hash TEXT NOT NULL DEFAULT '',
    model TEXT,
    dims INTEGER,
    embedding F32_BLOB(384)
);
CREATE INDEX idx_chunk_engram ON chunk(engram_id);
CREATE INDEX idx_chunk_hash ON chunk(text_hash);
CREATE INDEX idx_chunk_model ON chunk(model);
"#;

// A domain gains a `kind` discriminator so a virtual domain (engrams live in the
// database, no filesystem root) is told apart from a file domain. SQLite cannot
// cheaply drop the existing `path NOT NULL`, so a virtual domain stores `path=''`
// and the `kind` column is the authoritative discriminator. Existing rows default
// to 'file', so a resync is not required.
const SCHEMA_V3: &str = r#"
ALTER TABLE domain ADD COLUMN kind TEXT NOT NULL DEFAULT 'file';
"#;

// The single-writer-per-file-domain host lock for shared-database
// collaboration. One row per hosted file domain records the holding instance
// and its last heartbeat; a stale heartbeat or an explicit takeover lets another
// instance claim it. Virtual domains never take a row here (their concurrency is
// engram-level compare-and-swap). Times are TEXT ISO strings, compared
// lexically, matching every other temporal column.
const SCHEMA_V4: &str = r#"
CREATE TABLE domain_lock (
    domain_id INTEGER PRIMARY KEY REFERENCES domain(id),
    holder_instance_id TEXT NOT NULL,
    holder_label TEXT NOT NULL DEFAULT '',
    acquired_at TEXT NOT NULL,
    heartbeat_at TEXT NOT NULL
);
"#;

/// The tables cleared by `wipe()`, child rows first. `domain_lock` references
/// `domain(id)`, so it is cleared before `domain`.
pub const WIPE_TABLES: &[&str] = &[
    "observation_tag",
    "engram_tag",
    "chunk",
    "observation",
    "relation",
    "link",
    "tag",
    "engram",
    "domain_lock",
    "domain",
];

/// Ensure the migration ledger exists, then apply every migration above the
/// recorded version. Returns the resulting schema version.
pub async fn apply(conn: &Connection) -> Result<i64> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS schema_migration (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL)",
        (),
    )
    .await
    .map_err(|e| IndexError::Migration(e.to_string()))?;

    let current = current_version(conn).await?;
    for m in MIGRATIONS {
        if m.version <= current {
            continue;
        }
        conn.execute_batch(m.sql)
            .await
            .map_err(|e| IndexError::Migration(format!("v{} ({}): {e}", m.version, m.label)))?;
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO schema_migration (version, applied_at) VALUES (?1, ?2)",
            vec![turso::Value::Integer(m.version), turso::Value::Text(now)],
        )
        .await
        .map_err(|e| IndexError::Migration(e.to_string()))?;
    }
    current_version(conn).await
}

async fn current_version(conn: &Connection) -> Result<i64> {
    let mut rows = conn
        .query("SELECT COALESCE(MAX(version), 0) FROM schema_migration", ())
        .await
        .map_err(|e| IndexError::Migration(e.to_string()))?;
    let row = rows
        .next()
        .await
        .map_err(|e| IndexError::Migration(e.to_string()))?;
    match row {
        Some(r) => Ok(r
            .get_value(0)
            .ok()
            .and_then(|v| v.as_integer().copied())
            .unwrap_or(0)),
        None => Ok(0),
    }
}
