//! The PostgreSQL-backed [`Store`] implementation.
//!
//! This is the parallel of [`crate::turso`], selected at runtime by the
//! `database` config block. It uses sqlx's runtime query API (never the
//! compile-time macros) so no live database is needed at build time and the
//! trait object stays object-safe, plus the `pgvector` crate for the chunk
//! embeddings. The design choices are recorded in `research/postgresql.md`; the
//! load-bearing ones:
//!
//! - Temporal columns are TEXT ISO strings and `metadata` is JSONB, so the
//!   canonical current filter and every lexical comparison are byte-identical to
//!   Turso and the shared parity suite ports directly.
//! - Full-text search is the same LIKE-candidate scan plus the identical Rust
//!   weighted scorer Turso uses (see [`search`]), not tsvector, so hybrid
//!   ranking and every search test match across backends. `store_info().fts_mode`
//!   is `CandidateScan` on both.
//! - Transactions pin one pool connection in `begin()` and route every statement
//!   to it until `commit()`/`rollback()`. Because the engine serializes all store
//!   access behind one mutex, this reproduces Turso's single-connection model and
//!   gives the CAS the same atomicity.
//! - Unlike Turso's unenforced `F32_BLOB`, pgvector enforces the `chunk.embedding`
//!   column's declared width at insert. `store_embeddings` resizes it to the
//!   active provider's `dims` on the fly (see `ensure_embedding_width`), so any
//!   provider works on either backend; a dims change already invalidates every
//!   stored vector through the existing staleness machinery, so the resize rides
//!   that invalidation instead of adding a new one.
//! - `ensure_embedding_width`'s `ALTER TABLE ... TYPE vector({dims})` changes the
//!   `chunk.embedding` column's typmod, which is encoded in the row description
//!   of any sqlx-cached prepared statement whose result row includes that raw
//!   column. The pool has multiple connections; the ALTER only clears the plan
//!   cache of the one connection that ran it (`clear_cached_statements` in
//!   `ensure_embedding_width`), so any other pooled connection with a stale plan
//!   still cached raises Postgres's "cached plan must not change result type" on
//!   its next use. `replace_chunks`' carry SELECT is the only statement in this
//!   module (or in `search`) that returns that raw column - an expression over
//!   it (the `<=>` distance operator, which yields `float8`) or a statement that
//!   only binds a vector parameter is unaffected and stays cached - so it is the
//!   one exposed to the hazard.
//!
//!   Two fixes were tried and rejected before landing on the one below.
//!   `.persistent(false)` (never cache the statement) looked right but breaks on
//!   sqlx 0.9: that flag routes the statement through Postgres's unnamed
//!   prepared statement, which does not survive being interleaved with the raw
//!   `BEGIN`/`COMMIT` this module sends as plain SQL (see `begin()`) - every
//!   statement on that connection then fails with "unnamed prepared statement
//!   does not exist", observed deterministically across the whole parity suite,
//!   not just the carry SELECT. Retrying the failed statement once in place,
//!   after `clear_cached_statements` on just that connection, does not work
//!   either: the carry SELECT runs inside `sync`'s explicit transaction (the raw
//!   `BEGIN` in `begin()`), and Postgres aborts a transaction block on any error
//!   raised inside it, so the retry's first statement fails again immediately
//!   with "current transaction is aborted, commands ignored until end of
//!   transaction block" rather than a fresh attempt - also confirmed against a
//!   live database.
//!
//!   The fix instead prevents the stale plan from ever being reused:
//!   `embedding_generation`, an `AtomicU64` on `PostgresStore`, is bumped every
//!   time `ensure_embedding_width` actually runs its ALTER. `replace_chunks`
//!   folds the current generation into its carry SELECT's SQL text as a
//!   trailing comment, so a width change gives the statement different SQL
//!   text and therefore a different sqlx cache key; every connection, not just
//!   the one that ran the DDL, prepares fresh the next time it runs the carry
//!   SELECT after a resize, and the plan it had cached under the old
//!   generation's text simply ages out of the LRU unused.

mod migrations;
mod search;

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use sqlx::pool::PoolConnection;
use sqlx::postgres::{PgArguments, PgPoolOptions, PgRow};
use sqlx::query::Query;
use sqlx::{AssertSqlSafe, Connection, PgConnection, PgPool, Postgres, Row};
use tokio::sync::{Mutex as TokioMutex, MutexGuard};

use crate::error::{IndexError, Result};
use crate::store::{
    ChunkJob, ChunkModelCount, DomainHost, DomainId, DomainKind, DomainStats, EdgeKind,
    EmbeddingCoverage, EmbeddingRow, EngramDescriptor, EngramId, EngramRecord, EngramSummary,
    FileStamp, FtsMode, GraphSlice, HostClaim, InboundRef, NewChunk, Page, RecentFilter, SearchHit,
    SearchMode, SearchQuery, Store, StoreInfo, StoredEngram,
};

/// A PostgreSQL-backed store. Open one with [`PostgresStore::open`].
pub struct PostgresStore {
    pool: PgPool,
    // The pinned transaction connection, set by `begin()` and cleared by
    // `commit()`/`rollback()`. Every statement routes to it while it is present;
    // the engine mutex serializes access so there is never contention.
    tx: TokioMutex<Option<PoolConnection<Postgres>>>,
    // A sanitized `host:port/dbname`, never the credentials. `None` only if the
    // url could not be parsed for display.
    db_path: Option<String>,
    schema_version: i64,
    // The schema this store operates in, when scoped (test isolation). `None`
    // uses the connection's default search_path.
    schema: Option<String>,
    // Tag name to id, to avoid re-interning tags during a sync. Cleared on
    // rollback and wipe so a rolled-back tag id is never served stale.
    tag_cache: Mutex<HashMap<String, i64>>,
    // The `chunk.embedding` column width last confirmed by this store, so a
    // same-width `store_embeddings` call skips the catalog lookup. `None`
    // until the first call; set by `ensure_embedding_width`, never by wipe
    // (wiping rows never changes the column type).
    embedding_width: Mutex<Option<i64>>,
    // Bumped by `ensure_embedding_width` every time its `ALTER TABLE ... TYPE
    // vector({dims})` actually runs (never on the already-matches fast path).
    // `replace_chunks` folds the current value into its carry SELECT's SQL
    // text, which changes sqlx's cache key and so guarantees a fresh prepare
    // after every width change; see the module doc for why this exists.
    // `AtomicU64` rather than the `Mutex<_>` used elsewhere because it is only
    // ever read or bumped, never checked-and-set as part of a larger decision;
    // `Ordering::Relaxed` is enough since the engine mutex already serializes
    // all store access, so there is never a concurrent reader to order against.
    embedding_generation: AtomicU64,
    // The last computed embedding coverage snapshot, shared by effective_mode,
    // the search staleness gate and CLI status so they issue one aggregate scan,
    // not three. `None` means recompute on the next read. Interior mutability with
    // the same std::sync discipline as tag_cache: the guard is taken in a tight
    // scope and never held across an await (clippy::await_holding_lock).
    //
    // Invalidated (set to `None`) by exactly the Store methods that can change a
    // chunk's embedding state, derived from the trait: store_embeddings (writes
    // embeddings), replace_chunks (deletes and reinserts an engram's chunks),
    // delete_engram (deletes an engram's chunks), clear_domain (deletes a domain's
    // chunks), wipe (deletes every chunk) and rollback (reverts any of the above
    // that ran inside the transaction). upsert_engram, upsert_engram_checked and
    // rename_engram never touch the chunk table, so they are not invalidators.
    coverage_cache: Mutex<Option<EmbeddingCoverage>>,
}

/// A connection handle for one store method: either the pinned transaction
/// connection (holding the tx mutex for the method's duration) or a connection
/// borrowed from the pool for autocommit statements. Either way a single method
/// runs all its statements on one connection, reproducing Turso's model.
enum Conn<'a> {
    Pinned(MutexGuard<'a, Option<PoolConnection<Postgres>>>),
    Owned(PoolConnection<Postgres>),
}

impl Conn<'_> {
    fn as_mut(&mut self) -> &mut PgConnection {
        match self {
            Conn::Pinned(g) => g.as_mut().expect("pinned connection present"),
            Conn::Owned(c) => c,
        }
    }
}

