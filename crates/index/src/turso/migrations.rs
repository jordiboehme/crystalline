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
    Migration {
        version: 15,
        label: "engram body in its own table",
        sql: SCHEMA_V15,
    },
    Migration {
        version: 16,
        label: "domain spellings",
        sql: SCHEMA_V16,
    },
    Migration {
        version: 17,
        label: "unbind references an unknown prefix bound at home",
        sql: SCHEMA_V17,
    },
    Migration {
        version: 18,
        label: "contradiction scores",
        sql: SCHEMA_V18,
    },
    Migration {
        version: 19,
        label: "metadata values",
        sql: SCHEMA_V19,
    },
    Migration {
        version: 20,
        label: "reflection index",
        sql: SCHEMA_V20,
    },
    Migration {
        version: 21,
        label: "reflection assets",
        sql: SCHEMA_V21,
    },
    Migration {
        version: 22,
        label: "reflection chunks",
        sql: SCHEMA_V22,
    },
    Migration {
        version: 23,
        label: "reflection chunks by domain",
        sql: SCHEMA_V23,
    },
    Migration {
        version: 24,
        label: "engram rows under the Reflection folder",
        sql: SCHEMA_V24,
    },
    Migration {
        version: 25,
        label: "domain host move",
        sql: SCHEMA_V25,
    },
    Migration {
        version: 26,
        label: "wikilink display text",
        sql: SCHEMA_V26,
    },
    Migration {
        version: 27,
        label: "automation lease and runs",
        sql: SCHEMA_V27,
    },
    Migration {
        version: 28,
        label: "automation lease pull request and written paths",
        sql: SCHEMA_V28,
    },
    Migration {
        version: 29,
        label: "reflection disclosures",
        sql: SCHEMA_V29,
    },
    Migration {
        version: 30,
        label: "wikilink whole target text",
        sql: SCHEMA_V30,
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
// runs over it. The 384 matches the models in `embed::models`, all of which are
// 384 wide; turso 0.6.1 does not enforce the declared width, so other providers
// (whose dims are recorded in the `dims` column and validated in Rust) store
// their vectors here too. The chunk table is a derived, rebuildable cache, so
// recreating it loses nothing that a resync plus embed pass does not restore.
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

// The body moves out of `engram` into a table of its own, one row per engram,
// keyed by the engram's id.
//
// Why: turso materializes a whole row payload - overflow chain included - for
// any column read, so every seek into `engram` cost the body of that engram
// even when the statement wanted an 8-byte column. Phase 1 of the semantic scan
// seeks the parent row once per chunk to evaluate the actor screen, so a 7 MB
// body with 4,287 chunks was copied 4,287 times for one search: a 254 MiB index
// took two minutes a query. With the body in its own table those seeks read a
// row of about a kilobyte, and the body is read once per hit, by the hydrate
// that actually wants it. Measured on a throwaway bench (30 engrams of 2 MB,
// 300 chunks each): 1.60 s of phase 1 before, 13 ms after.
//
// Nothing else moves: `metadata`, `description` and `title` are bounded by what
// a frontmatter holds.
//
// The shape is v13's create-copy-swap, for the same reason - a column cannot be
// dropped in place in this dialect - and with the same two consequences. The
// ids are carried verbatim, which is what keeps `observation`, `relation`,
// `link`, `engram_tag`, `chunk` and `engram_content` itself pointing at the
// rows they already point at; and every index the old table carried is
// recreated, all seven of them, because they die with the dropped table. The
// copy into `engram_content` happens BEFORE the swap, since it is the doomed
// table it reads the bodies out of.
const SCHEMA_V15: &str = r#"
CREATE TABLE engram_content (
    engram_id INTEGER PRIMARY KEY REFERENCES engram(id),
    content TEXT NOT NULL DEFAULT ''
);

INSERT INTO engram_content (engram_id, content) SELECT id, content FROM engram;

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
    metadata TEXT NOT NULL DEFAULT '{}',
    mtime INTEGER NOT NULL DEFAULT 0,
    size INTEGER NOT NULL DEFAULT 0,
    sha256 TEXT NOT NULL DEFAULT '',
    actor TEXT NOT NULL DEFAULT '',
    tombstone INTEGER NOT NULL DEFAULT 0
);

INSERT INTO engram_new (id, domain_id, path, permalink, title, engram_type, status,
    recorded_at, valid_from, valid_to, timestamp, description, metadata,
    mtime, size, sha256, actor, tombstone)
SELECT id, domain_id, path, permalink, title, engram_type, status,
    recorded_at, valid_from, valid_to, timestamp, description, metadata,
    mtime, size, sha256, actor, tombstone
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

// Every spelling a domain answers to - its local name, its canonical name, its
// aliases - mapped to the domain row, so the resolve pass reads the target
// domain of a reference through one primary-key seek whatever spelling the
// reference used. Every existing row is backfilled as its own spelling, which
// is exactly what the resolve pass read before (`domain.name`), so nothing
// resolves differently until a canonical name or an alias is recorded. The
// cascade is declared for the record; this connection does not enforce foreign
// keys, so the store deletes spellings by hand where it deletes their rows.
//
// The two partial indexes serve the reads that ask by spelling rather than by
// row: the references a changed spelling unbinds, the engrams a rename has to
// respell and one domain's references by prefix for the sweep. Partial because
// most references carry no domain prefix at all, and those rows never need it.
const SCHEMA_V16: &str = r#"
CREATE TABLE domain_spelling (
    spelling TEXT PRIMARY KEY,
    domain_id INTEGER NOT NULL REFERENCES domain(id) ON DELETE CASCADE
);
CREATE INDEX idx_domain_spelling_domain ON domain_spelling(domain_id);
INSERT INTO domain_spelling (spelling, domain_id) SELECT name, id FROM domain;
CREATE INDEX idx_relation_to_domain ON relation(to_domain) WHERE to_domain IS NOT NULL;
CREATE INDEX idx_link_to_domain ON link(to_domain) WHERE to_domain IS NOT NULL;
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
// Idempotent: a second run finds nothing the first left behind.
const SCHEMA_V17: &str = r#"
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

// The contradiction check's persisted output. `contradiction_pair` is one row
// per scored engram pair and says "up to date at these checksums";
// `contradiction` holds the line pairs that cleared the store floor, both
// reading orders. Both carry `domain_id`, so a domain clear and the sweep's
// read never name the `engram` table. The cascades are declared for the
// record; this connection does not enforce foreign keys, so `delete_engram`
// and `clear_domain` delete by hand. Never in `delete_children`: that runs on
// every upsert, and `reindex --full` would erase every score.
//
// `idx_contradiction_pair_domain` carries the pair columns after the seek
// key so it also serves the read's `ORDER BY engram_a, engram_b`; on
// `(domain_id, model)` alone the planner scans the primary key for the
// order instead, which `tests/it/plans.rs` rejects.
//
// `idx_contradiction_pair_model` serves `Store::scored_pair_count`'s
// `WHERE model=?1`, which names no domain: `idx_contradiction_pair_domain`
// leads with `domain_id`, so it cannot seek on `model` alone and that count
// would otherwise scan the whole table. `ctl status` and a daemon-mode
// `doctor` run call it on every request.
//
// `delete_engram` deletes by `engram_a=?1 OR engram_b=?1` on both tables.
// Each primary key leads with `engram_a` and serves one half of the OR; the
// two `engram_b` indexes serve the other, so a delete never walks a table.
//
// `observation_vector` caches the embedding of each observation line the
// contradiction check pairs, keyed by the embedding model and the row hash of
// the folded text, so a text shared by many engrams is embedded once and an
// edit embeds only the lines whose text changed. It holds no text at all
// (hash, width and vector), and no engram id, so no per-engram statement ever
// reads it: it is derived and disposable, pruned by the daemon when no current
// line uses a hash, cleared when the check is switched off and by `wipe`, and
// kept by `reindex --full` because a vector keyed by its text stays valid.
// The primary key serves every read, which all name the model and one hash
// (a hash list on postgres).
//
// `domain.parse_generation` is the parser generation
// (`crystalline_core::PARSE_GENERATION`) the domain's rows were last derived
// with. An index this migration upgrades has every row at 0, older than any
// parser that records one, so the first sync after the upgrade reparses each
// domain once and stamps it; a domain row created later is stamped with the
// current generation on insert, so a fresh index reparses nothing. Unreleased
// when it joined this migration, so it rides here rather than in one of its own.
const SCHEMA_V18: &str = r#"
CREATE TABLE contradiction_pair (
    domain_id INTEGER NOT NULL REFERENCES domain(id) ON DELETE CASCADE,
    engram_a INTEGER NOT NULL REFERENCES engram(id) ON DELETE CASCADE,
    engram_b INTEGER NOT NULL REFERENCES engram(id) ON DELETE CASCADE,
    checksum_a TEXT NOT NULL,
    checksum_b TEXT NOT NULL,
    cosine REAL NOT NULL,
    model TEXT NOT NULL,
    scored_at TEXT NOT NULL,
    PRIMARY KEY (engram_a, engram_b, model)
);
CREATE INDEX idx_contradiction_pair_domain ON contradiction_pair(domain_id, model, engram_a, engram_b);
CREATE INDEX idx_contradiction_pair_model ON contradiction_pair(model);

