//! Versioned, idempotent schema migrations for the PostgreSQL backend.
//!
//! The mechanism is the same hand-rolled ledger the Turso backend uses: a
//! `schema_migration(version, applied_at)` table records applied versions and on
//! open every migration whose version is above the recorded maximum is applied.
//! The DDL itself is the Postgres dialect and is not shared with Turso
//! (`BIGINT GENERATED ALWAYS AS IDENTITY` vs `AUTOINCREMENT`, `JSONB` vs `TEXT`,
//! `vector(384)` vs `F32_BLOB(384)`), so a shared migrations directory buys
//! nothing. Postgres has no existing users, so its schema starts at its own v1
//! that bakes in everything the current [`crate::Store`] trait exercises in one
//! step rather than replaying the Turso migration history.

use sqlx::{Executor, PgConnection};

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
        label: "domain kind",
        sql: SCHEMA_V2,
    },
    Migration {
        version: 3,
        label: "domain host lock",
        sql: SCHEMA_V3,
    },
    Migration {
        version: 4,
        label: "title-lower expression index",
        sql: SCHEMA_V4,
    },
    Migration {
        version: 5,
        label: "link unresolved partial index",
        sql: SCHEMA_V5,
    },
];

// The whole current schema in one step. The temporal columns stay TEXT ISO
// strings, not `date`/`timestamptz`, so the canonical current filter and every
// lexical comparison are byte-identical to Turso and the shared parity suite
// ports directly. `metadata` is JSONB so filters use native `->>` operators.
// The chunk embedding starts at `vector(384)` with an HNSW cosine index,
// matching the local default model; `PostgresStore::ensure_embedding_width`
// resizes the column (and its index) to whatever the active provider's dims
// are, so this starting width is just the initial value, not a fixed limit.
const SCHEMA_V1: &str = r#"
CREATE EXTENSION IF NOT EXISTS vector WITH SCHEMA public;

CREATE TABLE domain (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    path TEXT NOT NULL,
    last_sync TEXT
);

CREATE TABLE engram (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    domain_id BIGINT NOT NULL REFERENCES domain(id),
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
    metadata JSONB NOT NULL DEFAULT '{}',
    mtime BIGINT NOT NULL DEFAULT 0,
    size BIGINT NOT NULL DEFAULT 0,
    sha256 TEXT NOT NULL DEFAULT '',
    UNIQUE(domain_id, permalink)
);

CREATE UNIQUE INDEX idx_engram_path ON engram(domain_id, path);
CREATE INDEX idx_engram_current ON engram(status, valid_from, valid_to);
CREATE INDEX idx_engram_type ON engram(engram_type);
CREATE INDEX idx_engram_recorded ON engram(recorded_at);
CREATE INDEX idx_engram_domain ON engram(domain_id);

CREATE TABLE observation (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    engram_id BIGINT NOT NULL REFERENCES engram(id),
    line BIGINT NOT NULL DEFAULT 0,
    category TEXT NOT NULL DEFAULT '',
    content TEXT NOT NULL DEFAULT '',
    context TEXT
);
CREATE INDEX idx_observation_engram ON observation(engram_id);

CREATE TABLE relation (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    engram_id BIGINT NOT NULL REFERENCES engram(id),
    domain_id BIGINT NOT NULL,
    line BIGINT NOT NULL DEFAULT 0,
    rel_type TEXT NOT NULL DEFAULT '',
    to_target TEXT NOT NULL DEFAULT '',
    to_domain TEXT,
    to_id BIGINT
);
CREATE INDEX idx_relation_engram ON relation(engram_id);
CREATE INDEX idx_relation_unresolved ON relation(domain_id, to_target) WHERE to_id IS NULL;
CREATE INDEX idx_relation_to ON relation(to_id);

CREATE TABLE link (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    engram_id BIGINT NOT NULL REFERENCES engram(id),
    domain_id BIGINT NOT NULL,
    line BIGINT NOT NULL DEFAULT 0,
    to_target TEXT NOT NULL DEFAULT '',
    to_domain TEXT,
    to_id BIGINT
);
CREATE INDEX idx_link_engram ON link(engram_id);
CREATE INDEX idx_link_to ON link(to_id);

CREATE TABLE tag (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    name TEXT NOT NULL UNIQUE
);

CREATE TABLE engram_tag (
    engram_id BIGINT NOT NULL REFERENCES engram(id),
    tag_id BIGINT NOT NULL REFERENCES tag(id),
    PRIMARY KEY (engram_id, tag_id)
);
CREATE INDEX idx_engram_tag_tag ON engram_tag(tag_id);

CREATE TABLE observation_tag (
    observation_id BIGINT NOT NULL REFERENCES observation(id),
    tag_id BIGINT NOT NULL REFERENCES tag(id),
    PRIMARY KEY (observation_id, tag_id)
);
CREATE INDEX idx_observation_tag_tag ON observation_tag(tag_id);

