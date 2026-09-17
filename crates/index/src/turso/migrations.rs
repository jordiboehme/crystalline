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
    Migration {
        version: 9,
        label: "engram attachments",
        sql: SCHEMA_V9,
    },
    Migration {
        version: 10,
        label: "raw reference text",
        sql: SCHEMA_V10,
    },
    Migration {
        version: 11,
        label: "domain registration stamp",
        sql: SCHEMA_V11,
    },
    Migration {
        version: 12,
        label: "domain rebuild marker",
        sql: SCHEMA_V12,
    },
    Migration {
        version: 13,
        label: "engram actor dimension",
        sql: SCHEMA_V13,
    },
    Migration {
        version: 14,
        label: "domain rebuild kind",
        sql: SCHEMA_V14,
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

// Binary attachments: one metadata row per asset under a domain's `assets/`
// folder, keyed by domain-relative path. A file domain keeps the bytes on disk
// and holds metadata only; a virtual domain has no filesystem, so its bytes live
// in `attachment_blob`, split off into its own table so the metadata listing
// never drags a blob through the row cache. `size` is the byte length and
// `modified` an RFC 3339 instant, matching the temporal columns' text form.
// The bracket text a reference was written with, kept beside the split of it.
//
// `LinkTarget::parse` is domain-agnostic: it splits `[[Log: Weekly Garden
// Notes]]` into a domain and a target exactly as it splits `[[ops:Runbook]]`,
// because nothing inside the brackets says which is which. Only the registry
// can tell them apart, and telling them apart means looking the whole original
// string up as a title - which `to_target` and `to_domain` have by then lost
// the whitespace of. So it is stored.
//
// Nullable, and deliberately not backfilled: a row written before this
// migration has no bracket text to recover, and there is no expression over
// `to_domain || to_target` that reconstructs it (the colon was trimmed around).
// Such a row resolves exactly as it does today - the fallback compares against
// NULL, which is never true - until the next reindex of its engram rewrites it.
const SCHEMA_V10: &str = r#"
ALTER TABLE relation ADD COLUMN to_raw TEXT;
ALTER TABLE link ADD COLUMN to_raw TEXT;
"#;

// When this domain was last seen in the configuration, RFC 3339, the same text
// convention `last_sync` uses so the two compare lexically.
//
// Nullable with no default and no backfill, and that is the point: every row
// that predates this migration reads NULL, and NULL means "never stamped", not
// "stamped infinitely long ago". A caller that ages the stamp to decide whether
// a domain has been gone long enough to collect must read NULL as no evidence
// at all and leave the row alone, so the first sweep after an upgrade collects
// nothing. Rows earn a stamp only by being seen registered.
const SCHEMA_V11: &str = r#"
ALTER TABLE domain ADD COLUMN last_registered TEXT;
"#;

// When a forced rebuild of this domain was stamped as started, RFC 3339, the
// same text convention `last_sync` and `last_registered` use.
//
// Nullable with no default and no backfill: NULL means no rebuild is in flight,
// and every row that predates this migration reads NULL, which is the truth for
// all of them - a rebuild that ran before the column existed cannot have been
// interrupted into it. The stamp is written before the rebuild reads a file and
// cleared inside the transaction that commits it, so it is set exactly while a
// domain's rebuild is unfinished. Nothing clears it when the process dies, and
// that is the point: a reader that finds it set after the fact knows the run
// never finished, and that the domain's rows are the complete ones from before
// it rather than a half-built set, because a rebuild clears nothing.
const SCHEMA_V12: &str = r#"
ALTER TABLE domain ADD COLUMN rebuild_started TEXT;
"#;

// Which verb stamped the rebuild marker beside it, `full` or `wipe`.
//
// Nullable with no default and no backfill, like the marker itself. NULL means
// no rebuild is in flight - and, for a row a binary older than this column
// stamped, that the kind is simply unknown. A reader treats the two alike: it
// says a rebuild did not finish and names the command that finishes it, and
// claims nothing about what the rows hold. It has to, because the two verbs
// leave opposite states behind: an interrupted `--full` left the complete rows
// from before it, an interrupted `--wipe` destroyed every row and every
// embedding before it began and left only what its rebuild managed. Telling a
// person the wrong one of those is the misreading the marker exists to prevent.
const SCHEMA_V14: &str = r#"
ALTER TABLE domain ADD COLUMN rebuild_kind TEXT;
"#;

// The actor dimension. Every engram row gains the actor it belongs to and a
// tombstone flag: `actor = ''` is the base row - the one the domain's files on
// disk say exists - and any other value is one actor's private draft of that
// path, a full row in its own right so chunks, embeddings and graph rows key to
// its id exactly as a base row's do. `tombstone` is that actor's draft deletion
// of a base row: a row that says "not for me" without touching what is on disk.
//
// Both defaults are the base reading, so every row an upgrade finds comes out
// of this migration as the base row it already was and nothing needs a resync.
//
// The two old uniqueness rules have to widen with the table, since one path and
// one permalink may now carry one row per actor: `UNIQUE(domain_id, permalink)`
// and `idx_engram_path` give way to `idx_engram_permalink_actor` and
// `idx_engram_path_actor`. A table-level UNIQUE cannot be dropped in place in
// this dialect, so the table is rebuilt by create-copy-swap: the copy carries
// ids across verbatim, which is what keeps `observation`, `relation`, `link`,
// `engram_tag` and `chunk` pointing at the rows they already point at. Their
// own `REFERENCES engram(id)` clauses survive the swap by name (the drop
// happens with foreign-key enforcement off, the default here, and the rename
// puts the name back), and every index the old table carried is recreated
// because they all die with the dropped table - the four from v1 and the
// expression index v5 added, which is the one a reader is most likely to
// forget, since nothing about the swap mentions it.
const SCHEMA_V13: &str = r#"
CREATE TABLE engram_new (
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
    actor TEXT NOT NULL DEFAULT '',
    tombstone INTEGER NOT NULL DEFAULT 0
);

INSERT INTO engram_new (id, domain_id, path, permalink, title, engram_type, status,
    recorded_at, valid_from, valid_to, timestamp, description, content, metadata,
    mtime, size, sha256, actor, tombstone)
SELECT id, domain_id, path, permalink, title, engram_type, status,
    recorded_at, valid_from, valid_to, timestamp, description, content, metadata,
    mtime, size, sha256, '', 0
FROM engram;

DROP TABLE engram;
ALTER TABLE engram_new RENAME TO engram;

CREATE UNIQUE INDEX idx_engram_permalink_actor ON engram(domain_id, permalink, actor);
CREATE UNIQUE INDEX idx_engram_path_actor ON engram(domain_id, path, actor);
CREATE INDEX idx_engram_current ON engram(status, valid_from, valid_to);
CREATE INDEX idx_engram_type ON engram(engram_type);
CREATE INDEX idx_engram_recorded ON engram(recorded_at);
CREATE INDEX idx_engram_domain ON engram(domain_id);
CREATE INDEX idx_engram_title_lower ON engram(domain_id, lower(title));
"#;

const SCHEMA_V9: &str = r#"
CREATE TABLE attachment (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    domain_id INTEGER NOT NULL REFERENCES domain(id) ON DELETE CASCADE,
    path TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    mime TEXT NOT NULL,
    size INTEGER NOT NULL,
    modified TEXT NOT NULL,
    UNIQUE(domain_id, path)
);
CREATE INDEX idx_attachment_domain ON attachment(domain_id);

CREATE TABLE attachment_blob (
    attachment_id INTEGER PRIMARY KEY REFERENCES attachment(id) ON DELETE CASCADE,
    content BLOB NOT NULL
);
"#;

/// The tables cleared by `wipe()`, child rows first. `tag_alias`, `attachment`
/// and `domain_lock` all reference `domain(id)`, so they are cleared before
/// `domain`; `attachment_blob` references `attachment`, so it goes first of the
/// three.
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
    "attachment_blob",
    "attachment",
    "domain_lock",
    "domain",
];