CREATE TABLE contradiction (
    domain_id INTEGER NOT NULL REFERENCES domain(id) ON DELETE CASCADE,
    engram_a INTEGER NOT NULL REFERENCES engram(id) ON DELETE CASCADE,
    engram_b INTEGER NOT NULL REFERENCES engram(id) ON DELETE CASCADE,
    line_a INTEGER NOT NULL,
    line_b INTEGER NOT NULL,
    hash_a TEXT NOT NULL,
    hash_b TEXT NOT NULL,
    model TEXT NOT NULL,
    score_ab REAL NOT NULL,
    score_ba REAL NOT NULL,
    similarity REAL NOT NULL DEFAULT 0,
    period INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (engram_a, engram_b, hash_a, hash_b, model)
);
CREATE INDEX idx_contradiction_engram_b ON contradiction(engram_b);
CREATE INDEX idx_contradiction_pair_engram_b ON contradiction_pair(engram_b);
CREATE INDEX idx_contradiction_domain ON contradiction(domain_id, model);

CREATE TABLE observation_vector (
    model TEXT NOT NULL,
    hash TEXT NOT NULL,
    dims INTEGER NOT NULL,
    vector BLOB NOT NULL,
    PRIMARY KEY (model, hash)
);

ALTER TABLE domain ADD COLUMN parse_generation INTEGER NOT NULL DEFAULT 0;
"#;

// One row per element of a list and one per scalar of every frontmatter key
// that is not promoted to a column, so `$contains` and `$contains_any` seek
// `(key, value)` instead of reading every engram's `metadata`. `value` is the
// compact JSON of the element (`crystalline_core::meta_value_text`), so `"1"`,
// `1` and `true` stay three values. The primary key drops a list's duplicates
// and serves the per-engram delete; the index carries `engram_id` after the
// seek key, so a filter is answered from the index alone. The cascade is
// declared for the record: this connection does not enforce foreign keys, so
// `delete_children` and `clear_domain` delete by hand. The rows are filled
// from the stored `metadata` by `backfill`, in this step's transaction, with
// the function every write uses.
const SCHEMA_V19: &str = r#"
CREATE TABLE engram_meta_value (
    engram_id INTEGER NOT NULL REFERENCES engram(id) ON DELETE CASCADE,
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    PRIMARY KEY (engram_id, key, value)
);
CREATE INDEX idx_engram_meta_value_key_value ON engram_meta_value(key, value, engram_id);
"#;

// The Reflection's own index: one row per reflection (and PACT.md) under a
// domain's `reflections/`, read by Reflection-mode search and the tree. Beside
// the engram tables and never joined to them, so nothing that reads engrams -
// search_engrams, evolve, the graph - can ever see a reflection. The unique
// index serves every statement (`WHERE domain_id=?1 [AND path=?2]
// ORDER BY path`).
const SCHEMA_V20: &str = r#"
CREATE TABLE reflection (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    domain_id INTEGER NOT NULL REFERENCES domain(id) ON DELETE CASCADE,
    path TEXT NOT NULL,
    title TEXT NOT NULL DEFAULT '',
    content TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    size INTEGER NOT NULL,
    modified TEXT NOT NULL,
    UNIQUE(domain_id, path)
);
"#;

// The Reflection's files: one row per allowlisted file under a domain's
// `reflections/assets/`, keyed by its path inside `reflections/`. The same
// columns as `attachment` and never joined to it, so no engram path can see
// one. The unique index serves every statement.
const SCHEMA_V21: &str = r#"
CREATE TABLE reflection_asset (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    domain_id INTEGER NOT NULL REFERENCES domain(id) ON DELETE CASCADE,
    path TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    mime TEXT NOT NULL,
    size INTEGER NOT NULL,
    modified TEXT NOT NULL,
    UNIQUE(domain_id, path)
);
"#;

// The Reflection's embedding chunks: one or more per `##` section of a page
// under `reflections/`, beside the engram `chunk` table and never joined to
// it, so no engram reader (search_engrams, lead vectors, the contradiction
// check, the graph) can meet one. Keyed by `(domain_id, path, seq)` with no
// foreign key to `reflection`: turso enforces none here, and a page move is a
// delete plus an upsert. The page stamps are blanked so the next walk reads
// every page once and chunks it (R58).
const SCHEMA_V22: &str = r#"
CREATE TABLE reflection_chunk (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    domain_id INTEGER NOT NULL REFERENCES domain(id),
    path TEXT NOT NULL,
    seq INTEGER NOT NULL DEFAULT 0,
    section_ord INTEGER NOT NULL DEFAULT 0,
    section TEXT,
    heading TEXT NOT NULL DEFAULT '',
    text TEXT NOT NULL DEFAULT '',
    text_hash TEXT NOT NULL DEFAULT '',
    model TEXT,
    dims INTEGER,
    embedding F32_BLOB(384)
);
CREATE INDEX idx_reflection_chunk_page ON reflection_chunk(domain_id, path, seq);
CREATE INDEX idx_reflection_chunk_model ON reflection_chunk(model);
UPDATE reflection SET modified='';
"#;

// One domain's reflection chunks of one model and width, the rows a reflection
// search reads: the vector statement and the per-domain coverage seek it, so
// a search reads its own domain's chunks and never every domain's chunks of
// the model (`idx_reflection_chunk_model` alone, filtered per row). That model
// index serves no statement any more (the backlog and the prune ask
// `model != ?`, which no index answers), so it goes, and a fresh install and
// an upgrade end with the same indexes. Both statements are guarded, so a
// replay changes nothing.
const SCHEMA_V23: &str = r#"
CREATE INDEX IF NOT EXISTS idx_reflection_chunk_domain_model ON reflection_chunk(domain_id, model, dims);
DROP INDEX IF EXISTS idx_reflection_chunk_model;
"#;

// The Reflection gate's probe (R82): a domain's base engram rows whose first
// path segment is `reflections/` in any letter case. The expression is the
// probe's own, byte for byte (`ENGRAM_ROWS_UNDER_REFLECTIONS_SQL`), so every
// gate is a seek on the full key and never a walk of the domain's rows. The
// expression leads: no other statement names it, so no other statement can
// seek a prefix of this index and leave the index it is pinned to (with
// `domain_id, actor` first it took the domain-scoped reads over from
// `idx_engram_domain` and `idx_engram_path_actor`). Guarded, so a replay
// changes nothing.
const SCHEMA_V24: &str = r#"
CREATE INDEX IF NOT EXISTS idx_engram_reflection_head ON engram(lower(substr(path, 1, 12)), domain_id, actor);
"#;

// The host move of a file domain on a shared database (Task 50), the
// Postgres v25 columns: the instance that last took the domain's host lock,
// and the unresolved move as JSON, NULL when there is none. A Turso index
// belongs to one instance, so nothing writes them here; the columns keep the
// two schemas the same shape. Nullable, no default, no backfill.
const SCHEMA_V25: &str = r#"
ALTER TABLE domain ADD COLUMN last_host_id TEXT;
ALTER TABLE domain ADD COLUMN last_host_label TEXT;
ALTER TABLE domain ADD COLUMN host_move TEXT;
"#;

// The display text of a `[[target|text]]` reference, beside `to_raw`, so the
// sweep can quote a repair with the bracket text exactly as the file holds
// it. NULL for a reference without one. Nullable, no default, no backfill:
// the parse generation moved in the same release, so every engram is parsed
// again and its rows are written with the column filled.
const SCHEMA_V26: &str = r#"
ALTER TABLE relation ADD COLUMN to_alias TEXT;
ALTER TABLE link ADD COLUMN to_alias TEXT;
"#;

// The automation loops (reflect and evolve run on their own): one lease per
// domain and loop, so only one runner works on a loop at a time, and the
// newest runs of each loop. `unshared_at_start`, `due_at_start` and
// `left_items` are JSON text. Times are TEXT ISO strings, compared lexically.
// Neither table cascades from `domain`: `wipe()` and the empty-row drop clear
// them by hand, as they do `domain_lock`.
const SCHEMA_V27: &str = r#"
CREATE TABLE automation_lease (
    domain_id INTEGER NOT NULL REFERENCES domain(id),
    loop_kind TEXT NOT NULL,
    instance_id TEXT NOT NULL,
    account TEXT,
    runner TEXT NOT NULL,
    token TEXT NOT NULL,
    taken_at TEXT NOT NULL,
    renewed_at TEXT NOT NULL,
    unshared_at_start TEXT NOT NULL,
    due_at_start TEXT NOT NULL,
    PRIMARY KEY(domain_id, loop_kind)
);

