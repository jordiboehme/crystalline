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
    ChunkJob, ChunkModelCount, DomainId, DomainStats, EmbeddingCoverage, EmbeddingRow, EngramId,
    EngramRecord, EngramSummary, FileStamp, FtsMode, GraphSlice, NewChunk, Page, RecentFilter,
    SearchHit, SearchQuery, Store, StoreInfo,
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
        let schema_version = migrations::apply(&conn).await?;
        let fts_native = probe_fts(&conn).await;
        Ok(TursoStore {
            _db: db,
            conn,
            db_path,
            schema_version,
            fts_native,
            tag_cache: Mutex::new(HashMap::new()),
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

    /// Resolve a `(domain, permalink)` pair to an engram id. Used by graph
    /// anchors and by tests; the M5 daemon resolves `crystalline://` anchors
    /// through this.
    pub async fn lookup_id(&self, domain: &str, permalink: &str) -> Result<Option<EngramId>> {
        let row = query_first(
            &self.conn,
            "SELECT e.id FROM engram e JOIN domain d ON d.id=e.domain_id WHERE d.name=?1 AND e.permalink=?2",
            vec![Value::Text(domain.to_string()), Value::Text(permalink.to_string())],
        )
        .await?;
        Ok(row.and_then(|r| cell_i64(&r, 0)).map(EngramId))
    }

    /// Return the `EXPLAIN QUERY PLAN` detail lines for a query. Diagnostic; used
    /// by the temporal-index test to assert the current filter is an index seek.
    pub async fn explain_query_plan(&self, sql: &str) -> Result<Vec<String>> {
        let rows = query_all(&self.conn, &format!("EXPLAIN QUERY PLAN {sql}"), vec![]).await?;
        Ok(rows.iter().filter_map(|r| cell_text(r, 3)).collect())
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

    async fn upsert_domain(&self, name: &str, path: &str) -> Result<DomainId> {
        self.conn
            .execute(
                "INSERT INTO domain(name, path) VALUES(?1, ?2) ON CONFLICT(name) DO UPDATE SET path=excluded.path",
                vec![Value::Text(name.to_string()), Value::Text(path.to_string())],
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

        for obs in &record.observations {
            self.conn
                .execute(
                    "INSERT INTO observation(engram_id, line, category, content, context) VALUES(?1,?2,?3,?4,?5)",
                    vec![
                        Value::Integer(engram_id),
                        Value::Integer(obs.line as i64),
                        Value::Text(obs.category.clone()),
                        Value::Text(obs.content.clone()),
                        opt_text(&obs.context),
                    ],
                )
                .await?;
            if !obs.tags.is_empty() {
                let oid = self.conn.last_insert_rowid();
                for tag in &obs.tags {
                    let tid = self.tag_id(tag).await?;
                    self.conn
                        .execute(
                            "INSERT OR IGNORE INTO observation_tag(observation_id, tag_id) VALUES(?1,?2)",
                            vec![Value::Integer(oid), Value::Integer(tid)],
                        )
                        .await?;
                }
            }
        }

        for rel in &record.relations {
            self.conn
                .execute(
                    "INSERT INTO relation(engram_id, domain_id, line, rel_type, to_target, to_domain, to_id) \
                     VALUES(?1,?2,?3,?4,?5,?6,NULL)",
                    vec![
                        Value::Integer(engram_id),
                        Value::Integer(domain.0),
                        Value::Integer(rel.line as i64),
                        Value::Text(rel.rel_type.clone()),
                        Value::Text(rel.to_target.clone()),
                        opt_text(&rel.to_domain),
                    ],
                )
                .await?;
        }

        for link in &record.links {
            self.conn
                .execute(
                    "INSERT INTO link(engram_id, domain_id, line, to_target, to_domain, to_id) \
                     VALUES(?1,?2,?3,?4,?5,NULL)",
                    vec![
                        Value::Integer(engram_id),
                        Value::Integer(domain.0),
                        Value::Integer(link.line as i64),
                        Value::Text(link.to_target.clone()),
                        opt_text(&link.to_domain),
                    ],
                )
                .await?;
        }

        for tag in &record.tags {
            let tid = self.tag_id(tag).await?;
            self.conn
                .execute(
                    "INSERT OR IGNORE INTO engram_tag(engram_id, tag_id) VALUES(?1,?2)",
                    vec![Value::Integer(engram_id), Value::Integer(tid)],
                )
                .await?;
        }

        Ok(EngramId(engram_id))
    }

    async fn delete_engram(&self, domain: DomainId, path: &str) -> Result<()> {
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

    async fn search(&self, query: &SearchQuery) -> Result<Page<SearchHit>> {
        search::run_search(&self.conn, query).await
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

        for c in chunks {
            match carry.get(&c.text_hash) {
                Some((model, dims, emb)) => {
                    self.conn
                        .execute(
                            "INSERT INTO chunk(engram_id, seq, text, text_hash, model, dims, embedding) \
                             VALUES(?1,?2,?3,?4,?5,?6,?7)",
                            vec![
                                Value::Integer(eid),
                                Value::Integer(c.seq),
                                Value::Text(c.text.clone()),
                                Value::Text(c.text_hash.clone()),
                                model.clone(),
                                dims.clone(),
                                Value::Blob(emb.clone()),
                            ],
                        )
                        .await?;
                }
                None => {
                    self.conn
                        .execute(
                            "INSERT INTO chunk(engram_id, seq, text, text_hash) VALUES(?1,?2,?3,?4)",
                            vec![
                                Value::Integer(eid),
                                Value::Integer(c.seq),
                                Value::Text(c.text.clone()),
                                Value::Text(c.text_hash.clone()),
                            ],
                        )
                        .await?;
                }
            }
        }
        Ok(())
    }

    async fn chunks_needing_embedding(&self, model: &str) -> Result<Vec<ChunkJob>> {
        let rows = query_all(
            &self.conn,
            "SELECT id, engram_id, seq, text, text_hash FROM chunk \
             WHERE embedding IS NULL OR model IS NULL OR model != ?1 \
             ORDER BY engram_id, seq",
            vec![Value::Text(model.to_string())],
        )
        .await?;
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
        for row in batch {
            if row.embedding.len() != row.dims {
                return Err(IndexError::Invalid(format!(
                    "embedding length {} does not match declared dims {}",
                    row.embedding.len(),
                    row.dims
                )));
            }
            let mut bytes = Vec::with_capacity(row.embedding.len() * 4);
            for f in &row.embedding {
                bytes.extend_from_slice(&f.to_le_bytes());
            }
            self.conn
                .execute(
                    "UPDATE chunk SET embedding=?1, dims=?2, model=?3 WHERE id=?4",
                    vec![
                        Value::Blob(bytes),
                        Value::Integer(row.dims as i64),
                        Value::Text(model.to_string()),
                        Value::Integer(row.chunk_id),
                    ],
                )
                .await?;
        }
        Ok(())
    }

    async fn embedding_coverage(&self) -> Result<EmbeddingCoverage> {
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

    async fn wipe(&self) -> Result<()> {
        self.conn.execute("BEGIN", ()).await?;
        for table in migrations::WIPE_TABLES {
            if let Err(e) = self.conn.execute(&format!("DELETE FROM {table}"), ()).await {
                let _ = self.conn.execute("ROLLBACK", ()).await;
                return Err(e.into());
            }
        }
        self.conn.execute("COMMIT", ()).await?;
        self.tag_cache.lock().unwrap().clear();
        Ok(())
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
            "SELECT d.id, d.name, d.path, d.last_sync, \
             (SELECT count(*) FROM engram e WHERE e.domain_id=d.id), \
             (SELECT count(*) FROM observation o JOIN engram e ON e.id=o.engram_id WHERE e.domain_id=d.id), \
             (SELECT count(*) FROM relation r WHERE r.domain_id=d.id), \
             (SELECT count(*) FROM relation r WHERE r.domain_id=d.id AND r.to_id IS NULL) \
             FROM domain d ORDER BY d.id",
            vec![],
        )
        .await?;
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
        self.conn.execute("BEGIN", ()).await?;
        Ok(())
    }

    async fn commit(&self) -> Result<()> {
        self.conn.execute("COMMIT", ()).await?;
        Ok(())
    }

    async fn rollback(&self) -> Result<()> {
        self.conn.execute("ROLLBACK", ()).await?;
        Ok(())
    }
}