impl PostgresStore {
    /// Open a pool at `url` (validated by the caller) and migrate. Uses the
    /// connection's default search_path. `max_connections` is small: the engine
    /// mutex plus the pinned-connection transaction model means effective write
    /// concurrency is one, matching Turso.
    pub async fn open(url: &str) -> Result<PostgresStore> {
        Self::build(url, None).await
    }

    /// Open a pool at `url`, routing every connection to `schema` (created if
    /// absent) via `search_path`, and migrate into it. The `public` schema stays
    /// on the path so the shared `vector` extension type still resolves. This is
    /// the per-test isolation seam for the parity suite; drop it afterwards with
    /// [`PostgresStore::drop_schema`].
    pub async fn open_in_schema(url: &str, schema: &str) -> Result<PostgresStore> {
        Self::build(url, Some(schema.to_string())).await
    }

    async fn build(url: &str, schema: Option<String>) -> Result<PostgresStore> {
        let db_path = sanitize_url(url);
        let mut opts = PgPoolOptions::new().max_connections(4);
        if let Some(s) = schema.clone() {
            let ident = quote_ident(&s);
            let setup =
                format!("CREATE SCHEMA IF NOT EXISTS {ident}; SET search_path TO {ident}, public");
            opts = opts.after_connect(move |conn, _meta| {
                let setup = setup.clone();
                Box::pin(async move {
                    sqlx::raw_sql(AssertSqlSafe(setup)).execute(conn).await?;
                    Ok(())
                })
            });
        }
        let pool = opts.connect(url).await.map_err(IndexError::from)?;
        let mut conn = pool.acquire().await.map_err(IndexError::from)?;
        let schema_version = migrations::apply(&mut conn).await?;
        drop(conn);
        Ok(PostgresStore {
            pool,
            tx: TokioMutex::new(None),
            db_path,
            schema_version,
            schema,
            tag_cache: Mutex::new(HashMap::new()),
            embedding_width: Mutex::new(None),
            embedding_generation: AtomicU64::new(0),
            coverage_cache: Mutex::new(None),
        })
    }

    /// Drop the cached embedding coverage snapshot so the next read recomputes it.
    /// Called by every mutator that can change a chunk's embedding state. Kept in a
    /// tight scope with no await so the std::sync guard never crosses a suspension
    /// point.
    fn invalidate_coverage(&self) {
        *self.coverage_cache.lock().unwrap() = None;
    }

    /// Recompute the embedding coverage snapshot from the chunk table. This is the
    /// one aggregate scan the cached `embedding_coverage()` fills from on a miss.
    async fn compute_coverage(&self) -> Result<EmbeddingCoverage> {
        let mut conn = self.acquire().await?;
        let c = conn.as_mut();
        let total = scalar_i64(&mut *c, "SELECT count(*) FROM chunk", vec![])
            .await?
            .max(0) as usize;
        let rows = query_all(
            &mut *c,
            "SELECT model, dims, count(*) FROM chunk WHERE embedding IS NOT NULL \
             GROUP BY model, dims ORDER BY count(*) DESC",
            vec![],
        )
        .await?;
        let mut models = Vec::new();
        let mut embedded = 0usize;
        for r in &rows {
            let count = cell_i64(r, 2).unwrap_or(0).max(0) as usize;
            embedded += count;
            let model = cell_text(r, 0).unwrap_or_default();
            if model.is_empty() {
                continue;
            }
            models.push(ChunkModelCount {
                model,
                dims: cell_i64(r, 1).unwrap_or(0).max(0) as usize,
                count,
            });
        }
        Ok(EmbeddingCoverage {
            total_chunks: total,
            embedded_chunks: embedded,
            models,
        })
    }

    /// Drop the schema this store was opened in (test teardown). A no-op for a
    /// store opened without a schema.
    pub async fn drop_schema(&self) -> Result<()> {
        if let Some(schema) = &self.schema {
            let mut conn = self.pool.acquire().await.map_err(IndexError::from)?;
            sqlx::query(AssertSqlSafe(format!(
                "DROP SCHEMA IF EXISTS {} CASCADE",
                quote_ident(schema)
            )))
            .execute(&mut *conn)
            .await
            .map_err(IndexError::from)?;
        }
        Ok(())
    }

    /// Acquire the connection this method runs on: the pinned transaction
    /// connection when a transaction is open, else one borrowed from the pool.
    async fn acquire(&self) -> Result<Conn<'_>> {
        let guard = self.tx.lock().await;
        if guard.is_some() {
            Ok(Conn::Pinned(guard))
        } else {
            drop(guard);
            Ok(Conn::Owned(
                self.pool.acquire().await.map_err(IndexError::from)?,
            ))
        }
    }

    /// Intern a tag name, returning its id, using the in-process cache. Runs on
    /// the caller's connection so it stays inside the current transaction.
    async fn tag_id(&self, conn: &mut PgConnection, name: &str) -> Result<i64> {
        let cached = self.tag_cache.lock().unwrap().get(name).copied();
        if let Some(id) = cached {
            return Ok(id);
        }
        let row: (i64,) = sqlx::query_as(
            "INSERT INTO tag(name) VALUES($1) ON CONFLICT(name) DO UPDATE SET name=EXCLUDED.name RETURNING id",
        )
        .bind(name)
        .fetch_one(&mut *conn)
        .await
        .map_err(IndexError::from)?;
        self.tag_cache
            .lock()
            .unwrap()
            .insert(name.to_string(), row.0);
        Ok(row.0)
    }

    /// Make the `chunk.embedding` column's width match `dims`, resizing it
    /// first when it does not. `dims` always comes from the active provider,
    /// so a resize means the provider (or its model) changed, which already
    /// invalidates every stored vector through the staleness machinery in
    /// [`crate::postgres::search`]: stored embeddings at another width or
    /// model are already treated as stale and re-embedded in the background,
    /// so this resize rides that invalidation rather than adding a new one.
    /// The `USING NULL` clears every existing vector regardless of its old
    /// width, since none of them are valid at the new one anyway. The HNSW
    /// index is dropped first and recreated after, because Postgres refuses
    /// to change a column's type while an index depends on it.
    ///
    /// Idempotent and cheap once the width is known: a single cached
    /// comparison, or failing that one `pg_attribute` lookup, on every call
    /// that already matches.
    ///
    /// The `ALTER ... TYPE` below changes the `chunk.embedding` column's
    /// typmod, which invalidates any sqlx-cached prepared statement whose
    /// result row includes that raw column (Postgres error: "cached plan must
    /// not change result type"). The `clear_cached_statements` at the end
    /// sheds `conn`'s own stale plans but cannot reach the pool's other
    /// connections, so on a successful resize this bumps
    /// `embedding_generation`: `replace_chunks`' carry SELECT folds the new
    /// value into its SQL text, which changes sqlx's cache key and forces a
    /// fresh prepare everywhere, on this connection and every other one,
    /// without needing to reach them. See the module doc for the full hazard
    /// and why that statement cannot instead retry in place.
    async fn ensure_embedding_width(&self, conn: &mut PgConnection, dims: usize) -> Result<()> {
        let dims = dims as i64;
        if *self.embedding_width.lock().unwrap() == Some(dims) {
            return Ok(());
        }
        // `atttypmod` on a pgvector column is the declared dimension count
        // itself (pgvector stores it unmodified, unlike the `length + 4`
        // convention `char(n)`/`varchar(n)` use), so no decoding is needed.
        // `to_regclass` resolves through the connection's `search_path`, so
        // this finds the right `chunk` table under per-test schema isolation
        // without any schema-qualification.
        let current: Option<(i32,)> = sqlx::query_as(
            "SELECT atttypmod FROM pg_attribute \
             WHERE attrelid = to_regclass('chunk') AND attname = 'embedding' AND NOT attisdropped",
        )
        .fetch_optional(&mut *conn)
        .await
        .map_err(IndexError::from)?;
        if current.map(|(t,)| t as i64) == Some(dims) {
            *self.embedding_width.lock().unwrap() = Some(dims);
            return Ok(());
        }
        sqlx::query("DROP INDEX IF EXISTS idx_chunk_embedding")
            .execute(&mut *conn)
            .await
            .map_err(IndexError::from)?;
        sqlx::query(AssertSqlSafe(format!(
            "ALTER TABLE chunk ALTER COLUMN embedding TYPE vector({dims}) USING NULL"
        )))
        .execute(&mut *conn)
        .await
        .map_err(IndexError::from)?;
        sqlx::query(
            "CREATE INDEX idx_chunk_embedding ON chunk USING hnsw (embedding vector_cosine_ops)",
        )
        .execute(&mut *conn)
        .await
        .map_err(IndexError::from)?;
        // The DDL above just changed the `chunk.embedding` column's typmod, so
        // any plan this connection had cached for a statement returning that
        // column is now stale. `clear_cached_statements` sheds `conn`'s own
        // stale plans directly; it only reaches this one connection, so it is
        // a courtesy for whichever statements this connection still might run
        // under the old SQL text (there are none, `replace_chunks` is the only
        // one and it always carries the current generation), not what makes
        // the pool's other connections safe. `embedding_generation` below is
        // what does that, by changing the cache key everywhere at once.
        Connection::clear_cached_statements(&mut *conn)
            .await
            .map_err(IndexError::from)?;
        self.embedding_generation.fetch_add(1, Ordering::Relaxed);
        *self.embedding_width.lock().unwrap() = Some(dims);
        Ok(())
    }
}

