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
    Migration {
        version: 14,
        label: "engram body in its own table",
        sql: SCHEMA_V14,
    },
    Migration {
        version: 15,
        label: "domain spellings",
        sql: SCHEMA_V15,
    },
    Migration {
        version: 16,
        label: "unbind references an unknown prefix bound at home",
        sql: SCHEMA_V16,
    },
    Migration {
        version: 17,
        label: "contradiction scores",
        sql: SCHEMA_V17,
    },
    Migration {
        version: 18,
        label: "metadata values",
        sql: SCHEMA_V18,
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

// The body moves out of `engram` into a table of its own, the Turso v15
// migration's twin. One row per engram, keyed by the engram's id.
//
// The reason is Turso's, not this dialect's: turso materializes a whole row
// payload for any column read, so every seek into `engram` paid for the body of
// that engram, and phase 1 of the semantic scan seeks the parent row once per
// chunk. Postgres TOASTs a large body out of line and never had that cost. The
// shape is mirrored anyway, because the two backends share every statement
// builder above them and a schema that differed here would mean two spellings
// of every read of a body.
//
// Where Turso has to rebuild the table to be rid of a column, this dialect
// drops it in place, so the ids, the child rows and every index are never
// disturbed. The copy runs before the drop, for the obvious reason.
//
// Every statement converges on a retry, the way the migrations above it do: the
// version stamp is a statement of its own, so a crash between the DDL and the
// stamp replays this migration on the next start. `IF NOT EXISTS` and `DROP
// COLUMN IF EXISTS` cover two of the three statements; the copy needs the
// block, because on a replay the column it reads is already gone and a bare
// `SELECT id, content FROM engram` would wedge the migration on an error rather
// than finding nothing to do.
//
// The guard asks `'engram'::regclass` rather than `information_schema` and a
// schema name, and that is load bearing: `regclass` resolves the bare name
// through `search_path` exactly as the `INSERT` and the `ALTER TABLE` below it
// do. A guard that named `current_schema()` instead would agree with them only
// while `engram` sits in the first entry of the path, and on a connection where
// it does not it would read false, skip the copy, and let the drop - which
// still finds the table through the path - take every body with it.
const SCHEMA_V14: &str = r#"
CREATE TABLE IF NOT EXISTS engram_content (
    engram_id BIGINT PRIMARY KEY REFERENCES engram(id),
    content TEXT NOT NULL DEFAULT ''
);

DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM pg_attribute
        WHERE attrelid = 'engram'::regclass
          AND attname = 'content'
          AND NOT attisdropped
    ) THEN
        INSERT INTO engram_content (engram_id, content)
        SELECT id, content FROM engram
        ON CONFLICT (engram_id) DO NOTHING;
    END IF;
END $$;

ALTER TABLE engram DROP COLUMN IF EXISTS content;
"#;

