//! The Turso-backed [`Store`] implementation.
//!
//! Turso 0.6.1 is SQLite-compatible but young. The design choices forced by its
//! current surface are recorded in `research/turso.md`; the load-bearing ones:
//! native FTS5 is unavailable so search uses a LIKE-candidate scan behind the
//! same `search()` signature; foreign-key cascade is not trusted so child rows
//! are deleted explicitly; `RETURNING` only works through the query path.
//!
//! A single [`Connection`] is used from a single task. The M5 daemon will own
//! this store; other processes never open the database concurrently, so Turso's
//! young multi-process path is never exercised. The socket dispatch that the
//! daemon adds will wrap calls to this store, not replace them.

mod migrations;
mod search;

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use async_trait::async_trait;
use turso::{Builder, Connection, Database, Row, Value};

use crate::error::{IndexError, Result};
use crate::store::{
    ChunkJob, ChunkModelCount, DomainHost, DomainId, DomainKind, DomainStats, EdgeKind,
    EmbeddingCoverage, EmbeddingRow, EngramDescriptor, EngramId, EngramRecord, EngramSummary,
    FileStamp, FtsMode, GraphSlice, HostClaim, InboundRef, NewChunk, Page, RecentFilter, SearchHit,
    SearchMode, SearchQuery, Store, StoreInfo, StoredEngram,
};

/// A Turso-backed store. Open one with [`TursoStore::open`].
pub struct TursoStore {
    // The database handle is retained so the connection stays valid.
    _db: Database,
    conn: Connection,
    db_path: Option<String>,
    schema_version: i64,
    fts_native: bool,
    // Tag name to id, to avoid re-interning tags during a sync.
    tag_cache: Mutex<HashMap<String, i64>>,
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

impl TursoStore {
    /// Open (creating if needed) a store at a filesystem path and migrate it.
    pub async fn open(path: &Path) -> Result<TursoStore> {
        let path_str = path.to_string_lossy().to_string();
        Self::build(&path_str, Some(path_str.clone())).await
    }

    /// Open an in-memory store, for tests. Nothing is persisted.
    pub async fn open_in_memory() -> Result<TursoStore> {
        Self::build(":memory:", None).await
    }

    /// Open a store, recovering from a corrupt database file by discarding it
    /// and starting fresh. Files on disk are the source of truth, so the index
    /// is always rebuildable; this is the `reindex --full` recovery path when the
    /// database will not open or fails a sanity check.
    pub async fn open_resilient(path: &Path) -> Result<TursoStore> {
        if let Ok(store) = TursoStore::open(path).await
            && store.store_info().await.is_ok()
        {
            return Ok(store);
        }
        for suffix in ["", "-wal", "-shm"] {
            let sidecar = if suffix.is_empty() {
                path.to_path_buf()
            } else {
                let mut s = path.as_os_str().to_os_string();
                s.push(suffix);
                std::path::PathBuf::from(s)
            };
            let _ = std::fs::remove_file(&sidecar);
        }
        TursoStore::open(path).await
    }