// --- helpers -----------------------------------------------------------------

/// A dynamically bound parameter for the query builder. The search planner and
/// the filtered listings build their WHERE clause at runtime, so their binds are
/// heterogeneous; the fixed-shape statements bind typed values directly instead.
pub(super) enum Param {
    Text(String),
    Int(i64),
    Vector(pgvector::Vector),
    // Nullable variants for multi-row child inserts, where a NULL context,
    // `to_domain`, or a carried-over chunk's model/dims/embedding binds as a
    // typed NULL rather than a literal.
    TextOpt(Option<String>),
    IntOpt(Option<i64>),
    VectorOpt(Option<pgvector::Vector>),
}

fn bind_all<'q>(
    mut q: Query<'q, Postgres, PgArguments>,
    params: Vec<Param>,
) -> Query<'q, Postgres, PgArguments> {
    for p in params {
        q = match p {
            Param::Text(s) => q.bind(s),
            Param::Int(i) => q.bind(i),
            Param::Vector(v) => q.bind(v),
            Param::TextOpt(s) => q.bind(s),
            Param::IntOpt(i) => q.bind(i),
            Param::VectorOpt(v) => q.bind(v),
        };
    }
    q
}

/// Execute a statement with dynamically bound parameters, returning the affected
/// row count. The multi-row child inserts route through this; the RETURNING
/// observation insert uses [`query_all`] instead.
pub(super) async fn exec(conn: &mut PgConnection, sql: &str, params: Vec<Param>) -> Result<u64> {
    Ok(bind_all(sqlx::query(AssertSqlSafe(sql)), params)
        .execute(&mut *conn)
        .await
        .map_err(IndexError::from)?
        .rows_affected())
}

/// The maximum number of value tuples in one multi-row INSERT. Postgres caps a
/// statement at 65535 bind parameters, far above this, so the chunk size stays in
/// step with the Turso backend rather than tracking a different ceiling.
const INSERT_CHUNK: usize = 100;

/// Build the parenthesized value tuples for a multi-row INSERT: `count` tuples of
/// `width` sequential `$n` placeholders each, plus an optional fixed trailing
/// column (a literal `NULL` `to_id` for relations and links). `count` must be
/// non-zero; callers iterate chunks of a possibly-empty slice and skip the
/// statement entirely when there is nothing to insert.
fn value_rows(width: usize, count: usize, trailing: Option<&str>) -> String {
    let mut n = 1;
    let mut rows = Vec::with_capacity(count);
    for _ in 0..count {
        let mut cells = Vec::with_capacity(width + usize::from(trailing.is_some()));
        for _ in 0..width {
            cells.push(format!("${n}"));
            n += 1;
        }
        if let Some(t) = trailing {
            cells.push(t.to_string());
        }
        rows.push(format!("({})", cells.join(",")));
    }
    rows.join(",")
}

/// The multi-row observation INSERT for `count` rows, returning each new row's id
/// with its source line so observation tags map to the right observation without
/// depending on `RETURNING` row order.
fn observation_insert_sql(count: usize) -> String {
    format!(
        "INSERT INTO observation(engram_id, line, category, content, context) VALUES {} RETURNING id, line",
        value_rows(5, count, None)
    )
}

pub(super) async fn query_all(
    conn: &mut PgConnection,
    sql: &str,
    params: Vec<Param>,
) -> Result<Vec<PgRow>> {
    bind_all(sqlx::query(AssertSqlSafe(sql)), params)
        .fetch_all(&mut *conn)
        .await
        .map_err(IndexError::from)
}

pub(super) async fn query_first(
    conn: &mut PgConnection,
    sql: &str,
    params: Vec<Param>,
) -> Result<Option<PgRow>> {
    bind_all(sqlx::query(AssertSqlSafe(sql)), params)
        .fetch_optional(&mut *conn)
        .await
        .map_err(IndexError::from)
}

pub(super) async fn scalar_i64(
    conn: &mut PgConnection,
    sql: &str,
    params: Vec<Param>,
) -> Result<i64> {
    Ok(query_first(conn, sql, params)
        .await?
        .and_then(|r| cell_i64(&r, 0))
        .unwrap_or(0))
}

pub(super) fn cell_i64(row: &PgRow, idx: usize) -> Option<i64> {
    row.try_get::<Option<i64>, _>(idx).ok().flatten()
}

pub(super) fn cell_text(row: &PgRow, idx: usize) -> Option<String> {
    row.try_get::<Option<String>, _>(idx).ok().flatten()
}

pub(super) fn cell_real(row: &PgRow, idx: usize) -> Option<f64> {
    row.try_get::<Option<f64>, _>(idx).ok().flatten()
}

fn descriptor_from_row(r: &PgRow) -> EngramDescriptor {
    EngramDescriptor {
        id: EngramId(cell_i64(r, 0).unwrap_or(0)),
        domain_id: DomainId(cell_i64(r, 1).unwrap_or(0)),
        domain: cell_text(r, 2).unwrap_or_default(),
        path: cell_text(r, 3).unwrap_or_default(),
        permalink: cell_text(r, 4).unwrap_or_default(),
        title: cell_text(r, 5).unwrap_or_default(),
        engram_type: cell_text(r, 6).unwrap_or_default(),
        status: cell_text(r, 7).unwrap_or_default(),
    }
}

/// Escape `%`, `_` and `\` for a `LIKE ... ESCAPE '\'` prefix pattern.
pub(super) fn like_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c == '%' || c == '_' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Quote a schema identifier for interpolation, doubling any embedded quote. The
/// schema names come from the test harness (safe identifiers) but this keeps the
/// interpolation defensive.
fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// The stored discriminator string for a domain kind.
fn kind_str(kind: DomainKind) -> &'static str {
    match kind {
        DomainKind::File => "file",
        DomainKind::Virtual => "virtual",
    }
}

/// Strip the scheme, any credentials and the query string from a connection url,
/// leaving `host:port/dbname` for display. Never returns credentials.
fn sanitize_url(url: &str) -> Option<String> {
    let after_scheme = url.split("://").nth(1)?;
    let after_at = after_scheme
        .rsplit_once('@')
        .map(|(_, rest)| rest)
        .unwrap_or(after_scheme);
    let host_db = after_at.split(['?', '#']).next().unwrap_or(after_at);
    Some(host_db.to_string())
}

/// Delete the child rows recreated on every upsert. Chunk rows are NOT cleared
/// here so [`Store::replace_chunks`] can carry over embeddings whose fingerprint
/// is unchanged; deleting an engram clears its chunks in [`Store::delete_engram`].
async fn delete_children(conn: &mut PgConnection, engram_id: i64) -> Result<()> {
    sqlx::query(
        "DELETE FROM observation_tag WHERE observation_id IN (SELECT id FROM observation WHERE engram_id=$1)",
    )
    .bind(engram_id)
    .execute(&mut *conn)
    .await
    .map_err(IndexError::from)?;
    for sql in [
        "DELETE FROM observation WHERE engram_id=$1",
        "DELETE FROM engram_tag WHERE engram_id=$1",
        "DELETE FROM relation WHERE engram_id=$1",
        "DELETE FROM link WHERE engram_id=$1",
    ] {
        sqlx::query(sql)
            .bind(engram_id)
            .execute(&mut *conn)
            .await
            .map_err(IndexError::from)?;
    }
    Ok(())
}