CREATE TABLE automation_run (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    domain_id INTEGER NOT NULL REFERENCES domain(id),
    loop_kind TEXT NOT NULL,
    started_at TEXT NOT NULL,
    ended_at TEXT NOT NULL,
    runner TEXT NOT NULL,
    account TEXT,
    outcome TEXT NOT NULL,
    done INTEGER NOT NULL,
    "left" INTEGER NOT NULL,
    judgment_waiting INTEGER NOT NULL,
    summary TEXT NOT NULL,
    pr_url TEXT,
    left_items TEXT NOT NULL
);
CREATE INDEX idx_automation_run_loop ON automation_run(domain_id, loop_kind, id);
"#;

// What a run shared and wrote, kept with its lease: the pull request it
// shared its changes in (null until it shares), and the paths its account
// wrote over MCP while it held the lease, as JSON text. Both go with the
// lease into the run record.
const SCHEMA_V28: &str = r#"
ALTER TABLE automation_lease ADD COLUMN pr_url TEXT;
ALTER TABLE automation_lease ADD COLUMN written TEXT NOT NULL DEFAULT '[]';
"#;

// What a person confirmed about a Reflection page: that a section may carry
// text from a domain fewer people may read than the page's own. One row per
// page, its records as JSON text the engine reads (who confirmed, when, for
// which source domains and which readers). Kept here and not in the page, so
// a confirmation can never arrive in a file edit, a pull or a page save, and
// the page's readers never see who else reads it. Does not cascade from
// `domain`: `wipe()` and the empty-row drop clear it by hand.
const SCHEMA_V29: &str = r#"
CREATE TABLE reflection_disclosure (
    domain_id INTEGER NOT NULL REFERENCES domain(id),
    path TEXT NOT NULL,
    records TEXT NOT NULL,
    PRIMARY KEY(domain_id, path)
);
"#;

// The target of a `[[target#heading]]` reference as 0.30.0 read it, the `#`
// and all, beside the split `to_target`, so an engram whose own title holds a
// `#` (`C# Basics`) is still found by its whole title before the text after
// the `#` is read as a heading. NULL when nothing was split off. Nullable, no
// default, no backfill: the parse generation moved in the same release, so
// every engram is parsed again and its rows are written with the column
// filled. The partial `lower(to_whole)` indexes carry the unbind a base write
// runs when a title that such a reference names whole appears
// (`unbind_shadowed_by_title_sql`).
const SCHEMA_V30: &str = r#"
ALTER TABLE relation ADD COLUMN to_whole TEXT;
ALTER TABLE link ADD COLUMN to_whole TEXT;
CREATE INDEX idx_relation_to_whole ON relation(lower(to_whole)) WHERE to_whole IS NOT NULL;
CREATE INDEX idx_link_to_whole ON link(lower(to_whole)) WHERE to_whole IS NOT NULL;
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

/// The tables cleared by `wipe()`, child rows first. `tag_alias`, `attachment`,
/// `domain_lock` and `domain_spelling` all reference `domain(id)`, so they are
/// cleared before `domain`; `attachment_blob` references `attachment`, so it
/// goes before it, and `engram_content` references `engram`, so it goes
/// before that. `contradiction` and `contradiction_pair` reference `engram`
/// and `domain`, so they go first. `observation_vector` references nothing
/// and leads the list. `engram_meta_value` references `engram`, so it goes
/// before it. `reflection` and `reflection_asset` reference `domain(id)` too,
/// and so does `reflection_chunk`, which goes before `reflection`.
/// `automation_lease` and `automation_run` reference `domain(id)` as well, and so does
/// `reflection_disclosure`.
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
    "reflection_chunk",
    "reflection",
    "reflection_asset",
    "domain_lock",
    "automation_lease",
    "automation_run",
    "reflection_disclosure",
    "domain_spelling",
    "domain",
];

/// Ensure the migration ledger exists, then apply every migration above the
/// recorded version. Returns the resulting schema version.
pub async fn apply(conn: &Connection) -> Result<i64> {
    apply_migrations(conn, MIGRATIONS).await
}

/// [`apply`] over a given list, so a test can hand it a migration that fails.
///
/// The migrations run with the connection's temp store in memory, and the
/// setting it had before is put back on every return, an error included. A
/// panic inside a migration, or the open's future dropped before it ends,
/// skips that restore and leaves the connection on memory. Several migrations
/// read and rewrite the same table (v7's `DELETE ... WHERE tag_id NOT IN
/// (SELECT ...)` is the plainest case), which the engine answers with an
/// ephemeral table. With the default temp store it creates each of those as a
/// fresh directory plus file under the process temp location, eagerly, even
/// when the table stays empty, so one store open made a run of directories in
/// the shared system temp folder. On Windows that create is refused now and
/// then with "permission denied", and the open failed with
/// `Migration("v7 ...: I/O error (tempdir): permission denied")`. What a
/// migration keeps in such a table is ids and short keys, so memory is the
/// right home for it. Inside that window the engine's sorters and hash joins
/// stay in memory too, so a migration that sorts or joins a large table holds
/// that work in RAM. Queries after the open keep the default: a runtime
/// sorter that outgrows its buffer must spill to disk, not to RAM (see
/// `research/2026-07-28-turso-sorter-spill.md`).
async fn apply_migrations(conn: &Connection, migrations: &[Migration]) -> Result<i64> {
    let temp_store = temp_store(conn).await?;
    conn.execute("PRAGMA temp_store = MEMORY", ())
        .await
        .map_err(|e| IndexError::Migration(e.to_string()))?;
    let applied = apply_migrations_in_order(conn, migrations).await;
    let restored = conn
        .execute(&format!("PRAGMA temp_store = {temp_store}"), ())
        .await
        .map_err(|e| IndexError::Migration(e.to_string()));
    let version = applied?;
    restored?;
    Ok(version)
}

/// The connection's `temp_store` setting: 0 default, 1 file, 2 memory.
async fn temp_store(conn: &Connection) -> Result<i64> {
    let mut rows = conn
        .query("PRAGMA temp_store", ())
        .await
        .map_err(|e| IndexError::Migration(e.to_string()))?;
    let row = rows
        .next()
        .await
        .map_err(|e| IndexError::Migration(e.to_string()))?;
    Ok(row
        .and_then(|r| r.get_value(0).ok())
        .and_then(|v| v.as_integer().copied())
        .unwrap_or(0))
}

