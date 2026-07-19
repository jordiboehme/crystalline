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
    Migration {
        version: 5,
        label: "title-lower expression index",
        sql: SCHEMA_V5,
    },
    Migration {
        version: 6,
        label: "link unresolved partial index",
        sql: SCHEMA_V6,
    },
    Migration {
        version: 7,
        label: "case-folded tag identity",
        sql: SCHEMA_V7,
    },
    Migration {
        version: 8,
        label: "tag alias map",
        sql: SCHEMA_V8,
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

// A case-insensitive title index for forward-reference resolution. Relations
// resolve their target with `lower(e.title) = lower(...)` scoped to a domain,
// and the find/inbound paths share the pattern; without an index each match is
// a full engram scan, once per unresolved reference on every sync. Turso 0.7.0
// accepts expression indexes and its planner seeks this one for the resolve
// subquery shape (`SEARCH e USING INDEX idx_engram_title_lower`), so the
// existing queries are left untouched and only gain the index. The `lower()`
// folded into the index is byte-identical to the one the queries already call,
// so resolution results are unchanged; the index just makes the match a seek.
const SCHEMA_V5: &str = r#"
CREATE INDEX idx_engram_title_lower ON engram(domain_id, lower(title));
"#;

// Prose wikilinks now resolve into the graph, so the batch resolver scans the
// `link` table for unresolved rows the same way the relation resolver scans
// `relation`. The partial index mirrors `idx_relation_unresolved` (the v1
// precedent) so each resolve pass seeks the pending links for a domain instead
// of scanning the whole table. Index-only, so no resync is required.
const SCHEMA_V6: &str = r#"
CREATE INDEX idx_link_unresolved ON link(domain_id, to_target) WHERE to_id IS NULL;
"#;

// Tag identity is now case-folded at intern time, so `Foo` and `foo` share one
// tag row. A database written before the fold can still hold case-duplicate
// rows; this migration merges them. Repoint every join row onto the lowest id
// per folded name, drop the join rows that still point at a duplicate, drop the
// duplicate tag rows and lowercase the survivors. The `INSERT OR IGNORE` step
// materializes the min-id form of each join row and silently absorbs the
// primary-key collision when one engram already carried both cases of a tag.
// SQLite `lower()` is ASCII-only; that is accepted because verify E007
// restricts a canonical tag to lowercase ASCII with hyphens, so a non-ASCII tag
// is already off-spec and folds to itself here. Every join row ends on a
// surviving id, so no foreign key is left dangling.
const SCHEMA_V7: &str = r#"
INSERT OR IGNORE INTO engram_tag(engram_id, tag_id)
SELECT et.engram_id, m.min_id
FROM engram_tag et
JOIN tag t ON t.id = et.tag_id
JOIN (SELECT lower(name) AS lname, MIN(id) AS min_id FROM tag GROUP BY lower(name)) m
  ON m.lname = lower(t.name);

INSERT OR IGNORE INTO observation_tag(observation_id, tag_id)
SELECT ot.observation_id, m.min_id
FROM observation_tag ot
JOIN tag t ON t.id = ot.tag_id
JOIN (SELECT lower(name) AS lname, MIN(id) AS min_id FROM tag GROUP BY lower(name)) m
  ON m.lname = lower(t.name);

DELETE FROM engram_tag WHERE tag_id NOT IN (SELECT MIN(id) FROM tag GROUP BY lower(name));
DELETE FROM observation_tag WHERE tag_id NOT IN (SELECT MIN(id) FROM tag GROUP BY lower(name));

DELETE FROM tag WHERE id NOT IN (SELECT MIN(id) FROM tag GROUP BY lower(name));

UPDATE tag SET name = lower(name);
"#;

// The derived tag-alias map. One row per `(domain, alias)` records the canonical
// spelling an old tag folds onto at query time, so a search on either spelling
// matches every engram tagged with a sibling. Derived purely from MANIFEST
// content and repopulated on the next sync per domain: an upgraded database
// carries no aliases until each domain resyncs (a new-feature grace), and a
// wipe+resync is the accepted way to backfill. The canonical index serves the
// reverse lookup during expansion.
const SCHEMA_V8: &str = r#"
CREATE TABLE tag_alias (
    domain_id INTEGER NOT NULL REFERENCES domain(id),
    alias TEXT NOT NULL,
    canonical TEXT NOT NULL,
    PRIMARY KEY (domain_id, alias)
);
CREATE INDEX idx_tag_alias_canonical ON tag_alias(domain_id, canonical);
"#;

/// The tables cleared by `wipe()`, child rows first. `tag_alias` and
/// `domain_lock` both reference `domain(id)`, so they are cleared before
/// `domain`.
pub const WIPE_TABLES: &[&str] = &[
    "observation_tag",
    "engram_tag",
    "chunk",
    "observation",
    "relation",
    "link",
    "tag",
    "engram",
    "tag_alias",
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

#[cfg(test)]
mod tests {
    use super::*;
    use turso::Builder;

    async fn scalar(conn: &Connection, sql: &str) -> i64 {
        let mut rows = conn.query(sql, ()).await.unwrap();
        let row = rows.next().await.unwrap().unwrap();
        row.get_value(0)
            .ok()
            .and_then(|v| v.as_integer().copied())
            .unwrap_or(0)
    }

    async fn names(conn: &Connection) -> Vec<String> {
        let mut rows = conn
            .query("SELECT name FROM tag ORDER BY id", ())
            .await
            .unwrap();
        let mut out = Vec::new();
        while let Some(r) = rows.next().await.unwrap() {
            if let Ok(turso::Value::Text(s)) = r.get_value(0) {
                out.push(s);
            }
        }
        out
    }

    /// Proves the v7 case-fold migration against a real turso connection: apply
    /// migrations through v6, plant case-duplicate tag rows (including a
    /// join-row primary-key collision an engram that carries both cases would
    /// produce), run the v7 SQL and assert the duplicates merged onto the
    /// lowest id per folded name with every surviving name lowercased and no
    /// dangling join row. Runs v7 a second time to prove idempotence.
    #[tokio::test]
    async fn v7_folds_and_merges_case_duplicate_tags() {
        let db = Builder::new_local(":memory:").build().await.unwrap();
        let conn = db.connect().unwrap();

        // Schema through v6 (the six migrations before the fold).
        for m in &MIGRATIONS[..6] {
            conn.execute_batch(m.sql).await.unwrap();
        }
        assert_eq!(MIGRATIONS[6].version, 7, "the seventh migration is v7");

        // Case-duplicate tag rows: `Foo`/`foo` fold to id 1, `Bar`/`bar` to id 3.
        // Join rows include a primary-key collision pair on each folded tag
        // (engram 5 carries both cases of foo; engram 7 both cases of bar; the
        // same for observation 9) plus single-case rows that must be repointed.
        conn.execute_batch(
            r#"
            INSERT INTO tag(id, name) VALUES (1,'Foo'),(2,'foo'),(3,'Bar'),(4,'bar');
            INSERT INTO engram_tag(engram_id, tag_id) VALUES (5,1),(5,2),(6,2),(7,3),(7,4),(8,4);
            INSERT INTO observation_tag(observation_id, tag_id) VALUES (9,1),(9,2),(10,4);
            "#,
        )
        .await
        .unwrap();

        conn.execute_batch(SCHEMA_V7).await.unwrap();

        // One tag row per folded name, both lowercase, min ids kept.
        assert_eq!(
            names(&conn).await,
            vec!["foo".to_string(), "bar".to_string()]
        );
        assert_eq!(scalar(&conn, "SELECT COUNT(*) FROM tag").await, 2);
        assert_eq!(
            scalar(&conn, "SELECT COUNT(*) FROM tag WHERE name <> lower(name)").await,
            0,
            "every surviving tag name is lowercase"
        );

        // Join rows are repointed onto the min ids with the collision absorbed:
        // engram 5 keeps a single (5,1), engram 6 becomes (6,1), engram 7 keeps
        // (7,3), engram 8 becomes (8,3).
        assert_eq!(scalar(&conn, "SELECT COUNT(*) FROM engram_tag").await, 4);
        assert_eq!(
            scalar(
                &conn,
                "SELECT COUNT(*) FROM engram_tag WHERE tag_id NOT IN (1,3)"
            )
            .await,
            0,
            "no join row points at a merged-away duplicate id"
        );
        assert_eq!(
            scalar(
                &conn,
                "SELECT COUNT(*) FROM engram_tag WHERE engram_id=5 AND tag_id=1"
            )
            .await,
            1,
            "the collision pair collapsed onto the single min-id row"
        );
        assert_eq!(
            scalar(
                &conn,
                "SELECT COUNT(*) FROM engram_tag WHERE engram_id=6 AND tag_id=1"
            )
            .await,
            1
        );
        assert_eq!(
            scalar(
                &conn,
                "SELECT COUNT(*) FROM engram_tag WHERE tag_id NOT IN (SELECT id FROM tag)"
            )
            .await,
            0,
            "no engram_tag row dangles"
        );

        // Observation join rows fold the same way: (9,1)/(9,2) collapse to
        // (9,1) and (10,4) repoints to (10,3).
        assert_eq!(
            scalar(&conn, "SELECT COUNT(*) FROM observation_tag").await,
            2
        );
        assert_eq!(
            scalar(
                &conn,
                "SELECT COUNT(*) FROM observation_tag WHERE tag_id NOT IN (SELECT id FROM tag)"
            )
            .await,
            0,
            "no observation_tag row dangles"
        );

        // Idempotence: re-running the migration on the merged database changes
        // nothing.
        conn.execute_batch(SCHEMA_V7).await.unwrap();
        assert_eq!(scalar(&conn, "SELECT COUNT(*) FROM tag").await, 2);
        assert_eq!(scalar(&conn, "SELECT COUNT(*) FROM engram_tag").await, 4);
        assert_eq!(
            scalar(&conn, "SELECT COUNT(*) FROM observation_tag").await,
            2
        );
        assert_eq!(
            names(&conn).await,
            vec!["foo".to_string(), "bar".to_string()]
        );
    }
}