#[async_trait]
impl Store for PostgresStore {
    async fn migrate(&self) -> Result<()> {
        let mut conn = self.acquire().await?;
        migrations::apply(conn.as_mut()).await?;
        Ok(())
    }

    async fn upsert_domain(
        &self,
        name: &str,
        path: Option<&str>,
        kind: DomainKind,
    ) -> Result<DomainId> {
        // A virtual domain stores `path = NULL`; `kind` discriminates on both
        // backends.
        let mut conn = self.acquire().await?;
        let row: (i64,) = sqlx::query_as(
            "INSERT INTO domain(name, path, kind) VALUES($1,$2,$3) \
             ON CONFLICT(name) DO UPDATE SET path=EXCLUDED.path, kind=EXCLUDED.kind RETURNING id",
        )
        .bind(name)
        .bind(path)
        .bind(kind_str(kind))
        .fetch_one(conn.as_mut())
        .await
        .map_err(IndexError::from)?;
        Ok(DomainId(row.0))
    }

    async fn file_stamps(&self, domain: DomainId) -> Result<HashMap<String, FileStamp>> {
        let mut conn = self.acquire().await?;
        let rows = sqlx::query("SELECT path, mtime, size, sha256 FROM engram WHERE domain_id=$1")
            .bind(domain.0)
            .fetch_all(conn.as_mut())
            .await
            .map_err(IndexError::from)?;
        let mut out = HashMap::with_capacity(rows.len());
        for r in &rows {
            let Some(path) = cell_text(r, 0) else {
                continue;
            };
            out.insert(
                path,
                FileStamp {
                    mtime: cell_i64(r, 1).unwrap_or(0),
                    size: cell_i64(r, 2).unwrap_or(0).max(0) as u64,
                    sha256: cell_text(r, 3).unwrap_or_default(),
                },
            );
        }
        Ok(out)
    }

    async fn upsert_engram(&self, domain: DomainId, record: &EngramRecord) -> Result<EngramId> {
        let mut conn = self.acquire().await?;
        let c = conn.as_mut();

        // One probe for both the existing-by-path row and a duplicate permalink
        // owned by a different path. Pre-checking the duplicate keeps a failing
        // unique-violation from aborting the surrounding batch transaction, so
        // the sync engine can collect it into `failed` instead.
        let probe = sqlx::query(
            "SELECT id, path FROM engram WHERE domain_id=$1 AND (path=$2 OR permalink=$3)",
        )
        .bind(domain.0)
        .bind(&record.path)
        .bind(&record.permalink)
        .fetch_all(&mut *c)
        .await
        .map_err(IndexError::from)?;
        let mut existing_id: Option<i64> = None;
        for r in &probe {
            let row_path = cell_text(r, 1).unwrap_or_default();
            if row_path == record.path {
                existing_id = cell_i64(r, 0);
            } else {
                return Err(IndexError::Constraint(format!(
                    "permalink '{}' already used by '{}'",
                    record.permalink, row_path
                )));
            }
        }

        // `RETURNING id` yields the id on both insert and conflict-update, so no
        // separate id lookup is needed.
        let row: (i64,) = sqlx::query_as(
            "INSERT INTO engram(domain_id, path, permalink, title, engram_type, status, \
             recorded_at, valid_from, valid_to, timestamp, description, content, metadata, \
             mtime, size, sha256) \
             VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13::jsonb,$14,$15,$16) \
             ON CONFLICT(domain_id, path) DO UPDATE SET \
             permalink=EXCLUDED.permalink, title=EXCLUDED.title, engram_type=EXCLUDED.engram_type, \
             status=EXCLUDED.status, recorded_at=EXCLUDED.recorded_at, valid_from=EXCLUDED.valid_from, \
             valid_to=EXCLUDED.valid_to, timestamp=EXCLUDED.timestamp, description=EXCLUDED.description, \
             content=EXCLUDED.content, metadata=EXCLUDED.metadata, mtime=EXCLUDED.mtime, \
             size=EXCLUDED.size, sha256=EXCLUDED.sha256 \
             RETURNING id",
        )
        .bind(domain.0)
        .bind(&record.path)
        .bind(&record.permalink)
        .bind(&record.title)
        .bind(&record.engram_type)
        .bind(&record.status)
        .bind(record.recorded_at.as_deref())
        .bind(record.valid_from.as_deref())
        .bind(record.valid_to.as_deref())
        .bind(record.timestamp.as_deref())
        .bind(record.description.as_deref())
        .bind(&record.content)
        .bind(record.metadata.to_string())
        .bind(record.stamp.mtime)
        .bind(record.stamp.size as i64)
        .bind(&record.stamp.sha256)
        .fetch_one(&mut *c)
        .await
        .map_err(IndexError::from)?;
        let engram_id = row.0;

        // Only an update needs its stale child rows cleared first.
        if existing_id.is_some() {
            delete_children(&mut *c, engram_id).await?;
        }

        // Observations: insert in chunks and read each new row's id back joined
        // on its source line (unique within one engram), so observation tags map
        // to the right observation without relying on RETURNING row order.
        let mut obs_id_by_line: HashMap<i64, i64> = HashMap::new();
        for batch in record.observations.chunks(INSERT_CHUNK) {
            let mut params: Vec<Param> = Vec::with_capacity(batch.len() * 5);
            for obs in batch {
                params.push(Param::Int(engram_id));
                params.push(Param::Int(obs.line as i64));
                params.push(Param::Text(obs.category.clone()));
                params.push(Param::Text(obs.content.clone()));
                params.push(Param::TextOpt(obs.context.clone()));
            }
            let rows = query_all(&mut *c, &observation_insert_sql(batch.len()), params).await?;
            for r in &rows {
                if let (Some(id), Some(line)) = (cell_i64(r, 0), cell_i64(r, 1)) {
                    obs_id_by_line.insert(line, id);
                }
            }
        }

        // Observation tags: intern each tag through the cache, then insert the
        // (observation, tag) pairs in multi-row statements.
        let mut obs_tag_pairs: Vec<(i64, i64)> = Vec::new();
        for obs in &record.observations {
            let Some(&oid) = obs_id_by_line.get(&(obs.line as i64)) else {
                continue;
            };
            for tag in &obs.tags {
                let tid = self.tag_id(&mut *c, tag).await?;
                obs_tag_pairs.push((oid, tid));
            }
        }
        for batch in obs_tag_pairs.chunks(INSERT_CHUNK) {
            let mut params: Vec<Param> = Vec::with_capacity(batch.len() * 2);
            for (oid, tid) in batch {
                params.push(Param::Int(*oid));
                params.push(Param::Int(*tid));
            }
            let sql = format!(
                "INSERT INTO observation_tag(observation_id, tag_id) VALUES {} ON CONFLICT DO NOTHING",
                value_rows(2, batch.len(), None)
            );
            exec(&mut *c, &sql, params).await?;
        }

        for batch in record.relations.chunks(INSERT_CHUNK) {
            let mut params: Vec<Param> = Vec::with_capacity(batch.len() * 6);
            for rel in batch {
                params.push(Param::Int(engram_id));
                params.push(Param::Int(domain.0));
                params.push(Param::Int(rel.line as i64));
                params.push(Param::Text(rel.rel_type.clone()));
                params.push(Param::Text(rel.to_target.clone()));
                params.push(Param::TextOpt(rel.to_domain.clone()));
            }
            let sql = format!(
                "INSERT INTO relation(engram_id, domain_id, line, rel_type, to_target, to_domain, to_id) VALUES {}",
                value_rows(6, batch.len(), Some("NULL"))
            );
            exec(&mut *c, &sql, params).await?;
        }

        for batch in record.links.chunks(INSERT_CHUNK) {
            let mut params: Vec<Param> = Vec::with_capacity(batch.len() * 5);
            for link in batch {
                params.push(Param::Int(engram_id));
                params.push(Param::Int(domain.0));
                params.push(Param::Int(link.line as i64));
                params.push(Param::Text(link.to_target.clone()));
                params.push(Param::TextOpt(link.to_domain.clone()));
            }
            let sql = format!(
                "INSERT INTO link(engram_id, domain_id, line, to_target, to_domain, to_id) VALUES {}",
                value_rows(5, batch.len(), Some("NULL"))
            );
            exec(&mut *c, &sql, params).await?;
        }

        // Engram tags: intern each tag, then insert the pairs in multi-row
        // statements.
        let mut tag_ids: Vec<i64> = Vec::with_capacity(record.tags.len());
        for tag in &record.tags {
            tag_ids.push(self.tag_id(&mut *c, tag).await?);
        }
        for batch in tag_ids.chunks(INSERT_CHUNK) {
            let mut params: Vec<Param> = Vec::with_capacity(batch.len() * 2);
            for tid in batch {
                params.push(Param::Int(engram_id));
                params.push(Param::Int(*tid));
            }
            let sql = format!(
                "INSERT INTO engram_tag(engram_id, tag_id) VALUES {} ON CONFLICT DO NOTHING",
                value_rows(2, batch.len(), None)
            );
            exec(&mut *c, &sql, params).await?;
        }

        Ok(EngramId(engram_id))
    }

