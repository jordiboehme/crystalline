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
    Migration {
        version: 6,
        label: "case-folded tag identity",
        sql: SCHEMA_V6,
    },
    Migration {
        version: 7,
        label: "tag alias map",
        sql: SCHEMA_V7,
    },
    Migration {
        version: 8,
        label: "engram attachments",
        sql: SCHEMA_V8,
    },
    Migration {
        version: 9,
        label: "raw reference text",
        sql: SCHEMA_V9,
    },
    Migration {
        version: 10,
        label: "domain registration stamp",
        sql: SCHEMA_V10,
    },
    Migration {
        version: 11,
        label: "domain rebuild marker",
        sql: SCHEMA_V11,
    },
    Migration {
        version: 12,
        label: "engram actor dimension",
        sql: SCHEMA_V12,
    },
    Migration {
        version: 13,
        label: "domain rebuild kind",
        sql: SCHEMA_V13,
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

// The Postgres twin of Turso's v7: case-folded tag identity. Tag identity is now
// case-folded at intern time, so `Foo` and `foo` share one tag row. A database
// written before the fold can still hold case-duplicate rows; this migration
// merges them. Repoint every join row onto the lowest id per folded name, drop
// the join rows that still point at a duplicate, drop the duplicate tag rows and
// lowercase the survivors. The `ON CONFLICT DO NOTHING` step materializes the
// min-id form of each join row and silently absorbs the primary-key collision
// when one engram already carried both cases of a tag. Postgres `lower()` folds
// ASCII (and more), which is a superset of what verify E007 admits, so a
// canonical tag folds identically to Turso. Every join row ends on a surviving
// id, so no foreign key is left dangling.
const SCHEMA_V6: &str = r#"
INSERT INTO engram_tag(engram_id, tag_id)
SELECT et.engram_id, m.min_id
FROM engram_tag et
JOIN tag t ON t.id = et.tag_id
JOIN (SELECT lower(name) AS lname, MIN(id) AS min_id FROM tag GROUP BY lower(name)) m
  ON m.lname = lower(t.name)
ON CONFLICT DO NOTHING;

INSERT INTO observation_tag(observation_id, tag_id)
SELECT ot.observation_id, m.min_id
FROM observation_tag ot
JOIN tag t ON t.id = ot.tag_id
JOIN (SELECT lower(name) AS lname, MIN(id) AS min_id FROM tag GROUP BY lower(name)) m
  ON m.lname = lower(t.name)
ON CONFLICT DO NOTHING;

DELETE FROM engram_tag WHERE tag_id NOT IN (SELECT MIN(id) FROM tag GROUP BY lower(name));
DELETE FROM observation_tag WHERE tag_id NOT IN (SELECT MIN(id) FROM tag GROUP BY lower(name));

DELETE FROM tag WHERE id NOT IN (SELECT MIN(id) FROM tag GROUP BY lower(name));

UPDATE tag SET name = lower(name);
"#;

// The Postgres twin of Turso's v8: the derived tag-alias map. One row per
// `(domain, alias)` records the canonical spelling an old tag folds onto at
// query time, so a search on either spelling matches every engram tagged with a
// sibling. Derived purely from MANIFEST content and repopulated on the next sync
// per domain: an upgraded database carries no aliases until each domain resyncs
// (a new-feature grace), and a wipe+resync is the accepted way to backfill. The
// canonical index serves the reverse lookup during expansion.
const SCHEMA_V7: &str = r#"
CREATE TABLE tag_alias (
    domain_id BIGINT NOT NULL REFERENCES domain(id),
    alias TEXT NOT NULL,
    canonical TEXT NOT NULL,
    PRIMARY KEY (domain_id, alias)
);
CREATE INDEX idx_tag_alias_canonical ON tag_alias(domain_id, canonical);
"#;

// The Postgres twin of Turso's v9: binary attachments. One metadata row per
// asset under a domain's `assets/` folder, keyed by domain-relative path. A file
// domain keeps the bytes on disk and holds metadata only; a virtual domain has
// no filesystem, so its bytes live in `attachment_blob` as BYTEA, split off into
// its own table so the metadata listing never drags a blob through the row
// cache. `size` is the byte length and `modified` an RFC 3339 instant, matching
// the temporal columns' text form.
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
const SCHEMA_V9: &str = r#"
ALTER TABLE relation ADD COLUMN IF NOT EXISTS to_raw TEXT;
ALTER TABLE link ADD COLUMN IF NOT EXISTS to_raw TEXT;
"#;

// When this domain was last seen in the configuration. TEXT RFC 3339 rather
// than `timestamptz`, matching `last_sync` and the Turso twin so the column
// compares lexically and identically on both backends.
//
// Nullable with no default and no backfill, and that is the point: every row
// that predates this migration reads NULL, and NULL means "never stamped", not
// "stamped infinitely long ago". A caller that ages the stamp to decide whether
// a domain has been gone long enough to collect must read NULL as no evidence
// at all and leave the row alone, so the first sweep after an upgrade collects
// nothing. Rows earn a stamp only by being seen registered.
const SCHEMA_V10: &str = r#"
ALTER TABLE domain ADD COLUMN IF NOT EXISTS last_registered TEXT;
"#;

// The Turso v12 column, same meaning: when a forced rebuild of this domain was
// stamped as started, RFC 3339, NULL when none is in flight. Nullable, no
// default, no backfill - a row written before this migration reads NULL, which
// is the truth for every one of them.
const SCHEMA_V11: &str = r#"
ALTER TABLE domain ADD COLUMN IF NOT EXISTS rebuild_started TEXT;
"#;

// The Turso v14 column, same meaning: which verb stamped the rebuild marker
// beside it, `full` or `wipe`, NULL when no rebuild is in flight and for a
// marker a binary older than the column stamped. Nullable, no default, no
// backfill, and converging on a retry the way every migration in this dialect
// does.
const SCHEMA_V13: &str = r#"
ALTER TABLE domain ADD COLUMN IF NOT EXISTS rebuild_kind TEXT;
"#;

// The actor dimension, the Turso v13 migration's twin. `actor = ''` is the base
// row - the one the domain's files on disk say exists - and any other value is
// one actor's private draft at that path, a full row in its own right so
// chunks, embeddings and graph rows key to its id exactly as a base row's do.
// `tombstone` is that actor's draft deletion of a base row. Both defaults are
// the base reading, so every row an upgrade finds stays the row it was and
// nothing needs a resync.
//
// Where Turso has to rebuild the table to be rid of a table-level UNIQUE, this
// dialect drops the constraint in place, so the ids, the child rows and the
// four non-unique indexes are never disturbed at all. What replaces the old
// pair is the same pair of actor-aware unique indexes, under the same names, so
// the two backends read alike: one path and one permalink may now carry one row
// per actor, and no actor may hold two rows at either.
//
// Every statement converges on a retry, the way v10 and v11 above it do. This
// dialect runs the DDL batch as one implicit transaction, so no half-applied
// schema is possible - but the version stamp is a separate statement, so a
// crash between the two would replay this migration on the next start, and a
// bare `ADD COLUMN` would wedge it on a duplicate-column error.
const SCHEMA_V12: &str = r#"
ALTER TABLE engram ADD COLUMN IF NOT EXISTS actor TEXT NOT NULL DEFAULT '';
ALTER TABLE engram ADD COLUMN IF NOT EXISTS tombstone BOOLEAN NOT NULL DEFAULT FALSE;

ALTER TABLE engram DROP CONSTRAINT IF EXISTS engram_domain_id_permalink_key;
DROP INDEX IF EXISTS idx_engram_path;

CREATE UNIQUE INDEX IF NOT EXISTS idx_engram_permalink_actor ON engram(domain_id, permalink, actor);
CREATE UNIQUE INDEX IF NOT EXISTS idx_engram_path_actor ON engram(domain_id, path, actor);
"#;

const SCHEMA_V8: &str = r#"
CREATE TABLE attachment (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    domain_id BIGINT NOT NULL REFERENCES domain(id) ON DELETE CASCADE,
    path TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    mime TEXT NOT NULL,
    size BIGINT NOT NULL,
    modified TEXT NOT NULL,
    UNIQUE(domain_id, path)
);
CREATE INDEX idx_attachment_domain ON attachment(domain_id);

CREATE TABLE attachment_blob (
    attachment_id BIGINT PRIMARY KEY REFERENCES attachment(id) ON DELETE CASCADE,
    content BYTEA NOT NULL
);
"#;

/// The tables cleared by `wipe()`, child rows first so the enforced foreign
/// keys are satisfied at every step. `tag_alias`, `attachment` and
/// `domain_lock` all reference `domain(id)`, so they are cleared before
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

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::Connection;

    /// The v11 column against a database written before it, which is the case
    /// an upgrade actually meets: a schema raised to v10, domain rows already
    /// in it, then v11 applied over the top.
    ///
    /// Runs only when `CRYSTALLINE_TEST_POSTGRES_URL` is set, the same gate the
    /// parity suite uses; without it there is no server to migrate and the test
    /// is a silent no-op rather than a failure. It talks to sqlx directly
    /// rather than through `PostgresStore`, because opening a store applies
    /// every migration at once and there would be no pre-existing database
    /// left to migrate.
    #[tokio::test]
    async fn v11_leaves_a_domain_row_written_before_it_unmarked() {
        let Ok(url) = std::env::var("CRYSTALLINE_TEST_POSTGRES_URL") else {
            return;
        };
        if url.is_empty() {
            return;
        }
        let schema = format!("mig_{}", std::process::id());
        let mut conn = sqlx::PgConnection::connect(&url).await.unwrap();
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "DROP SCHEMA IF EXISTS {schema} CASCADE; CREATE SCHEMA {schema}; SET search_path TO {schema}, public"
        )))
        .execute(&mut conn)
        .await
        .unwrap();

        // A database at v10: everything up to but not including the marker.
        for m in &MIGRATIONS[..10] {
            sqlx::raw_sql(m.sql).execute(&mut conn).await.unwrap();
        }
        assert_eq!(MIGRATIONS[10].version, 11, "the eleventh migration is v11");
        sqlx::raw_sql(
            "INSERT INTO domain(name, path) VALUES ('old','/tmp/old'),('busy','/tmp/busy')",
        )
        .execute(&mut conn)
        .await
        .unwrap();

        sqlx::raw_sql(MIGRATIONS[10].sql)
            .execute(&mut conn)
            .await
            .unwrap();

        let unmarked: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM domain WHERE rebuild_started IS NULL")
                .fetch_one(&mut conn)
                .await
                .unwrap();
        assert_eq!(
            unmarked.0, 2,
            "no backfill: both pre-existing rows read NULL, meaning no rebuild in flight"
        );

        sqlx::raw_sql("UPDATE domain SET rebuild_started='2026-09-14T00:00:00Z' WHERE name='busy'")
            .execute(&mut conn)
            .await
            .unwrap();
        let marked: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM domain WHERE rebuild_started IS NOT NULL")
                .fetch_one(&mut conn)
                .await
                .unwrap();
        assert_eq!(
            marked.0, 1,
            "the stamped row carries a value and the row beside it still does not"
        );

        sqlx::raw_sql(sqlx::AssertSqlSafe(format!("DROP SCHEMA {schema} CASCADE")))
            .execute(&mut conn)
            .await
            .unwrap();
    }

    /// The v13 column against a database written before it, carrying a marker
    /// an older binary stamped: the instant survives and the kind reads NULL,
    /// which is the shape a reader has to tolerate - it says a rebuild did not
    /// finish and claims nothing about what the rows hold.
    ///
    /// Runs only when `CRYSTALLINE_TEST_POSTGRES_URL` is set, the same gate the
    /// parity suite uses.
    #[tokio::test]
    async fn v13_leaves_a_marker_written_before_it_without_a_kind() {
        let Ok(url) = std::env::var("CRYSTALLINE_TEST_POSTGRES_URL") else {
            return;
        };
        if url.is_empty() {
            return;
        }
        let schema = format!("mig13_{}", std::process::id());
        let mut conn = sqlx::PgConnection::connect(&url).await.unwrap();
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "DROP SCHEMA IF EXISTS {schema} CASCADE; CREATE SCHEMA {schema}; SET search_path TO {schema}, public"
        )))
        .execute(&mut conn)
        .await
        .unwrap();

        // A database at v12: everything up to but not including the kind.
        for m in &MIGRATIONS[..12] {
            sqlx::raw_sql(m.sql).execute(&mut conn).await.unwrap();
        }
        assert_eq!(
            MIGRATIONS[12].version, 13,
            "the thirteenth migration is v13"
        );
        sqlx::raw_sql(
            "INSERT INTO domain(name, path, rebuild_started) \
             VALUES ('old','/tmp/old',NULL),('busy','/tmp/busy','2026-09-14T00:00:00Z')",
        )
        .execute(&mut conn)
        .await
        .unwrap();

        sqlx::raw_sql(MIGRATIONS[12].sql)
            .execute(&mut conn)
            .await
            .unwrap();

        let kindless: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM domain WHERE rebuild_kind IS NULL")
                .fetch_one(&mut conn)
                .await
                .unwrap();
        assert_eq!(
            kindless.0, 2,
            "no backfill: the standing marker keeps its instant and has no kind"
        );
        let marked: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM domain WHERE rebuild_started IS NOT NULL")
                .fetch_one(&mut conn)
                .await
                .unwrap();
        assert_eq!(marked.0, 1, "and the marker itself survived the migration");

        sqlx::raw_sql(
            "UPDATE domain SET rebuild_started='2026-09-17T00:00:00Z', rebuild_kind='wipe' WHERE name='old'",
        )
        .execute(&mut conn)
        .await
        .unwrap();
        let wiped: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM domain WHERE rebuild_kind='wipe'")
            .fetch_one(&mut conn)
            .await
            .unwrap();
        assert_eq!(wiped.0, 1, "a new stamp carries the verb that is running");

        sqlx::raw_sql(sqlx::AssertSqlSafe(format!("DROP SCHEMA {schema} CASCADE")))
            .execute(&mut conn)
            .await
            .unwrap();
    }

    /// The v12 widening of `engram`, over a database that already carries both
    /// of the `domain` columns added since - the shape an upgrade actually
    /// meets. Postgres alters in place rather than rebuilding, so what has to
    /// be proven here is different from Turso's swap: that the table-level
    /// `UNIQUE(domain_id, permalink)` is really gone and the two actor-aware
    /// unique indexes really took over, since a dropped constraint that was
    /// never dropped would refuse a second actor's draft at the first write.
    ///
    /// Runs only when `CRYSTALLINE_TEST_POSTGRES_URL` is set, the same gate the
    /// parity suite uses.
    #[tokio::test]
    async fn v12_widens_engram_over_a_database_carrying_the_domain_columns() {
        let Ok(url) = std::env::var("CRYSTALLINE_TEST_POSTGRES_URL") else {
            return;
        };
        if url.is_empty() {
            return;
        }
        let schema = format!("mig12_{}", std::process::id());
        let mut conn = sqlx::PgConnection::connect(&url).await.unwrap();
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "DROP SCHEMA IF EXISTS {schema} CASCADE; CREATE SCHEMA {schema}; SET search_path TO {schema}, public"
        )))
        .execute(&mut conn)
        .await
        .unwrap();

        // A database at v11: everything up to but not including the widening.
        for m in &MIGRATIONS[..11] {
            sqlx::raw_sql(m.sql).execute(&mut conn).await.unwrap();
        }
        assert_eq!(MIGRATIONS[11].version, 12, "the twelfth migration is v12");
        sqlx::raw_sql(
            "INSERT INTO domain(name, path, last_registered) VALUES ('d','/tmp/d','2026-09-14T00:00:00Z'); \
             INSERT INTO engram(domain_id, path, permalink, title, sha256) \
             SELECT id, 'a.md', 'a', 'A', 'ff' FROM domain WHERE name='d'",
        )
        .execute(&mut conn)
        .await
        .unwrap();

        sqlx::raw_sql(MIGRATIONS[11].sql)
            .execute(&mut conn)
            .await
            .unwrap();
        // And again, because the version stamp is a statement of its own: a
        // crash between the DDL and the stamp replays this migration on the
        // next start, and it has to converge rather than wedge on a duplicate
        // column.
        sqlx::raw_sql(MIGRATIONS[11].sql)
            .execute(&mut conn)
            .await
            .expect("v12 applies twice");

        let base: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM engram WHERE path='a.md' AND sha256='ff' AND actor='' AND NOT tombstone",
        )
        .fetch_one(&mut conn)
        .await
        .unwrap();
        assert_eq!(
            base.0, 1,
            "the row carried over whole, as a base row, with no resync"
        );

        sqlx::raw_sql(
            "INSERT INTO engram(domain_id, path, permalink, actor) \
             SELECT id, 'a.md', 'a', 'alice' FROM domain WHERE name='d'",
        )
        .execute(&mut conn)
        .await
        .unwrap();
        let both: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM engram WHERE path='a.md'")
            .fetch_one(&mut conn)
            .await
            .unwrap();
        assert_eq!(
            both.0, 2,
            "a draft sits beside the base row at the same path and the same permalink"
        );

        let dup = sqlx::raw_sql(
            "INSERT INTO engram(domain_id, path, permalink, actor) \
             SELECT id, 'a.md', 'a2', 'alice' FROM domain WHERE name='d'",
        )
        .execute(&mut conn)
        .await;
        assert!(
            dup.is_err(),
            "but one actor still gets only one row at a path"
        );

        sqlx::raw_sql(sqlx::AssertSqlSafe(format!("DROP SCHEMA {schema} CASCADE")))
            .execute(&mut conn)
            .await
            .unwrap();
    }
}