/// Ensure the migration ledger exists, then apply every migration above the
/// recorded version. Returns the resulting schema version.
pub async fn apply(conn: &Connection) -> Result<i64> {
    apply_migrations(conn, MIGRATIONS).await
}

/// [`apply`] over a given list, so a test can hand it a migration that fails.
async fn apply_migrations(conn: &Connection, migrations: &[Migration]) -> Result<i64> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS schema_migration (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL)",
        (),
    )
    .await
    .map_err(|e| IndexError::Migration(e.to_string()))?;

    let current = current_version(conn).await?;
    for m in migrations {
        if m.version <= current {
            continue;
        }
        // The DDL and the row that stamps it are one transaction, rolled back
        // together if anything in either fails.
        //
        // Not belt and braces: `execute_batch` prepares and runs the statements
        // one at a time with no transaction of its own, so an unwrapped v13 -
        // which drops `engram` and renames another table into its place - can
        // die between those two statements and leave a database with no
        // `engram` table at all, no version row, and a retry that fails on
        // `INSERT INTO engram_new ... FROM engram` forever. A virtual domain's
        // index is its source of truth, so that loss has nothing to resync
        // from. Turso honours DDL inside an explicit transaction and rolls a
        // dropped table back whole, which is what makes this the fix.
        //
        // Keeping the stamp inside the same transaction closes the other half:
        // a migration that applied but was not stamped would be replayed on the
        // next start, and v13 replayed over an already-swapped table would
        // flatten every draft back to a base row.
        conn.execute("BEGIN", ())
            .await
            .map_err(|e| IndexError::Migration(e.to_string()))?;
        if let Err(e) = conn.execute_batch(m.sql).await {
            let _ = conn.execute("ROLLBACK", ()).await;
            return Err(IndexError::Migration(format!(
                "v{} ({}): {e}",
                m.version, m.label
            )));
        }
        let now = chrono::Utc::now().to_rfc3339();
        if let Err(e) = conn
            .execute(
                "INSERT INTO schema_migration (version, applied_at) VALUES (?1, ?2)",
                vec![turso::Value::Integer(m.version), turso::Value::Text(now)],
            )
            .await
        {
            let _ = conn.execute("ROLLBACK", ()).await;
            return Err(IndexError::Migration(e.to_string()));
        }
        conn.execute("COMMIT", ())
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

    /// The v12 column against a domain row that predates it.
    ///
    /// A row written before the marker existed comes out of the migration with
    /// `rebuild_started` NULL, which is the truth for every one of them: a
    /// rebuild that ran before the column existed cannot have been interrupted
    /// into it. Nothing backfills it, so the first `status` after an upgrade
    /// reports no rebuild in flight anywhere. The stamped row beside it is the
    /// control that proves the column accepts a value and clears back to NULL.
    #[tokio::test]
    async fn v12_leaves_a_domain_row_written_before_it_unmarked() {
        let db = Builder::new_local(":memory:").build().await.unwrap();
        let conn = db.connect().unwrap();
        for m in &MIGRATIONS[..11] {
            conn.execute_batch(m.sql).await.unwrap();
        }
        assert_eq!(MIGRATIONS[11].version, 12, "the twelfth migration is v12");

        conn.execute_batch(
            "INSERT INTO domain(id, name, path) VALUES (1,'old','/tmp/old'),(2,'busy','/tmp/busy');",
        )
        .await
        .unwrap();

        conn.execute_batch(MIGRATIONS[11].sql).await.unwrap();

        assert_eq!(
            scalar(
                &conn,
                "SELECT COUNT(*) FROM domain WHERE rebuild_started IS NULL"
            )
            .await,
            2,
            "no backfill: both pre-existing rows read NULL, meaning no rebuild in flight"
        );

        conn.execute(
            "UPDATE domain SET rebuild_started='2026-09-14T00:00:00Z' WHERE name='busy'",
            (),
        )
        .await
        .unwrap();
        assert_eq!(
            scalar(
                &conn,
                "SELECT COUNT(*) FROM domain WHERE rebuild_started IS NOT NULL"
            )
            .await,
            1,
            "the stamped row carries a value and the row beside it still does not"
        );

        conn.execute(
            "UPDATE domain SET rebuild_started=NULL WHERE name='busy'",
            (),
        )
        .await
        .unwrap();
        assert_eq!(
            scalar(
                &conn,
                "SELECT COUNT(*) FROM domain WHERE rebuild_started IS NULL"
            )
            .await,
            2,
            "and the finished rebuild clears back to NULL"
        );
    }

    /// The v14 column against a domain row that predates it.
    ///
    /// A marker a binary older than the kind column stamped comes out of the
    /// migration with `rebuild_kind` NULL beside a `rebuild_started` that is
    /// set, which is the shape a reader has to tolerate: it says a rebuild did
    /// not finish and claims nothing about what the rows hold, because the two
    /// verbs leave opposite states behind.
    #[tokio::test]
    async fn v14_leaves_a_marker_written_before_it_without_a_kind() {
        let db = Builder::new_local(":memory:").build().await.unwrap();
        let conn = db.connect().unwrap();
        for m in &MIGRATIONS[..13] {
            conn.execute_batch(m.sql).await.unwrap();
        }
        assert_eq!(
            MIGRATIONS[13].version, 14,
            "the fourteenth migration is v14"
        );

        conn.execute_batch(
            "INSERT INTO domain(id, name, path, rebuild_started) \
             VALUES (1,'old','/tmp/old',NULL),(2,'busy','/tmp/busy','2026-09-14T00:00:00Z');",
        )
        .await
        .unwrap();

        conn.execute_batch(MIGRATIONS[13].sql).await.unwrap();

        assert_eq!(
            scalar(
                &conn,
                "SELECT COUNT(*) FROM domain WHERE rebuild_kind IS NULL"
            )
            .await,
            2,
            "no backfill: the standing marker keeps its instant and has no kind"
        );
        assert_eq!(
            scalar(
                &conn,
                "SELECT COUNT(*) FROM domain WHERE rebuild_started IS NOT NULL"
            )
            .await,
            1,
            "and the marker itself survived the migration"
        );

        conn.execute(
            "UPDATE domain SET rebuild_started='2026-09-17T00:00:00Z', rebuild_kind='wipe' WHERE name='old'",
            (),
        )
        .await
        .unwrap();
        assert_eq!(
            scalar(
                &conn,
                "SELECT COUNT(*) FROM domain WHERE rebuild_kind='wipe'"
            )
            .await,
            1,
            "a new stamp carries the verb that is running"
        );
    }

    /// The v11 column against a domain row that predates it.
    ///
    /// The whole grace-period design rests on this reading: a row written
    /// before the stamp existed comes out of the migration with
    /// `last_registered` NULL, and NULL is "never stamped", not "stamped
    /// infinitely long ago". Nothing backfills it, so on the first sweep after
    /// an upgrade every domain in the index is unstamped and none of them is
    /// old enough to collect. The row stamped afterwards is the control that
    /// proves the column accepts a value at all.
    #[tokio::test]
    async fn v11_leaves_a_domain_row_written_before_it_unstamped() {
        let db = Builder::new_local(":memory:").build().await.unwrap();
        let conn = db.connect().unwrap();
        for m in &MIGRATIONS[..10] {
            conn.execute_batch(m.sql).await.unwrap();
        }
        assert_eq!(MIGRATIONS[10].version, 11, "the eleventh migration is v11");

        conn.execute_batch(
            "INSERT INTO domain(id, name, path) VALUES (1,'old','/tmp/old'),(2,'seen','/tmp/seen');",
        )
        .await
        .unwrap();

        conn.execute_batch(MIGRATIONS[10].sql).await.unwrap();

        assert_eq!(
            scalar(
                &conn,
                "SELECT COUNT(*) FROM domain WHERE last_registered IS NULL"
            )
            .await,
            2,
            "no backfill: both pre-existing rows read NULL, meaning never stamped"
        );

        // Only the domain seen in the configuration is stamped, and the row
        // beside it stays NULL rather than aging into a date.
        conn.execute(
            "UPDATE domain SET last_registered='2026-09-09T00:00:00Z' WHERE name='seen'",
            (),
        )
        .await
        .unwrap();
        assert_eq!(
            scalar(
                &conn,
                "SELECT COUNT(*) FROM domain WHERE last_registered IS NOT NULL"
            )
            .await,
            1,
            "the stamped row carries a value and the unstamped one still does not"
        );
        assert_eq!(
            scalar(
                &conn,
                "SELECT COUNT(*) FROM domain WHERE name='old' AND last_registered IS NULL"
            )
            .await,
            1,
        );
    }

    /// The v10 column against a row that predates it.
    ///
    /// A reference written before `to_raw` existed has no bracket text to
    /// recover and none can be reconstructed, so the fallback reading must not
    /// fire for it: `to_raw` is NULL, every comparison against NULL is NULL,
    /// and the row resolves exactly as it did before until its engram is
    /// reindexed. The row beside it, written with the text, is the control that
    /// proves the guard is doing the work rather than the expression failing
    /// everywhere.
    #[tokio::test]
    async fn v10_leaves_a_reference_written_before_it_resolving_as_it_did() {
        let db = Builder::new_local(":memory:").build().await.unwrap();
        let conn = db.connect().unwrap();
        for m in &MIGRATIONS[..9] {
            conn.execute_batch(m.sql).await.unwrap();
        }
        assert_eq!(MIGRATIONS[9].version, 10, "the tenth migration is v10");

        // One domain, the engram somebody links to by its colon-bearing title,
        // and two links to it: one written before the column, one after.
        conn.execute_batch(
            r#"
            INSERT INTO domain(id, name, path) VALUES (1,'d','/tmp/d');
            INSERT INTO engram(id, domain_id, path, permalink, title)
                VALUES (1,1,'log.md','log-weekly','Log: Weekly Garden Notes'),
                       (2,1,'old.md','old','Old'),
                       (3,1,'new.md','new','New');
            INSERT INTO relation(id, engram_id, domain_id, line, rel_type, to_target, to_domain)
                VALUES (1,2,1,3,'superseded_by','Weekly Garden Notes','Log'),
                       (2,3,1,3,'superseded_by','Weekly Garden Notes','Log');
            "#,
        )
        .await
        .unwrap();

        conn.execute_batch(MIGRATIONS[9].sql).await.unwrap();
        // And the migrations after it, because `reference_match` below is the
        // current expression and reads the current schema: it names the actor
        // column v13 added, so a database stopped at v10 has no column for the
        // statement to compile against. The rows under test are untouched by
        // any of them.
        for m in &MIGRATIONS[10..] {
            conn.execute_batch(m.sql).await.unwrap();
        }
        // Only the second row is reindexed, which is what a reindex does: it
        // rewrites the reference rows of the engram it read.
        conn.execute(
            "UPDATE relation SET to_raw = 'Log: Weekly Garden Notes' WHERE id = 2",
            (),
        )
        .await
        .unwrap();

        let sql = format!(
            "UPDATE relation SET to_id = {resolved} WHERE relation.to_id IS NULL AND {resolved} IS NOT NULL",
            resolved =
                crate::store::reference_match("relation", crate::store::ReferenceCandidates::Base,)
        );
        conn.execute(&sql, ()).await.unwrap();

        assert_eq!(
            scalar(
                &conn,
                "SELECT COUNT(*) FROM relation WHERE id=1 AND to_id IS NULL"
            )
            .await,
            1,
            "the row written before the column resolves exactly as it did"
        );
        assert_eq!(
            scalar(&conn, "SELECT COALESCE(to_id, 0) FROM relation WHERE id=2").await,
            1,
            "the reindexed row reaches the engram whose title carries the colon"
        );
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

    /// The v13 widening of `engram`, over a database that already carries both
    /// of the `domain` columns added since - which is the shape an upgrade
    /// actually meets, and the one that matters: the swap rebuilds a table
    /// whose children point at it by name, and a rebuild proven only over a
    /// fresh schema proves nothing about the database in the field.
    ///
    /// Three claims are pinned here. Every base row survives with `actor = ''`
    /// and `tombstone = 0`, so an upgrade needs no resync to keep reading what
    /// it read before. The child rows keep pointing at the ids they pointed at,
    /// because the swap preserves ids rather than reassigning them. And the two
    /// actor-aware unique indexes replace the two the old table carried: one
    /// path can now hold one row per actor, while a second row for the same
    /// actor at that path is still refused.
    #[tokio::test]
    async fn v13_widens_engram_over_a_database_carrying_the_domain_columns() {
        let db = Builder::new_local(":memory:").build().await.unwrap();
        let conn = db.connect().unwrap();
        for m in &MIGRATIONS[..12] {
            conn.execute_batch(m.sql).await.unwrap();
        }
        assert_eq!(
            MIGRATIONS[12].version, 13,
            "the thirteenth migration is v13"
        );

        // A database in the field: the two `domain` columns v11 and v12 added
        // are already there, and an engram with child rows hangs off it.
        conn.execute_batch(
            "INSERT INTO domain(id, name, path, last_registered, rebuild_started) \
             VALUES (1,'d','/tmp/d','2026-09-14T00:00:00Z',NULL);\n\
             INSERT INTO engram(id, domain_id, path, permalink, title, sha256) \
             VALUES (7,1,'a.md','a','A','ff');\n\
             INSERT INTO observation(engram_id, line, category, content) VALUES (7,1,'note','x');\n\
             INSERT INTO chunk(engram_id, seq, text) VALUES (7,0,'x');\n",
        )
        .await
        .unwrap();

        conn.execute_batch(MIGRATIONS[12].sql).await.unwrap();

        assert_eq!(
            scalar(
                &conn,
                "SELECT COUNT(*) FROM engram WHERE id=7 AND path='a.md' AND sha256='ff' \
                 AND actor='' AND tombstone=0"
            )
            .await,
            1,
            "the row carried over whole, at its own id, as a base row"
        );
        assert_eq!(
            scalar(&conn, "SELECT COUNT(*) FROM observation WHERE engram_id=7").await
                + scalar(&conn, "SELECT COUNT(*) FROM chunk WHERE engram_id=7").await,
            2,
            "the child rows still point at the engram they pointed at"
        );
        assert!(
            scalar(
                &conn,
                "SELECT COUNT(*) FROM domain WHERE name='d' AND last_registered IS NOT NULL"
            )
            .await
                == 1,
            "and the domain columns the swap did not touch are untouched"
        );

        // `WIPE_TABLES` names `engram` as a table, and the swap dropped the
        // table that name pointed at. It is only still a valid name because the
        // rename put it back, so every name in that list is checked against the
        // swapped database rather than assumed - `wipe_clears_everything` runs
        // on a store where v13 was part of the initial chain and would not
        // notice a name the swap had orphaned.
        for table in WIPE_TABLES {
            let found = scalar(
                &conn,
                &format!(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='{table}'"
                ),
            )
            .await;
            assert_eq!(
                found, 1,
                "wipe names a table that exists after the swap: {table}"
            );
            conn.execute_batch(&format!("DELETE FROM {table};"))
                .await
                .unwrap_or_else(|e| panic!("wipe can clear {table} after the swap: {e}"));
        }
        assert_eq!(
            scalar(&conn, "SELECT COUNT(*) FROM engram").await,
            0,
            "and the wipe it drives empties the swapped table"
        );

        // Restore the two rows the wipe above just took, so the uniqueness
        // assertions below still have a base row to sit beside.
        conn.execute_batch(
            "INSERT INTO domain(id, name, path) VALUES (1,'d','/tmp/d');\n\
             INSERT INTO engram(id, domain_id, path, permalink) VALUES (7,1,'a.md','a');\n",
        )
        .await
        .unwrap();

        // Every index the table carried is back. A swap that forgets one is a
        // silent full scan later, not an error now, so the set is pinned here
        // rather than left to whichever query happens to notice.
        let mut indexes = Vec::new();
        {
            let mut rows = conn
                .query(
                    "SELECT name FROM sqlite_master WHERE type='index' AND tbl_name='engram' \
                     AND name IS NOT NULL ORDER BY name",
                    (),
                )
                .await
                .unwrap();
            while let Some(r) = rows.next().await.unwrap() {
                if let Ok(turso::Value::Text(n)) = r.get_value(0) {
                    indexes.push(n);
                }
            }
        }
        assert_eq!(
            indexes,
            vec![
                "idx_engram_current".to_string(),
                "idx_engram_domain".to_string(),
                "idx_engram_path_actor".to_string(),
                "idx_engram_permalink_actor".to_string(),
                "idx_engram_recorded".to_string(),
                "idx_engram_title_lower".to_string(),
                "idx_engram_type".to_string(),
            ],
            "the swap carries every index across, with the two old uniqueness \
             rules replaced by their actor-aware successors"
        );

        // One path, one row per actor.
        conn.execute(
            "INSERT INTO engram(domain_id, path, permalink, actor) VALUES (1,'a.md','a','alice')",
            (),
        )
        .await
        .unwrap();
        assert_eq!(
            scalar(&conn, "SELECT COUNT(*) FROM engram WHERE path='a.md'").await,
            2,
            "a draft sits beside the base row at the same path"
        );
        assert!(
            conn.execute(
                "INSERT INTO engram(domain_id, path, permalink, actor) VALUES (1,'a.md','a2','alice')",
                (),
            )
            .await
            .is_err(),
            "but one actor still gets only one row at a path"
        );
        assert!(
            conn.execute(
                "INSERT INTO engram(domain_id, path, permalink, actor) VALUES (1,'b.md','a','alice')",
                (),
            )
            .await
            .is_err(),
            "and only one row per permalink, per actor"
        );
    }

    /// A migration that dies partway through leaves the database exactly as it
    /// was, and unstamped.
    ///
    /// v13 is the migration that makes this matter. Every one before it was a
    /// single `ALTER TABLE ... ADD COLUMN` whose worst case was "applied but
    /// not stamped", with the data untouched; v13 drops `engram` and renames
    /// another table into its place, so a statement error, a kill or a power
    /// loss anywhere after that `DROP` would - unwrapped - leave a database
    /// with no `engram` table at all, no version row, and a retry that fails on
    /// `INSERT INTO engram_new ... FROM engram` forever. For a virtual domain
    /// the index is the source of truth, so that loss has nothing to resync
    /// from.
    ///
    /// The failure is injected the way it would really arrive: a statement
    /// after the `DROP` that does not run. What is asserted is both halves of
    /// the invariant - the table and its rows are still there, and the ledger
    /// still says v12 - because the stamp is written inside the same
    /// transaction as the DDL it stamps.
    #[tokio::test]
    async fn a_migration_that_dies_after_the_drop_leaves_the_table_and_no_stamp() {
        // v13, cut off immediately after the `DROP TABLE engram` and given a
        // statement that cannot run. That is the exact window the unwrapped
        // batch could die in: the old table is gone and the new one has not
        // been renamed into its place yet.
        let cut = SCHEMA_V13
            .find("DROP TABLE engram;")
            .expect("v13 drops the table")
            + "DROP TABLE engram;".len();
        let poisoned: &'static str = Box::leak(
            format!(
                "{}\nSELECT no_such_function_at_all();\n",
                &SCHEMA_V13[..cut]
            )
            .into_boxed_str(),
        );
        let db = Builder::new_local(":memory:").build().await.unwrap();
        let conn = db.connect().unwrap();

        let mut list: Vec<Migration> = Vec::new();
        for m in &MIGRATIONS[..12] {
            list.push(Migration {
                version: m.version,
                label: m.label,
                sql: m.sql,
            });
        }
        list.push(Migration {
            version: 13,
            label: "engram actor dimension (poisoned)",
            sql: poisoned,
        });
        apply_migrations(&conn, &list[..12]).await.unwrap();

        conn.execute_batch(
            "INSERT INTO domain(id, name, path) VALUES (1,'d','/tmp/d');\n\
             INSERT INTO engram(id, domain_id, path, permalink, sha256) \
             VALUES (7,1,'a.md','a','ff');\n",
        )
        .await
        .unwrap();

        let err = apply_migrations(&conn, &list)
            .await
            .expect_err("the poisoned migration fails");
        assert!(
            err.to_string().contains("v13"),
            "and says which migration died: {err}"
        );

        assert_eq!(
            scalar(
                &conn,
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='engram'"
            )
            .await,
            1,
            "the engram table survives a migration that dropped it and then died"
        );
        assert_eq!(
            scalar(
                &conn,
                "SELECT COUNT(*) FROM engram WHERE id=7 AND sha256='ff'"
            )
            .await,
            1,
            "with its rows, so there is something to retry over"
        );
        assert_eq!(
            scalar(
                &conn,
                "SELECT COALESCE(MAX(version), 0) FROM schema_migration"
            )
            .await,
            12,
            "and the ledger still says v12, so the retry runs v13 again rather than \
             skipping it as applied"
        );

        // And the retry over that untouched database finishes the job.
        apply_migrations(&conn, &MIGRATIONS[..13]).await.unwrap();
        assert_eq!(
            scalar(&conn, "SELECT COUNT(*) FROM engram WHERE id=7 AND actor=''").await,
            1,
            "the row came through the retry as a base row"
        );
    }
}