    async fn upsert_engram_checked(
        &self,
        domain: DomainId,
        record: &EngramRecord,
        expected_sha: Option<&str>,
    ) -> Result<EngramId> {
        // Compare-and-swap: if a row exists at this path and the caller's
        // expected sha no longer matches the stored one, refuse. The engine
        // holds the transaction open (the pinned connection), so the compare and
        // the write are one atomic unit.
        if let Some(expected) = expected_sha {
            let mut conn = self.acquire().await?;
            let stored = sqlx::query("SELECT sha256 FROM engram WHERE domain_id=$1 AND path=$2")
                .bind(domain.0)
                .bind(&record.path)
                .fetch_optional(conn.as_mut())
                .await
                .map_err(IndexError::from)?
                .and_then(|r| cell_text(&r, 0));
            drop(conn);
            if let Some(found) = stored
                && found != expected
            {
                return Err(IndexError::StaleEdit {
                    expected: expected.to_string(),
                    found,
                });
            }
        }
        self.upsert_engram(domain, record).await
    }

    async fn engram_content(&self, domain: DomainId, path: &str) -> Result<Option<String>> {
        let mut conn = self.acquire().await?;
        let row = sqlx::query("SELECT content FROM engram WHERE domain_id=$1 AND path=$2")
            .bind(domain.0)
            .bind(path)
            .fetch_optional(conn.as_mut())
            .await
            .map_err(IndexError::from)?;
        Ok(row.and_then(|r| cell_text(&r, 0)))
    }

    async fn all_engram_contents(&self, domain: DomainId) -> Result<Vec<StoredEngram>> {
        let mut conn = self.acquire().await?;
        let rows = sqlx::query(
            "SELECT path, permalink, content, sha256 FROM engram WHERE domain_id=$1 ORDER BY path",
        )
        .bind(domain.0)
        .fetch_all(conn.as_mut())
        .await
        .map_err(IndexError::from)?;
        Ok(rows
            .iter()
            .map(|r| StoredEngram {
                path: cell_text(r, 0).unwrap_or_default(),
                permalink: cell_text(r, 1).unwrap_or_default(),
                content: cell_text(r, 2).unwrap_or_default(),
                sha256: cell_text(r, 3).unwrap_or_default(),
            })
            .collect())
    }

    async fn clear_domain(&self, domain: DomainId) -> Result<()> {
        // Deletes this domain's chunks, so the coverage snapshot is now stale.
        self.invalidate_coverage();
        // Delete a single domain's engram and child rows, keeping the domain
        // row. Child rows first so the enforced foreign keys are satisfied.
        let mut conn = self.acquire().await?;
        let c = conn.as_mut();
        for sql in [
            "DELETE FROM observation_tag WHERE observation_id IN \
             (SELECT o.id FROM observation o JOIN engram e ON e.id=o.engram_id WHERE e.domain_id=$1)",
            "DELETE FROM engram_tag WHERE engram_id IN (SELECT id FROM engram WHERE domain_id=$1)",
            "DELETE FROM chunk WHERE engram_id IN (SELECT id FROM engram WHERE domain_id=$1)",
            "DELETE FROM observation WHERE engram_id IN (SELECT id FROM engram WHERE domain_id=$1)",
            "DELETE FROM relation WHERE domain_id=$1",
            "DELETE FROM link WHERE domain_id=$1",
            "DELETE FROM engram WHERE domain_id=$1",
        ] {
            sqlx::query(sql)
                .bind(domain.0)
                .execute(&mut *c)
                .await
                .map_err(IndexError::from)?;
        }
        Ok(())
    }

    async fn delete_engram(&self, domain: DomainId, path: &str) -> Result<()> {
        // Deletes the engram's chunks, so the coverage snapshot is now stale.
        self.invalidate_coverage();
        let mut conn = self.acquire().await?;
        let c = conn.as_mut();
        let id = sqlx::query("SELECT id FROM engram WHERE domain_id=$1 AND path=$2")
            .bind(domain.0)
            .bind(path)
            .fetch_optional(&mut *c)
            .await
            .map_err(IndexError::from)?
            .and_then(|r| cell_i64(&r, 0));
        if let Some(id) = id {
            delete_children(&mut *c, id).await?;
            sqlx::query("DELETE FROM chunk WHERE engram_id=$1")
                .bind(id)
                .execute(&mut *c)
                .await
                .map_err(IndexError::from)?;
            sqlx::query("DELETE FROM engram WHERE id=$1")
                .bind(id)
                .execute(&mut *c)
                .await
                .map_err(IndexError::from)?;
        }
        Ok(())
    }

    async fn rename_engram(&self, domain: DomainId, from: &str, to: &str) -> Result<()> {
        // The permalink follows the move only when it was path-derived, so an
        // explicit frontmatter permalink is preserved across a move.
        let old_slug = crystalline_core::slugify(from);
        let new_slug = crystalline_core::slugify(to);
        let mut conn = self.acquire().await?;
        sqlx::query(
            "UPDATE engram SET path=$1, \
             permalink = CASE WHEN permalink=$2 THEN $3 ELSE permalink END \
             WHERE domain_id=$4 AND path=$5",
        )
        .bind(to)
        .bind(&old_slug)
        .bind(&new_slug)
        .bind(domain.0)
        .bind(from)
        .execute(conn.as_mut())
        .await
        .map_err(IndexError::from)?;
        Ok(())
    }

    async fn resolve_pending_relations(&self, domain: DomainId) -> Result<u64> {
        // One statement. Target domain is `to_domain` when set, else the
        // relation's own domain. Prefer a permalink match, then a title match.
        let tgt_dom = "COALESCE((SELECT d.id FROM domain d WHERE d.name = relation.to_domain), relation.domain_id)";
        let by_perma = format!(
            "(SELECT e.id FROM engram e WHERE e.permalink = relation.to_target AND e.domain_id = {tgt_dom} LIMIT 1)"
        );
        let by_title = format!(
            "(SELECT e.id FROM engram e WHERE lower(e.title) = lower(relation.to_target) AND e.domain_id = {tgt_dom} LIMIT 1)"
        );
        let sql = format!(
            "UPDATE relation SET to_id = COALESCE({by_perma}, {by_title}) \
             WHERE relation.to_id IS NULL AND relation.domain_id = $1 \
             AND (EXISTS (SELECT 1 FROM engram e WHERE e.permalink = relation.to_target AND e.domain_id = {tgt_dom}) \
                  OR EXISTS (SELECT 1 FROM engram e WHERE lower(e.title) = lower(relation.to_target) AND e.domain_id = {tgt_dom}))"
        );
        let mut conn = self.acquire().await?;
        let done = sqlx::query(AssertSqlSafe(sql))
            .bind(domain.0)
            .execute(conn.as_mut())
            .await
            .map_err(IndexError::from)?;
        Ok(done.rows_affected())
    }

    async fn lookup_id(&self, domain: &str, permalink: &str) -> Result<Option<EngramId>> {
        let mut conn = self.acquire().await?;
        let row = sqlx::query(
            "SELECT e.id FROM engram e JOIN domain d ON d.id=e.domain_id WHERE d.name=$1 AND e.permalink=$2",
        )
        .bind(domain)
        .bind(permalink)
        .fetch_optional(conn.as_mut())
        .await
        .map_err(IndexError::from)?;
        Ok(row.and_then(|r| cell_i64(&r, 0)).map(EngramId))
    }