    async fn build(open_path: &str, db_path: Option<String>) -> Result<TursoStore> {
        let db = Builder::new_local(open_path)
            .build()
            .await
            .map_err(IndexError::from)?;
        let conn = db.connect().map_err(IndexError::from)?;

        // PRAGMA probe against turso 0.7.0, verified two ways: reading
        // turso_core's translate/pragma.rs (the PRAGMA setter dispatch) and a
        // throwaway runtime test exercising each PRAGMA against a real
        // connection. Findings:
        // - busy_timeout: honored. `PRAGMA busy_timeout=5000` round-trips
        //   through `PRAGMA busy_timeout` and drives a real retrying busy
        //   handler (`Connection::set_busy_timeout`). Set below.
        // - wal_checkpoint(TRUNCATE): honored. In the probe it shrank a
        //   populated WAL file from over 1 MiB to 0 bytes. Used in `wipe()`
        //   and by the CLI's `reindex --full` path (see
        //   `Store::checkpoint_wal`), covering the same need the downstream
        //   Docker image build met by shelling out to `sqlite3`.
        // - synchronous: honored (round-tripped OFF and FULL in the probe)
        //   but deliberately left at whatever `migrations::apply` and turso's
        //   own default leave it at - not changed here, per plan.
        // - wal_autocheckpoint: NOT honored. `turso_parser`'s `PragmaName`
        //   enum has no `WalAutocheckpoint` variant, and turso_core's
        //   `translate_pragma` silently ignores any unrecognized PRAGMA name
        //   (SQLite's documented behavior for unknown pragmas): setting it
        //   neither errors nor takes effect. The probe confirmed this -
        //   `PRAGMA wal_autocheckpoint = 1000` returned `Ok`, but a
        //   subsequent `PRAGMA wal_autocheckpoint` query returned no rows.
        //   Not set here, and not needed: a source read of vendored
        //   turso_core 0.7.0 (storage/wal.rs, storage/pager.rs) confirms the
        //   engine passive-checkpoints on every commit once un-backfilled
        //   frames pass a hardcoded threshold of 1000, and separately
        //   restarts the WAL file from the start once fully backfilled, both
        //   default-on for the plain local-database path this store uses. So
        //   the WAL never grows unbounded on its own; it is bounded near
        //   whatever burst set its high-water mark. What the engine does not
        //   do is zero the file: passive checkpoints backfill without
        //   truncating, and its own last-connection shutdown truncate never
        //   fires here because these bindings' `Connection` has no
        //   Drop-close and this store never calls `close()`. wal_checkpoint
        //   (TRUNCATE) above is reclamation of that high-water mark, not a
        //   substitute for growth control the engine already provides.
        conn.execute("PRAGMA busy_timeout = 5000", ()).await?;

        let schema_version = migrations::apply(&conn).await?;
        let fts_native = probe_fts(&conn).await;
        Ok(TursoStore {
            _db: db,
            conn,
            db_path,
            schema_version,
            fts_native,
            tag_cache: Mutex::new(HashMap::new()),
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
        let total = scalar_i64(&self.conn, "SELECT count(*) FROM chunk", vec![])
            .await?
            .max(0) as usize;
        let rows = query_all(
            &self.conn,
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

    /// Intern a tag name, returning its id, using the in-process cache.
    async fn tag_id(&self, name: &str) -> Result<i64> {
        if let Some(id) = self.tag_cache.lock().unwrap().get(name).copied() {
            return Ok(id);
        }
        // ON CONFLICT DO UPDATE (a no-op self-set) so RETURNING yields the id on
        // both insert and conflict. RETURNING must run through the query path.
        let row = query_first(
            &self.conn,
            "INSERT INTO tag(name) VALUES(?1) ON CONFLICT(name) DO UPDATE SET name=excluded.name RETURNING id",
            vec![Value::Text(name.to_string())],
        )
        .await?
        .ok_or_else(|| IndexError::Db("tag upsert returned no id".into()))?;
        let id = cell_i64(&row, 0).ok_or_else(|| IndexError::Db("tag id not an integer".into()))?;
        self.tag_cache.lock().unwrap().insert(name.to_string(), id);
        Ok(id)
    }

    /// Return the `EXPLAIN QUERY PLAN` detail lines for a query. Diagnostic; used
    /// by the temporal-index test to assert the current filter is an index seek.
    pub async fn explain_query_plan(&self, sql: &str) -> Result<Vec<String>> {
        let rows = query_all(&self.conn, &format!("EXPLAIN QUERY PLAN {sql}"), vec![]).await?;
        Ok(rows.iter().filter_map(|r| cell_text(r, 3)).collect())
    }

    /// Run `PRAGMA wal_checkpoint(TRUNCATE)`, shrinking the on-disk WAL file
    /// back to (ideally) zero bytes. Confirmed to work by the runtime probe
    /// documented on `build()`. The pragma returns one row (`busy`, `log`,
    /// `checkpointed`) so it goes through the query path, not `execute`.
    async fn truncate_wal(&self) -> Result<()> {
        query_all(&self.conn, "PRAGMA wal_checkpoint(TRUNCATE)", vec![]).await?;
        Ok(())
    }

    /// Delete the child rows recreated on every upsert. Chunk rows are NOT
    /// cleared here: an upsert preserves them so [`Store::replace_chunks`] can
    /// carry over embeddings whose fingerprint is unchanged. Deleting an engram
    /// clears its chunks explicitly in [`Store::delete_engram`].
    async fn delete_children(&self, engram_id: i64) -> Result<()> {
        let eid = vec![Value::Integer(engram_id)];
        self.conn
            .execute(
                "DELETE FROM observation_tag WHERE observation_id IN (SELECT id FROM observation WHERE engram_id=?1)",
                eid.clone(),
            )
            .await?;
        for sql in [
            "DELETE FROM observation WHERE engram_id=?1",
            "DELETE FROM engram_tag WHERE engram_id=?1",
            "DELETE FROM relation WHERE engram_id=?1",
            "DELETE FROM link WHERE engram_id=?1",
        ] {
            self.conn.execute(sql, eid.clone()).await?;
        }
        Ok(())
    }
}

// --- query helpers -----------------------------------------------------------

async fn query_all(conn: &Connection, sql: &str, params: Vec<Value>) -> Result<Vec<Row>> {
    let mut rows = conn.query(sql, params).await?;
    let mut out = Vec::new();
    while let Some(r) = rows.next().await? {
        out.push(r);
    }
    Ok(out)
}

async fn query_first(conn: &Connection, sql: &str, params: Vec<Value>) -> Result<Option<Row>> {
    let mut rows = conn.query(sql, params).await?;
    let first = rows.next().await?;
    // Drain so the statement runs to completion and does not roll back on drop.
    while rows.next().await?.is_some() {}
    Ok(first)
}

async fn scalar_i64(conn: &Connection, sql: &str, params: Vec<Value>) -> Result<i64> {
    Ok(query_first(conn, sql, params)
        .await?
        .and_then(|r| cell_i64(&r, 0))
        .unwrap_or(0))
}

fn descriptor_from_row(r: &Row) -> EngramDescriptor {
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
fn like_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c == '%' || c == '_' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn cell_text(row: &Row, idx: usize) -> Option<String> {
    match row.get_value(idx) {
        Ok(Value::Text(s)) => Some(s),
        _ => None,
    }
}

fn cell_i64(row: &Row, idx: usize) -> Option<i64> {
    match row.get_value(idx) {
        Ok(Value::Integer(i)) => Some(i),
        _ => None,
    }
}

fn cell_real(row: &Row, idx: usize) -> Option<f64> {
    match row.get_value(idx) {
        Ok(Value::Real(f)) => Some(f),
        Ok(Value::Integer(i)) => Some(i as f64),
        _ => None,
    }
}

fn opt_text(o: &Option<String>) -> Value {
    match o {
        Some(s) => Value::Text(s.clone()),
        None => Value::Null,
    }
}

/// The maximum number of value tuples in one multi-row INSERT. Chunking keeps the
/// bind-parameter count under SQLite's 999-parameter ceiling for every child
/// table (the widest is chunk at 7 params per row).
const INSERT_CHUNK: usize = 100;

/// Build the parenthesized value tuples for a multi-row INSERT: `count` tuples of
/// `width` sequential `?n` placeholders each, plus an optional fixed trailing
/// column (a literal `NULL` `to_id` for relations and links). `count` must be
/// non-zero; an empty VALUES list is not valid SQL, so callers iterate chunks of
/// a possibly-empty slice and skip the statement entirely when there is nothing.
fn value_rows(width: usize, count: usize, trailing: Option<&str>) -> String {
    let mut n = 1;
    let mut rows = Vec::with_capacity(count);
    for _ in 0..count {
        let mut cells = Vec::with_capacity(width + usize::from(trailing.is_some()));
        for _ in 0..width {
            cells.push(format!("?{n}"));
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

/// The stored discriminator string for a domain kind.
fn kind_str(kind: DomainKind) -> &'static str {
    match kind {
        DomainKind::File => "file",
        DomainKind::Virtual => "virtual",
    }
}

async fn probe_fts(conn: &Connection) -> bool {
    // Attempt a native FTS5 virtual table. Turso 0.6.1 rejects it, so this is
    // expected to fail and the candidate-scan fallback stays active. When a
    // future Turso passes the probe, the native search path slots in here.
    let ok = conn
        .execute(
            "CREATE VIRTUAL TABLE IF NOT EXISTS _crystalline_fts_probe USING fts5(x)",
            (),
        )
        .await
        .is_ok();
    if ok {
        let _ = conn
            .execute("DROP TABLE IF EXISTS _crystalline_fts_probe", ())
            .await;
    }
    ok
}

#[async_trait]
impl Store for TursoStore {
    async fn migrate(&self) -> Result<()> {
        migrations::apply(&self.conn).await?;
        Ok(())
    }

    async fn upsert_domain(
        &self,
        name: &str,
        path: Option<&str>,
        kind: DomainKind,
    ) -> Result<DomainId> {
        // Virtual domains have no filesystem root; SQLite keeps `path NOT NULL`,
        // so an absent path stores the empty string and `kind` discriminates.
        let path = path.unwrap_or("");
        let kind = kind_str(kind);
        self.conn
            .execute(
                "INSERT INTO domain(name, path, kind) VALUES(?1, ?2, ?3) \
                 ON CONFLICT(name) DO UPDATE SET path=excluded.path, kind=excluded.kind",
                vec![
                    Value::Text(name.to_string()),
                    Value::Text(path.to_string()),
                    Value::Text(kind.to_string()),
                ],
            )
            .await?;
        let id = scalar_i64(
            &self.conn,
            "SELECT id FROM domain WHERE name=?1",
            vec![Value::Text(name.to_string())],
        )
        .await?;
        Ok(DomainId(id))
    }

    async fn file_stamps(&self, domain: DomainId) -> Result<HashMap<String, FileStamp>> {
        let rows = query_all(
            &self.conn,
            "SELECT path, mtime, size, sha256 FROM engram WHERE domain_id=?1",
            vec![Value::Integer(domain.0)],
        )
        .await?;
        let mut out = HashMap::with_capacity(rows.len());
        for r in &rows {
            let Some(path) = cell_text(r, 0) else {
                continue;
            };
            let mtime = cell_i64(r, 1).unwrap_or(0);
            let size = cell_i64(r, 2).unwrap_or(0).max(0) as u64;
            let sha256 = cell_text(r, 3).unwrap_or_default();
            out.insert(
                path,
                FileStamp {
                    mtime,
                    size,
                    sha256,
                },
            );
        }
        Ok(out)
    }

    async fn upsert_engram(&self, domain: DomainId, record: &EngramRecord) -> Result<EngramId> {
        // One probe for both the existing-by-path row (insert vs update) and a
        // duplicate-permalink owned by a different path. Pre-checking the
        // duplicate means no failing statement aborts the batch transaction.
        let probe = query_all(
            &self.conn,
            "SELECT id, path FROM engram WHERE domain_id=?1 AND (path=?2 OR permalink=?3)",
            vec![
                Value::Integer(domain.0),
                Value::Text(record.path.clone()),
                Value::Text(record.permalink.clone()),
            ],
        )
        .await?;
        let mut existing_id: Option<i64> = None;
        for r in &probe {
            let row_path = cell_text(r, 1).unwrap_or_default();
            if row_path == record.path {
                existing_id = cell_i64(r, 0);
            } else {
                // The permalink is owned by a different path.
                return Err(IndexError::Constraint(format!(
                    "permalink '{}' already used by '{}'",
                    record.permalink, row_path
                )));
            }
        }

        let params = vec![
            Value::Integer(domain.0),
            Value::Text(record.path.clone()),
            Value::Text(record.permalink.clone()),
            Value::Text(record.title.clone()),
            Value::Text(record.engram_type.clone()),
            Value::Text(record.status.clone()),
            opt_text(&record.recorded_at),
            opt_text(&record.valid_from),
            opt_text(&record.valid_to),
            opt_text(&record.timestamp),
            opt_text(&record.description),
            Value::Text(record.content.clone()),
            Value::Text(record.metadata.to_string()),
            Value::Integer(record.stamp.mtime),
            Value::Integer(record.stamp.size as i64),
            Value::Text(record.stamp.sha256.clone()),
        ];
        self.conn
            .execute(
                "INSERT INTO engram(domain_id, path, permalink, title, engram_type, status, \
                 recorded_at, valid_from, valid_to, timestamp, description, content, metadata, \
                 mtime, size, sha256) \
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16) \
                 ON CONFLICT(domain_id, path) DO UPDATE SET \
                 permalink=excluded.permalink, title=excluded.title, engram_type=excluded.engram_type, \
                 status=excluded.status, recorded_at=excluded.recorded_at, valid_from=excluded.valid_from, \
                 valid_to=excluded.valid_to, timestamp=excluded.timestamp, description=excluded.description, \
                 content=excluded.content, metadata=excluded.metadata, mtime=excluded.mtime, \
                 size=excluded.size, sha256=excluded.sha256",
                params,
            )
            .await?;

        // A new row's id is the last insert; an updated row keeps its id. Only
        // an update needs its stale child rows cleared first.
        let engram_id = match existing_id {
            Some(id) => {
                self.delete_children(id).await?;
                id
            }
            None => self.conn.last_insert_rowid(),
        };

        // Observations: insert in chunks and read each new row's id back joined
        // on its source line (unique within one engram), so observation tags map
        // to the right observation without relying on RETURNING row order.
        let mut obs_id_by_line: HashMap<i64, i64> = HashMap::new();
        for batch in record.observations.chunks(INSERT_CHUNK) {
            let mut params: Vec<Value> = Vec::with_capacity(batch.len() * 5);
            for obs in batch {
                params.push(Value::Integer(engram_id));
                params.push(Value::Integer(obs.line as i64));
                params.push(Value::Text(obs.category.clone()));
                params.push(Value::Text(obs.content.clone()));
                params.push(opt_text(&obs.context));
            }
            let rows = query_all(&self.conn, &observation_insert_sql(batch.len()), params).await?;
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
                let tid = self.tag_id(tag).await?;
                obs_tag_pairs.push((oid, tid));
            }
        }
        for batch in obs_tag_pairs.chunks(INSERT_CHUNK) {
            let mut params: Vec<Value> = Vec::with_capacity(batch.len() * 2);
            for (oid, tid) in batch {
                params.push(Value::Integer(*oid));
                params.push(Value::Integer(*tid));
            }
            let sql = format!(
                "INSERT OR IGNORE INTO observation_tag(observation_id, tag_id) VALUES {}",
                value_rows(2, batch.len(), None)
            );
            self.conn.execute(&sql, params).await?;
        }

        for batch in record.relations.chunks(INSERT_CHUNK) {
            let mut params: Vec<Value> = Vec::with_capacity(batch.len() * 6);
            for rel in batch {
                params.push(Value::Integer(engram_id));
                params.push(Value::Integer(domain.0));
                params.push(Value::Integer(rel.line as i64));
                params.push(Value::Text(rel.rel_type.clone()));
                params.push(Value::Text(rel.to_target.clone()));
                params.push(opt_text(&rel.to_domain));
            }
            let sql = format!(
                "INSERT INTO relation(engram_id, domain_id, line, rel_type, to_target, to_domain, to_id) VALUES {}",
                value_rows(6, batch.len(), Some("NULL"))
            );
            self.conn.execute(&sql, params).await?;
        }

        for batch in record.links.chunks(INSERT_CHUNK) {
            let mut params: Vec<Value> = Vec::with_capacity(batch.len() * 5);
            for link in batch {
                params.push(Value::Integer(engram_id));
                params.push(Value::Integer(domain.0));
                params.push(Value::Integer(link.line as i64));
                params.push(Value::Text(link.to_target.clone()));
                params.push(opt_text(&link.to_domain));
            }
            let sql = format!(
                "INSERT INTO link(engram_id, domain_id, line, to_target, to_domain, to_id) VALUES {}",
                value_rows(5, batch.len(), Some("NULL"))
            );
            self.conn.execute(&sql, params).await?;
        }

        // Engram tags: intern each tag, then insert the pairs in multi-row
        // statements.
        let mut tag_ids: Vec<i64> = Vec::with_capacity(record.tags.len());
        for tag in &record.tags {
            tag_ids.push(self.tag_id(tag).await?);
        }
        for batch in tag_ids.chunks(INSERT_CHUNK) {
            let mut params: Vec<Value> = Vec::with_capacity(batch.len() * 2);
            for tid in batch {
                params.push(Value::Integer(engram_id));
                params.push(Value::Integer(*tid));
            }
            let sql = format!(
                "INSERT OR IGNORE INTO engram_tag(engram_id, tag_id) VALUES {}",
                value_rows(2, batch.len(), None)
            );
            self.conn.execute(&sql, params).await?;
        }

        Ok(EngramId(engram_id))
    }

    async fn upsert_engram_checked(
        &self,
        domain: DomainId,
        record: &EngramRecord,
        expected_sha: Option<&str>,
    ) -> Result<EngramId> {
        // Compare-and-swap: if a row exists at this path and the caller supplied
        // an expected sha that no longer matches the stored one, refuse. The
        // engine holds the transaction open around this, so the compare and the
        // subsequent write are one atomic unit.
        if let Some(expected) = expected_sha {
            let stored = query_first(
                &self.conn,
                "SELECT sha256 FROM engram WHERE domain_id=?1 AND path=?2",
                vec![Value::Integer(domain.0), Value::Text(record.path.clone())],
            )
            .await?
            .and_then(|r| cell_text(&r, 0));
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
        let row = query_first(
            &self.conn,
            "SELECT content FROM engram WHERE domain_id=?1 AND path=?2",
            vec![Value::Integer(domain.0), Value::Text(path.to_string())],
        )
        .await?;
        Ok(row.and_then(|r| cell_text(&r, 0)))
    }

    async fn all_engram_contents(&self, domain: DomainId) -> Result<Vec<StoredEngram>> {
        let rows = query_all(
            &self.conn,
            "SELECT path, permalink, content, sha256 FROM engram WHERE domain_id=?1 ORDER BY path",
            vec![Value::Integer(domain.0)],
        )
        .await?;
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
        // row. Child rows first, then chunks, then the engram rows themselves.
        let did = vec![Value::Integer(domain.0)];
        for sql in [
            "DELETE FROM observation_tag WHERE observation_id IN \
             (SELECT o.id FROM observation o JOIN engram e ON e.id=o.engram_id WHERE e.domain_id=?1)",
            "DELETE FROM engram_tag WHERE engram_id IN (SELECT id FROM engram WHERE domain_id=?1)",
            "DELETE FROM chunk WHERE engram_id IN (SELECT id FROM engram WHERE domain_id=?1)",
            "DELETE FROM observation WHERE engram_id IN (SELECT id FROM engram WHERE domain_id=?1)",
            "DELETE FROM relation WHERE domain_id=?1",
            "DELETE FROM link WHERE domain_id=?1",
            "DELETE FROM engram WHERE domain_id=?1",
        ] {
            self.conn.execute(sql, did.clone()).await?;
        }
        Ok(())
    }

    async fn delete_engram(&self, domain: DomainId, path: &str) -> Result<()> {
        // Deletes the engram's chunks, so the coverage snapshot is now stale.
        self.invalidate_coverage();
        let id = query_first(
            &self.conn,
            "SELECT id FROM engram WHERE domain_id=?1 AND path=?2",
            vec![Value::Integer(domain.0), Value::Text(path.to_string())],
        )
        .await?
        .and_then(|r| cell_i64(&r, 0));
        if let Some(id) = id {
            self.delete_children(id).await?;
            self.conn
                .execute(
                    "DELETE FROM chunk WHERE engram_id=?1",
                    vec![Value::Integer(id)],
                )
                .await?;
            self.conn
                .execute("DELETE FROM engram WHERE id=?1", vec![Value::Integer(id)])
                .await?;
        }
        Ok(())
    }

    async fn rename_engram(&self, domain: DomainId, from: &str, to: &str) -> Result<()> {
        // The permalink follows the move only when it was path-derived, so an
        // explicit frontmatter permalink is preserved across a move.
        let old_slug = crystalline_core::slugify(from);
        let new_slug = crystalline_core::slugify(to);
        self.conn
            .execute(
                "UPDATE engram SET path=?1, \
                 permalink = CASE WHEN permalink=?2 THEN ?3 ELSE permalink END \
                 WHERE domain_id=?4 AND path=?5",
                vec![
                    Value::Text(to.to_string()),
                    Value::Text(old_slug),
                    Value::Text(new_slug),
                    Value::Integer(domain.0),
                    Value::Text(from.to_string()),
                ],
            )
            .await?;
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
             WHERE relation.to_id IS NULL AND relation.domain_id = ?1 \
             AND (EXISTS (SELECT 1 FROM engram e WHERE e.permalink = relation.to_target AND e.domain_id = {tgt_dom}) \
                  OR EXISTS (SELECT 1 FROM engram e WHERE lower(e.title) = lower(relation.to_target) AND e.domain_id = {tgt_dom}))"
        );
        let n = self
            .conn
            .execute(&sql, vec![Value::Integer(domain.0)])
            .await?;
        Ok(n)
    }

    async fn lookup_id(&self, domain: &str, permalink: &str) -> Result<Option<EngramId>> {
        let row = query_first(
            &self.conn,
            "SELECT e.id FROM engram e JOIN domain d ON d.id=e.domain_id WHERE d.name=?1 AND e.permalink=?2",
            vec![Value::Text(domain.to_string()), Value::Text(permalink.to_string())],
        )
        .await?;
        Ok(row.and_then(|r| cell_i64(&r, 0)).map(EngramId))
    }

    async fn find_engram(&self, domain: &str, key: &str) -> Result<Option<EngramDescriptor>> {
        let rows = query_all(
            &self.conn,
            "SELECT e.id, e.domain_id, d.name, e.path, e.permalink, e.title, e.engram_type, e.status \
             FROM engram e JOIN domain d ON d.id=e.domain_id \
             WHERE d.name=?1 AND (e.permalink=?2 OR lower(e.title)=lower(?2)) \
             ORDER BY CASE WHEN e.permalink=?2 THEN 0 ELSE 1 END, e.path LIMIT 1",
            vec![Value::Text(domain.to_string()), Value::Text(key.to_string())],
        )
        .await?;
        Ok(rows.first().map(descriptor_from_row))
    }

    async fn find_engram_any(&self, key: &str) -> Result<Vec<EngramDescriptor>> {
        let rows = query_all(
            &self.conn,
            "SELECT e.id, e.domain_id, d.name, e.path, e.permalink, e.title, e.engram_type, e.status \
             FROM engram e JOIN domain d ON d.id=e.domain_id \
             WHERE e.permalink=?1 OR lower(e.title)=lower(?1) \
             ORDER BY CASE WHEN e.permalink=?1 THEN 0 ELSE 1 END, d.name, e.path",
            vec![Value::Text(key.to_string())],
        )
        .await?;
        Ok(rows.iter().map(descriptor_from_row).collect())
    }

    async fn list_engrams(
        &self,
        domain: &str,
        path_prefix: Option<&str>,
        engram_type: Option<&str>,
    ) -> Result<Vec<EngramDescriptor>> {
        let mut clauses = vec!["d.name=?1".to_string()];
        let mut params = vec![Value::Text(domain.to_string())];
        let mut n = 2;
        if let Some(prefix) = path_prefix.filter(|p| !p.is_empty()) {
            clauses.push(format!("e.path LIKE ?{n} ESCAPE '\\'"));
            params.push(Value::Text(format!("{}%", like_escape(prefix))));
            n += 1;
        }
        if let Some(t) = engram_type {
            clauses.push(format!("e.engram_type=?{n}"));
            params.push(Value::Text(t.to_string()));
        }
        let sql = format!(
            "SELECT e.id, e.domain_id, d.name, e.path, e.permalink, e.title, e.engram_type, e.status \
             FROM engram e JOIN domain d ON d.id=e.domain_id WHERE {} ORDER BY e.path",
            clauses.join(" AND ")
        );
        let rows = query_all(&self.conn, &sql, params).await?;
        Ok(rows.iter().map(descriptor_from_row).collect())
    }

    async fn inbound_refs(
        &self,
        engram_id: EngramId,
        domain_id: DomainId,
        permalink: &str,
        title: &str,
    ) -> Result<Vec<InboundRef>> {
        // Matches both already-resolved references (`to_id`) and unresolved
        // *bare* references in the engram's own domain whose target text is the
        // engram's permalink or title. Prose wikilinks are never assigned a
        // `to_id`, so the text match is what catches them; `to_domain IS NULL`
        // restricts the rewrite to bare links that would otherwise dangle after
        // the move.
        let params = vec![
            Value::Integer(engram_id.0),
            Value::Integer(domain_id.0),
            Value::Text(permalink.to_string()),
            Value::Text(title.to_string()),
        ];
        let rows = query_all(
            &self.conn,
            "SELECT d.name, r.domain_id, e.path, r.to_target, 0 \
             FROM relation r JOIN engram e ON e.id=r.engram_id JOIN domain d ON d.id=e.domain_id \
             WHERE r.to_id=?1 \
                OR (r.to_id IS NULL AND r.domain_id=?2 AND r.to_domain IS NULL \
                    AND (r.to_target=?3 OR lower(r.to_target)=lower(?4))) \
             UNION ALL \
             SELECT d.name, l.domain_id, e.path, l.to_target, 1 \
             FROM link l JOIN engram e ON e.id=l.engram_id JOIN domain d ON d.id=e.domain_id \
             WHERE l.to_id=?1 \
                OR (l.to_id IS NULL AND l.domain_id=?2 AND l.to_domain IS NULL \
                    AND (l.to_target=?3 OR lower(l.to_target)=lower(?4)))",
            params,
        )
        .await?;
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
        // The semantic and hybrid modes gate on embedding staleness, which reads
        // the coverage snapshot; the cache makes that essentially free once
        // effective_mode has warmed it, and folds the old second aggregate scan
        // into the same shared snapshot. The lexical modes never touch embeddings,
        // so they skip the snapshot and pay nothing for it.
        let coverage = match query.mode {
            SearchMode::Semantic | SearchMode::Hybrid => Some(self.embedding_coverage().await?),
            _ => None,
        };
        search::run_search(&self.conn, query, coverage.as_ref()).await
    }

    async fn neighbors(&self, ids: &[EngramId], depth: u8) -> Result<GraphSlice> {
        search::neighbors(&self.conn, ids, depth).await
    }

    async fn recent(&self, filter: &RecentFilter) -> Result<Vec<EngramSummary>> {
        let mut where_clauses: Vec<String> = Vec::new();
        let mut params: Vec<Value> = Vec::new();
        let mut n = 1;

        if let Some(domains) = &filter.domains
            && !domains.is_empty()
        {
            let placeholders: Vec<String> = domains
                .iter()
                .map(|d| {
                    params.push(Value::Text(d.clone()));
                    let p = format!("?{n}");
                    n += 1;
                    p
                })
                .collect();
            where_clauses.push(format!("d.name IN ({})", placeholders.join(",")));
        }
        if let Some(after) = &filter.after {
            where_clauses.push(format!("e.recorded_at >= ?{n}"));
            params.push(Value::Text(after.clone()));
            n += 1;
        }
        if let Some(types) = &filter.engram_types
            && !types.is_empty()
        {
            let placeholders: Vec<String> = types
                .iter()
                .map(|t| {
                    params.push(Value::Text(t.clone()));
                    let p = format!("?{n}");
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
             (SELECT group_concat(t.name, ',') FROM engram_tag et JOIN tag t ON t.id=et.tag_id WHERE et.engram_id=e.id) \
             FROM engram e JOIN domain d ON d.id=e.domain_id {where_sql} \
             ORDER BY e.recorded_at DESC, e.permalink ASC LIMIT {limit}"
        );
        let rows = query_all(&self.conn, &sql, params).await?;
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
        // Carry over the embedding of any chunk whose fingerprint is unchanged so
        // an edit only re-embeds the paragraphs that actually changed. The
        // fingerprint folds in the model id, so a chunk embedded by a different
        // model does not match and is left for re-embedding.
        let existing = query_all(
            &self.conn,
            "SELECT text_hash, model, dims, embedding FROM chunk WHERE engram_id=?1 AND embedding IS NOT NULL",
            vec![Value::Integer(eid)],
        )
        .await?;
        let mut carry: HashMap<String, (Value, Value, Vec<u8>)> = HashMap::new();
        for r in &existing {
            let Some(hash) = cell_text(r, 0) else {
                continue;
            };
            let model = match r.get_value(1) {
                Ok(Value::Text(s)) => Value::Text(s),
                _ => Value::Null,
            };
            let dims = match r.get_value(2) {
                Ok(Value::Integer(i)) => Value::Integer(i),
                _ => Value::Null,
            };
            let Ok(Value::Blob(emb)) = r.get_value(3) else {
                continue;
            };
            carry.insert(hash, (model, dims, emb));
        }

        self.conn
            .execute(
                "DELETE FROM chunk WHERE engram_id=?1",
                vec![Value::Integer(eid)],
            )
            .await?;

        // One 7-column multi-row insert for every chunk: a carry hit binds its
        // preserved model, dims and embedding, a miss binds NULL for all three
        // and leaves the chunk pending re-embedding.
        for batch in chunks.chunks(INSERT_CHUNK) {
            let mut params: Vec<Value> = Vec::with_capacity(batch.len() * 7);
            for c in batch {
                params.push(Value::Integer(eid));
                params.push(Value::Integer(c.seq));
                params.push(Value::Text(c.text.clone()));
                params.push(Value::Text(c.text_hash.clone()));
                match carry.get(&c.text_hash) {
                    Some((model, dims, emb)) => {
                        params.push(model.clone());
                        params.push(dims.clone());
                        params.push(Value::Blob(emb.clone()));
                    }
                    None => {
                        params.push(Value::Null);
                        params.push(Value::Null);
                        params.push(Value::Null);
                    }
                }
            }
            let sql = format!(
                "INSERT INTO chunk(engram_id, seq, text, text_hash, model, dims, embedding) VALUES {}",
                value_rows(7, batch.len(), None)
            );
            self.conn.execute(&sql, params).await?;
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
             WHERE (embedding IS NULL OR model IS NULL OR model != ?1)",
        );
        let mut params = vec![Value::Text(model.to_string())];
        if let Some(ids) = domains {
            if ids.is_empty() {
                return Ok(Vec::new());
            }
            let mut n = 2;
            let placeholders: Vec<String> = ids
                .iter()
                .map(|id| {
                    params.push(Value::Integer(id.0));
                    let p = format!("?{n}");
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
        let rows = query_all(&self.conn, &sql, params).await?;
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
        // Validate the whole batch before opening the transaction so a bad row
        // never leaves earlier rows committed. The transaction then makes the
        // per-row updates all-or-nothing against a mid-batch database error,
        // mirroring `wipe`.
        for row in batch {
            if row.embedding.len() != row.dims {
                return Err(IndexError::Invalid(format!(
                    "embedding length {} does not match declared dims {}",
                    row.embedding.len(),
                    row.dims
                )));
            }
        }
        self.conn.execute("BEGIN", ()).await?;
        for row in batch {
            let mut bytes = Vec::with_capacity(row.embedding.len() * 4);
            for f in &row.embedding {
                bytes.extend_from_slice(&f.to_le_bytes());
            }
            if let Err(e) = self
                .conn
                .execute(
                    "UPDATE chunk SET embedding=?1, dims=?2, model=?3 WHERE id=?4",
                    vec![
                        Value::Blob(bytes),
                        Value::Integer(row.dims as i64),
                        Value::Text(model.to_string()),
                        Value::Integer(row.chunk_id),
                    ],
                )
                .await
            {
                let _ = self.conn.execute("ROLLBACK", ()).await;
                return Err(e.into());
            }
        }
        self.conn.execute("COMMIT", ()).await?;
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
        self.conn.execute("BEGIN", ()).await?;
        for table in migrations::WIPE_TABLES {
            if let Err(e) = self.conn.execute(&format!("DELETE FROM {table}"), ()).await {
                let _ = self.conn.execute("ROLLBACK", ()).await;
                return Err(e.into());
            }
        }
        self.conn.execute("COMMIT", ()).await?;
        self.tag_cache.lock().unwrap().clear();
        // A wipe deletes every row, which is the biggest single write a store
        // sees; truncate the WAL back down rather than leaving it to grow
        // until the next natural checkpoint.
        self.truncate_wal().await?;
        Ok(())
    }

    async fn checkpoint_wal(&self) -> Result<()> {
        self.truncate_wal().await
    }

    async fn record_sync(&self, domain: DomainId, when: &str) -> Result<()> {
        self.conn
            .execute(
                "UPDATE domain SET last_sync=?1 WHERE id=?2",
                vec![Value::Text(when.to_string()), Value::Integer(domain.0)],
            )
            .await?;
        Ok(())
    }

    async fn store_info(&self) -> Result<StoreInfo> {
        // The active full-text path in this milestone is always the candidate
        // scan. `fts_native` records the probe outcome for diagnostics; when a
        // native search path is implemented it flips this to `Native`.
        let db_size = self
            .db_path
            .as_ref()
            .and_then(|p| std::fs::metadata(p).ok())
            .map(|m| m.len());
        let _ = self.fts_native;
        Ok(StoreInfo {
            fts_mode: FtsMode::CandidateScan,
            schema_version: self.schema_version,
            db_path: self.db_path.clone(),
            db_size,
        })
    }

    async fn domain_stats(&self) -> Result<Vec<DomainStats>> {
        let rows = query_all(
            &self.conn,
            "SELECT d.id, d.name, d.path, d.kind, d.last_sync, \
             (SELECT count(*) FROM engram e WHERE e.domain_id=d.id), \
             (SELECT count(*) FROM observation o JOIN engram e ON e.id=o.engram_id WHERE e.domain_id=d.id), \
             (SELECT count(*) FROM relation r WHERE r.domain_id=d.id), \
             (SELECT count(*) FROM relation r WHERE r.domain_id=d.id AND r.to_id IS NULL), \
             dl.holder_instance_id, dl.holder_label, dl.heartbeat_at \
             FROM domain d LEFT JOIN domain_lock dl ON dl.domain_id=d.id ORDER BY d.id",
            vec![],
        )
        .await?;
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
        // One atomic upsert: insert when unheld, or take over when this instance
        // already holds it (a re-claim, refreshing the heartbeat), or when
        // `take_over` is set, or when the current heartbeat is older than
        // `stale_before`. The guarded `ON CONFLICT ... DO UPDATE ... WHERE` leaves
        // a live holder untouched; the re-read then tells us who holds it now.
        self.conn
            .execute(
                "INSERT INTO domain_lock(domain_id, holder_instance_id, holder_label, acquired_at, heartbeat_at) \
                 VALUES(?1, ?2, ?3, ?4, ?5) \
                 ON CONFLICT(domain_id) DO UPDATE SET \
                 holder_instance_id=excluded.holder_instance_id, holder_label=excluded.holder_label, \
                 acquired_at=excluded.acquired_at, heartbeat_at=excluded.heartbeat_at \
                 WHERE domain_lock.holder_instance_id=excluded.holder_instance_id \
                    OR ?6 \
                    OR domain_lock.heartbeat_at < ?7",
                vec![
                    Value::Integer(domain.0),
                    Value::Text(instance_id.to_string()),
                    Value::Text(label.to_string()),
                    Value::Text(now.to_string()),
                    Value::Text(now.to_string()),
                    Value::Integer(if take_over { 1 } else { 0 }),
                    Value::Text(stale_before.to_string()),
                ],
            )
            .await?;
        match self.domain_host(domain).await? {
            Some(h) if h.instance_id == instance_id => Ok(HostClaim::Acquired),
            Some(h) => Ok(HostClaim::HeldByOther(h)),
            None => Ok(HostClaim::Acquired),
        }
    }

    async fn renew_domain_host(
        &self,
        domain: DomainId,
        instance_id: &str,
        now: &str,
    ) -> Result<bool> {
        let n = self
            .conn
            .execute(
                "UPDATE domain_lock SET heartbeat_at=?1 WHERE domain_id=?2 AND holder_instance_id=?3",
                vec![
                    Value::Text(now.to_string()),
                    Value::Integer(domain.0),
                    Value::Text(instance_id.to_string()),
                ],
            )
            .await?;
        Ok(n > 0)
    }

    async fn release_domain_host(&self, domain: DomainId, instance_id: &str) -> Result<()> {
        self.conn
            .execute(
                "DELETE FROM domain_lock WHERE domain_id=?1 AND holder_instance_id=?2",
                vec![
                    Value::Integer(domain.0),
                    Value::Text(instance_id.to_string()),
                ],
            )
            .await?;
        Ok(())
    }

    async fn domain_host(&self, domain: DomainId) -> Result<Option<DomainHost>> {
        let row = query_first(
            &self.conn,
            "SELECT holder_instance_id, holder_label, heartbeat_at FROM domain_lock WHERE domain_id=?1",
            vec![Value::Integer(domain.0)],
        )
        .await?;
        Ok(row.map(|r| DomainHost {
            instance_id: cell_text(&r, 0).unwrap_or_default(),
            label: cell_text(&r, 1).unwrap_or_default(),
            heartbeat_at: cell_text(&r, 2).unwrap_or_default(),
        }))
    }

    async fn begin(&self) -> Result<()> {
        self.conn.execute("BEGIN", ()).await?;
        Ok(())
    }

    async fn commit(&self) -> Result<()> {
        self.conn.execute("COMMIT", ()).await?;
        Ok(())
    }

    async fn rollback(&self) -> Result<()> {
        // A rollback reverts any chunk mutation made in the transaction, so a
        // snapshot recomputed mid-transaction must not survive it.
        self.invalidate_coverage();
        self.conn.execute("ROLLBACK", ()).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::store::{FileStamp, ObservationRecord};

    #[test]
    fn observation_insert_is_one_statement_with_a_tuple_per_row() {
        let sql = observation_insert_sql(3);
        assert_eq!(sql.matches("INSERT").count(), 1, "one INSERT: {sql}");
        // One `(?` opens each value tuple, so three tuples for three rows.
        assert_eq!(sql.matches("(?").count(), 3, "three value tuples: {sql}");
        assert!(
            sql.contains("VALUES (?1,?2,?3,?4,?5),(?6,?7,?8,?9,?10),(?11,?12,?13,?14,?15)"),
            "sequential placeholders across all rows: {sql}"
        );
        assert!(
            sql.ends_with("RETURNING id, line"),
            "returns id and line: {sql}"
        );
    }

    fn obs(line: usize, content: &str, tags: &[&str]) -> ObservationRecord {
        ObservationRecord {
            line,
            category: "fact".to_string(),
            content: content.to_string(),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            context: None,
        }
    }

    fn record_with_observations(observations: Vec<ObservationRecord>) -> EngramRecord {
        EngramRecord {
            path: "a.md".to_string(),
            permalink: "a".to_string(),
            title: "A".to_string(),
            engram_type: "engram".to_string(),
            status: "current".to_string(),
            recorded_at: Some("2026-01-01".to_string()),
            valid_from: None,
            valid_to: None,
            timestamp: None,
            description: None,
            content: "body".to_string(),
            metadata: serde_json::json!({}),
            tags: Vec::new(),
            observations,
            relations: Vec::new(),
            links: Vec::new(),
            stamp: FileStamp {
                mtime: 0,
                size: 4,
                sha256: "sha".to_string(),
            },
        }
    }

    /// Every observation's tag set read back keyed by source line, so a
    /// mismapped tag surfaces against the wrong line.
    async fn tags_by_line(store: &TursoStore, engram_id: i64) -> Vec<(i64, Vec<String>)> {
        let rows = query_all(
            &store.conn,
            "SELECT o.line, t.name FROM observation o \
             LEFT JOIN observation_tag ot ON ot.observation_id=o.id \
             LEFT JOIN tag t ON t.id=ot.tag_id \
             WHERE o.engram_id=?1 ORDER BY o.line, t.name",
            vec![Value::Integer(engram_id)],
        )
        .await
        .unwrap();
        let mut map: BTreeMap<i64, Vec<String>> = BTreeMap::new();
        for r in &rows {
            let line = cell_i64(r, 0).unwrap();
            let entry = map.entry(line).or_default();
            if let Some(name) = cell_text(r, 1) {
                entry.push(name);
            }
        }
        map.into_iter().collect()
    }

    #[tokio::test]
    async fn observation_tags_map_to_correct_observations() {
        let store = TursoStore::open_in_memory().await.unwrap();
        let domain = store
            .upsert_domain("d", Some("/tmp/d"), DomainKind::File)
            .await
            .unwrap();
        // Distinct lines, distinct tag sets, one observation deliberately
        // untagged, and a tag shared by two observations.
        let record = record_with_observations(vec![
            obs(3, "first fact", &["alpha", "shared"]),
            obs(7, "second fact", &["beta"]),
            obs(11, "third fact", &[]),
            obs(15, "fourth fact", &["shared", "gamma"]),
        ]);
        let id = store.upsert_engram(domain, &record).await.unwrap().0;

        let expected = vec![
            (3i64, vec!["alpha".to_string(), "shared".to_string()]),
            (7, vec!["beta".to_string()]),
            (11, Vec::new()),
            (15, vec!["gamma".to_string(), "shared".to_string()]),
        ];
        assert_eq!(
            tags_by_line(&store, id).await,
            expected,
            "each tag set maps to its own observation after the first upsert"
        );

        // A re-upsert clears and recreates the child rows; join-on-line keeps the
        // mapping stable where a positional RETURNING map could scramble it.
        let id2 = store.upsert_engram(domain, &record).await.unwrap().0;
        assert_eq!(id2, id, "the same path keeps its engram id");
        assert_eq!(
            tags_by_line(&store, id).await,
            expected,
            "the mapping survives a re-upsert"
        );
    }
}