// Every spelling a domain answers to - its local name, its canonical name, its
// aliases - mapped to the domain row; the Turso v16 twin. Written to replay
// cleanly (`IF NOT EXISTS`, `ON CONFLICT DO NOTHING`) because the ledger stamp
// below is a separate statement, as v14 is. On a shared index the backfill
// covers every instance's rows, which is right: each row is its own name
// whichever instance registered it. The two partial `to_domain` indexes are
// the Turso twin's too: they serve the reads that ask by spelling.
const SCHEMA_V15: &str = r#"
CREATE TABLE IF NOT EXISTS domain_spelling (
    spelling TEXT PRIMARY KEY,
    domain_id BIGINT NOT NULL REFERENCES domain(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_domain_spelling_domain ON domain_spelling(domain_id);
INSERT INTO domain_spelling (spelling, domain_id) SELECT name, id FROM domain
ON CONFLICT (spelling) DO NOTHING;
CREATE INDEX IF NOT EXISTS idx_relation_to_domain ON relation(to_domain) WHERE to_domain IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_link_to_domain ON link(to_domain) WHERE to_domain IS NOT NULL;
"#;

// A reference with a domain prefix resolves only in the domain the prefix
// spells. Before this, a prefix that spelled no domain fell back to the
// source's own domain and bound the bare target there, so `[[ops:Runbook]]`
// written where `ops` is not registered landed on a home Runbook. The resolve
// pass no longer does that, but the rows it bound that way stay bound until
// their engram is reindexed, so this unbinds them once.
//
// Only the rows the current rule would not bind: a prefix no spelling holds,
// and a bound engram that is not the whole bracket text at home, the one
// reading such a prefix still gets. A row bound to that engram keeps it. A row
// written before `to_raw` existed compares against NULL and is unbound, which
// is what the current rule gives it too. The startup sync binds a file
// domain's rows again, and the engine's pass after it binds virtual domains
// and drafts, so every unbound row the whole text now reaches is bound. A row whose
// engram matches the whole text by title while another matches it by
// permalink keeps the title match (contrived, left as is).
// Idempotent, so a replay after a missed ledger stamp finds nothing the
// first run left behind. The Turso v17 twin.
const SCHEMA_V16: &str = r#"
UPDATE relation SET to_id = NULL
WHERE to_domain IS NOT NULL AND to_id IS NOT NULL
  AND NOT EXISTS (SELECT 1 FROM domain_spelling s WHERE s.spelling = relation.to_domain)
  AND NOT EXISTS (SELECT 1 FROM engram e WHERE e.id = relation.to_id
                  AND e.domain_id = relation.domain_id
                  AND (e.permalink = relation.to_raw OR lower(e.title) = lower(relation.to_raw)));
UPDATE link SET to_id = NULL
WHERE to_domain IS NOT NULL AND to_id IS NOT NULL
  AND NOT EXISTS (SELECT 1 FROM domain_spelling s WHERE s.spelling = link.to_domain)
  AND NOT EXISTS (SELECT 1 FROM engram e WHERE e.id = link.to_id
                  AND e.domain_id = link.domain_id
                  AND (e.permalink = link.to_raw OR lower(e.title) = lower(link.to_raw)));
"#;

// The Turso v18 tables, same meaning. Foreign keys are enforced here, so the
// declared cascades do the work on a delete; `delete_engram` and
// `clear_domain` still delete by hand for symmetry with Turso. Written to
// replay (`IF NOT EXISTS`), because the ledger stamp is a separate statement.
// `idx_contradiction_pair_domain` carries the order columns as its Turso twin
// does, so the two schemas stay the same, and the two `engram_b` indexes serve
// the second half of `delete_engram`'s `engram_a=$1 OR engram_b=$1` beside the
// primary keys that serve the first. `idx_contradiction_pair_model` serves
// `Store::scored_pair_count`'s `WHERE model=$1`, which names no domain and so
// cannot seek `idx_contradiction_pair_domain` - see the Turso twin's comment.
// `observation_vector` caches the embedding of each observation line the
// contradiction check pairs, keyed by the embedding model and the row hash of
// the folded text; it holds no text and no engram id, and its primary key
// serves every read - see the Turso twin's comment. A plain `BYTEA`, not a
// `vector` column: nothing searches these vectors in SQL.
//
// `domain.parse_generation` is the parser generation
// (`crystalline_core::PARSE_GENERATION`) the domain's rows were last derived
// with. An index this migration (the Turso v18 twin) upgrades has every row at 0, older than any
// parser that records one, so the first sync after the upgrade reparses each
// domain once and stamps it; a domain row created later is stamped with the
// current generation on insert, so a fresh index reparses nothing. Unreleased
// when it joined this migration (the Turso v18 twin), so it rides here rather than in one of its own.
const SCHEMA_V17: &str = r#"
CREATE TABLE IF NOT EXISTS contradiction_pair (
    domain_id BIGINT NOT NULL REFERENCES domain(id) ON DELETE CASCADE,
    engram_a BIGINT NOT NULL REFERENCES engram(id) ON DELETE CASCADE,
    engram_b BIGINT NOT NULL REFERENCES engram(id) ON DELETE CASCADE,
    checksum_a TEXT NOT NULL,
    checksum_b TEXT NOT NULL,
    cosine DOUBLE PRECISION NOT NULL,
    model TEXT NOT NULL,
    scored_at TEXT NOT NULL,
    PRIMARY KEY (engram_a, engram_b, model)
);
CREATE INDEX IF NOT EXISTS idx_contradiction_pair_domain ON contradiction_pair(domain_id, model, engram_a, engram_b);
CREATE INDEX IF NOT EXISTS idx_contradiction_pair_model ON contradiction_pair(model);

CREATE TABLE IF NOT EXISTS contradiction (
    domain_id BIGINT NOT NULL REFERENCES domain(id) ON DELETE CASCADE,
    engram_a BIGINT NOT NULL REFERENCES engram(id) ON DELETE CASCADE,
    engram_b BIGINT NOT NULL REFERENCES engram(id) ON DELETE CASCADE,
    line_a BIGINT NOT NULL,
    line_b BIGINT NOT NULL,
    hash_a TEXT NOT NULL,
    hash_b TEXT NOT NULL,
    model TEXT NOT NULL,
    score_ab DOUBLE PRECISION NOT NULL,
    score_ba DOUBLE PRECISION NOT NULL,
    similarity DOUBLE PRECISION NOT NULL DEFAULT 0,
    period BIGINT NOT NULL DEFAULT 0,
    PRIMARY KEY (engram_a, engram_b, hash_a, hash_b, model)
);
CREATE INDEX IF NOT EXISTS idx_contradiction_engram_b ON contradiction(engram_b);
CREATE INDEX IF NOT EXISTS idx_contradiction_pair_engram_b ON contradiction_pair(engram_b);
CREATE INDEX IF NOT EXISTS idx_contradiction_domain ON contradiction(domain_id, model);

CREATE TABLE IF NOT EXISTS observation_vector (
    model TEXT NOT NULL,
    hash TEXT NOT NULL,
    dims BIGINT NOT NULL,
    vector BYTEA NOT NULL,
    PRIMARY KEY (model, hash)
);

ALTER TABLE domain ADD COLUMN IF NOT EXISTS parse_generation BIGINT NOT NULL DEFAULT 0;
"#;

// The Turso v19 twin. `IF NOT EXISTS`, because here a step and its stamp are
// not one transaction: a step that died during its backfill is replayed whole,
// and the backfill clears the table before it fills it, so a replay
// converges. Foreign keys are enforced here, so the cascade is real; the
// deletes are written out all the same, as on Turso.
const SCHEMA_V18: &str = r#"
CREATE TABLE IF NOT EXISTS engram_meta_value (
    engram_id BIGINT NOT NULL REFERENCES engram(id) ON DELETE CASCADE,
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    PRIMARY KEY (engram_id, key, value)
);
CREATE INDEX IF NOT EXISTS idx_engram_meta_value_key_value ON engram_meta_value(key, value, engram_id);
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
/// keys are satisfied at every step. `tag_alias`, `attachment`, `domain_lock`
/// and `domain_spelling` all reference `domain(id)`, so they are cleared before
/// `domain`; `attachment_blob` references `attachment`, so it goes before it,
/// and `engram_content` references `engram`, so it goes before that.
/// `contradiction` and `contradiction_pair` reference `engram` and `domain`,
/// so they go first. `observation_vector` references nothing and leads the
/// list. `engram_meta_value` references `engram`, so it goes before it.
pub const WIPE_TABLES: &[&str] = &[
    "observation_vector",
    "contradiction",
    "contradiction_pair",
    "observation_tag",
    "engram_tag",
    "engram_meta_value",
    "chunk",
    "observation",
    "relation",
    "link",
    "tag",
    "engram_content",
    "engram",
    "tag_alias",
    "attachment_blob",
    "attachment",
    "domain_lock",
    "domain_spelling",
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
    // The Turso twin of this guard: a schema raised past every migration this
    // binary ships is a newer Crystalline's work, not damage, and there is no
    // migration list here that could replay backwards to what this binary
    // expects. Checked before the loop touches anything.
    let known = MIGRATIONS.last().map(|m| m.version).unwrap_or(0);
    if current > known {
        return Err(IndexError::SchemaTooNew {
            found: current,
            known,
        });
    }
    for m in MIGRATIONS {
        if m.version <= current {
            continue;
        }
        // The DDL, its backfill and the stamp are one transaction, as on
        // Turso: a step that fails anywhere rolls back whole (dropping `tx`
        // without a commit rolls it back), the fill is never seen half done
        // by another connection, and its inserts share one commit instead of
        // one each. No step uses a statement Postgres refuses inside a
        // transaction (`CREATE INDEX CONCURRENTLY`, `VACUUM`), and a new one
        // must not. The steps stay written to replay cleanly all the same,
        // because a database another binary migrated may hold a step without
        // its stamp.
        let mut tx = sqlx::Connection::begin(&mut *conn)
            .await
            .map_err(|e| IndexError::Migration(e.to_string()))?;
        sqlx::raw_sql(m.sql)
            .execute(&mut *tx)
            .await
            .map_err(|e| IndexError::Migration(format!("v{} ({}): {e}", m.version, m.label)))?;
        backfill(&mut tx, m.version)
            .await
            .map_err(|e| IndexError::Migration(format!("v{} ({}): {e}", m.version, m.label)))?;
        let now = chrono::Utc::now().to_rfc3339();
        sqlx::query("INSERT INTO schema_migration (version, applied_at) VALUES ($1, $2)")
            .bind(m.version)
            .bind(&now)
            .execute(&mut *tx)
            .await
            .map_err(|e| IndexError::Migration(e.to_string()))?;
        tx.commit()
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

/// The Turso `backfill` twin: the data step a migration runs after its DDL
/// and before its stamp, for rows only Rust can derive.
async fn backfill(conn: &mut PgConnection, version: i64) -> Result<()> {
    match version {
        18 => backfill_meta_values(conn).await,
        _ => Ok(()),
    }
}

/// Fill `engram_meta_value` from every engram row's stored `metadata`, every
/// actor's rows included. `metadata::text` reads the JSONB back as JSON, so
/// it parses with the same function the writer's rows come from. The table is
/// cleared first, so a replay converges. The rows are read a page of
/// [`super::BACKFILL_PAGE`] engrams at a time in id order, so memory stays
/// bounded on a large index, and each page's rows go in as multi-row inserts
/// across its engrams.
async fn backfill_meta_values(conn: &mut PgConnection) -> Result<()> {
    sqlx::raw_sql("DELETE FROM engram_meta_value")
        .execute(&mut *conn)
        .await
        .map_err(IndexError::from)?;
    let mut after = i64::MIN;
    loop {
        let page: Vec<(i64, String)> = sqlx::query_as(
            "SELECT id, metadata::text FROM engram WHERE id > $1 ORDER BY id LIMIT $2",
        )
        .bind(after)
        .bind(super::BACKFILL_PAGE)
        .fetch_all(&mut *conn)
        .await
        .map_err(IndexError::from)?;
        let Some(&(last, _)) = page.last() else {
            break;
        };
        after = last;
        let mut rows = Vec::new();
        for (id, text) in &page {
            let Ok(metadata) = serde_json::from_str::<serde_json::Value>(text) else {
                continue;
            };
            for (key, value) in crate::store::meta_value_rows(&metadata) {
                rows.push((*id, key, value));
            }
        }
        super::insert_meta_rows(&mut *conn, &rows).await?;
        if (page.len() as i64) < super::BACKFILL_PAGE {
            break;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::Connection;

    /// v16 against an index the old resolve pass wrote, the Turso v17 twin's
    /// fixture row for row: a prefix that spelled no domain had bound the bare
    /// target at home. The migration unbinds exactly those rows (and a row
    /// bound to the whole text in another domain), keeps a row bound to the
    /// whole bracket text at home, a row whose prefix names a domain and a bare
    /// row, the resolve pass that follows binds the way the current rule does,
    /// and a replay changes nothing.
    ///
    /// Gated on `CRYSTALLINE_TEST_POSTGRES_URL` like the v11 test beside it,
    /// and talking to sqlx directly for the same reason.
    #[tokio::test]
    async fn v16_unbinds_what_an_unknown_prefix_bound_at_home() {
        let Ok(url) = std::env::var("CRYSTALLINE_TEST_POSTGRES_URL") else {
            return;
        };
        if url.is_empty() {
            return;
        }
        let schema = format!("mig16_{}", std::process::id());
        let mut conn = sqlx::PgConnection::connect(&url).await.unwrap();
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "DROP SCHEMA IF EXISTS {schema} CASCADE; CREATE SCHEMA {schema}; SET search_path TO {schema}, public"
        )))
        .execute(&mut conn)
        .await
        .unwrap();

        let (before, v16) = (&MIGRATIONS[..15], &MIGRATIONS[15]);
        assert_eq!(v16.version, 16, "the sixteenth migration is v16");
        for m in before {
            sqlx::raw_sql(m.sql).execute(&mut conn).await.unwrap();
        }
        sqlx::raw_sql(
            "INSERT INTO domain(id, name, path) OVERRIDING SYSTEM VALUE \
                 VALUES (1,'home','/tmp/h'), (2,'eng','/tmp/e'); \
             INSERT INTO domain_spelling(spelling, domain_id) VALUES ('home',1), ('eng',2) \
                 ON CONFLICT DO NOTHING; \
             INSERT INTO engram(id, domain_id, path, permalink, title) OVERRIDING SYSTEM VALUE \
                 VALUES (1,1,'foo.md','foo','Foo'), \
                        (2,1,'typo-foo.md','typo-foo','typo:Foo'), \
                        (3,1,'src.md','src','Src'), \
                        (4,2,'runbook.md','runbook','Runbook'), \
                        (5,1,'runbook.md','runbook','Runbook'), \
                        (6,2,'typo-foo.md','typo-foo','typo:Foo'); \
             INSERT INTO link(id, engram_id, domain_id, line, to_target, to_domain, to_raw, to_id) \
                 OVERRIDING SYSTEM VALUE \
                 VALUES (1,3,1,1,'Foo','typo','typo:Foo',1), \
                        (2,3,1,2,'Foo','typo','typo:Foo',2), \
                        (3,3,1,3,'Runbook','eng','eng:Runbook',4), \
                        (4,3,1,4,'Foo',NULL,'Foo',1), \
                        (5,3,1,5,'Runbook','ops',NULL,5), \
                        (6,3,1,7,'Foo','typo','typo:Foo',6); \
             INSERT INTO relation(id, engram_id, domain_id, line, rel_type, to_target, to_domain, to_raw, to_id) \
                 OVERRIDING SYSTEM VALUE \
                 VALUES (1,3,1,6,'relates_to','Runbook','ops','ops:Runbook',5);",
        )
        .execute(&mut conn)
        .await
        .unwrap();

        sqlx::raw_sql(v16.sql).execute(&mut conn).await.unwrap();
        async fn bound(conn: &mut PgConnection, table: &str, id: i64) -> i64 {
            let row: (i64,) = sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "SELECT COALESCE(to_id, 0) FROM {table} WHERE id={id}"
            )))
            .fetch_one(&mut *conn)
            .await
            .unwrap();
            row.0
        }
        assert_eq!(
            bound(&mut conn, "link", 1).await,
            0,
            "the bare target at home is unbound"
        );
        assert_eq!(
            bound(&mut conn, "link", 2).await,
            2,
            "the whole text at home stays bound"
        );
        assert_eq!(
            bound(&mut conn, "link", 3).await,
            4,
            "a known prefix keeps its binding"
        );
        assert_eq!(
            bound(&mut conn, "link", 4).await,
            1,
            "a bare reference keeps its binding"
        );
        assert_eq!(
            bound(&mut conn, "link", 5).await,
            0,
            "a row with no raw text is unbound"
        );
        assert_eq!(
            bound(&mut conn, "link", 6).await,
            0,
            "the whole text matched in another domain is not the whole text at home"
        );
        assert_eq!(
            bound(&mut conn, "relation", 1).await,
            0,
            "relations are unbound too"
        );

        for table in ["relation", "link"] {
            sqlx::raw_sql(sqlx::AssertSqlSafe(crate::store::resolve_pending_sql(
                table, "1",
            )))
            .execute(&mut conn)
            .await
            .unwrap();
        }
        assert_eq!(
            bound(&mut conn, "link", 1).await,
            2,
            "rebound to the whole text at home"
        );
        assert_eq!(bound(&mut conn, "link", 5).await, 0);
        assert_eq!(bound(&mut conn, "link", 6).await, 2);
        assert_eq!(bound(&mut conn, "relation", 1).await, 0);

        sqlx::raw_sql(v16.sql).execute(&mut conn).await.unwrap();
        assert_eq!(
            bound(&mut conn, "link", 1).await,
            2,
            "a replay changes nothing"
        );
        assert_eq!(bound(&mut conn, "link", 3).await, 4);

        sqlx::raw_sql(sqlx::AssertSqlSafe(format!("DROP SCHEMA {schema} CASCADE")))
            .execute(&mut conn)
            .await
            .unwrap();
    }

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

    /// The v14 move of the body out of `engram`, over a database carrying a
    /// draft beside its base row - the Turso v15 migration's twin.
    ///
    /// Every body comes through at the id it was written under, base and draft
    /// alike, there is one `engram_content` row per engram, and the column the
    /// body used to sit in is gone. The migration is applied twice, because the
    /// version stamp is a statement of its own and a crash between the two
    /// replays it: it has to converge rather than wedge on a table that is
    /// already there or a column that is already dropped.
    ///
    /// Runs only when `CRYSTALLINE_TEST_POSTGRES_URL` is set, the same gate the
    /// parity suite uses.
    #[tokio::test]
    async fn v14_moves_every_body_into_its_own_table() {
        let Ok(url) = std::env::var("CRYSTALLINE_TEST_POSTGRES_URL") else {
            return;
        };
        if url.is_empty() {
            return;
        }
        let schema = format!("mig14_{}", std::process::id());
        let mut conn = sqlx::PgConnection::connect(&url).await.unwrap();
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "DROP SCHEMA IF EXISTS {schema} CASCADE; CREATE SCHEMA {schema}; SET search_path TO {schema}, public"
        )))
        .execute(&mut conn)
        .await
        .unwrap();

        // A database at v13: everything up to but not including the move.
        for m in &MIGRATIONS[..13] {
            sqlx::raw_sql(m.sql).execute(&mut conn).await.unwrap();
        }
        assert_eq!(
            MIGRATIONS[13].version, 14,
            "the fourteenth migration is v14"
        );
        // Bodies of several sizes, the largest past the TOAST threshold, and a
        // draft beside the base row at the same path.
        let big = "x".repeat(200_000);
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "INSERT INTO domain(name, path) VALUES ('d','/tmp/d'); \
             INSERT INTO engram(domain_id, path, permalink, content, actor) \
             SELECT id, 'a.md', 'a', 'short body', '' FROM domain WHERE name='d'; \
             INSERT INTO engram(domain_id, path, permalink, content, actor) \
             SELECT id, 'b.md', 'b', '{big}', '' FROM domain WHERE name='d'; \
             INSERT INTO engram(domain_id, path, permalink, content, actor) \
             SELECT id, 'a.md', 'a', 'alice''s draft', 'alice' FROM domain WHERE name='d'"
        )))
        .execute(&mut conn)
        .await
        .unwrap();

        sqlx::raw_sql(MIGRATIONS[13].sql)
            .execute(&mut conn)
            .await
            .unwrap();
        sqlx::raw_sql(MIGRATIONS[13].sql)
            .execute(&mut conn)
            .await
            .expect("v14 applies twice");

        let paired: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM engram e JOIN engram_content ec ON ec.engram_id=e.id",
        )
        .fetch_one(&mut conn)
        .await
        .unwrap();
        let rows: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM engram")
            .fetch_one(&mut conn)
            .await
            .unwrap();
        assert_eq!(
            paired.0, rows.0,
            "one body row per engram row after the move"
        );

        let base: (String,) = sqlx::query_as(
            "SELECT ec.content FROM engram e JOIN engram_content ec ON ec.engram_id=e.id \
             WHERE e.path='a.md' AND e.actor=''",
        )
        .fetch_one(&mut conn)
        .await
        .unwrap();
        assert_eq!(base.0, "short body", "the base body came through whole");
        let draft: (String,) = sqlx::query_as(
            "SELECT ec.content FROM engram e JOIN engram_content ec ON ec.engram_id=e.id \
             WHERE e.path='a.md' AND e.actor='alice'",
        )
        .fetch_one(&mut conn)
        .await
        .unwrap();
        assert_eq!(
            draft.0, "alice's draft",
            "and the draft's is its own, at its own id"
        );
        let long: (i64,) = sqlx::query_as(
            "SELECT length(ec.content)::bigint FROM engram e JOIN engram_content ec \
             ON ec.engram_id=e.id WHERE e.path='b.md'",
        )
        .fetch_one(&mut conn)
        .await
        .unwrap();
        assert_eq!(
            long.0,
            big.len() as i64,
            "a body past the TOAST threshold too"
        );

        let gone = sqlx::query_as::<_, (String,)>("SELECT content FROM engram LIMIT 1")
            .fetch_optional(&mut conn)
            .await;
        assert!(
            gone.is_err(),
            "the column the body used to sit in is gone: {gone:?}"
        );

        assert!(
            WIPE_TABLES.contains(&"engram_content"),
            "a wipe that leaves the bodies behind fails on the foreign key"
        );

        sqlx::raw_sql(sqlx::AssertSqlSafe(format!("DROP SCHEMA {schema} CASCADE")))
            .execute(&mut conn)
            .await
            .unwrap();
    }

    /// v15 against domains written before it: every existing domain row is
    /// backfilled as its own spelling, and a second application is a no-op
    /// rather than a failure, since the ledger stamp is a separate statement
    /// here and a crash between the two replays the step.
    ///
    /// Runs only when `CRYSTALLINE_TEST_POSTGRES_URL` is set.
    #[tokio::test]
    async fn v15_backfills_every_domain_as_its_own_spelling() {
        let Ok(url) = std::env::var("CRYSTALLINE_TEST_POSTGRES_URL") else {
            return;
        };
        if url.is_empty() {
            return;
        }
        let schema = format!("mig15_{}", std::process::id());
        let mut conn = sqlx::PgConnection::connect(&url).await.unwrap();
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "DROP SCHEMA IF EXISTS {schema} CASCADE; CREATE SCHEMA {schema}; SET search_path TO {schema}, public"
        )))
        .execute(&mut conn)
        .await
        .unwrap();

        for m in &MIGRATIONS[..14] {
            sqlx::raw_sql(m.sql).execute(&mut conn).await.unwrap();
        }
        assert_eq!(MIGRATIONS[14].version, 15, "the fifteenth migration is v15");
        sqlx::raw_sql(
            "INSERT INTO domain(name, path) VALUES ('eng','/tmp/eng'),('ops','/tmp/ops')",
        )
        .execute(&mut conn)
        .await
        .unwrap();

        sqlx::raw_sql(MIGRATIONS[14].sql)
            .execute(&mut conn)
            .await
            .unwrap();
        sqlx::raw_sql(MIGRATIONS[14].sql)
            .execute(&mut conn)
            .await
            .expect("v15 applies twice");

        let own: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM domain_spelling s JOIN domain d \
             ON d.id=s.domain_id AND d.name=s.spelling",
        )
        .fetch_one(&mut conn)
        .await
        .unwrap();
        assert_eq!(
            own.0, 2,
            "both domain rows are backfilled as their own spelling"
        );
        let all: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM domain_spelling")
            .fetch_one(&mut conn)
            .await
            .unwrap();
        assert_eq!(all.0, 2, "and nothing else, even after the replay");

        let at = |t: &str| WIPE_TABLES.iter().position(|w| *w == t);
        assert!(
            at("domain_spelling").is_some() && at("domain_spelling") < at("domain"),
            "the spellings are wiped before the domains they point at, or the foreign key refuses"
        );

        sqlx::raw_sql(sqlx::AssertSqlSafe(format!("DROP SCHEMA {schema} CASCADE")))
            .execute(&mut conn)
            .await
            .unwrap();
    }

    /// The Turso twin's Postgres counterpart: a database stamped one version
    /// above the newest migration this binary knows must refuse rather than
    /// silently report back as current. Runs `apply` itself (not raw SQL for
    /// the buildup), since it is the entry point the guard has to protect and
    /// the one every backend caller actually goes through.
    ///
    /// Runs only when `CRYSTALLINE_TEST_POSTGRES_URL` is set, the same gate
    /// every other test in this module uses.
    #[tokio::test]
    async fn a_schema_stamped_above_the_newest_known_migration_is_refused() {
        let Ok(url) = std::env::var("CRYSTALLINE_TEST_POSTGRES_URL") else {
            return;
        };
        if url.is_empty() {
            return;
        }
        let schema = format!("mig_toonew_{}", std::process::id());
        let mut conn = sqlx::PgConnection::connect(&url).await.unwrap();
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "DROP SCHEMA IF EXISTS {schema} CASCADE; CREATE SCHEMA {schema}; SET search_path TO {schema}, public"
        )))
        .execute(&mut conn)
        .await
        .unwrap();

        apply(&mut conn).await.unwrap();
        let known = MIGRATIONS.last().unwrap().version;

        sqlx::query("INSERT INTO schema_migration (version, applied_at) VALUES ($1, $2)")
            .bind(known + 1)
            .bind(chrono::Utc::now().to_rfc3339())
            .execute(&mut conn)
            .await
            .unwrap();

        let err = apply(&mut conn)
            .await
            .expect_err("a schema newer than this binary knows is refused, not applied over");
        match err {
            IndexError::SchemaTooNew { found, known: k } => {
                assert_eq!(found, known + 1, "the recorded version is reported back");
                assert_eq!(k, known, "alongside the newest version this binary ships");
            }
            other => panic!("expected SchemaTooNew, got: {other}"),
        }
        assert!(
            !err.is_locked_by_another_process(),
            "a version mismatch is not a held file, so the owner's startup retry must not \
             wait it out: {err}"
        );

        sqlx::raw_sql(sqlx::AssertSqlSafe(format!("DROP SCHEMA {schema} CASCADE")))
            .execute(&mut conn)
            .await
            .unwrap();
    }

    /// The Turso v18 upgrade test's twin: a schema at v16 with a domain row
    /// in it, built and stamped migration by migration as `apply` would have,
    /// then opened by the store (which runs v17) and synced once.
    #[tokio::test]
    async fn a_schema_from_before_v17_reparses_on_its_first_sync() {
        let Ok(url) = std::env::var("CRYSTALLINE_TEST_POSTGRES_URL") else {
            return;
        };
        if url.is_empty() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("d");
        let schema = format!("mig_reparse_{}", std::process::id());
        let mut conn = sqlx::PgConnection::connect(&url).await.unwrap();
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "DROP SCHEMA IF EXISTS {schema} CASCADE; CREATE SCHEMA {schema}; SET search_path TO {schema}, public"
        )))
        .execute(&mut conn)
        .await
        .unwrap();
        sqlx::raw_sql(
            "CREATE TABLE IF NOT EXISTS schema_migration (version BIGINT PRIMARY KEY, applied_at TEXT NOT NULL)",
        )
        .execute(&mut conn)
        .await
        .unwrap();
        assert_eq!(
            MIGRATIONS[16].version, 17,
            "the seventeenth migration is v17"
        );
        for m in &MIGRATIONS[..16] {
            sqlx::raw_sql(m.sql).execute(&mut conn).await.unwrap();
            sqlx::query("INSERT INTO schema_migration (version, applied_at) VALUES ($1, $2)")
                .bind(m.version)
                .bind(chrono::Utc::now().to_rfc3339())
                .execute(&mut conn)
                .await
                .unwrap();
        }
        sqlx::query("INSERT INTO domain(name, path, kind) VALUES ('d', $1, 'file')")
            .bind(root.to_string_lossy().into_owned())
            .execute(&mut conn)
            .await
            .unwrap();
        drop(conn);

        let store = crate::PostgresStore::open_in_schema(&url, &schema)
            .await
            .unwrap();
        crate::sync::upgrade_fixture::first_sync_after_the_upgrade_reparses(&store, &root).await;
        store.drop_schema().await.unwrap();
    }

    /// A schema stamped at v17 holding one domain and one engram row per
    /// `(permalink, actor, metadata)`, at `<permalink>.md`; answers each
    /// row's id.
    async fn a_v17_schema_with(
        url: &str,
        schema: &str,
        engrams: &[(&str, &str, &serde_json::Value)],
    ) -> Vec<i64> {
        let mut conn = sqlx::PgConnection::connect(url).await.unwrap();
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "DROP SCHEMA IF EXISTS {schema} CASCADE; CREATE SCHEMA {schema}; SET search_path TO {schema}, public"
        )))
        .execute(&mut conn)
        .await
        .unwrap();
        sqlx::raw_sql(
            "CREATE TABLE IF NOT EXISTS schema_migration (version BIGINT PRIMARY KEY, applied_at TEXT NOT NULL)",
        )
        .execute(&mut conn)
        .await
        .unwrap();
        assert_eq!(
            MIGRATIONS[17].version, 18,
            "the eighteenth migration is v18"
        );
        for m in &MIGRATIONS[..17] {
            sqlx::raw_sql(m.sql).execute(&mut conn).await.unwrap();
            sqlx::query("INSERT INTO schema_migration (version, applied_at) VALUES ($1, $2)")
                .bind(m.version)
                .bind(chrono::Utc::now().to_rfc3339())
                .execute(&mut conn)
                .await
                .unwrap();
        }
        let (domain,): (i64,) = sqlx::query_as(
            "INSERT INTO domain(name, path, kind) VALUES ('d', '/tmp/d', 'file') RETURNING id",
        )
        .fetch_one(&mut conn)
        .await
        .unwrap();
        let mut ids = Vec::new();
        for (permalink, actor, metadata) in engrams {
            let (id,): (i64,) = sqlx::query_as(
                "INSERT INTO engram(domain_id, path, permalink, metadata, actor) \
                 VALUES ($1, $2 || '.md', $2, $3::jsonb, $4) RETURNING id",
            )
            .bind(domain)
            .bind(*permalink)
            .bind(metadata.to_string())
            .bind(*actor)
            .fetch_one(&mut conn)
            .await
            .unwrap();
            ids.push(id);
        }
        ids
    }

    /// One engram's rows, as `(key, value)` in byte order.
    async fn meta_rows_of(store: &crate::PostgresStore, id: i64) -> Vec<(String, String)> {
        use crate::store::Store;
        store
            .meta_values()
            .await
            .unwrap()
            .into_iter()
            .filter(|(e, _, _)| e.0 == id)
            .map(|(_, k, v)| (k, v))
            .collect()
    }

    /// The Turso v19 test's twin for v18, gated like the tests beside it.
    /// JSONB refuses metadata that does not parse at insert time, so that
    /// row has no twin here. The rows are compared in byte order
    /// (`COLLATE "C"`), the order `meta_value_rows` sorts in.
    ///
    /// The backfill reads `metadata::text` back from JSONB, whose numbers are
    /// `numeric`: the one place where the migration's text could differ from
    /// the writer's. `1e-7`, `2.0`, `1e20` (`1e+20`) and `u64::MAX` come back as the
    /// same text a fresh write gives. `-0.0` does not: `numeric` has no
    /// negative zero, so an upgraded index holds `0.0` where a fresh write
    /// holds `-0.0`. That is pinned below as a known, harmless difference.
    #[tokio::test]
    async fn v18_fills_engram_meta_value_from_the_stored_metadata() {
        let Ok(url) = std::env::var("CRYSTALLINE_TEST_POSTGRES_URL") else {
            return;
        };
        if url.is_empty() {
            return;
        }
        let schema = format!("mig_meta_{}", std::process::id());
        let base = serde_json::json!({
            "sources": ["a \"q\" \\ b", "line\nbreak", "ünïcode", "a \"q\" \\ b"],
            "numbers": [-3, i64::MAX, 1.5, 1, "1", 1e-7, 2.0, 1e20, u64::MAX],
            "flags": [true, 1],
            "verified": [{"by": "jordi", "at": "2026-01-01T00:00:00Z"}],
            "kind": "anchor",
        });
        let draft = serde_json::json!({ "sources": ["draft-only"] });
        let zero = serde_json::json!({ "z": -0.0 });
        let ids = a_v17_schema_with(
            &url,
            &schema,
            &[
                ("p0", "", &base),
                ("p1", "alice", &draft),
                ("p2", "", &zero),
            ],
        )
        .await;

        let store = crate::PostgresStore::open_in_schema(&url, &schema)
            .await
            .unwrap();
        assert_eq!(
            meta_rows_of(&store, ids[0]).await,
            crate::store::meta_value_rows(&base),
            "the base row"
        );
        assert_eq!(
            meta_rows_of(&store, ids[1]).await,
            crate::store::meta_value_rows(&draft),
            "a draft gets its rows too"
        );
        let base_rows = meta_rows_of(&store, ids[0]).await;
        for value in ["1e-7", "2.0", "1e+20", "18446744073709551615"] {
            assert!(
                base_rows.contains(&("numbers".to_string(), value.to_string())),
                "numbers = {value} in {base_rows:?}"
            );
        }
        assert_eq!(
            crate::store::meta_value_rows(&zero),
            vec![("z".to_string(), "-0.0".to_string())],
            "a fresh write keeps the negative zero"
        );
        assert_eq!(
            meta_rows_of(&store, ids[2]).await,
            vec![("z".to_string(), "0.0".to_string())],
            "JSONB numeric has no negative zero, so the migration stores 0.0"
        );
        store.drop_schema().await.unwrap();
    }

    /// v18 replayed: a database where an earlier attempt ran the DDL and the
    /// backfill but never stamped the step, and where a stray row sits in the
    /// table. The next open runs the step again, and the result is exactly
    /// the rows of the stored metadata, the stray row gone.
    #[tokio::test]
    async fn v18_replayed_over_a_filled_table_converges() {
        let Ok(url) = std::env::var("CRYSTALLINE_TEST_POSTGRES_URL") else {
            return;
        };
        if url.is_empty() {
            return;
        }
        let schema = format!("mig_meta_replay_{}", std::process::id());
        let metadata = serde_json::json!({ "sources": ["a", "b"], "n": 7 });
        let ids = a_v17_schema_with(&url, &schema, &[("p0", "", &metadata)]).await;
        {
            let mut conn = sqlx::PgConnection::connect(&url).await.unwrap();
            sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
                "SET search_path TO {schema}, public"
            )))
            .execute(&mut conn)
            .await
            .unwrap();
            sqlx::raw_sql(MIGRATIONS[17].sql)
                .execute(&mut conn)
                .await
                .unwrap();
            backfill(&mut conn, 18).await.unwrap();
            sqlx::query(
                "INSERT INTO engram_meta_value(engram_id, key, value) VALUES ($1, 'stray', '\"x\"')",
            )
            .bind(ids[0])
            .execute(&mut conn)
            .await
            .unwrap();
            let (stamped,): (i64,) =
                sqlx::query_as("SELECT COALESCE(MAX(version), 0) FROM schema_migration")
                    .fetch_one(&mut conn)
                    .await
                    .unwrap();
            assert_eq!(stamped, 17, "the step ran but was never stamped");
        }

        let store = crate::PostgresStore::open_in_schema(&url, &schema)
            .await
            .unwrap();
        assert_eq!(
            meta_rows_of(&store, ids[0]).await,
            crate::store::meta_value_rows(&metadata),
            "the replay clears the table and fills it again"
        );
        store.drop_schema().await.unwrap();
    }

    /// The Turso `v19_backfill_cost_at_50k` twin: v18 over a schema of
    /// 50,000 engrams with five `sources` anchors each, timed from the open
    /// that runs the backfill. Ignored and gated like the tests beside it.
    #[tokio::test]
    #[ignore = "perf evidence: run by hand with --ignored --nocapture"]
    async fn v18_backfill_cost_at_50k() {
        use crate::store::Store;
        const ENGRAMS: i64 = 50_000;
        let Ok(url) = std::env::var("CRYSTALLINE_TEST_POSTGRES_URL") else {
            return;
        };
        if url.is_empty() {
            return;
        }
        let schema = format!("mig_meta_perf_{}", std::process::id());
        a_v17_schema_with(&url, &schema, &[]).await;
        {
            let mut conn = sqlx::PgConnection::connect(&url).await.unwrap();
            sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
                "SET search_path TO {schema}, public"
            )))
            .execute(&mut conn)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO engram(domain_id, path, permalink, metadata, actor) \
                 SELECT d.id, 'n' || i || '.md', 'note-' || i, \
                        jsonb_build_object('sources', jsonb_build_array( \
                            'notedown://jordi/n' || i || '/p0#b0', \
                            'notedown://jordi/n' || i || '/p0#b1', \
                            'notedown://jordi/n' || i || '/p0#b2', \
                            'notedown://jordi/n' || i || '/p0#b3', \
                            'notedown://jordi/shared/p' || (i % 1000) || '#b0')), '' \
                 FROM domain d, generate_series(0, $1 - 1) AS i",
            )
            .bind(ENGRAMS)
            .execute(&mut conn)
            .await
            .unwrap();
        }
        let started = std::time::Instant::now();
        let store = crate::PostgresStore::open_in_schema(&url, &schema)
            .await
            .unwrap();
        let ms = started.elapsed().as_millis();
        let rows = store.meta_values().await.unwrap().len();
        assert_eq!(rows, 5 * ENGRAMS as usize, "five rows an engram");
        eprintln!(
            "PERF v18 backfill 50k: open with the migration {ms} ms, {rows} rows, {:.1} us per engram",
            ms as f64 * 1000.0 / ENGRAMS as f64
        );
        store.drop_schema().await.unwrap();
    }
}