    async fn find_engram(&self, domain: &str, key: &str) -> Result<Option<EngramDescriptor>> {
        let mut conn = self.acquire().await?;
        let row = sqlx::query(
            "SELECT e.id, e.domain_id, d.name, e.path, e.permalink, e.title, e.engram_type, e.status \
             FROM engram e JOIN domain d ON d.id=e.domain_id \
             WHERE d.name=$1 AND (e.permalink=$2 OR lower(e.title)=lower($2)) \
             ORDER BY CASE WHEN e.permalink=$2 THEN 0 ELSE 1 END, e.path LIMIT 1",
        )
        .bind(domain)
        .bind(key)
        .fetch_optional(conn.as_mut())
        .await
        .map_err(IndexError::from)?;
        Ok(row.as_ref().map(descriptor_from_row))
    }

    async fn find_engram_any(&self, key: &str) -> Result<Vec<EngramDescriptor>> {
        let mut conn = self.acquire().await?;
        let rows = sqlx::query(
            "SELECT e.id, e.domain_id, d.name, e.path, e.permalink, e.title, e.engram_type, e.status \
             FROM engram e JOIN domain d ON d.id=e.domain_id \
             WHERE e.permalink=$1 OR lower(e.title)=lower($1) \
             ORDER BY CASE WHEN e.permalink=$1 THEN 0 ELSE 1 END, d.name, e.path",
        )
        .bind(key)
        .fetch_all(conn.as_mut())
        .await
        .map_err(IndexError::from)?;
        Ok(rows.iter().map(descriptor_from_row).collect())
    }

    async fn list_engrams(
        &self,
        domain: &str,
        path_prefix: Option<&str>,
        engram_type: Option<&str>,
    ) -> Result<Vec<EngramDescriptor>> {
        let mut clauses = vec!["d.name=$1".to_string()];
        let mut params = vec![Param::Text(domain.to_string())];
        let mut n = 2;
        if let Some(prefix) = path_prefix.filter(|p| !p.is_empty()) {
            clauses.push(format!("e.path LIKE ${n} ESCAPE '\\'"));
            params.push(Param::Text(format!("{}%", like_escape(prefix))));
            n += 1;
        }
        if let Some(t) = engram_type {
            clauses.push(format!("e.engram_type=${n}"));
            params.push(Param::Text(t.to_string()));
        }
        let sql = format!(
            "SELECT e.id, e.domain_id, d.name, e.path, e.permalink, e.title, e.engram_type, e.status \
             FROM engram e JOIN domain d ON d.id=e.domain_id WHERE {} ORDER BY e.path",
            clauses.join(" AND ")
        );
        let mut conn = self.acquire().await?;
        let rows = query_all(conn.as_mut(), &sql, params).await?;
        Ok(rows.iter().map(descriptor_from_row).collect())
    }

    async fn inbound_refs(
        &self,
        engram_id: EngramId,
        domain_id: DomainId,
        permalink: &str,
        title: &str,
    ) -> Result<Vec<InboundRef>> {
        let mut conn = self.acquire().await?;
        let rows = sqlx::query(
            "SELECT d.name, r.domain_id, e.path, r.to_target, 0 AS kind \
             FROM relation r JOIN engram e ON e.id=r.engram_id JOIN domain d ON d.id=e.domain_id \
             WHERE r.to_id=$1 \
                OR (r.to_id IS NULL AND r.domain_id=$2 AND r.to_domain IS NULL \
                    AND (r.to_target=$3 OR lower(r.to_target)=lower($4))) \
             UNION ALL \
             SELECT d.name, l.domain_id, e.path, l.to_target, 1 AS kind \
             FROM link l JOIN engram e ON e.id=l.engram_id JOIN domain d ON d.id=e.domain_id \
             WHERE l.to_id=$1 \
                OR (l.to_id IS NULL AND l.domain_id=$2 AND l.to_domain IS NULL \
                    AND (l.to_target=$3 OR lower(l.to_target)=lower($4)))",
        )
        .bind(engram_id.0)
        .bind(domain_id.0)
        .bind(permalink)
        .bind(title)
        .fetch_all(conn.as_mut())
        .await
        .map_err(IndexError::from)?;
        Ok(rows
            .iter()
            .map(|r| InboundRef {
                src_domain: cell_text(r, 0).unwrap_or_default(),
                src_domain_id: DomainId(cell_i64(r, 1).unwrap_or(0)),
                src_path: cell_text(r, 2).unwrap_or_default(),
                to_target: cell_text(r, 3).unwrap_or_default(),
                kind: if cell_i64(r, 4).unwrap_or(0) == 0 {
                    EdgeKind::Relation
                } else {
                    EdgeKind::Link
                },
            })
            .collect())
    }

    async fn search(&self, query: &SearchQuery) -> Result<Page<SearchHit>> {
        // Compute the coverage snapshot for the semantic and hybrid modes (which
        // gate on embedding staleness) BEFORE acquiring the search connection:
        // embedding_coverage() acquires its own connection, and holding two
        // acquisitions at once would relock the tx mutex on the pinned path. The
        // cache makes this essentially free once effective_mode has warmed it, and
        // folds the old second aggregate scan into the same shared snapshot. The
        // lexical modes never touch embeddings, so they skip the snapshot.
        let coverage = match query.mode {
            SearchMode::Semantic | SearchMode::Hybrid => Some(self.embedding_coverage().await?),
            _ => None,
        };
        let mut conn = self.acquire().await?;
        search::run_search(conn.as_mut(), query, coverage.as_ref()).await
    }

    async fn neighbors(&self, ids: &[EngramId], depth: u8) -> Result<GraphSlice> {
        let mut conn = self.acquire().await?;
        search::neighbors(conn.as_mut(), ids, depth).await
    }

