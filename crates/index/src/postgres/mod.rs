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

mod migrations;
mod search;

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use sqlx::pool::PoolConnection;
use sqlx::postgres::{PgArguments, PgPoolOptions, PgRow};
use sqlx::query::Query;
use sqlx::{AssertSqlSafe, PgConnection, PgPool, Postgres, Row};
use tokio::sync::{Mutex as TokioMutex, MutexGuard};

use crate::error::{IndexError, Result};
use crate::store::{
    ChunkJob, ChunkModelCount, DomainId, DomainStats, EdgeKind, EmbeddingCoverage, EmbeddingRow,
    EngramDescriptor, EngramId, EngramRecord, EngramSummary, FileStamp, FtsMode, GraphSlice,
    InboundRef, NewChunk, Page, RecentFilter, SearchHit, SearchQuery, Store, StoreInfo,
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
}

// --- helpers -----------------------------------------------------------------

/// A dynamically bound parameter for the query builder. The search planner and
/// the filtered listings build their WHERE clause at runtime, so their binds are
/// heterogeneous; the fixed-shape statements bind typed values directly instead.
pub(super) enum Param {
    Text(String),
    Int(i64),
    Vector(pgvector::Vector),
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
        };
    }
    q
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

    async fn upsert_domain(&self, name: &str, path: &str) -> Result<DomainId> {
        let mut conn = self.acquire().await?;
        let row: (i64,) = sqlx::query_as(
            "INSERT INTO domain(name, path) VALUES($1,$2) ON CONFLICT(name) DO UPDATE SET path=EXCLUDED.path RETURNING id",
        )
        .bind(name)
        .bind(path)
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

        for obs in &record.observations {
            let oid: (i64,) = sqlx::query_as(
                "INSERT INTO observation(engram_id, line, category, content, context) VALUES($1,$2,$3,$4,$5) RETURNING id",
            )
            .bind(engram_id)
            .bind(obs.line as i64)
            .bind(&obs.category)
            .bind(&obs.content)
            .bind(obs.context.as_deref())
            .fetch_one(&mut *c)
            .await
            .map_err(IndexError::from)?;
            for tag in &obs.tags {
                let tid = self.tag_id(&mut *c, tag).await?;
                sqlx::query("INSERT INTO observation_tag(observation_id, tag_id) VALUES($1,$2) ON CONFLICT DO NOTHING")
                    .bind(oid.0)
                    .bind(tid)
                    .execute(&mut *c)
                    .await
                    .map_err(IndexError::from)?;
            }
        }

        for rel in &record.relations {
            sqlx::query(
                "INSERT INTO relation(engram_id, domain_id, line, rel_type, to_target, to_domain, to_id) \
                 VALUES($1,$2,$3,$4,$5,$6,NULL)",
            )
            .bind(engram_id)
            .bind(domain.0)
            .bind(rel.line as i64)
            .bind(&rel.rel_type)
            .bind(&rel.to_target)
            .bind(rel.to_domain.as_deref())
            .execute(&mut *c)
            .await
            .map_err(IndexError::from)?;
        }

        for link in &record.links {
            sqlx::query(
                "INSERT INTO link(engram_id, domain_id, line, to_target, to_domain, to_id) \
                 VALUES($1,$2,$3,$4,$5,NULL)",
            )
            .bind(engram_id)
            .bind(domain.0)
            .bind(link.line as i64)
            .bind(&link.to_target)
            .bind(link.to_domain.as_deref())
            .execute(&mut *c)
            .await
            .map_err(IndexError::from)?;
        }

        for tag in &record.tags {
            let tid = self.tag_id(&mut *c, tag).await?;
            sqlx::query(
                "INSERT INTO engram_tag(engram_id, tag_id) VALUES($1,$2) ON CONFLICT DO NOTHING",
            )
            .bind(engram_id)
            .bind(tid)
            .execute(&mut *c)
            .await
            .map_err(IndexError::from)?;
        }

        Ok(EngramId(engram_id))
    }

    async fn delete_engram(&self, domain: DomainId, path: &str) -> Result<()> {
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
        let mut conn = self.acquire().await?;
        search::run_search(conn.as_mut(), query).await
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
        let eid = engram_id.0;
        let mut conn = self.acquire().await?;
        let c = conn.as_mut();
        // Carry over the embedding of any chunk whose fingerprint is unchanged so
        // an edit only re-embeds the paragraphs that changed. The fingerprint
        // folds in the model id, so a chunk embedded by a different model does
        // not match and is left for re-embedding.
        let existing = sqlx::query(
            "SELECT text_hash, model, dims, embedding FROM chunk WHERE engram_id=$1 AND embedding IS NOT NULL",
        )
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

        for chunk in chunks {
            match carry.get(&chunk.text_hash) {
                Some((model, dims, emb)) => {
                    sqlx::query(
                        "INSERT INTO chunk(engram_id, seq, text, text_hash, model, dims, embedding) \
                         VALUES($1,$2,$3,$4,$5,$6,$7)",
                    )
                    .bind(eid)
                    .bind(chunk.seq)
                    .bind(&chunk.text)
                    .bind(&chunk.text_hash)
                    .bind(model.as_deref())
                    .bind(*dims)
                    .bind(emb.clone())
                    .execute(&mut *c)
                    .await
                    .map_err(IndexError::from)?;
                }
                None => {
                    sqlx::query(
                        "INSERT INTO chunk(engram_id, seq, text, text_hash) VALUES($1,$2,$3,$4)",
                    )
                    .bind(eid)
                    .bind(chunk.seq)
                    .bind(&chunk.text)
                    .bind(&chunk.text_hash)
                    .execute(&mut *c)
                    .await
                    .map_err(IndexError::from)?;
                }
            }
        }
        Ok(())
    }

    async fn chunks_needing_embedding(&self, model: &str) -> Result<Vec<ChunkJob>> {
        let mut conn = self.acquire().await?;
        let rows = sqlx::query(
            "SELECT id, engram_id, seq, text, text_hash FROM chunk \
             WHERE embedding IS NULL OR model IS NULL OR model != $1 \
             ORDER BY engram_id, seq",
        )
        .bind(model)
        .fetch_all(conn.as_mut())
        .await
        .map_err(IndexError::from)?;
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
        let mut conn = self.acquire().await?;
        let c = conn.as_mut();
        for row in batch {
            if row.embedding.len() != row.dims {
                return Err(IndexError::Invalid(format!(
                    "embedding length {} does not match declared dims {}",
                    row.embedding.len(),
                    row.dims
                )));
            }
            let vector = pgvector::Vector::from(row.embedding.clone());
            sqlx::query("UPDATE chunk SET embedding=$1, dims=$2, model=$3 WHERE id=$4")
                .bind(vector)
                .bind(row.dims as i64)
                .bind(model)
                .bind(row.chunk_id)
                .execute(&mut *c)
                .await
                .map_err(IndexError::from)?;
        }
        Ok(())
    }

    async fn embedding_coverage(&self) -> Result<EmbeddingCoverage> {
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

    async fn wipe(&self) -> Result<()> {
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
            "SELECT d.id, d.name, d.path, d.last_sync, \
             (SELECT count(*) FROM engram e WHERE e.domain_id=d.id), \
             (SELECT count(*) FROM observation o JOIN engram e ON e.id=o.engram_id WHERE e.domain_id=d.id), \
             (SELECT count(*) FROM relation r WHERE r.domain_id=d.id), \
             (SELECT count(*) FROM relation r WHERE r.domain_id=d.id AND r.to_id IS NULL) \
             FROM domain d ORDER BY d.id",
        )
        .fetch_all(conn.as_mut())
        .await
        .map_err(IndexError::from)?;
        Ok(rows
            .iter()
            .map(|r| DomainStats {
                name: cell_text(r, 1).unwrap_or_default(),
                path: cell_text(r, 2).unwrap_or_default(),
                last_sync: cell_text(r, 3),
                engrams: cell_i64(r, 4).unwrap_or(0),
                observations: cell_i64(r, 5).unwrap_or(0),
                relations: cell_i64(r, 6).unwrap_or(0),
                unresolved_relations: cell_i64(r, 7).unwrap_or(0),
            })
            .collect())
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
        Ok(())
    }
}