/// The body of [`apply_migrations`], run while the temp store is in memory.
async fn apply_migrations_in_order(conn: &Connection, migrations: &[Migration]) -> Result<i64> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS schema_migration (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL)",
        (),
    )
    .await
    .map_err(|e| IndexError::Migration(e.to_string()))?;

    let current = current_version(conn).await?;
    // A database raised past every migration this binary ships is a newer
    // Crystalline's work, not damage: applying nothing and refusing is the
    // only safe move, since there is no migration list here that could ever
    // replay backwards to the version this binary expects. Checked once,
    // before the loop touches anything, so an out-of-date binary never
    // partially applies a step it does have against a schema whose later
    // history it cannot see.
    let known = migrations.last().map(|m| m.version).unwrap_or(0);
    if current > known {
        return Err(IndexError::SchemaTooNew {
            found: current,
            known,
        });
    }
    for m in migrations {
        if m.version <= current {
            continue;
        }
        // The DDL and the row that stamps it are one transaction, rolled back
        // together if anything in either fails.
        //
        // Since turso 0.8.0 `execute_batch` takes exclusive use of the
        // connection and fails with Misuse "connection is busy with another
        // operation" while any `Rows` on it is still alive, so nothing here may
        // hold one across this call.
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
        if let Err(e) = backfill(conn, m.version).await {
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

/// The data step a migration runs after its DDL and before its stamp, in the
/// same transaction, for rows only Rust can derive. v19's rows come from the
/// function every write uses, so an upgraded index and a freshly written one
/// hold the same rows by construction. A SQL fill could not promise that:
/// `json_each` reads a JSON `true` back as the integer 1, and floats print
/// differently in each engine.
async fn backfill(conn: &Connection, version: i64) -> Result<()> {
    match version {
        19 => backfill_meta_values(conn).await,
        _ => Ok(()),
    }
}

/// Fill `engram_meta_value` from every engram row's stored `metadata`, every
/// actor's rows included: a draft is searchable, so it needs its rows too. A
/// row whose metadata does not parse gets no rows instead of failing the
/// migration, because a failed migration makes `open_resilient` set the
/// whole index aside. The rows are read a page of [`super::BACKFILL_PAGE`]
/// engrams at a time in id order, so memory stays bounded on a large index,
/// and each page's rows go in as multi-row inserts across its engrams. Every
/// `Rows` is dropped before the page's inserts (see the `execute_batch` note
/// in `apply_migrations`).
async fn backfill_meta_values(conn: &Connection) -> Result<()> {
    conn.execute("DELETE FROM engram_meta_value", ()).await?;
    let mut after = i64::MIN;
    loop {
        let mut seen = 0_i64;
        let mut page: Vec<(i64, String)> = Vec::new();
        {
            let mut rows = conn
                .query(
                    "SELECT id, metadata FROM engram WHERE id > ?1 ORDER BY id LIMIT ?2",
                    vec![
                        turso::Value::Integer(after),
                        turso::Value::Integer(super::BACKFILL_PAGE),
                    ],
                )
                .await?;
            while let Some(r) = rows.next().await? {
                let Ok(turso::Value::Integer(id)) = r.get_value(0) else {
                    continue;
                };
                seen += 1;
                after = after.max(id);
                if let Ok(turso::Value::Text(text)) = r.get_value(1) {
                    page.push((id, text));
                }
            }
        }
        let mut meta_rows = Vec::new();
        for (id, text) in &page {
            let Ok(metadata) = serde_json::from_str::<serde_json::Value>(text) else {
                continue;
            };
            for (key, value) in crate::store::meta_value_rows(&metadata) {
                meta_rows.push((*id, key, value));
            }
        }
        super::insert_meta_rows(conn, &meta_rows).await?;
        if seen < super::BACKFILL_PAGE {
            break;
        }
    }
    Ok(())
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

    /// v17 against an index the old resolve pass wrote: a prefix that spelled
    /// no domain had bound the bare target at home. The migration unbinds
    /// exactly those rows, keeps a row bound to the whole bracket text at home,
    /// a row whose prefix names a domain and a bare row, and the resolve pass
    /// that follows binds the unbound row the way the current rule does.
    #[tokio::test]
    async fn v17_unbinds_what_an_unknown_prefix_bound_at_home() {
        let db = Builder::new_local(":memory:").build().await.unwrap();
        let conn = db.connect().unwrap();
        let (before, v17) = (&MIGRATIONS[..16], &MIGRATIONS[16]);
        assert_eq!(v17.version, 17, "the seventeenth migration is v17");
        apply_migrations(&conn, before).await.unwrap();

        conn.execute_batch(
            r#"
            INSERT INTO domain(id, name, path) VALUES (1,'home','/tmp/h'), (2,'eng','/tmp/e');
            INSERT INTO domain_spelling(spelling, domain_id) VALUES ('home',1), ('eng',2);
            INSERT INTO engram(id, domain_id, path, permalink, title)
                VALUES (1,1,'foo.md','foo','Foo'),
                       (2,1,'typo-foo.md','typo-foo','typo:Foo'),
                       (3,1,'src.md','src','Src'),
                       (4,2,'runbook.md','runbook','Runbook'),
                       (5,1,'runbook.md','runbook','Runbook'),
                       (6,2,'typo-foo.md','typo-foo','typo:Foo');
            INSERT INTO link(id, engram_id, domain_id, line, to_target, to_domain, to_raw, to_id)
                VALUES (1,3,1,1,'Foo','typo','typo:Foo',1),
                       (2,3,1,2,'Foo','typo','typo:Foo',2),
                       (3,3,1,3,'Runbook','eng','eng:Runbook',4),
                       (4,3,1,4,'Foo',NULL,'Foo',1),
                       (5,3,1,5,'Runbook','ops',NULL,5),
                       (6,3,1,7,'Foo','typo','typo:Foo',6);
            INSERT INTO relation(id, engram_id, domain_id, line, rel_type, to_target, to_domain, to_raw, to_id)
                VALUES (1,3,1,6,'relates_to','Runbook','ops','ops:Runbook',5);
            "#,
        )
        .await
        .unwrap();

        apply_migrations(&conn, MIGRATIONS).await.unwrap();
        let bound = |id: i64, table: &'static str| {
            let conn = &conn;
            async move {
                scalar(
                    conn,
                    &format!("SELECT COALESCE(to_id, 0) FROM {table} WHERE id={id}"),
                )
                .await
            }
        };
        assert_eq!(
            bound(1, "link").await,
            0,
            "the bare target at home is unbound"
        );
        assert_eq!(
            bound(2, "link").await,
            2,
            "the whole text at home stays bound"
        );
        assert_eq!(
            bound(3, "link").await,
            4,
            "a known prefix keeps its binding"
        );
        assert_eq!(
            bound(4, "link").await,
            1,
            "a bare reference keeps its binding"
        );
        assert_eq!(
            bound(5, "link").await,
            0,
            "a row with no raw text is unbound"
        );
        assert_eq!(bound(1, "relation").await, 0, "relations are unbound too");
        assert_eq!(
            bound(6, "link").await,
            0,
            "the whole text matched in another domain is not the whole text at home"
        );

        // The next resolve pass binds what the current rule binds, and nothing
        // it unbound comes back to the bare target.
        for table in ["relation", "link"] {
            conn.execute(&crate::store::resolve_pending_sql(table, "1"), ())
                .await
                .unwrap();
        }
        assert_eq!(
            bound(1, "link").await,
            2,
            "rebound to the whole text at home"
        );
        assert_eq!(bound(5, "link").await, 0);
        assert_eq!(bound(1, "relation").await, 0);
        assert_eq!(
            bound(6, "link").await,
            2,
            "the row bound away from home is rebound at home"
        );

        // Idempotent: a replay changes nothing.
        conn.execute_batch(v17.sql).await.unwrap();
        assert_eq!(bound(1, "link").await, 2);
        assert_eq!(bound(3, "link").await, 4);
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

        // And the migrations after it, because `WIPE_TABLES` below is the
        // current list and v15 added a table to it. The rows under test come
        // through those migrations untouched, which is the point of them.
        for m in &MIGRATIONS[13..] {
            conn.execute_batch(m.sql).await.unwrap();
        }

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
                // Added by v24, which ran above with the rest after v13.
                "idx_engram_reflection_head".to_string(),
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

    /// The v15 move of the body out of `engram`, over a database carrying a
    /// draft beside its base row and bodies of several sizes - the shape an
    /// upgrade meets on a machine somebody has been drafting on.
    ///
    /// Four claims. Every body comes through at the id it was written under,
    /// base and draft alike, so nothing needs a resync and no actor's draft is
    /// flattened onto another's. The child rows still point at the ids they
    /// pointed at, because this swap preserves ids exactly as v13's did. There
    /// is one `engram_content` row per engram, which is what makes the join an
    /// inner-shaped read even where it is spelled `LEFT`. And the swapped table
    /// carries every index it carried before, since they all die with the
    /// dropped table.
    ///
    /// The last leg opens a real store over the migrated file, because the
    /// whole point of the move is that the statements still answer: a lexical
    /// search snippets out of a body that now lives in another table, and the
    /// two content reads answer for the base row and the draft.
    #[tokio::test]
    async fn v15_moves_every_body_into_its_own_table() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.db");
        let bodies: [(i64, &str, &str, usize); 3] = [
            (7, "a.md", "", 3),
            (8, "b.md", "", 200_000),
            (9, "a.md", "alice", 64),
        ];
        {
            let db = Builder::new_local(path.to_str().unwrap())
                .build()
                .await
                .unwrap();
            let conn = db.connect().unwrap();
            for m in &MIGRATIONS[..14] {
                conn.execute_batch(m.sql).await.unwrap();
            }
            assert_eq!(MIGRATIONS[14].version, 15, "the fifteenth migration is v15");
            // The ledger too, so opening a store over this file later runs v15
            // and nothing before it.
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS schema_migration \
                 (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL);\n\
                 INSERT INTO schema_migration(version, applied_at) VALUES (14,'2026-09-21T00:00:00Z');\n\
                 INSERT INTO domain(id, name, path) VALUES (1,'d','/tmp/d');\n\
                 INSERT INTO observation(engram_id, line, category, content) VALUES (7,1,'note','x');\n\
                 INSERT INTO chunk(engram_id, seq, text) VALUES (7,0,'x');\n",
            )
            .await
            .unwrap();
            for (id, file, actor, len) in bodies {
                conn.execute(
                    "INSERT INTO engram(id, domain_id, path, permalink, title, status, content, \
                     sha256, actor) VALUES (?1,1,?2,?3,'A','stable',?4,'ff',?5)",
                    vec![
                        turso::Value::Integer(id),
                        turso::Value::Text(file.to_string()),
                        turso::Value::Text(format!("p{id}")),
                        turso::Value::Text(body_of(len)),
                        turso::Value::Text(actor.to_string()),
                    ],
                )
                .await
                .unwrap();
            }

            conn.execute_batch(MIGRATIONS[14].sql).await.unwrap();
            // The stamp `apply` would have written in the same transaction, so
            // the store opened on this file below runs nothing further.
            conn.execute_batch(
                "INSERT INTO schema_migration(version, applied_at) \
                 VALUES (15,'2026-09-21T00:00:00Z');",
            )
            .await
            .unwrap();

            assert_eq!(
                scalar(&conn, "SELECT COUNT(*) FROM engram_content").await,
                scalar(&conn, "SELECT COUNT(*) FROM engram").await,
                "one body row per engram row after the move"
            );
            for (id, _, _, len) in bodies {
                let bytes = body_of(len).len();
                assert_eq!(
                    scalar(
                        &conn,
                        &format!(
                            "SELECT COUNT(*) FROM engram e JOIN engram_content ec \
                             ON ec.engram_id=e.id \
                             WHERE e.id={id} AND length(ec.content)={bytes} \
                             AND ec.content LIKE 'needle %'"
                        ),
                    )
                    .await,
                    1,
                    "the body of engram {id} came through at its own id"
                );
            }
            assert_eq!(
                scalar(&conn, "SELECT COUNT(*) FROM observation WHERE engram_id=7").await
                    + scalar(&conn, "SELECT COUNT(*) FROM chunk WHERE engram_id=7").await,
                2,
                "the child rows still point at the engram they pointed at"
            );
            assert!(
                conn.query("SELECT content FROM engram", ()).await.is_err(),
                "and the column the body used to sit in is gone"
            );

            // Every index the table carried is back, the same seven v13 left.
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
                "the swap carries every index across"
            );

            // `WIPE_TABLES` has to name the new table, and every name in it has
            // to still be a table after the swap. The migrations after v15 run
            // first (stamped, so the store opened below runs nothing again):
            // `WIPE_TABLES` is the current list, and the tables they add
            // (`domain_spelling` among them) are checked with the rest rather
            // than skipped by name.
            assert!(
                WIPE_TABLES.contains(&"engram_content"),
                "a wipe that leaves the bodies behind leaves the whole index behind"
            );
            for m in &MIGRATIONS[15..] {
                conn.execute_batch(m.sql).await.unwrap();
                conn.execute_batch(&format!(
                    "INSERT INTO schema_migration(version, applied_at) \
                     VALUES ({},'2026-09-21T00:00:00Z');",
                    m.version
                ))
                .await
                .unwrap();
            }
            assert!(WIPE_TABLES.contains(&"domain_spelling"));
            for table in WIPE_TABLES {
                assert_eq!(
                    scalar(
                        &conn,
                        &format!(
                            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='{table}'"
                        ),
                    )
                    .await,
                    1,
                    "wipe names a table that exists after the swap: {table}"
                );
            }
        }

        // And the statements answer over the migrated file: a store opened on
        // it reads both bodies back and snippets a lexical hit out of one.
        let store = crate::TursoStore::open(&path).await.unwrap();
        let domain = crate::DomainId(1);
        assert_eq!(
            crate::Store::engram_content(&store, domain, "a.md")
                .await
                .unwrap()
                .as_deref(),
            Some(body_of(3).as_str()),
            "the base row's body reads back through the join"
        );
        let all = crate::Store::all_engram_contents(&store, domain)
            .await
            .unwrap();
        assert_eq!(
            all.iter().map(|e| e.content.len()).collect::<Vec<_>>(),
            vec![body_of(3).len(), body_of(200_000).len()],
            "every base body in the domain, path-ordered, unchanged"
        );
        let mine = crate::Store::overlay_entry(&store, domain, "alice", "a.md")
            .await
            .unwrap()
            .expect("alice's draft survived the move");
        assert_eq!(mine.content, body_of(64), "and so did her body");
        let hits = crate::Store::search(
            &store,
            &crate::SearchQuery {
                text: Some("needle".to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert!(
            hits.items.iter().any(|h| h.snippet.contains("needle")),
            "a lexical search still snippets out of the body: {:?}",
            hits.items
        );
    }

    /// v16 against domains written before it: every existing domain row comes
    /// out of the migration as its own spelling, so a reference by local name
    /// resolves through `domain_spelling` from the first statement after the
    /// upgrade, before any sync has run. Then a store opened on the file reads
    /// the backfilled rows back through the trait.
    #[tokio::test]
    async fn v16_backfills_every_domain_as_its_own_spelling() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.db");
        {
            let db = Builder::new_local(path.to_str().unwrap())
                .build()
                .await
                .unwrap();
            let conn = db.connect().unwrap();
            for m in &MIGRATIONS[..15] {
                conn.execute_batch(m.sql).await.unwrap();
            }
            assert_eq!(MIGRATIONS[15].version, 16, "the sixteenth migration is v16");
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS schema_migration \
                 (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL);\n\
                 INSERT INTO schema_migration(version, applied_at) VALUES (15,'2026-09-25T00:00:00Z');\n\
                 INSERT INTO domain(id, name, path) VALUES (3,'eng','/tmp/eng'),(5,'ops','/tmp/ops');\n",
            )
            .await
            .unwrap();

            conn.execute_batch(MIGRATIONS[15].sql).await.unwrap();
            conn.execute_batch(
                "INSERT INTO schema_migration(version, applied_at) \
                 VALUES (16,'2026-09-25T00:00:00Z');",
            )
            .await
            .unwrap();

            assert_eq!(
                scalar(
                    &conn,
                    "SELECT COUNT(*) FROM domain_spelling s JOIN domain d \
                     ON d.id=s.domain_id AND d.name=s.spelling"
                )
                .await,
                2,
                "both domain rows are backfilled as their own spelling"
            );
            assert_eq!(
                scalar(&conn, "SELECT COUNT(*) FROM domain_spelling").await,
                2,
                "and nothing else"
            );
            // `WIPE_TABLES` is the current list, so the migrations after v16
            // run first (stamped, so the store opened below runs nothing
            // again) and the tables they add are checked with the rest.
            for m in &MIGRATIONS[16..] {
                conn.execute_batch(m.sql).await.unwrap();
                conn.execute_batch(&format!(
                    "INSERT INTO schema_migration(version, applied_at) \
                     VALUES ({},'2026-09-27T00:00:00Z');",
                    m.version
                ))
                .await
                .unwrap();
            }
            for table in WIPE_TABLES {
                assert_eq!(
                    scalar(
                        &conn,
                        &format!(
                            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='{table}'"
                        ),
                    )
                    .await,
                    1,
                    "wipe names a table that exists once every migration ran: {table}"
                );
            }
            let at = |t: &str| WIPE_TABLES.iter().position(|w| *w == t);
            assert!(
                at("domain_spelling").is_some() && at("domain_spelling") < at("domain"),
                "the spellings are wiped before the domains they point at"
            );
        }

        let store = crate::TursoStore::open(&path).await.unwrap();
        assert_eq!(
            crate::Store::domain_spellings(&store).await.unwrap(),
            vec![
                ("eng".to_string(), crate::DomainId(3)),
                ("ops".to_string(), crate::DomainId(5)),
            ],
            "the backfill reads back through the store"
        );
    }

    /// A body of `len` bytes carrying the word a lexical search looks for.
    fn body_of(len: usize) -> String {
        let mut body = "needle ".to_string();
        while body.len() < len {
            body.push('x');
        }
        body
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

    /// A database stamped one version above the newest migration this binary
    /// knows: exactly what an older binary meets when it opens an index a
    /// newer Crystalline already raised. `apply` must refuse rather than
    /// silently skip every migration in its list (all of them are already
    /// `<= current` and would simply be passed over) and report back as
    /// current, which is the bug this guard exists to close.
    #[tokio::test]
    async fn a_schema_stamped_above_the_newest_known_migration_is_refused() {
        let db = Builder::new_local(":memory:").build().await.unwrap();
        let conn = db.connect().unwrap();

        // A real database at the newest version this binary knows, reached
        // through the actual entry point so the ledger and every table are in
        // the shape a real open leaves them in.
        apply_migrations(&conn, MIGRATIONS).await.unwrap();
        let known = MIGRATIONS.last().unwrap().version;

        // Stamp one version past it, the way a newer binary's own migration
        // would have: same ledger table, same shape, a version this binary's
        // list does not contain.
        conn.execute(
            "INSERT INTO schema_migration (version, applied_at) VALUES (?1, ?2)",
            vec![
                turso::Value::Integer(known + 1),
                turso::Value::Text(chrono::Utc::now().to_rfc3339()),
            ],
        )
        .await
        .unwrap();

        let err = apply_migrations(&conn, MIGRATIONS)
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
        assert_eq!(
            err.to_string(),
            format!(
                "this index was upgraded by a newer Crystalline (schema v{}, this binary knows v{known}). \
                 This copy is out of date. If Claude Desktop runs Crystalline as an extension, install \
                 the current .mcpb over it; otherwise upgrade this binary.",
                known + 1
            ),
            "the message names both versions and both remedies"
        );
        assert!(
            crate::is_schema_too_new_text(&err.to_string()),
            "the advice composers recognise the refusal by its opening words"
        );

        // Nothing was applied: the ledger still says the future version, not
        // something this binary invented by running a migration that was
        // never supposed to run again.
        assert_eq!(
            scalar(
                &conn,
                "SELECT COALESCE(MAX(version), 0) FROM schema_migration"
            )
            .await,
            known + 1,
            "the refusal touches nothing; the recorded version is untouched"
        );
    }

    /// An index an older binary built, at v17 with a domain row in it: opening
    /// it runs v18, whose `parse_generation` default leaves that row behind,
    /// and the first plain sync reparses its files and stamps it. Built the way
    /// the other upgrade tests build an old database, on a file so the store
    /// can open it afterwards.
    #[tokio::test]
    async fn an_index_from_before_v18_reparses_on_its_first_sync() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("index.db");
        let root = tmp.path().join("d");
        {
            let db = Builder::new_local(db_path.to_str().unwrap())
                .build()
                .await
                .unwrap();
            let conn = db.connect().unwrap();
            assert_eq!(
                MIGRATIONS[17].version, 18,
                "the eighteenth migration is v18"
            );
            apply_migrations(&conn, &MIGRATIONS[..17]).await.unwrap();
            conn.execute(
                "INSERT INTO domain(name, path, kind) VALUES (?1, ?2, 'file')",
                vec![
                    turso::Value::Text("d".to_string()),
                    turso::Value::Text(root.to_string_lossy().into_owned()),
                ],
            )
            .await
            .unwrap();
        }
        let store = crate::TursoStore::open(&db_path).await.unwrap();
        crate::sync::upgrade_fixture::first_sync_after_the_upgrade_reparses(&store, &root).await;
    }

    async fn meta_rows(conn: &Connection, id: i64) -> Vec<(String, String)> {
        let mut rows = conn
            .query(
                "SELECT key, value FROM engram_meta_value WHERE engram_id=?1 ORDER BY key, value",
                vec![turso::Value::Integer(id)],
            )
            .await
            .unwrap();
        let mut out = Vec::new();
        while let Some(r) = rows.next().await.unwrap() {
            if let (Ok(turso::Value::Text(k)), Ok(turso::Value::Text(v))) =
                (r.get_value(0), r.get_value(1))
            {
                out.push((k, v));
            }
        }
        out
    }

    /// The upgrade cost at scale: v19 over an index of 50,000 engrams an
    /// older binary wrote, each citing five `sources` anchors (the
    /// `perf_meta` corpus), timed from the open that runs the backfill.
    /// Ignored and never a gate; run with
    /// `cargo test -p crystalline-index --lib --release -- v19_backfill_cost_at_50k --ignored --nocapture`.
    #[tokio::test]
    #[ignore = "perf evidence: run by hand with --ignored --nocapture"]
    async fn v19_backfill_cost_at_50k() {
        use crate::store::Store;
        const ENGRAMS: i64 = 50_000;
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("index.db");
        {
            let db = Builder::new_local(db_path.to_str().unwrap())
                .build()
                .await
                .unwrap();
            let conn = db.connect().unwrap();
            apply_migrations(&conn, &MIGRATIONS[..18]).await.unwrap();
            conn.execute(
                "INSERT INTO domain(id, name, path, kind) VALUES (1, 'notes', '/tmp/n', 'file')",
                (),
            )
            .await
            .unwrap();
            conn.execute("BEGIN", ()).await.unwrap();
            for start in (0..ENGRAMS).step_by(100) {
                let mut params = Vec::new();
                let mut tuples = Vec::new();
                for i in start..start + 100 {
                    let mut sources: Vec<String> = (0..4)
                        .map(|b| format!("notedown://jordi/n{i}/p0#b{b}"))
                        .collect();
                    sources.push(format!("notedown://jordi/shared/p{}#b0", i % 1000));
                    let n = params.len();
                    tuples.push(format!(
                        "(?{}, 1, ?{}, ?{}, ?{}, '')",
                        n + 1,
                        n + 2,
                        n + 3,
                        n + 4
                    ));
                    params.push(turso::Value::Integer(i + 1));
                    params.push(turso::Value::Text(format!("n{i}.md")));
                    params.push(turso::Value::Text(format!("note-{i}")));
                    params.push(turso::Value::Text(
                        serde_json::json!({ "sources": sources }).to_string(),
                    ));
                }
                conn.execute(
                    &format!(
                        "INSERT INTO engram(id, domain_id, path, permalink, metadata, actor) VALUES {}",
                        tuples.join(",")
                    ),
                    params,
                )
                .await
                .unwrap();
            }
            conn.execute("COMMIT", ()).await.unwrap();
        }
        let started = std::time::Instant::now();
        let store = crate::TursoStore::open(&db_path).await.unwrap();
        let ms = started.elapsed().as_millis();
        assert_eq!(
            store.store_info().await.unwrap().schema_version,
            MIGRATIONS.last().unwrap().version
        );
        let rows = store.meta_values().await.unwrap().len();
        assert_eq!(rows, 5 * ENGRAMS as usize, "five rows an engram");
        eprintln!(
            "PERF v19 backfill 50k: open with the migration {ms} ms, {rows} rows, {:.1} us per engram",
            ms as f64 * 1000.0 / ENGRAMS as f64
        );
    }

    /// v19 over an index an older binary wrote. Every engram row, a draft
    /// among them, comes out holding the rows a fresh write of the same
    /// metadata gives, so `$contains` answers right after the upgrade and
    /// nothing needs a reindex. The fixture is the hard one: a quote, a
    /// backslash, a newline, non-ASCII text, a duplicate element, a negative
    /// number, i64::MAX, a float, "1" beside 1, true beside 1 and a list of
    /// objects; and one row whose metadata does not parse, which gets no
    /// rows and fails nothing, because a failed migration sets the index
    /// aside.
    #[tokio::test]
    async fn v19_fills_engram_meta_value_from_the_stored_metadata() {
        use crate::store::Store;
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("index.db");
        let base = serde_json::json!({
            "sources": ["a \"q\" \\ b", "line\nbreak", "ünïcode", "a \"q\" \\ b"],
            "numbers": [-3, i64::MAX, 1.5, 1, "1", 1e-7, 2.0, 1e20, u64::MAX, -0.0],
            "flags": [true, 1],
            "verified": [{"by": "jordi", "at": "2026-01-01T00:00:00Z"}],
            "kind": "anchor",
        });
        let draft = serde_json::json!({ "sources": ["draft-only"] });
        {
            let db = Builder::new_local(db_path.to_str().unwrap())
                .build()
                .await
                .unwrap();
            let conn = db.connect().unwrap();
            assert_eq!(
                MIGRATIONS[18].version, 19,
                "the nineteenth migration is v19"
            );
            apply_migrations(&conn, &MIGRATIONS[..18]).await.unwrap();
            conn.execute(
                "INSERT INTO domain(id, name, path, kind) VALUES (1, 'd', '/tmp/d', 'file')",
                (),
            )
            .await
            .unwrap();
            for (id, path, actor, metadata) in [
                (7_i64, "a.md", "", base.to_string()),
                (8, "a.md", "alice", draft.to_string()),
                (9, "b.md", "", "not json".to_string()),
            ] {
                conn.execute(
                    "INSERT INTO engram(id, domain_id, path, permalink, metadata, actor) \
                     VALUES (?1, 1, ?2, ?3, ?4, ?5)",
                    vec![
                        turso::Value::Integer(id),
                        turso::Value::Text(path.to_string()),
                        turso::Value::Text(format!("p{id}")),
                        turso::Value::Text(metadata),
                        turso::Value::Text(actor.to_string()),
                    ],
                )
                .await
                .unwrap();
            }
        }
        let store = crate::TursoStore::open(&db_path).await.unwrap();
        assert_eq!(
            store.store_info().await.unwrap().schema_version,
            MIGRATIONS.last().unwrap().version
        );

        let base_rows = meta_rows(store.conn.raw(), 7).await;
        assert_eq!(base_rows, crate::store::meta_value_rows(&base));
        assert_eq!(
            meta_rows(store.conn.raw(), 8).await,
            crate::store::meta_value_rows(&draft),
            "a draft gets its rows too"
        );
        assert!(
            meta_rows(store.conn.raw(), 9).await.is_empty(),
            "metadata that does not parse gets no rows"
        );
        for (key, value) in [
            ("numbers", "\"1\""),
            ("numbers", "1"),
            ("numbers", "9223372036854775807"),
            ("numbers", "1e-7"),
            ("numbers", "2.0"),
            ("numbers", "1e+20"),
            ("numbers", "18446744073709551615"),
            // Turso stores `metadata` as text, so the negative zero survives
            // here; the Postgres twin documents that JSONB turns it into 0.0.
            ("numbers", "-0.0"),
            ("flags", "true"),
            ("flags", "1"),
            ("sources", "\"line\\nbreak\""),
        ] {
            assert!(
                base_rows.contains(&(key.to_string(), value.to_string())),
                "{key} = {value} in {base_rows:?}"
            );
        }
        assert!(
            !base_rows.iter().any(|(k, _)| k == "verified"),
            "a list of objects gives no row"
        );
        assert_eq!(
            base_rows.iter().filter(|(k, _)| k == "sources").count(),
            3,
            "the duplicate element is one row"
        );
    }

    /// The migrations borrow an in-memory temp store and give the setting back,
    /// after a clean run and after one that fails, so the queries that follow
    /// an open keep spilling to disk.
    #[tokio::test]
    async fn the_temp_store_is_given_back_after_the_migrations() {
        let db = Builder::new_local(":memory:").build().await.unwrap();
        let conn = db.connect().unwrap();
        apply_migrations(&conn, MIGRATIONS).await.unwrap();
        assert_eq!(
            scalar(&conn, "PRAGMA temp_store").await,
            0,
            "a clean run leaves the default"
        );

        let db = Builder::new_local(":memory:").build().await.unwrap();
        let conn = db.connect().unwrap();
        conn.execute("PRAGMA temp_store = FILE", ()).await.unwrap();
        let failing = [Migration {
            version: 1,
            label: "broken",
            sql: "CREATE TABLE ok_so_far (id INTEGER); THIS IS NOT SQL;",
        }];
        let err = apply_migrations(&conn, &failing)
            .await
            .expect_err("the broken migration fails");
        assert!(err.to_string().contains("v1 (broken)"), "{err}");
        assert_eq!(
            scalar(&conn, "PRAGMA temp_store").await,
            1,
            "a failed run puts back the setting the connection had"
        );
    }

    /// Set in the child run of
    /// [`a_store_opens_and_migrates_with_no_usable_temp_directory`].
    const NO_TEMP_DIR_CHILD: &str = "CRYSTALLINE_TEST_MIGRATE_WITHOUT_TEMP_DIR";

    /// A store opens and migrates when the process temp directory cannot be
    /// used at all.
    ///
    /// The embedded engine gives every ephemeral table a fresh directory under
    /// the process temp location, eagerly, even when the table never holds a
    /// row, and v7's `NOT IN (SELECT ...)` deletes need one each. On the
    /// Windows CI legs that directory create is refused now and then with
    /// "permission denied", so a store's first open failed with
    /// `Migration("v7 ...: I/O error (tempdir): permission denied")`. Pointing
    /// the temp location at a folder that does not exist makes every such
    /// create fail, on every platform and every time, so this pins that the
    /// migrations create none. The environment can only be changed for a
    /// whole process, so the test runs itself again as a child with the
    /// variables set and asserts in there.
    #[tokio::test]
    async fn a_store_opens_and_migrates_with_no_usable_temp_directory() {
        if std::env::var_os(NO_TEMP_DIR_CHILD).is_some() {
            let store = crate::turso::TursoStore::open_in_memory().await;
            assert!(
                store.is_ok(),
                "the open must not need the temp directory: {:?}",
                store.err()
            );
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let missing = root.path().join("no-such-dir");
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "turso::migrations::tests::a_store_opens_and_migrates_with_no_usable_temp_directory",
                "--exact",
                "--test-threads=1",
                "--nocapture",
            ])
            .env(NO_TEMP_DIR_CHILD, "1")
            .env("TMPDIR", &missing)
            .env("TMP", &missing)
            .env("TEMP", &missing)
            .env_remove("TURSO_TMPDIR")
            .env_remove("SQLITE_TMPDIR")
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            out.status.success(),
            "the child open failed:\n{stdout}\n{stderr}"
        );
        assert!(
            stdout.contains("1 passed"),
            "the child ran the test rather than filtering it out:\n{stdout}"
        );
    }

    #[tokio::test]
    async fn the_automation_migration_is_v27_and_its_tables_are_wiped_before_domain() {
        let at_version = |v: i64| {
            MIGRATIONS
                .iter()
                .position(|m| m.version == v)
                .unwrap_or_else(|| panic!("no migration v{v}"))
        };
        assert_eq!(
            MIGRATIONS[at_version(27)].label,
            "automation lease and runs"
        );
        assert_eq!(at_version(27), at_version(26) + 1, "it follows v26");
        let db = Builder::new_local(":memory:").build().await.unwrap();
        let conn = db.connect().unwrap();
        apply_migrations(&conn, MIGRATIONS).await.unwrap();
        for table in ["automation_lease", "automation_run"] {
            assert_eq!(
                scalar(
                    &conn,
                    &format!(
                        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='{table}'"
                    ),
                )
                .await,
                1,
                "v27 creates {table}"
            );
        }
        let at = |name: &str| {
            WIPE_TABLES
                .iter()
                .position(|t| *t == name)
                .unwrap_or_else(|| panic!("{name} is not in WIPE_TABLES"))
        };
        assert!(
            at("automation_lease") < at("domain") && at("automation_run") < at("domain"),
            "the lease and the runs go before their domain"
        );
    }

    #[tokio::test]
    async fn the_lease_columns_migration_is_v28_and_a_lease_row_defaults_to_nothing_written() {
        let at_version = |v: i64| {
            MIGRATIONS
                .iter()
                .position(|m| m.version == v)
                .unwrap_or_else(|| panic!("no migration v{v}"))
        };
        assert_eq!(
            MIGRATIONS[at_version(28)].label,
            "automation lease pull request and written paths"
        );
        assert_eq!(at_version(28), at_version(27) + 1, "it follows v27");
        let db = Builder::new_local(":memory:").build().await.unwrap();
        let conn = db.connect().unwrap();
        apply_migrations(&conn, MIGRATIONS).await.unwrap();
        conn.execute(
            "INSERT INTO domain(id, name, path, kind) VALUES (1, 'd', '/tmp/d', 'file')",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO automation_lease (domain_id, loop_kind, instance_id, runner, token, \
             taken_at, renewed_at, unshared_at_start, due_at_start) \
             VALUES (1, 'reflect', 'i', 'cli', 't', 'a', 'a', '[]', '[]')",
            (),
        )
        .await
        .unwrap();
        assert_eq!(
            scalar(
                &conn,
                "SELECT COUNT(*) FROM automation_lease WHERE pr_url IS NULL AND written = '[]'"
            )
            .await,
            1,
            "v28 adds pr_url (null) and written (an empty list)"
        );
    }

    #[tokio::test]
    async fn the_reflection_migration_is_v20_and_creates_its_table() {
        let v20 = MIGRATIONS.iter().find(|m| m.version == 20).unwrap();
        assert_eq!(v20.label, "reflection index");
        let db = Builder::new_local(":memory:").build().await.unwrap();
        let conn = db.connect().unwrap();
        apply_migrations(&conn, MIGRATIONS).await.unwrap();
        conn.execute(
            "INSERT INTO domain(id, name, path, kind) VALUES (1, 'd', '/tmp/d', 'file')",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO reflection(domain_id, path, title, content, sha256, size, modified) VALUES(1, 'a.md', 'A', 'x', 's', 1, 't')",
            (),
        )
        .await
        .unwrap();
        let at = |name: &str| {
            WIPE_TABLES
                .iter()
                .position(|t| *t == name)
                .unwrap_or_else(|| panic!("{name} is not in WIPE_TABLES"))
        };
        assert!(
            at("reflection") < at("domain"),
            "the reflection rows go before their domain"
        );
    }

    #[tokio::test]
    async fn the_reflection_chunk_migration_is_v22_and_rereads_every_page() {
        let at_version = |v: i64| {
            MIGRATIONS
                .iter()
                .position(|m| m.version == v)
                .unwrap_or_else(|| panic!("no migration v{v}"))
        };
        let v22 = at_version(22);
        assert_eq!(MIGRATIONS[v22].label, "reflection chunks");
        assert_eq!(
            v22,
            at_version(21) + 1,
            "it follows the reflection asset table"
        );
        let db = Builder::new_local(":memory:").build().await.unwrap();
        let conn = db.connect().unwrap();
        // Up to v21 first: a page indexed by a build before this migration.
        apply_migrations(&conn, &MIGRATIONS[..v22]).await.unwrap();
        conn.execute(
            "INSERT INTO domain(id, name, path, kind) VALUES (1, 'd', '/tmp/d', 'file')",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO reflection(domain_id, path, title, content, sha256, size, modified) \
             VALUES(1, 'a.md', 'A', 'x', 's', 1, '2026-10-14T00:00:00+00:00')",
            (),
        )
        .await
        .unwrap();
        apply_migrations(&conn, MIGRATIONS).await.unwrap();
        {
            let mut rows = conn
                .query("SELECT modified FROM reflection", ())
                .await
                .unwrap();
            let row = rows.next().await.unwrap().unwrap();
            assert_eq!(
                row.get_value(0).unwrap(),
                turso::Value::Text(String::new()),
                "the stamp is blank, so the next walk reads the page"
            );
        }
        assert_eq!(
            scalar(&conn, "SELECT count(*) FROM reflection_chunk").await,
            0
        );
        let at = |name: &str| {
            WIPE_TABLES
                .iter()
                .position(|t| *t == name)
                .unwrap_or_else(|| panic!("{name} is not in WIPE_TABLES"))
        };
        assert!(at("reflection_chunk") < at("reflection") && at("reflection") < at("domain"));
    }

    /// v25, the host move columns, over a database at v24 with a domain in
    /// it: the row reads no last host and no move, the columns take values,
    /// and opening again applies nothing twice.
    #[tokio::test]
    async fn v25_over_v24_adds_the_host_move_columns() {
        let v25 = MIGRATIONS
            .iter()
            .position(|m| m.version == 25)
            .expect("no migration v25");
        assert_eq!(MIGRATIONS[v25 - 1].version, 24, "it follows v24");
        let db = Builder::new_local(":memory:").build().await.unwrap();
        let conn = db.connect().unwrap();
        apply_migrations(&conn, &MIGRATIONS[..v25]).await.unwrap();
        conn.execute(
            "INSERT INTO domain(id, name, path, kind) VALUES (1, 'd', '/tmp/d', 'file')",
            (),
        )
        .await
        .unwrap();
        for round in 0..2 {
            apply_migrations(&conn, &MIGRATIONS[..=v25]).await.unwrap();
            assert_eq!(
                scalar(
                    &conn,
                    "SELECT count(*) FROM domain WHERE last_host_id IS NULL \
                     AND last_host_label IS NULL AND host_move IS NULL"
                )
                .await,
                1,
                "round {round}: the old row reads no last host and no move"
            );
        }
        conn.execute(
            "UPDATE domain SET last_host_id='a', last_host_label='a', host_move='{}' WHERE id=1",
            (),
        )
        .await
        .unwrap();
        assert_eq!(
            scalar(&conn, "SELECT count(*) FROM domain WHERE host_move = '{}'").await,
            1
        );
    }

    /// v24, the Reflection gate's folder index, over a database at v23 with
    /// a domain of engram rows in it: the index is there afterwards, a replay
    /// changes nothing, and the shipped probe seeks it on the folder
    /// expression and answers the base rows under the folder in any ASCII
    /// letter case.
    #[tokio::test]
    async fn v24_over_v23_lets_the_folder_probe_seek() {
        let v24 = MIGRATIONS
            .iter()
            .position(|m| m.version == 24)
            .expect("no migration v24");
        assert_eq!(MIGRATIONS[v24 - 1].version, 23, "it follows v23");
        let db = Builder::new_local(":memory:").build().await.unwrap();
        let conn = db.connect().unwrap();
        apply_migrations(&conn, &MIGRATIONS[..v24]).await.unwrap();
        conn.execute(
            "INSERT INTO domain(id, name, path, kind) VALUES (1, 'd', '/tmp/d', 'file')",
            (),
        )
        .await
        .unwrap();
        // A domain big enough that a walk of it would show: 2000 base rows
        // over 100 folders, three rows under the folder, a draft under it and
        // a sibling whose name only starts the same way.
        let mut values: Vec<String> = (0..2000)
            .map(|g| format!("(1, 'f{}/e{g}.md', 'f{}/e{g}', '')", g % 100, g % 100))
            .collect();
        for (path, actor) in [
            ("reflections/a.md", ""),
            ("Reflections/b.md", ""),
            ("REFLECTIONS/deep/c.md", ""),
            ("reflections/d.md", "alice"),
            ("reflectionsx/e.md", ""),
        ] {
            values.push(format!(
                "(1, '{path}', '{}', '{actor}')",
                path.trim_end_matches(".md")
            ));
        }
        conn.execute(
            &format!(
                "INSERT INTO engram(domain_id, path, permalink, actor) VALUES {}",
                values.join(", ")
            ),
            (),
        )
        .await
        .unwrap();
        let probe = crate::turso::ENGRAM_ROWS_UNDER_REFLECTIONS_SQL
            .replace("?1", "1")
            .replace("?2", "64");
        for round in ["v24", "a replay of v24"] {
            conn.execute_batch(MIGRATIONS[v24].sql).await.unwrap();
            assert_eq!(
                scalar(
                    &conn,
                    "SELECT count(*) FROM sqlite_schema \
                     WHERE type = 'index' AND name = 'idx_engram_reflection_head'"
                )
                .await,
                1,
                "after {round}"
            );
            let mut rows = conn
                .query(&format!("EXPLAIN QUERY PLAN {probe}"), ())
                .await
                .unwrap();
            let mut plan = Vec::new();
            while let Some(row) = rows.next().await.unwrap() {
                if let Ok(turso::Value::Text(line)) = row.get_value(3) {
                    plan.push(line);
                }
            }
            assert!(
                plan.iter().any(|line| line
                    .contains("idx_engram_reflection_head (lower (substr (path, 1, 12))=?")),
                "after {round}, the probe seeks the folder expression: {plan:?}"
            );
            let mut rows = conn.query(&probe, ()).await.unwrap();
            let mut found = Vec::new();
            while let Some(row) = rows.next().await.unwrap() {
                if let Ok(turso::Value::Text(path)) = row.get_value(0) {
                    found.push(path);
                }
            }
            found.sort();
            assert_eq!(
                found,
                [
                    "REFLECTIONS/deep/c.md",
                    "Reflections/b.md",
                    "reflections/a.md"
                ],
                "after {round}"
            );
        }
    }

    /// v23, the reflection chunks by domain, over a database at v22 with
    /// chunk rows in it: the domain index is there afterwards, the model
    /// index (which no statement seeks any more) is gone, the rows are kept,
    /// and a replay of v23 changes nothing.
    #[tokio::test]
    async fn v23_over_v22_swaps_the_model_index_for_the_domain_index() {
        let v23 = MIGRATIONS
            .iter()
            .position(|m| m.version == 23)
            .expect("no migration v23");
        assert_eq!(MIGRATIONS[v23 - 1].version, 22, "it follows v22");
        let db = Builder::new_local(":memory:").build().await.unwrap();
        let conn = db.connect().unwrap();
        apply_migrations(&conn, &MIGRATIONS[..v23]).await.unwrap();
        conn.execute(
            "INSERT INTO domain(id, name, path, kind) VALUES (1, 'd', '/tmp/d', 'file')",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO reflection_chunk(domain_id, path, seq, text, text_hash, model, dims) \
             VALUES (1, 'a.md', 0, 't', 'h', 'm', 8), (1, 'a.md', 1, 'u', 'i', NULL, NULL)",
            (),
        )
        .await
        .unwrap();
        let indexes = || async {
            let mut rows = conn
                .query(
                    "SELECT name FROM sqlite_schema \
                     WHERE type = 'index' AND tbl_name = 'reflection_chunk' ORDER BY name",
                    (),
                )
                .await
                .unwrap();
            let mut names = Vec::new();
            while let Some(row) = rows.next().await.unwrap() {
                if let Ok(turso::Value::Text(n)) = row.get_value(0) {
                    names.push(n);
                }
            }
            names
        };
        assert!(
            indexes()
                .await
                .contains(&"idx_reflection_chunk_model".to_string()),
            "precondition: v22 has the model index"
        );
        for round in ["v23", "a replay of v23"] {
            conn.execute_batch(MIGRATIONS[v23].sql).await.unwrap();
            assert_eq!(
                indexes().await,
                [
                    "idx_reflection_chunk_domain_model",
                    "idx_reflection_chunk_page"
                ],
                "after {round}"
            );
            assert_eq!(
                scalar(&conn, "SELECT count(*) FROM reflection_chunk").await,
                2,
                "after {round}: the rows stay"
            );
        }
    }

    #[tokio::test]
    async fn the_reflection_asset_migration_follows_the_reflection_index() {
        let at_version = |v: i64| {
            MIGRATIONS
                .iter()
                .position(|m| m.version == v)
                .unwrap_or_else(|| panic!("no migration v{v}"))
        };
        let v21 = at_version(21);
        let v20 = at_version(20);
        assert_eq!(MIGRATIONS[v21].label, "reflection assets");
        assert_eq!(
            MIGRATIONS[v20].label, "reflection index",
            "it follows the reflection table's migration"
        );
        assert_eq!(v21, v20 + 1, "nothing between the two");
        let db = Builder::new_local(":memory:").build().await.unwrap();
        let conn = db.connect().unwrap();
        apply_migrations(&conn, MIGRATIONS).await.unwrap();
        conn.execute(
            "INSERT INTO reflection_asset(domain_id, path, sha256, mime, size, modified) VALUES(1, 'assets/a.png', 's', 'image/png', 1, 't')",
            (),
        )
        .await
        .unwrap();
        let at = |name: &str| {
            WIPE_TABLES
                .iter()
                .position(|t| *t == name)
                .unwrap_or_else(|| panic!("{name} is not in WIPE_TABLES"))
        };
        assert!(
            at("reflection_asset") < at("domain"),
            "the file rows go before their domain"
        );
    }
}