    async fn recent(&self, filter: &RecentFilter) -> Result<Vec<EngramSummary>> {
        let mut where_clauses: Vec<String> = Vec::new();
        let mut params: Vec<Param> = Vec::new();
        let mut n = 1;

        if let Some(domains) = &filter.domains
            && !domains.is_empty()
        {
            let placeholders: Vec<String> = domains
                .iter()
                .map(|d| {
                    params.push(Param::Text(d.clone()));
                    let p = format!("${n}");
                    n += 1;
                    p
                })
                .collect();
            where_clauses.push(format!("d.name IN ({})", placeholders.join(",")));
        }
        if let Some(after) = &filter.after {
            where_clauses.push(format!("e.recorded_at >= ${n}"));
            params.push(Param::Text(after.clone()));
            n += 1;
        }
        if let Some(types) = &filter.engram_types
            && !types.is_empty()
        {
            let placeholders: Vec<String> = types
                .iter()
                .map(|t| {
                    params.push(Param::Text(t.clone()));
                    let p = format!("${n}");
                    n += 1;
                    p
                })
                .collect();
            where_clauses.push(format!("e.engram_type IN ({})", placeholders.join(",")));
        }

        let where_sql = if where_clauses.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", where_clauses.join(" AND "))
        };
        let limit = if filter.limit == 0 { 20 } else { filter.limit };
        let sql = format!(
            "SELECT d.name, e.permalink, e.title, e.engram_type, e.status, e.recorded_at, \
             (SELECT string_agg(t.name, ',') FROM engram_tag et JOIN tag t ON t.id=et.tag_id WHERE et.engram_id=e.id) \
             FROM engram e JOIN domain d ON d.id=e.domain_id {where_sql} \
             ORDER BY e.recorded_at DESC, e.permalink ASC LIMIT {limit}"
        );
        let mut conn = self.acquire().await?;
        let rows = query_all(conn.as_mut(), &sql, params).await?;
        let mut out = Vec::with_capacity(rows.len());
        for r in &rows {
            out.push(EngramSummary {
                domain: cell_text(r, 0).unwrap_or_default(),
                permalink: cell_text(r, 1).unwrap_or_default(),
                title: cell_text(r, 2).unwrap_or_default(),
                engram_type: cell_text(r, 3).unwrap_or_default(),
                status: cell_text(r, 4).unwrap_or_default(),
                recorded_at: cell_text(r, 5),
                tags: cell_text(r, 6)
                    .filter(|s| !s.is_empty())
                    .map(|s| s.split(',').map(str::to_string).collect())
                    .unwrap_or_default(),
            });
        }
        Ok(out)
    }

    async fn replace_chunks(&self, engram_id: EngramId, chunks: &[NewChunk]) -> Result<()> {
        // Deletes and reinserts this engram's chunks, so the coverage snapshot is
        // now stale.
        self.invalidate_coverage();
        let eid = engram_id.0;
        let mut conn = self.acquire().await?;
        let c = conn.as_mut();
        // Carry over the embedding of any chunk whose fingerprint is unchanged so
        // an edit only re-embeds the paragraphs that changed. The fingerprint
        // folds in the model id, so a chunk embedded by a different model does
        // not match and is left for re-embedding.
        //
        // This selects the raw `embedding` column, whose type includes the
        // column's typmod, so it is the one statement in this module exposed to
        // `ensure_embedding_width`'s DDL-vs-cached-plan hazard (see the module
        // doc). The trailing `/* w{generation} */` comment is inert to Postgres but not
        // to sqlx's statement cache: it makes the SQL text - and so the cache
        // key - change every time the column is resized, which forces a fresh
        // prepare against the current column shape on whichever connection runs
        // this next, without needing to reach every pooled connection directly.
        // This runs inside `sync`'s explicit transaction (the raw `BEGIN` in
        // `begin()`), where a failed statement would abort the whole
        // transaction with no way to recover by retrying in place, which is why
        // prevention rather than retry is the fix here.
        let generation = self.embedding_generation.load(Ordering::Relaxed);
        let carry_sql = format!(
            "SELECT text_hash, model, dims, embedding FROM chunk WHERE engram_id=$1 AND embedding IS NOT NULL /* w{generation} */"
        );
        let existing = sqlx::query(AssertSqlSafe(carry_sql))
            .bind(eid)
            .fetch_all(&mut *c)
            .await
            .map_err(IndexError::from)?;
        let mut carry: HashMap<String, (Option<String>, Option<i64>, pgvector::Vector)> =
            HashMap::new();
        for r in &existing {
            let Some(hash) = cell_text(r, 0) else {
                continue;
            };
            let model = r.try_get::<Option<String>, _>(1).ok().flatten();
            let dims = r.try_get::<Option<i64>, _>(2).ok().flatten();
            let Some(emb) = r.try_get::<Option<pgvector::Vector>, _>(3).ok().flatten() else {
                continue;
            };
            carry.insert(hash, (model, dims, emb));
        }

        sqlx::query("DELETE FROM chunk WHERE engram_id=$1")
            .bind(eid)
            .execute(&mut *c)
            .await
            .map_err(IndexError::from)?;

        // One 7-column multi-row insert for every chunk: a carry hit binds its
        // preserved model, dims and embedding, a miss binds NULL for all three
        // and leaves the chunk pending re-embedding.
        for batch in chunks.chunks(INSERT_CHUNK) {
            let mut params: Vec<Param> = Vec::with_capacity(batch.len() * 7);
            for chunk in batch {
                params.push(Param::Int(eid));
                params.push(Param::Int(chunk.seq));
                params.push(Param::Text(chunk.text.clone()));
                params.push(Param::Text(chunk.text_hash.clone()));
                match carry.get(&chunk.text_hash) {
                    Some((model, dims, emb)) => {
                        params.push(Param::TextOpt(model.clone()));
                        params.push(Param::IntOpt(*dims));
                        params.push(Param::VectorOpt(Some(emb.clone())));
                    }
                    None => {
                        params.push(Param::TextOpt(None));
                        params.push(Param::IntOpt(None));
                        params.push(Param::VectorOpt(None));
                    }
                }
            }
            let sql = format!(
                "INSERT INTO chunk(engram_id, seq, text, text_hash, model, dims, embedding) VALUES {}",
                value_rows(7, batch.len(), None)
            );
            exec(&mut *c, &sql, params).await?;
        }
        Ok(())
    }

    async fn chunks_needing_embedding(
        &self,
        model: &str,
        domains: Option<&[DomainId]>,
    ) -> Result<Vec<ChunkJob>> {
        // The base predicate is unchanged; an optional domain scope adds an
        // `engram_id IN (SELECT id FROM engram WHERE domain_id IN (...))` clause
        // so a non-host embeds only the chunks it owns. An empty scope matches
        // nothing (this instance hosts nothing embeddable).
        let mut sql = String::from(
            "SELECT id, engram_id, seq, text, text_hash FROM chunk \
             WHERE (embedding IS NULL OR model IS NULL OR model != $1)",
        );
        let mut params = vec![Param::Text(model.to_string())];
        if let Some(ids) = domains {
            if ids.is_empty() {
                return Ok(Vec::new());
            }
            let mut n = 2;
            let placeholders: Vec<String> = ids
                .iter()
                .map(|id| {
                    params.push(Param::Int(id.0));
                    let p = format!("${n}");
                    n += 1;
                    p
                })
                .collect();
            sql.push_str(&format!(
                " AND engram_id IN (SELECT id FROM engram WHERE domain_id IN ({}))",
                placeholders.join(",")
            ));
        }
        sql.push_str(" ORDER BY engram_id, seq");
        let mut conn = self.acquire().await?;
        let rows = query_all(conn.as_mut(), &sql, params).await?;
        Ok(rows
            .iter()
            .map(|r| ChunkJob {
                chunk_id: cell_i64(r, 0).unwrap_or(0),
                engram_id: cell_i64(r, 1).unwrap_or(0),
                seq: cell_i64(r, 2).unwrap_or(0),
                text: cell_text(r, 3).unwrap_or_default(),
                text_hash: cell_text(r, 4).unwrap_or_default(),
            })
            .collect())
    }

    async fn store_embeddings(&self, batch: &[EmbeddingRow], model: &str) -> Result<()> {
        // Writes embeddings, so the coverage snapshot is now stale.
        self.invalidate_coverage();
        // Validate the whole batch before any write so a bad row never leaves
        // earlier rows committed.
        for row in batch {
            if row.embedding.len() != row.dims {
                return Err(IndexError::Invalid(format!(
                    "embedding length {} does not match declared dims {}",
                    row.embedding.len(),
                    row.dims
                )));
            }
        }
        let mut conn = self.acquire().await?;
        let c = conn.as_mut();
        // `dims` is per row, and a width change may DDL (resize the column, drop
        // and recreate the index), so make each distinct width current once and
        // before the transaction opens rather than once per row inside it.
        let mut widths: Vec<usize> = Vec::new();
        for row in batch {
            if !widths.contains(&row.dims) {
                widths.push(row.dims);
            }
        }
        for dims in widths {
            self.ensure_embedding_width(&mut *c, dims).await?;
        }
        // The per-row updates then commit together, mirroring `wipe`.
        sqlx::query("BEGIN")
            .execute(&mut *c)
            .await
            .map_err(IndexError::from)?;
        for row in batch {
            let vector = pgvector::Vector::from(row.embedding.clone());
            if let Err(e) =
                sqlx::query("UPDATE chunk SET embedding=$1, dims=$2, model=$3 WHERE id=$4")
                    .bind(vector)
                    .bind(row.dims as i64)
                    .bind(model)
                    .bind(row.chunk_id)
                    .execute(&mut *c)
                    .await
            {
                let _ = sqlx::query("ROLLBACK").execute(&mut *c).await;
                return Err(e.into());
            }
        }
        sqlx::query("COMMIT")
            .execute(&mut *c)
            .await
            .map_err(IndexError::from)?;
        Ok(())
    }

    async fn embedding_coverage(&self) -> Result<EmbeddingCoverage> {
        // Fast path in a tight scope so the std::sync guard is dropped before the
        // recompute await below (clippy::await_holding_lock), tag_cache style.
        {
            let cached = self.coverage_cache.lock().unwrap();
            if let Some(cov) = cached.as_ref() {
                return Ok(cov.clone());
            }
        }
        let cov = self.compute_coverage().await?;
        *self.coverage_cache.lock().unwrap() = Some(cov.clone());
        Ok(cov)
    }

    async fn wipe(&self) -> Result<()> {
        // Deletes every chunk, so the coverage snapshot is now stale.
        self.invalidate_coverage();
        let mut conn = self.acquire().await?;
        let c = conn.as_mut();
        sqlx::query("BEGIN")
            .execute(&mut *c)
            .await
            .map_err(IndexError::from)?;
        for table in migrations::WIPE_TABLES {
            if let Err(e) = sqlx::query(AssertSqlSafe(format!("DELETE FROM {table}")))
                .execute(&mut *c)
                .await
            {
                let _ = sqlx::query("ROLLBACK").execute(&mut *c).await;
                return Err(e.into());
            }
        }
        sqlx::query("COMMIT")
            .execute(&mut *c)
            .await
            .map_err(IndexError::from)?;
        self.tag_cache.lock().unwrap().clear();
        Ok(())
    }

    async fn record_sync(&self, domain: DomainId, when: &str) -> Result<()> {
        let mut conn = self.acquire().await?;
        sqlx::query("UPDATE domain SET last_sync=$1 WHERE id=$2")
            .bind(when)
            .bind(domain.0)
            .execute(conn.as_mut())
            .await
            .map_err(IndexError::from)?;
        Ok(())
    }

    async fn store_info(&self) -> Result<StoreInfo> {
        // The active full-text path is the candidate scan on both backends, so
        // hybrid ranking and every search test match across them.
        Ok(StoreInfo {
            fts_mode: FtsMode::CandidateScan,
            schema_version: self.schema_version,
            db_path: self.db_path.clone(),
            db_size: None,
        })
    }

    async fn domain_stats(&self) -> Result<Vec<DomainStats>> {
        let mut conn = self.acquire().await?;
        let rows = sqlx::query(
            "SELECT d.id, d.name, d.path, d.kind, d.last_sync, \
             (SELECT count(*) FROM engram e WHERE e.domain_id=d.id), \
             (SELECT count(*) FROM observation o JOIN engram e ON e.id=o.engram_id WHERE e.domain_id=d.id), \
             (SELECT count(*) FROM relation r WHERE r.domain_id=d.id), \
             (SELECT count(*) FROM relation r WHERE r.domain_id=d.id AND r.to_id IS NULL), \
             dl.holder_instance_id, dl.holder_label, dl.heartbeat_at \
             FROM domain d LEFT JOIN domain_lock dl ON dl.domain_id=d.id ORDER BY d.id",
        )
        .fetch_all(conn.as_mut())
        .await
        .map_err(IndexError::from)?;
        Ok(rows
            .iter()
            .map(|r| DomainStats {
                name: cell_text(r, 1).unwrap_or_default(),
                path: cell_text(r, 2).unwrap_or_default(),
                kind: DomainKind::from_stored(&cell_text(r, 3).unwrap_or_default()),
                last_sync: cell_text(r, 4),
                engrams: cell_i64(r, 5).unwrap_or(0),
                observations: cell_i64(r, 6).unwrap_or(0),
                relations: cell_i64(r, 7).unwrap_or(0),
                unresolved_relations: cell_i64(r, 8).unwrap_or(0),
                host_instance_id: cell_text(r, 9),
                host_label: cell_text(r, 10),
                host_heartbeat_at: cell_text(r, 11),
            })
            .collect())
    }

    async fn claim_domain_host(
        &self,
        domain: DomainId,
        instance_id: &str,
        label: &str,
        now: &str,
        stale_before: &str,
        take_over: bool,
    ) -> Result<HostClaim> {
        // One atomic upsert (insert, or take over on a re-claim, an explicit
        // `take_over` or a stale heartbeat), then a re-read on the same
        // connection so we see our own write and never re-enter the tx mutex.
        let mut conn = self.acquire().await?;
        let c = conn.as_mut();
        sqlx::query(
            "INSERT INTO domain_lock(domain_id, holder_instance_id, holder_label, acquired_at, heartbeat_at) \
             VALUES($1,$2,$3,$4,$4) \
             ON CONFLICT(domain_id) DO UPDATE SET \
             holder_instance_id=EXCLUDED.holder_instance_id, holder_label=EXCLUDED.holder_label, \
             acquired_at=EXCLUDED.acquired_at, heartbeat_at=EXCLUDED.heartbeat_at \
             WHERE domain_lock.holder_instance_id=EXCLUDED.holder_instance_id \
                OR $5 \
                OR domain_lock.heartbeat_at < $6",
        )
        .bind(domain.0)
        .bind(instance_id)
        .bind(label)
        .bind(now)
        .bind(take_over)
        .bind(stale_before)
        .execute(&mut *c)
        .await
        .map_err(IndexError::from)?;
        let row = sqlx::query(
            "SELECT holder_instance_id, holder_label, heartbeat_at FROM domain_lock WHERE domain_id=$1",
        )
        .bind(domain.0)
        .fetch_optional(&mut *c)
        .await
        .map_err(IndexError::from)?;
        Ok(match row {
            Some(r) => {
                let host = DomainHost {
                    instance_id: cell_text(&r, 0).unwrap_or_default(),
                    label: cell_text(&r, 1).unwrap_or_default(),
                    heartbeat_at: cell_text(&r, 2).unwrap_or_default(),
                };
                if host.instance_id == instance_id {
                    HostClaim::Acquired
                } else {
                    HostClaim::HeldByOther(host)
                }
            }
            None => HostClaim::Acquired,
        })
    }

    async fn renew_domain_host(
        &self,
        domain: DomainId,
        instance_id: &str,
        now: &str,
    ) -> Result<bool> {
        let mut conn = self.acquire().await?;
        let done = sqlx::query(
            "UPDATE domain_lock SET heartbeat_at=$1 WHERE domain_id=$2 AND holder_instance_id=$3",
        )
        .bind(now)
        .bind(domain.0)
        .bind(instance_id)
        .execute(conn.as_mut())
        .await
        .map_err(IndexError::from)?;
        Ok(done.rows_affected() > 0)
    }

    async fn release_domain_host(&self, domain: DomainId, instance_id: &str) -> Result<()> {
        let mut conn = self.acquire().await?;
        sqlx::query("DELETE FROM domain_lock WHERE domain_id=$1 AND holder_instance_id=$2")
            .bind(domain.0)
            .bind(instance_id)
            .execute(conn.as_mut())
            .await
            .map_err(IndexError::from)?;
        Ok(())
    }

    async fn domain_host(&self, domain: DomainId) -> Result<Option<DomainHost>> {
        let mut conn = self.acquire().await?;
        let row = sqlx::query(
            "SELECT holder_instance_id, holder_label, heartbeat_at FROM domain_lock WHERE domain_id=$1",
        )
        .bind(domain.0)
        .fetch_optional(conn.as_mut())
        .await
        .map_err(IndexError::from)?;
        Ok(row.map(|r| DomainHost {
            instance_id: cell_text(&r, 0).unwrap_or_default(),
            label: cell_text(&r, 1).unwrap_or_default(),
            heartbeat_at: cell_text(&r, 2).unwrap_or_default(),
        }))
    }

    async fn begin(&self) -> Result<()> {
        let mut guard = self.tx.lock().await;
        if guard.is_some() {
            return Err(IndexError::Db("a transaction is already open".into()));
        }
        let mut c = self.pool.acquire().await.map_err(IndexError::from)?;
        sqlx::query("BEGIN")
            .execute(&mut *c)
            .await
            .map_err(IndexError::from)?;
        *guard = Some(c);
        Ok(())
    }

    async fn commit(&self) -> Result<()> {
        let mut guard = self.tx.lock().await;
        if let Some(mut c) = guard.take() {
            sqlx::query("COMMIT")
                .execute(&mut *c)
                .await
                .map_err(IndexError::from)?;
        }
        Ok(())
    }

    async fn rollback(&self) -> Result<()> {
        let mut guard = self.tx.lock().await;
        if let Some(mut c) = guard.take() {
            let _ = sqlx::query("ROLLBACK").execute(&mut *c).await;
        }
        // A rolled-back tag insert leaves its cached id pointing at a row that no
        // longer exists, so drop the cache after a rollback.
        self.tag_cache.lock().unwrap().clear();
        // A rollback also reverts any chunk mutation made in the transaction, so a
        // snapshot recomputed mid-transaction must not survive it.
        self.invalidate_coverage();
        Ok(())
    }
}