CREATE TABLE chunk (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    engram_id BIGINT NOT NULL REFERENCES engram(id),
    seq BIGINT NOT NULL DEFAULT 0,
    text TEXT NOT NULL DEFAULT '',
    text_hash TEXT NOT NULL DEFAULT '',
    model TEXT,
    dims BIGINT,
    embedding vector(384)
);
CREATE INDEX idx_chunk_engram ON chunk(engram_id);
CREATE INDEX idx_chunk_hash ON chunk(text_hash);
CREATE INDEX idx_chunk_model ON chunk(model);
CREATE INDEX idx_chunk_embedding ON chunk USING hnsw (embedding vector_cosine_ops);
"#;

// A domain gains a `kind` discriminator so a virtual domain (engrams in the
// database, no filesystem root) is told apart from a file domain. Unlike Turso,
// Postgres can drop the `path NOT NULL` cheaply, so a virtual domain stores
// `path = NULL`; `kind` is still the authoritative discriminator. Existing rows
// default to 'file'.
const SCHEMA_V2: &str = r#"
ALTER TABLE domain ADD COLUMN kind TEXT NOT NULL DEFAULT 'file';
ALTER TABLE domain ALTER COLUMN path DROP NOT NULL;
"#;

// The single-writer-per-file-domain host lock for shared-database
// collaboration, the Postgres twin of Turso's v4. One row per hosted file domain
// records the holding instance and its last heartbeat; a stale heartbeat or an
// explicit takeover lets another instance claim it. Times are TEXT ISO strings,
// compared lexically, matching every other temporal column.
const SCHEMA_V3: &str = r#"
CREATE TABLE domain_lock (
    domain_id BIGINT PRIMARY KEY REFERENCES domain(id),
    holder_instance_id TEXT NOT NULL,
    holder_label TEXT NOT NULL DEFAULT '',
    acquired_at TEXT NOT NULL,
    heartbeat_at TEXT NOT NULL
);
"#;

// The Postgres twin of Turso's v5: a case-insensitive title index for
// forward-reference resolution. Relations resolve their target with
// `lower(e.title) = lower(...)` scoped to a domain, and the find/inbound paths
// share the pattern; without an index each match is a full engram scan. Postgres
// supports functional indexes natively, so the existing queries are left
// untouched and only gain the index.
const SCHEMA_V4: &str = r#"
CREATE INDEX idx_engram_title_lower ON engram(domain_id, lower(title));
"#;

// The Postgres twin of Turso's v6. Prose wikilinks now resolve into the graph,
// so the batch resolver scans the `link` table for unresolved rows the same way
// the relation resolver scans `relation`. The partial index mirrors
// `idx_relation_unresolved` so each resolve pass seeks the pending links for a
// domain instead of scanning the whole table. Index-only, so no resync.
const SCHEMA_V5: &str = r#"
CREATE INDEX idx_link_unresolved ON link(domain_id, to_target) WHERE to_id IS NULL;
"#;

/// The tables cleared by `wipe()`, child rows first so the enforced foreign
/// keys are satisfied at every step. `domain_lock` references `domain(id)`, so
/// it is cleared before `domain`.
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
/// recorded version. Returns the resulting schema version. Runs on the given
/// connection so the caller controls whether it is pinned or pool-acquired.
pub async fn apply(conn: &mut PgConnection) -> Result<i64> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS schema_migration (version BIGINT PRIMARY KEY, applied_at TEXT NOT NULL)",
    )
    .await
    .map_err(|e| IndexError::Migration(e.to_string()))?;

    let current = current_version(conn).await?;
    for m in MIGRATIONS {
        if m.version <= current {
            continue;
        }
        // sqlx `raw_sql` runs the multi-statement DDL as one simple-query batch,
        // so `CREATE EXTENSION` and the tables that reference its `vector` type
        // apply together in one implicit transaction.
        sqlx::raw_sql(m.sql)
            .execute(&mut *conn)
            .await
            .map_err(|e| IndexError::Migration(format!("v{} ({}): {e}", m.version, m.label)))?;
        let now = chrono::Utc::now().to_rfc3339();
        sqlx::query("INSERT INTO schema_migration (version, applied_at) VALUES ($1, $2)")
            .bind(m.version)
            .bind(&now)
            .execute(&mut *conn)
            .await
            .map_err(|e| IndexError::Migration(e.to_string()))?;
    }
    current_version(conn).await
}

async fn current_version(conn: &mut PgConnection) -> Result<i64> {
    let row: (i64,) = sqlx::query_as("SELECT COALESCE(MAX(version), 0) FROM schema_migration")
        .fetch_one(&mut *conn)
        .await
        .map_err(|e| IndexError::Migration(e.to_string()))?;
    Ok(row.0)
}
