//! The shared service engine.
//!
//! Every data operation (the 12 MCP tools, the CLI data commands and the ctl
//! sync and reindex) runs through one [`Engine`]. It owns a single boxed
//! [`Store`] (`dyn Store`) behind a [`tokio::sync::Mutex`] so the backend's
//! single-connection model is honoured across the daemon's many tasks, the
//! optional embedding provider (built once), the resolved config and the chunk
//! parameters. The concrete backend is chosen at open time by the store factory
//! from the `database` config block.
//!
//! Files are the source of truth: every mutation writes the file first, then
//! upserts that single file into the store using the on-disk file stamp, so the
//! daemon's debounced watcher classifies the file as unchanged and never
//! reprocesses it (the idempotency guard, see `research/single-instance-ipc.md`).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, Duration, FixedOffset};
use crystalline_core::config::{DomainEntry, GlobalConfig};
use crystalline_core::emit::{
    append_body, insert_after_section, insert_before_section, prepend_body, replace_section,
    touch_timestamp,
};
use crystalline_core::schema::{self, Schema};
use crystalline_core::{
    CrystallineUrl, Engram, Frontmatter, Manifest, YamlValue, parse_engram, slugify,
};
use crystalline_index::{
    ChunkParams, DomainId, DomainKind, EmbeddingProvider, EngramDescriptor, EngramId, EngramRecord,
    FileStamp, RecentFilter, SearchMode, SearchQuery, Store, chunk_engram, configured_model_id,
    parse_metadata_filters, provider_from_config, sync_domain_with,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

use crate::params::*;

/// How many chunks are embedded per background batch.
const EMBED_BATCH: usize = 16;

/// An error from an engine operation, mapped to actionable tool errors.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    /// A referenced domain is not registered.
    #[error("domain '{domain}' not registered; registered: [{}]", .registered.join(", "))]
    UnknownDomain {
        /// The requested domain.
        domain: String,
        /// The registered domain names.
        registered: Vec<String>,
    },
    /// The engram or section was not found.
    #[error("{0}")]
    NotFound(String),
    /// A bare identifier matched engrams in more than one domain.
    #[error("{0}")]
    Ambiguous(String),
    /// A write would clobber an existing engram without `overwrite`.
    #[error("{0}")]
    Conflict(String),
    /// The request was malformed.
    #[error("{0}")]
    Invalid(String),
    /// A content mutation was attempted against a read-only instance.
    #[error("this instance is read-only; content mutations are disabled")]
    ReadOnly,
    /// A filesystem error.
    #[error("io error at {path}: {source}")]
    Io {
        /// The path involved.
        path: String,
        /// The underlying error.
        source: std::io::Error,
    },
    /// An error from the storage or parse layer.
    #[error("{0}")]
    Internal(String),
}

impl From<crystalline_index::IndexError> for EngineError {
    fn from(e: crystalline_index::IndexError) -> Self {
        match e {
            crystalline_index::IndexError::Constraint(m) => EngineError::Conflict(m),
            crystalline_index::IndexError::NotFound(m) => EngineError::NotFound(m),
            crystalline_index::IndexError::Invalid(m) => EngineError::Invalid(m),
            // A stale compare-and-swap surfaces as a conflict, mirroring the
            // expected_replacements ergonomics: re-read and retry.
            crystalline_index::IndexError::StaleEdit { expected, found } => {
                EngineError::Conflict(format!(
                    "engram changed since you last read it (expected checksum {expected}, found {found}); re-read and retry"
                ))
            }
            other => EngineError::Internal(other.to_string()),
        }
    }
}

/// The result type used across the engine.
pub type Result<T> = std::result::Result<T, EngineError>;

/// A message from the engine to the daemon's file watcher: a domain root to
/// start or stop watching, raised when a domain registered after the daemon
/// started is first resolved (see [`Engine::domain_entry`]) or removed (see
/// [`Engine::forget_domain`]). Only the daemon's watcher task consumes these;
/// embedded stdio and standalone CLI commands never install a receiver.
#[derive(Debug, Clone)]
pub enum WatchEvent {
    /// Start watching this domain's root.
    Add(String, PathBuf),
    /// Stop watching this domain's root.
    Remove(String),
}

/// Where a domain's engram content comes from and goes to: files on disk for a
/// file domain, or the database for a virtual domain. This is the one seam every
/// content mutation branches on; everything after `parse_engram` is shared (see
/// [`Engine::index_markdown`]).
enum ContentSource {
    /// A file domain rooted at this filesystem path.
    File {
        /// The tilde-expanded domain root.
        root: PathBuf,
    },
    /// A virtual domain whose engrams live only in the database.
    Virtual,
}

/// The shared service engine.
pub struct Engine {
    store: Arc<Mutex<dyn Store>>,
    config: GlobalConfig,
    // The `--config` override this engine was started with, so a domain
    // registered after startup (`domain add` only ever touches the file on
    // disk) can be found by re-reading the same file. See `refresh_domain`.
    config_path: Option<PathBuf>,
    // Domains discovered by re-reading the global config after startup,
    // layered on top of the immutable `config` snapshot taken at construction.
    // The full entry is kept (kind plus optional path) so a virtual domain
    // added mid-session is served from the database, not mistaken for a file
    // domain with an empty root.
    discovered_domains: std::sync::RwLock<HashMap<String, DomainEntry>>,
    // Told about domains discovered this way so the daemon's watcher can pick
    // them up without a restart. `None` outside the daemon.
    watch_tx: Option<tokio::sync::mpsc::UnboundedSender<WatchEvent>>,
    // Swappable so the daemon can build the (possibly downloading) provider in the
    // background without blocking readiness or text search.
    provider: std::sync::RwLock<Option<Arc<dyn EmbeddingProvider>>>,
    model_id: String,
    chunk_params: ChunkParams,
    // When true the four content-mutating methods refuse early with
    // `EngineError::ReadOnly`. Set at construction from the effective mode
    // (explicit flag or `service.read_only`). Index maintenance is unaffected.
    read_only: bool,
}

impl Engine {
    /// Build an engine around an already-open store, an optional provider and a
    /// config. A `None` provider can be installed later with [`Engine::set_provider`].
    /// `config_path` is the `--config` override (if any) this engine started
    /// with, used to re-read the config file when a domain is not in the
    /// startup snapshot; pass `None` when the caller never re-reads (a
    /// one-shot standalone CLI command already sees a fresh config).
    pub fn new(
        store: Arc<Mutex<dyn Store>>,
        config: GlobalConfig,
        provider: Option<Arc<dyn EmbeddingProvider>>,
        config_path: Option<PathBuf>,
    ) -> Engine {
        let model_id = configured_model_id(config.embeddings.as_ref());
        let chunk_params = ChunkParams::for_model(model_id.clone());
        Engine {
            store,
            config,
            config_path,
            discovered_domains: std::sync::RwLock::new(HashMap::new()),
            watch_tx: None,
            provider: std::sync::RwLock::new(provider),
            model_id,
            chunk_params,
            read_only: false,
        }
    }

    /// Install the channel the daemon's watcher listens on for domains
    /// discovered after startup. Only wired by `run_serve`.
    pub fn with_watch_channel(
        mut self,
        tx: tokio::sync::mpsc::UnboundedSender<WatchEvent>,
    ) -> Engine {
        self.watch_tx = Some(tx);
        self
    }

    /// Set the read-only mode. In read-only mode the four content-mutating
    /// methods refuse with `EngineError::ReadOnly`; every read path and all
    /// index maintenance (sync, reindex, embedding) run unchanged.
    pub fn with_read_only(mut self, read_only: bool) -> Engine {
        self.read_only = read_only;
        self
    }

    /// Whether this engine serves the content API read-only.
    pub fn read_only(&self) -> bool {
        self.read_only
    }

    /// The shared store handle, for the daemon's watcher and embed loop.
    pub fn store(&self) -> Arc<Mutex<dyn Store>> {
        self.store.clone()
    }

    /// The active embedding provider, if one has been installed.
    pub fn provider(&self) -> Option<Arc<dyn EmbeddingProvider>> {
        self.provider.read().unwrap().clone()
    }

    /// Install (or replace) the embedding provider. Used by the daemon after it
    /// builds the provider in the background.
    pub fn set_provider(&self, provider: Arc<dyn EmbeddingProvider>) {
        *self.provider.write().unwrap() = Some(provider);
    }

    /// The registered config.
    pub fn config(&self) -> &GlobalConfig {
        &self.config
    }

    /// The active embedding model id.
    pub fn model_id(&self) -> &str {
        &self.model_id
    }

    // --- domain helpers ------------------------------------------------------

    /// Resolve a registered domain to its content source: a filesystem root for
    /// a file domain, or the database for a virtual domain. Errors when the
    /// domain is not registered (the write path wants that), the layered lookup
    /// mirroring [`Engine::domain_entry`].
    fn content_source(&self, name: &str) -> Result<ContentSource> {
        let entry = self.domain_entry(name)?;
        Ok(self.source_of(&entry))
    }

    /// The content source implied by a domain entry: its filesystem root when it
    /// is a file domain with a path, else the database. A file domain with no
    /// path (an impossible config, but defended) falls back to the database.
    fn source_of(&self, entry: &DomainEntry) -> ContentSource {
        match entry.file_path() {
            Some(root) if !entry.is_virtual() => ContentSource::File { root },
            _ => ContentSource::Virtual,
        }
    }

    /// The content source to read a resolved engram through: a locally
    /// registered file domain's root, or the database. Never errors, so a
    /// database-only domain (virtual, or a file domain whose rows this instance
    /// sees but whose files it does not hold) still resolves for reading.
    fn read_source(&self, name: &str) -> ContentSource {
        match self.domain_entry(name) {
            Ok(entry) => self.source_of(&entry),
            Err(_) => ContentSource::Virtual,
        }
    }

    /// The registered entry for a domain: the startup snapshot, then the
    /// discovered overlay, then a fresh re-read of the config from disk (a
    /// `domain add` only ever edits the file, never this in-memory snapshot).
    fn domain_entry(&self, name: &str) -> Result<DomainEntry> {
        if let Some(entry) = self.config.domains.get(name) {
            return Ok(entry.clone());
        }
        if let Some(entry) = self.discovered_domains.read().unwrap().get(name) {
            return Ok(entry.clone());
        }
        if let Some(entry) = self.refresh_domain(name) {
            return Ok(entry);
        }
        Err(EngineError::UnknownDomain {
            domain: name.to_string(),
            registered: self.known_domain_names(),
        })
    }

    /// Re-read the global config from disk looking for a domain registered
    /// after this engine started. A hit is cached in `discovered_domains` and,
    /// for a file domain on the daemon, reported over `watch_tx` so the watcher
    /// starts watching its root without a restart. A virtual domain has no root,
    /// so it is cached but never watched.
    fn refresh_domain(&self, name: &str) -> Option<DomainEntry> {
        let fresh = crate::daemon::load_config(self.config_path.as_deref()).ok()?;
        let entry = fresh.domains.get(name)?.clone();
        self.discovered_domains
            .write()
            .unwrap()
            .insert(name.to_string(), entry.clone());
        if let Some(tx) = &self.watch_tx
            && let Some(root) = entry.file_path()
            && !entry.is_virtual()
        {
            let _ = tx.send(WatchEvent::Add(name.to_string(), root));
        }
        Some(entry)
    }

    /// Every domain name this engine currently knows about: the startup
    /// snapshot plus anything discovered since.
    fn known_domain_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.config.domains.keys().cloned().collect();
        names.extend(self.discovered_domains.read().unwrap().keys().cloned());
        names
    }

    /// Forget a domain removed by `domain remove` while this engine is live:
    /// drop it from the discovered overlay and, on the daemon, tell the
    /// watcher to stop watching its root. The index rows are never touched
    /// here; they are left for the next full reindex.
    pub fn forget_domain(&self, name: &str) {
        self.discovered_domains.write().unwrap().remove(name);
        if let Some(tx) = &self.watch_tx {
            let _ = tx.send(WatchEvent::Remove(name.to_string()));
        }
    }

    /// Resolve an identifier (permalink, domain/permalink, title or
    /// `crystalline://` URL) to a descriptor and the content source to read it
    /// through. Resolution goes through the store, so a virtual domain (or any
    /// database-only domain) resolves without a filesystem root.
    async fn resolve(
        &self,
        identifier: &str,
        domain: Option<&str>,
    ) -> Result<(EngramDescriptor, ContentSource)> {
        if let Some(url) = CrystallineUrl::parse(identifier) {
            let store = self.store.lock().await;
            let d = store
                .find_engram(&url.domain, &url.permalink)
                .await?
                .ok_or_else(|| {
                    EngineError::NotFound(format!(
                        "no engram '{}' in domain '{}'",
                        url.permalink, url.domain
                    ))
                })?;
            drop(store);
            let source = self.read_source(&url.domain);
            return Ok((d, source));
        }

        if let Some(dom) = domain {
            let store = self.store.lock().await;
            let d = store.find_engram(dom, identifier).await?.ok_or_else(|| {
                EngineError::NotFound(format!("no engram '{identifier}' in domain '{dom}'"))
            })?;
            drop(store);
            let source = self.read_source(dom);
            return Ok((d, source));
        }

        // `domain/permalink` form: the leading segment is a known domain.
        if let Some((maybe_dom, rest)) = identifier.split_once('/')
            && self.domain_entry(maybe_dom).is_ok()
        {
            let store = self.store.lock().await;
            if let Some(d) = store.find_engram(maybe_dom, rest).await? {
                drop(store);
                let source = self.read_source(maybe_dom);
                return Ok((d, source));
            }
        }

        // Bare identifier across all domains.
        let store = self.store.lock().await;
        let mut matches = store.find_engram_any(identifier).await?;
        drop(store);
        match matches.len() {
            0 => Err(EngineError::NotFound(format!(
                "no engram matches '{identifier}'"
            ))),
            1 => {
                let d = matches.remove(0);
                let source = self.read_source(&d.domain);
                Ok((d, source))
            }
            _ => {
                let doms: Vec<String> = matches.iter().map(|d| d.domain.clone()).collect();
                Err(EngineError::Ambiguous(format!(
                    "'{identifier}' matches engrams in multiple domains: [{}]; pass a domain",
                    doms.join(", ")
                )))
            }
        }
    }

    /// Parse and index one markdown document into a domain, whatever its origin.
    /// This is the content-agnostic tail shared by every mutation: file writes
    /// pass the on-disk stamp and `None`; virtual writes pass a synthesized stamp
    /// and, for an edit, the CAS `expected_sha`. Everything after `parse_engram`
    /// (upsert, chunk, resolve refs) is identical, and it all runs in one
    /// transaction.
    ///
    /// `store_full` controls what lands in the `content` column. A file domain
    /// stores the body only (its source of truth is the file on disk, read back
    /// verbatim), matching the historical projection. A virtual domain has no
    /// file, so it stores the full markdown (frontmatter plus body): that is the
    /// exact document a read, edit, export or CAS checksum must round-trip, and
    /// `virtual_stamp` hashed the same full markdown.
    #[allow(clippy::too_many_arguments)]
    async fn index_markdown(
        &self,
        store: &dyn Store,
        domain_id: DomainId,
        rel: &str,
        text: &str,
        stamp: FileStamp,
        expected_sha: Option<&str>,
        store_full: bool,
    ) -> Result<EngramId> {
        let engram = parse_engram(text).map_err(|e| EngineError::Invalid(e.to_string()))?;
        let mut record = EngramRecord::from_engram(&engram, rel, stamp);
        if store_full {
            record.content = text.to_string();
        }

        store.begin().await?;
        let result = async {
            let id = store
                .upsert_engram_checked(domain_id, &record, expected_sha)
                .await?;
            let chunks = chunk_engram(
                &record.title,
                record.description.as_deref(),
                &record.content,
                &self.chunk_params,
            );
            store.replace_chunks(id, &chunks).await?;
            store.resolve_pending_relations(domain_id).await?;
            Ok::<EngramId, EngineError>(id)
        }
        .await;
        match result {
            Ok(id) => {
                store.commit().await?;
                Ok(id)
            }
            Err(e) => {
                let _ = store.rollback().await;
                Err(e)
            }
        }
    }

    /// Upsert a single file into the store from disk, carrying the on-disk stamp
    /// so the watcher does not reprocess it. The file-origin wrapper over
    /// [`Engine::index_markdown`].
    async fn reindex_file(
        &self,
        store: &dyn Store,
        domain_id: DomainId,
        root: &Path,
        rel: &str,
    ) -> Result<EngramId> {
        let abs = join_rel(root, rel);
        let bytes = std::fs::read(&abs).map_err(|source| EngineError::Io {
            path: abs.display().to_string(),
            source,
        })?;
        let meta = std::fs::metadata(&abs).map_err(|source| EngineError::Io {
            path: abs.display().to_string(),
            source,
        })?;
        let stamp = FileStamp {
            mtime: mtime_secs(&meta),
            size: meta.len(),
            sha256: sha256_hex(&bytes),
        };
        let text = String::from_utf8(bytes)
            .map_err(|_| EngineError::Invalid(format!("{} is not valid UTF-8", abs.display())))?;
        // A file domain stores the body only; its source of truth is the file.
        self.index_markdown(store, domain_id, rel, &text, stamp, None, false)
            .await
    }

    /// Load an engram's parsed form through a content source: the file on disk
    /// for a file domain, or the stored `content` column for a virtual domain.
    /// Backs validation and schema inference across both kinds.
    async fn load_engram(
        &self,
        source: &ContentSource,
        domain_id: DomainId,
        rel: &str,
    ) -> Option<Engram> {
        match source {
            ContentSource::File { root } => read_engram_file(root, rel),
            ContentSource::Virtual => {
                let store = self.store.lock().await;
                let content = store.engram_content(domain_id, rel).await.ok().flatten()?;
                parse_engram(&content).ok()
            }
        }
    }

    /// Load an engram's full markdown through the read-path policy: the local
    /// file when a file domain holds it on disk, else the stored `content`
    /// column. This keeps files-are-truth for the host while serving virtual and
    /// non-host reads from the database.
    async fn load_content(
        &self,
        source: &ContentSource,
        desc: &EngramDescriptor,
    ) -> Result<String> {
        if let ContentSource::File { root } = source {
            let abs = join_rel(root, &desc.path);
            if let Ok(text) = std::fs::read_to_string(&abs) {
                return Ok(text);
            }
        }
        let store = self.store.lock().await;
        store
            .engram_content(desc.domain_id, &desc.path)
            .await?
            .ok_or_else(|| {
                EngineError::NotFound(format!(
                    "no content stored for '{}' in domain '{}'",
                    desc.permalink, desc.domain
                ))
            })
    }

    // --- write ---------------------------------------------------------------

    /// Create or overwrite an engram, then index it. A file domain writes the
    /// markdown file first (files-are-truth) then reindexes it from disk; a
    /// virtual domain builds the markdown in memory and indexes it straight into
    /// the database, touching no filesystem.
    pub async fn write_engram(&self, p: &WriteParams) -> Result<Value> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        let source = self.content_source(&p.domain)?;
        let engram_type = p
            .engram_type
            .clone()
            .unwrap_or_else(|| "engram".to_string());
        let status = p.status.clone().unwrap_or_else(|| "current".to_string());
        let tags = p.tags.clone().unwrap_or_default();

        let folder = p.folder.clone().unwrap_or_default();
        let title_slug = slugify(&p.title);
        if title_slug.is_empty() {
            return Err(EngineError::Invalid(
                "title does not slugify to a permalink; provide a title with letters or digits"
                    .into(),
            ));
        }
        let rel = if folder.trim_matches('/').is_empty() {
            format!("{title_slug}.md")
        } else {
            format!("{}/{title_slug}.md", folder.trim_matches('/'))
        };
        let permalink = slugify(&rel);

        // Enforce overwrite semantics against the existing permalink.
        {
            let store = self.store.lock().await;
            if let Some(existing) = store.find_engram(&p.domain, &permalink).await?
                && !p.overwrite
            {
                return Err(EngineError::Conflict(format!(
                    "permalink '{permalink}' already exists in domain '{}' (at {}); pass overwrite=true to replace",
                    p.domain, existing.path
                )));
            }
        }

        let today = chrono::Utc::now().date_naive();
        let now = now_offset();
        let markdown = build_markdown(
            &engram_type,
            &p.title,
            &permalink,
            &tags,
            &status,
            &today.format("%Y-%m-%d").to_string(),
            &now.to_rfc3339(),
            p.metadata.as_ref(),
            &p.content,
        )?;

        match &source {
            ContentSource::File { root } => {
                let abs = join_rel(root, &rel);
                write_file(&abs, &markdown)?;
                let store = self.store.lock().await;
                let domain_id = store
                    .upsert_domain(&p.domain, Some(&root.to_string_lossy()), DomainKind::File)
                    .await?;
                self.reindex_file(&*store, domain_id, root, &rel).await?;
            }
            ContentSource::Virtual => {
                let store = self.store.lock().await;
                let domain_id = store
                    .upsert_domain(&p.domain, None, DomainKind::Virtual)
                    .await?;
                let stamp = virtual_stamp(&markdown);
                self.index_markdown(&*store, domain_id, &rel, &markdown, stamp, None, true)
                    .await?;
            }
        }

        Ok(json!({
            "domain": p.domain,
            "permalink": permalink,
            "path": rel,
            "title": p.title,
            "type": engram_type,
            "status": status,
            "action": if p.overwrite { "written" } else { "created" },
        }))
    }

    // --- read ----------------------------------------------------------------

    /// Read an engram's full markdown and resolved frontmatter. The content
    /// comes from the local file when a file domain holds it, else from the
    /// database (virtual domains, and non-host reads over a shared database). The
    /// returned `checksum` is the CAS token an `edit_engram` can pass back as
    /// `expected_checksum` to detect a change since this read.
    pub async fn read_engram(&self, p: &ReadParams) -> Result<Value> {
        let (desc, source) = self.resolve(&p.identifier, p.domain.as_deref()).await?;
        let content = self.load_content(&source, &desc).await?;
        let engram = parse_engram(&content).map_err(|e| EngineError::Invalid(e.to_string()))?;
        let checksum = sha256_hex(content.as_bytes());
        Ok(json!({
            "domain": desc.domain,
            "permalink": desc.permalink,
            "title": desc.title,
            "type": desc.engram_type,
            "status": desc.status,
            "path": desc.path,
            "url": format!("crystalline://{}/{}", desc.domain, desc.permalink),
            "content": content,
            "checksum": checksum,
            "frontmatter": engram.frontmatter,
            "observations": engram.observations,
            "relations": engram.relations,
        }))
    }

    // --- edit ----------------------------------------------------------------

    /// Apply a surgical edit to an engram, then reindex it. A file domain edits
    /// the file on disk and reindexes it; a virtual domain reads the current
    /// content from the database, applies the same edit and writes it back under
    /// a compare-and-swap guard so a stale edit is refused rather than silently
    /// clobbering a concurrent change (see `expected_checksum`).
    pub async fn edit_engram(&self, p: &EditParams) -> Result<Value> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        let (desc, source) = self.resolve(&p.identifier, Some(&p.domain)).await?;

        match &source {
            ContentSource::File { root } => {
                let abs = join_rel(root, &desc.path);
                let current = std::fs::read_to_string(&abs).map_err(|source| EngineError::Io {
                    path: abs.display().to_string(),
                    source,
                })?;
                let edited = self.apply_edit(&current, p, &desc.permalink)?;
                let edited = touch_timestamp(&edited, now_offset());
                write_file(&abs, &edited)?;
                let store = self.store.lock().await;
                self.reindex_file(&*store, desc.domain_id, root, &desc.path)
                    .await?;
            }
            ContentSource::Virtual => {
                let current = {
                    let store = self.store.lock().await;
                    store
                        .engram_content(desc.domain_id, &desc.path)
                        .await?
                        .ok_or_else(|| {
                            EngineError::NotFound(format!(
                                "no content stored for '{}' in domain '{}'",
                                desc.permalink, desc.domain
                            ))
                        })?
                };
                // The CAS token: the caller's expected_checksum when supplied
                // (guarding against a change since their read), else the sha of
                // what we just read (last-write-wins, matching the settled
                // semantics).
                let expected = p
                    .expected_checksum
                    .clone()
                    .unwrap_or_else(|| sha256_hex(current.as_bytes()));
                let edited = self.apply_edit(&current, p, &desc.permalink)?;
                let edited = touch_timestamp(&edited, now_offset());
                let stamp = virtual_stamp(&edited);
                let store = self.store.lock().await;
                self.index_markdown(
                    &*store,
                    desc.domain_id,
                    &desc.path,
                    &edited,
                    stamp,
                    Some(&expected),
                    true,
                )
                .await?;
            }
        }

        Ok(json!({
            "domain": desc.domain,
            "permalink": desc.permalink,
            "path": desc.path,
            "operation": p.operation,
        }))
    }

    /// Apply one edit operation to an engram's markdown, returning the edited
    /// text. Content-agnostic: the same logic serves file and virtual edits.
    fn apply_edit(&self, source: &str, p: &EditParams, permalink: &str) -> Result<String> {
        Ok(match p.operation.as_str() {
            "append" => append_body(source, &p.content),
            "prepend" => prepend_body(source, &p.content),
            "find_replace" => {
                let find = p.find_text.as_deref().ok_or_else(|| {
                    EngineError::Invalid("find_replace requires find_text".into())
                })?;
                if find.is_empty() {
                    return Err(EngineError::Invalid("find_text must not be empty".into()));
                }
                let count = source.matches(find).count();
                if count == 0 {
                    return Err(EngineError::NotFound(format!(
                        "find_text '{find}' not found in '{permalink}'"
                    )));
                }
                if let Some(expected) = p.expected_replacements
                    && expected != count
                {
                    return Err(EngineError::Invalid(format!(
                        "expected {expected} replacements of '{find}' but found {count}"
                    )));
                }
                source.replace(find, &p.content)
            }
            "replace_section" => {
                let section = self.require_section(p)?;
                replace_section(source, section, &p.content, p.include_subsections)
                    .map_err(section_err)?
            }
            "insert_before_section" => {
                let section = self.require_section(p)?;
                insert_before_section(source, section, &p.content).map_err(section_err)?
            }
            "insert_after_section" => {
                let section = self.require_section(p)?;
                insert_after_section(source, section, &p.content).map_err(section_err)?
            }
            other => {
                return Err(EngineError::Invalid(format!(
                    "unknown edit operation '{other}'; expected append, prepend, find_replace, replace_section, insert_before_section or insert_after_section"
                )));
            }
        })
    }

    fn require_section<'a>(&self, p: &'a EditParams) -> Result<&'a str> {
        p.section
            .as_deref()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                EngineError::Invalid(format!("operation '{}' requires a section", p.operation))
            })
    }

    // --- move ----------------------------------------------------------------

    /// Move an engram to a new path or domain, rewriting inbound bare links on a
    /// cross-domain move. Source and destination may each be a file or virtual
    /// domain, so a move carries content between the two truths: a same-domain
    /// move is a rename (no reparse), a cross-domain move reads the source
    /// content and re-indexes it into the destination's source.
    pub async fn move_engram(&self, p: &MoveParams) -> Result<Value> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        let (src, src_source) = self.resolve(&p.identifier, Some(&p.domain)).await?;
        let dest_domain = p
            .destination_domain
            .clone()
            .unwrap_or_else(|| p.domain.clone());
        let dest_source = self.content_source(&dest_domain)?;
        let dest_rel = normalize_md(&p.destination);
        if dest_rel.is_empty() {
            return Err(EngineError::Invalid("destination path is empty".into()));
        }
        let cross = dest_domain != p.domain;

        // Destination collision check, on disk or in the database.
        self.ensure_dest_free(&dest_source, &dest_domain, &dest_rel)
            .await?;

        // Gather inbound refs before the move while `to_id` still points at src.
        let inbound = if cross && p.update_links.unwrap_or(true) {
            let store = self.store.lock().await;
            store
                .inbound_refs(src.id, src.domain_id, &src.permalink, &src.title)
                .await?
        } else {
            Vec::new()
        };

        if cross {
            // Read the source content (file when present, else database), index
            // it into the destination source, then remove the source.
            let content = self.load_content(&src_source, &src).await?;
            match &dest_source {
                ContentSource::File { root } => {
                    let dest_abs = join_rel(root, &dest_rel);
                    write_file(&dest_abs, &content)?;
                    let store = self.store.lock().await;
                    let dest_id = store
                        .upsert_domain(
                            &dest_domain,
                            Some(&root.to_string_lossy()),
                            DomainKind::File,
                        )
                        .await?;
                    self.reindex_file(&*store, dest_id, root, &dest_rel).await?;
                }
                ContentSource::Virtual => {
                    let store = self.store.lock().await;
                    let dest_id = store
                        .upsert_domain(&dest_domain, None, DomainKind::Virtual)
                        .await?;
                    let stamp = virtual_stamp(&content);
                    self.index_markdown(&*store, dest_id, &dest_rel, &content, stamp, None, true)
                        .await?;
                }
            }
            if let ContentSource::File { root } = &src_source {
                let src_abs = join_rel(root, &src.path);
                let _ = std::fs::remove_file(&src_abs);
            }
            let store = self.store.lock().await;
            store.delete_engram(src.domain_id, &src.path).await?;
        } else {
            // Same-domain rename: move the file when file-backed, then rename the
            // row in place with no reparse (the permalink follows only when it
            // was path-derived).
            if let ContentSource::File { root } = &src_source {
                let src_abs = join_rel(root, &src.path);
                let dest_abs = join_rel(root, &dest_rel);
                let content = std::fs::read(&src_abs).map_err(|source| EngineError::Io {
                    path: src_abs.display().to_string(),
                    source,
                })?;
                write_bytes(&dest_abs, &content)?;
                std::fs::remove_file(&src_abs).map_err(|source| EngineError::Io {
                    path: src_abs.display().to_string(),
                    source,
                })?;
            }
            let store = self.store.lock().await;
            store
                .rename_engram(src.domain_id, &src.path, &dest_rel)
                .await?;
        }

        // Rewrite inbound bare links from other domains to the prefixed form.
        let mut rewritten = 0usize;
        for r in inbound {
            if r.src_domain == dest_domain || r.to_target.contains(':') {
                continue;
            }
            let needle = format!("[[{}]]", r.to_target);
            let prefixed = format!("[[{dest_domain}:{}]]", r.to_target);
            match self.read_source(&r.src_domain) {
                ContentSource::File { root } => {
                    let linker_abs = join_rel(&root, &r.src_path);
                    let Ok(text) = std::fs::read_to_string(&linker_abs) else {
                        continue;
                    };
                    if !text.contains(&needle) {
                        continue;
                    }
                    let replaced = touch_timestamp(&text.replace(&needle, &prefixed), now_offset());
                    write_file(&linker_abs, &replaced)?;
                    let store = self.store.lock().await;
                    self.reindex_file(&*store, r.src_domain_id, &root, &r.src_path)
                        .await?;
                    rewritten += 1;
                }
                ContentSource::Virtual => {
                    let current = {
                        let store = self.store.lock().await;
                        store.engram_content(r.src_domain_id, &r.src_path).await?
                    };
                    let Some(text) = current else { continue };
                    if !text.contains(&needle) {
                        continue;
                    }
                    let replaced = touch_timestamp(&text.replace(&needle, &prefixed), now_offset());
                    let stamp = virtual_stamp(&replaced);
                    let store = self.store.lock().await;
                    self.index_markdown(
                        &*store,
                        r.src_domain_id,
                        &r.src_path,
                        &replaced,
                        stamp,
                        None,
                        true,
                    )
                    .await?;
                    rewritten += 1;
                }
            }
        }

        Ok(json!({
            "from": { "domain": p.domain, "permalink": src.permalink, "path": src.path },
            "to": { "domain": dest_domain, "path": dest_rel },
            "cross_domain": cross,
            "links_rewritten": rewritten,
        }))
    }

    /// Refuse a move whose destination path is already taken, checking disk for
    /// a file domain and the database for a virtual one.
    async fn ensure_dest_free(
        &self,
        dest_source: &ContentSource,
        dest_domain: &str,
        dest_rel: &str,
    ) -> Result<()> {
        let taken = match dest_source {
            ContentSource::File { root } => join_rel(root, dest_rel).exists(),
            ContentSource::Virtual => {
                let store = self.store.lock().await;
                let dest_id = store
                    .upsert_domain(dest_domain, None, DomainKind::Virtual)
                    .await?;
                store.engram_content(dest_id, dest_rel).await?.is_some()
            }
        };
        if taken {
            return Err(EngineError::Conflict(format!(
                "destination '{dest_rel}' already exists in domain '{dest_domain}'"
            )));
        }
        Ok(())
    }

    // --- delete --------------------------------------------------------------

    /// Delete an engram and its index rows. A file domain also removes the file
    /// on disk; a virtual domain only drops the database rows.
    pub async fn delete_engram(&self, p: &DeleteParams) -> Result<Value> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        let (desc, source) = self.resolve(&p.identifier, Some(&p.domain)).await?;
        if let ContentSource::File { root } = &source {
            let abs = join_rel(root, &desc.path);
            std::fs::remove_file(&abs).map_err(|source| EngineError::Io {
                path: abs.display().to_string(),
                source,
            })?;
        }
        let store = self.store.lock().await;
        store.delete_engram(desc.domain_id, &desc.path).await?;
        Ok(json!({
            "domain": desc.domain,
            "permalink": desc.permalink,
            "path": desc.path,
            "deleted": true,
        }))
    }

    // --- search --------------------------------------------------------------

    /// Search across domains, embedding the query when the mode needs it.
    pub async fn search_engrams(&self, p: &SearchParams) -> Result<Value> {
        let requested = parse_mode(p.search_type.as_deref())?;
        let text = p.query.clone().filter(|s| !s.trim().is_empty());
        let mut query = SearchQuery {
            text: text.clone(),
            domains: p.domains.clone().filter(|d| !d.is_empty()),
            engram_type: p.engram_type.clone(),
            status: p.status.clone(),
            tags: p.tags.clone().filter(|t| !t.is_empty()),
            after: p.after.clone(),
            min_similarity: p.min_similarity,
            limit: p.limit.unwrap_or(10).max(1),
            page: p.page.unwrap_or(1).max(1),
            ..SearchQuery::default()
        };
        if let Some(mf) = &p.metadata_filters {
            query.metadata_filters =
                parse_metadata_filters(mf).map_err(|e| EngineError::Invalid(e.to_string()))?;
        }

        let store = self.store.lock().await;
        let effective = self
            .effective_mode(&*store, requested, text.is_some())
            .await?;
        query.mode = effective;
        if matches!(effective, SearchMode::Semantic | SearchMode::Hybrid)
            && let Some(provider) = self.provider()
        {
            let q = text.clone().unwrap_or_default();
            let vecs = provider
                .embed_queries(&[q])
                .await
                .map_err(|e| EngineError::Internal(e.to_string()))?;
            query.query_embedding = vecs.into_iter().next();
            query.active_model = Some(self.model_id.clone());
        }

        let page = store.search(&query).await?;
        Ok(json!({
            "mode": mode_str(effective),
            "total": page.total,
            "page": page.page,
            "limit": page.limit,
            "count": page.items.len(),
            "hits": serde_json::to_value(&page.items).unwrap_or(Value::Null),
        }))
    }

    async fn effective_mode(
        &self,
        store: &dyn Store,
        requested: SearchMode,
        has_text: bool,
    ) -> Result<SearchMode> {
        if !matches!(requested, SearchMode::Semantic | SearchMode::Hybrid) {
            return Ok(requested);
        }
        if !has_text || self.provider().is_none() {
            return Ok(SearchMode::Text);
        }
        let coverage = store.embedding_coverage().await?;
        if coverage.has_active_embeddings(&self.model_id) {
            Ok(requested)
        } else {
            Ok(SearchMode::Text)
        }
    }

    // --- context -------------------------------------------------------------

    /// Traverse the graph around a `crystalline://` anchor.
    pub async fn build_context(&self, p: &ContextParams) -> Result<Value> {
        let url = CrystallineUrl::parse(&p.anchor).ok_or_else(|| {
            EngineError::Invalid(format!("anchor '{}' is not a crystalline:// URL", p.anchor))
        })?;
        let depth = p.depth.unwrap_or(1).clamp(1, 3);
        let max_related = p.max_related.unwrap_or(10);
        let domain_filter = p.domains.clone().filter(|d| !d.is_empty());

        let store = self.store.lock().await;
        let seeds: Vec<EngramDescriptor> = if url.glob {
            store
                .list_engrams(&url.domain, None, None)
                .await?
                .into_iter()
                .filter(|d| url.matches(&d.domain, &d.permalink))
                .collect()
        } else {
            match store.find_engram(&url.domain, &url.permalink).await? {
                Some(d) => vec![d],
                None => {
                    return Err(EngineError::NotFound(format!(
                        "no engram '{}' in domain '{}'",
                        url.permalink, url.domain
                    )));
                }
            }
        };
        if seeds.is_empty() {
            return Err(EngineError::NotFound(format!(
                "anchor '{}' matched no engrams",
                p.anchor
            )));
        }
        let seed_ids: HashSet<i64> = seeds.iter().map(|d| d.id.0).collect();
        let ids: Vec<EngramId> = seeds.iter().map(|d| d.id).collect();
        let slice = store.neighbors(&ids, depth).await?;

        // Keep every seed, cap related nodes and apply the optional domain filter.
        let mut kept: HashSet<i64> = HashSet::new();
        let mut nodes = Vec::new();
        let mut related = 0usize;
        for node in &slice.nodes {
            if let Some(filter) = &domain_filter
                && !filter.contains(&node.domain)
            {
                continue;
            }
            let is_seed = seed_ids.contains(&node.id.0);
            if !is_seed {
                if related >= max_related {
                    continue;
                }
                related += 1;
            }
            kept.insert(node.id.0);
            nodes.push(json!({
                "id": node.id.0,
                "domain": node.domain,
                "permalink": node.permalink,
                "title": node.title,
                "type": node.engram_type,
                "seed": is_seed,
            }));
        }
        let edges: Vec<Value> = slice
            .edges
            .iter()
            .filter(|e| kept.contains(&e.from.0) && kept.contains(&e.to.0))
            .map(|e| {
                json!({
                    "from": e.from.0,
                    "to": e.to.0,
                    "rel_type": e.rel_type,
                    "kind": match e.kind {
                        crystalline_index::EdgeKind::Relation => "relation",
                        crystalline_index::EdgeKind::Link => "link",
                    },
                })
            })
            .collect();

        Ok(json!({
            "anchor": url.to_url(),
            "depth": depth,
            "timeframe": p.timeframe,
            "nodes": nodes,
            "edges": edges,
        }))
    }

    // --- recent --------------------------------------------------------------

    /// Recent engrams within a timeframe.
    pub async fn recent_activity(&self, p: &RecentParams) -> Result<Value> {
        let timeframe = p.timeframe.clone().unwrap_or_else(|| "7d".to_string());
        let filter = RecentFilter {
            domains: p.domains.clone().filter(|d| !d.is_empty()),
            after: timeframe_cutoff(&timeframe),
            engram_types: p.types.clone().filter(|t| !t.is_empty()),
            limit: 50,
        };
        let store = self.store.lock().await;
        let items = store.recent(&filter).await?;
        Ok(json!({
            "timeframe": timeframe,
            "count": items.len(),
            "engrams": serde_json::to_value(&items).unwrap_or(Value::Null),
        }))
    }

    // --- list domains --------------------------------------------------------

    /// List registered domains with counts and optional routing bullets. A file
    /// domain reports its path and reads routing bullets from its `MANIFEST.md`
    /// on disk; a virtual domain reports a null path, its kind and reads routing
    /// bullets from its MANIFEST engram in the database.
    pub async fn list_domains(&self, p: &ListDomainsParams) -> Result<Value> {
        let store = self.store.lock().await;
        let stats = store.domain_stats().await.unwrap_or_default();
        drop(store);

        let mut out = Vec::new();
        for (name, entry) in &self.config.domains {
            let source = self.source_of(entry);
            let s = stats.iter().find(|d| &d.name == name);
            let mut obj = json!({
                "name": name,
                "kind": if entry.is_virtual() { "virtual" } else { "file" },
                "path": entry.file_path().map(|r| r.display().to_string()),
                "engrams": s.map(|d| d.engrams),
                "observations": s.map(|d| d.observations),
                "relations": s.map(|d| d.relations),
                "last_sync": s.and_then(|d| d.last_sync.clone()),
            });
            if p.include_routing {
                let bullets = match &source {
                    ContentSource::File { root } => routing_bullets(root),
                    ContentSource::Virtual => self.virtual_routing_bullets_for(name).await,
                };
                obj["when_to_use"] = json!(bullets);
            }
            out.push(obj);
        }
        Ok(json!({ "domains": out }))
    }

    /// Routing bullets for one virtual domain, read from its `MANIFEST.md`
    /// engram in the database. Empty when there is no MANIFEST engram yet.
    async fn virtual_routing_bullets_for(&self, name: &str) -> Vec<String> {
        let content = {
            let store = self.store.lock().await;
            match store.find_engram(name, "manifest").await.ok().flatten() {
                Some(d) => store
                    .engram_content(d.domain_id, &d.path)
                    .await
                    .ok()
                    .flatten(),
                None => None,
            }
        };
        let Some(source) = content else {
            return Vec::new();
        };
        let Ok(engram) = parse_engram(&source) else {
            return Vec::new();
        };
        Manifest::from_engram(&engram, &source)
            .routing_bullets()
            .to_vec()
    }

    /// Routing bullets for every virtual domain, keyed by domain name. Supplied
    /// to `crystalline_core::generate_prompt` (which never touches a database)
    /// and served over the `routing_bullets` ctl request so `prompt system` stays
    /// inside its latency budget for virtual domains too.
    pub async fn virtual_routing_bullets(&self) -> BTreeMap<String, Vec<String>> {
        let mut out = BTreeMap::new();
        for (name, entry) in &self.config.domains {
            if entry.is_virtual() {
                out.insert(name.clone(), self.virtual_routing_bullets_for(name).await);
            }
        }
        out
    }

    // --- browse --------------------------------------------------------------

    /// Browse a domain's engrams under a folder path. Works for any registered
    /// domain, file or virtual, since it lists rows from the store rather than
    /// walking a filesystem.
    pub async fn browse_domain(&self, p: &BrowseParams) -> Result<Value> {
        // A domain-exists check, not a filesystem-root requirement, so a virtual
        // domain browses.
        self.domain_entry(&p.domain)?;
        let raw = p.path.clone().unwrap_or_else(|| "/".to_string());
        let prefix = raw.trim_start_matches("./").trim_matches('/').to_string();
        let depth = p.depth.unwrap_or(1).max(1);
        let matcher = match &p.glob {
            Some(g) => Some(
                globset::Glob::new(g)
                    .map_err(|e| EngineError::Invalid(format!("invalid glob '{g}': {e}")))?
                    .compile_matcher(),
            ),
            None => None,
        };

        let prefix_pat = if prefix.is_empty() {
            None
        } else {
            Some(format!("{prefix}/"))
        };
        let store = self.store.lock().await;
        let all = store
            .list_engrams(&p.domain, prefix_pat.as_deref(), None)
            .await?;
        drop(store);

        let mut entries = Vec::new();
        let mut folders: HashSet<String> = HashSet::new();
        for d in &all {
            let rel: &str = if prefix.is_empty() {
                d.path.as_str()
            } else {
                d.path
                    .strip_prefix(&format!("{prefix}/"))
                    .unwrap_or(&d.path)
            };
            if let Some(m) = &matcher
                && !m.is_match(&d.path)
            {
                continue;
            }
            let segments: Vec<&str> = rel.split('/').collect();
            if segments.len() > 1 {
                folders.insert(segments[0].to_string());
            }
            if segments.len() <= depth {
                entries.push(json!({
                    "permalink": d.permalink,
                    "title": d.title,
                    "type": d.engram_type,
                    "path": d.path,
                }));
            }
        }
        let mut folders: Vec<String> = folders.into_iter().collect();
        folders.sort();

        Ok(json!({
            "domain": p.domain,
            "path": raw,
            "folders": folders,
            "engrams": entries,
        }))
    }

    // --- validate ------------------------------------------------------------

    /// Validate a domain's engrams against its schema engrams. Engram content is
    /// loaded from disk for a file domain and from the database for a virtual
    /// domain, so validation covers both kinds.
    pub async fn validate_engrams(&self, p: &ValidateParams) -> Result<Value> {
        let source = self.content_source(&p.domain)?;
        let store = self.store.lock().await;
        let schema_descs = store.list_engrams(&p.domain, None, Some("schema")).await?;
        let targets = if let Some(id) = &p.identifier {
            match store.find_engram(&p.domain, id).await? {
                Some(d) => vec![d],
                None => {
                    return Err(EngineError::NotFound(format!(
                        "no engram '{id}' in domain '{}'",
                        p.domain
                    )));
                }
            }
        } else {
            store
                .list_engrams(&p.domain, None, p.engram_type.as_deref())
                .await?
        };
        drop(store);

        let mut schemas: Vec<Schema> = Vec::new();
        for d in &schema_descs {
            if let Some(engram) = self.load_engram(&source, d.domain_id, &d.path).await
                && let Some(schema) = Schema::from_engram(&engram)
            {
                schemas.push(schema);
            }
        }

        let mut issues = Vec::new();
        let mut checked = 0usize;
        for d in &targets {
            let Some(engram) = self.load_engram(&source, d.domain_id, &d.path).await else {
                continue;
            };
            checked += 1;
            if let Some(schema) = schema::select_schema(&engram, &schemas) {
                for issue in schema::validate(&engram, &schema) {
                    issues.push(json!({
                        "permalink": d.permalink,
                        "path": d.path,
                        "severity": issue.severity,
                        "kind": issue.kind,
                        "field": issue.field,
                        "message": issue.message,
                        "line": issue.line,
                    }));
                }
            }
        }

        Ok(json!({
            "domain": p.domain,
            "checked": checked,
            "schemas": schemas.len(),
            "issue_count": issues.len(),
            "issues": issues,
        }))
    }

    // --- infer schema --------------------------------------------------------

    /// Infer a Picoschema from a domain's engrams of a type. Engram content is
    /// loaded from disk for a file domain and from the database for a virtual
    /// domain.
    pub async fn infer_schema(&self, p: &InferParams) -> Result<Value> {
        let source = self.content_source(&p.domain)?;
        let store = self.store.lock().await;
        let descs = store
            .list_engrams(&p.domain, None, Some(&p.engram_type))
            .await?;
        drop(store);

        let mut engrams = Vec::new();
        for d in &descs {
            if let Some(engram) = self.load_engram(&source, d.domain_id, &d.path).await {
                engrams.push(engram);
            }
        }
        let threshold = p.threshold.unwrap_or(0.25);
        let schema = schema::infer(&engrams, threshold);
        Ok(json!({
            "domain": p.domain,
            "type": p.engram_type,
            "count": engrams.len(),
            "threshold": threshold,
            "schema": schema,
        }))
    }

    // --- domain import / export / scaffold -----------------------------------

    /// Scaffold a MANIFEST engram into a virtual domain from prebuilt markdown,
    /// unless one already exists. A no-op that reports `created: false` when the
    /// domain already has a `MANIFEST.md`. Refuses on a file domain (its MANIFEST
    /// belongs on disk via `domain init`).
    pub async fn scaffold_virtual_manifest(&self, domain: &str, markdown: &str) -> Result<Value> {
        if let ContentSource::File { .. } = self.content_source(domain)? {
            return Err(EngineError::Invalid(format!(
                "domain '{domain}' is a file domain; scaffold its MANIFEST on disk with `crystalline domain init`"
            )));
        }
        let store = self.store.lock().await;
        let domain_id = store
            .upsert_domain(domain, None, DomainKind::Virtual)
            .await?;
        let existing = store.engram_content(domain_id, "MANIFEST.md").await?;
        drop(store);
        if existing.is_some() {
            return Ok(json!({ "domain": domain, "manifest": "MANIFEST.md", "created": false }));
        }
        let stamp = virtual_stamp(markdown);
        let store = self.store.lock().await;
        self.index_markdown(
            &*store,
            domain_id,
            "MANIFEST.md",
            markdown,
            stamp,
            None,
            true,
        )
        .await?;
        Ok(json!({ "domain": domain, "manifest": "MANIFEST.md", "created": true }))
    }

    /// Import already-well-formed engram `.md` files from `src` into a virtual
    /// domain verbatim. Refuses a file target (that would desync the DB from its
    /// files). Collisions on an existing path or permalink are skipped unless
    /// `overwrite`; `dry_run` reports without writing.
    pub async fn import_domain(
        &self,
        domain: &str,
        src: &Path,
        overwrite: bool,
        dry_run: bool,
    ) -> Result<Value> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        if let ContentSource::File { .. } = self.content_source(domain)? {
            return Err(EngineError::Invalid(format!(
                "domain import loads into a virtual domain; '{domain}' is a file domain. \
                 Use `crystalline import` then `crystalline sync` for a file domain."
            )));
        }
        if !src.is_dir() {
            return Err(EngineError::Invalid(format!(
                "import source '{}' is not a directory",
                src.display()
            )));
        }

        let files = walk_markdown(src);
        let store = self.store.lock().await;
        let domain_id = store
            .upsert_domain(domain, None, DomainKind::Virtual)
            .await?;
        let existing = store.all_engram_contents(domain_id).await?;
        drop(store);
        let existing_paths: HashSet<String> = existing.iter().map(|e| e.path.clone()).collect();
        let existing_perms: HashSet<String> =
            existing.iter().map(|e| e.permalink.clone()).collect();

        let mut written = 0usize;
        let mut skipped = 0usize;
        let mut collisions: Vec<String> = Vec::new();
        let mut warnings: Vec<String> = Vec::new();
        let mut changes: Vec<Value> = Vec::new();

        for (rel, abs) in files {
            let text = match std::fs::read_to_string(&abs) {
                Ok(t) => t,
                Err(e) => {
                    warnings.push(format!("{rel}: could not read: {e}"));
                    continue;
                }
            };
            let engram = match parse_engram(&text) {
                Ok(e) => e,
                Err(e) => {
                    warnings.push(format!("{rel}: could not parse: {e}"));
                    continue;
                }
            };
            let record = EngramRecord::from_engram(&engram, &rel, virtual_stamp(&text));
            let collides = (existing_paths.contains(&rel)
                || existing_perms.contains(&record.permalink))
                && !overwrite;
            if collides {
                collisions.push(rel.clone());
                skipped += 1;
                continue;
            }
            if dry_run {
                changes.push(json!({ "path": rel, "permalink": record.permalink }));
                written += 1;
                continue;
            }
            let stamp = virtual_stamp(&text);
            let store = self.store.lock().await;
            match self
                .index_markdown(&*store, domain_id, &rel, &text, stamp, None, true)
                .await
            {
                Ok(_) => {
                    changes.push(json!({ "path": rel, "permalink": record.permalink }));
                    written += 1;
                }
                Err(e) => {
                    warnings.push(format!("{rel}: {e}"));
                    skipped += 1;
                }
            }
        }

        Ok(json!({
            "domain": domain,
            "dry_run": dry_run,
            "files_written": written,
            "files_skipped": skipped,
            "collisions": collisions,
            "warnings": warnings,
            "files": changes,
        }))
    }

    /// Export every engram of a domain (file or virtual) from the database to
    /// `dest` as a normal filesystem engram folder. Refuses to write into a
    /// non-empty directory unless `force`; `dry_run` reports without writing.
    pub async fn export_domain(
        &self,
        domain: &str,
        dest: &Path,
        force: bool,
        dry_run: bool,
    ) -> Result<Value> {
        let entry = self.domain_entry(domain)?;
        let store = self.store.lock().await;
        let domain_id = match self.source_of(&entry) {
            ContentSource::File { root } => {
                store
                    .upsert_domain(domain, Some(&root.to_string_lossy()), DomainKind::File)
                    .await?
            }
            ContentSource::Virtual => {
                store
                    .upsert_domain(domain, None, DomainKind::Virtual)
                    .await?
            }
        };
        let all = store.all_engram_contents(domain_id).await?;
        drop(store);

        if !dry_run && dir_is_nonempty(dest) && !force {
            return Err(EngineError::Conflict(format!(
                "destination '{}' is not empty; pass force to overwrite",
                dest.display()
            )));
        }

        let mut written = 0usize;
        let mut files: Vec<Value> = Vec::new();
        for e in &all {
            files.push(json!({ "path": e.path, "permalink": e.permalink }));
            if dry_run {
                continue;
            }
            let abs = join_rel(dest, &e.path);
            write_file(&abs, &e.content)?;
            written += 1;
        }

        Ok(json!({
            "domain": domain,
            "dest": dest.display().to_string(),
            "dry_run": dry_run,
            "files_written": if dry_run { all.len() } else { written },
            "files": files,
        }))
    }

    // --- sync / reindex (ctl + CLI) ------------------------------------------

    /// Sync one or all registered domains, returning per-domain reports.
    pub async fn sync(&self, only: Option<&str>) -> Result<Value> {
        let targets = self.sync_targets(only)?;
        let store = self.store.lock().await;
        let mut reports = Vec::new();
        for (name, root) in &targets {
            let report = sync_domain_with(&*store, name, root, &self.chunk_params)
                .await
                .map_err(|e| EngineError::Internal(format!("sync of '{name}' failed: {e}")))?;
            reports.push(report);
        }
        Ok(json!({ "reports": serde_json::to_value(&reports).unwrap_or(Value::Null) }))
    }

    /// Reindex all file domains. `full` clears each file domain's rows first
    /// (per-domain, not a global wipe) and resyncs from disk, so virtual-domain
    /// rows, whose only source of truth is the database, are never destroyed.
    pub async fn reindex(&self, full: bool) -> Result<Value> {
        let targets = self.sync_targets(None)?;
        let store = self.store.lock().await;
        if full {
            for (name, root) in &targets {
                let domain_id = store
                    .upsert_domain(name, Some(&root.to_string_lossy()), DomainKind::File)
                    .await?;
                store.clear_domain(domain_id).await?;
            }
        }
        let mut reports = Vec::new();
        for (name, root) in &targets {
            let report = sync_domain_with(&*store, name, root, &self.chunk_params)
                .await
                .map_err(|e| EngineError::Internal(format!("reindex of '{name}' failed: {e}")))?;
            reports.push(report);
        }
        Ok(json!({
            "full": full,
            "reports": serde_json::to_value(&reports).unwrap_or(Value::Null),
        }))
    }

    /// The file domains to sync, as `(name, root)` pairs. Virtual domains have
    /// no files, so they are skipped everywhere sync and reindex walk domains; a
    /// named sync of a virtual domain is a clean no-op.
    fn sync_targets(&self, only: Option<&str>) -> Result<Vec<(String, PathBuf)>> {
        match only {
            Some(name) => match self.content_source(name)? {
                ContentSource::File { root } => Ok(vec![(name.to_string(), root)]),
                ContentSource::Virtual => Ok(Vec::new()),
            },
            None => {
                let mut targets: Vec<(String, PathBuf)> = Vec::new();
                for (name, entry) in &self.config.domains {
                    if let Some(root) = entry.file_path().filter(|_| !entry.is_virtual()) {
                        targets.push((name.clone(), root));
                    }
                }
                // A domain registered after startup and already resolved once
                // (e.g. by a named `ctl sync`) rides along on a full sync too.
                for (name, entry) in self.discovered_domains.read().unwrap().iter() {
                    if self.config.domains.contains_key(name) {
                        continue;
                    }
                    if let Some(root) = entry.file_path().filter(|_| !entry.is_virtual()) {
                        targets.push((name.clone(), root));
                    }
                }
                Ok(targets)
            }
        }
    }

    /// Diagnostics for ctl `status`: per-domain stats, embedding coverage and the
    /// active full-text mode.
    pub async fn status_report(&self) -> Result<Value> {
        let store = self.store.lock().await;
        let info = store.store_info().await?;
        let stats = store.domain_stats().await?;
        let coverage = store.embedding_coverage().await?;
        drop(store);
        let active_embedded = coverage.embedded_for(&self.model_id);
        Ok(json!({
            "fts_mode": info.fts_mode,
            "schema_version": info.schema_version,
            "db_path": info.db_path,
            "db_size": info.db_size,
            "domains": serde_json::to_value(&stats).unwrap_or(Value::Null),
            "embeddings": {
                "active_model": self.model_id,
                "provider": self.provider().is_some(),
                "embedded_chunks": active_embedded,
                "total_chunks": coverage.total_chunks,
                "hybrid_available": coverage.has_active_embeddings(&self.model_id),
            },
        }))
    }

    /// Embed outstanding chunks for the active model in bounded batches, locking
    /// the store only to pull jobs and to store vectors so long embeds do not
    /// block searches. Returns the number of chunks embedded.
    pub async fn embed_pending(&self) -> Result<usize> {
        let Some(provider) = self.provider() else {
            return Ok(0);
        };
        let model = self.model_id.clone();
        // One snapshot of outstanding chunks; the store lock is held only to pull
        // jobs and to write vectors, never across the embed call.
        let jobs = {
            let store = self.store.lock().await;
            store.chunks_needing_embedding(&model).await?
        };
        if jobs.is_empty() {
            return Ok(0);
        }
        let mut embedded = 0usize;
        for batch in jobs.chunks(EMBED_BATCH) {
            let texts: Vec<String> = batch.iter().map(|j| j.text.clone()).collect();
            let vectors = provider
                .embed(&texts)
                .await
                .map_err(|e| EngineError::Internal(e.to_string()))?;
            if vectors.len() != batch.len() {
                return Err(EngineError::Internal(
                    "embedding provider returned a mismatched vector count".into(),
                ));
            }
            let rows: Vec<crystalline_index::EmbeddingRow> = batch
                .iter()
                .zip(vectors)
                .map(|(job, embedding)| crystalline_index::EmbeddingRow {
                    chunk_id: job.chunk_id,
                    dims: embedding.len(),
                    embedding,
                })
                .collect();
            let store = self.store.lock().await;
            store.store_embeddings(&rows, &model).await?;
            embedded += batch.len();
        }
        Ok(embedded)
    }
}

/// Build an engine that opens the store directly for a one-shot standalone CLI
/// command. Builds the embedding provider only when the command may need it.
pub async fn open_standalone(
    config: GlobalConfig,
    db: &Path,
    want_embeddings: bool,
) -> anyhow::Result<Engine> {
    // The factory resolves the backend from `database`, creates the parent
    // directory for a Turso file and unsizes the concrete store into a
    // `dyn Store`. `db` is the resolved `--db` override for the Turso arm.
    let store = crystalline_index::open_store(&config.database(), Some(db), false).await?;
    // No `config_path`: a standalone command is one-shot, so the config it
    // already loaded is as fresh as any re-read would be. A standalone data
    // command has no `--read-only` flag of its own, so the mode comes purely
    // from `service.read_only`; a read-only config refuses CLI writes here the
    // same way the daemon refuses them over the socket.
    let read_only = config.read_only();
    let engine = Engine::new(store, config, None, None).with_read_only(read_only);
    // Build the provider (which may download the model) only when the index
    // already holds embeddings for the active model, so a text or filter search
    // never triggers a surprise download. With no embeddings, search falls back
    // to text without a provider anyway.
    if want_embeddings {
        let has_embeddings = {
            let store = engine.store.lock().await;
            store
                .embedding_coverage()
                .await
                .map(|c| c.has_active_embeddings(&engine.model_id))
                .unwrap_or(false)
        };
        if has_embeddings && let Some(provider) = build_provider(&engine.config).await {
            engine.set_provider(provider);
        }
    }
    Ok(engine)
}

/// Build the configured embedding provider, tolerating failure (the daemon logs
/// and continues text-only). Returns `None` when no provider could be built.
pub async fn build_provider(config: &GlobalConfig) -> Option<Arc<dyn EmbeddingProvider>> {
    let ecfg =
        config
            .embeddings
            .clone()
            .unwrap_or_else(|| crystalline_core::config::EmbeddingsConfig {
                provider: "local".to_string(),
                model: crystalline_index::embed::DEFAULT_MODEL_ID.to_string(),
                endpoint: None,
                api_key_env: None,
            });
    match provider_from_config(&ecfg).await {
        Ok(p) => Some(Arc::from(p)),
        Err(e) => {
            tracing::warn!("embedding provider unavailable, continuing text-only: {e}");
            None
        }
    }
}

// --- free helpers ------------------------------------------------------------

fn parse_mode(s: Option<&str>) -> Result<SearchMode> {
    Ok(match s.unwrap_or("hybrid") {
        "hybrid" => SearchMode::Hybrid,
        "text" => SearchMode::Text,
        "semantic" => SearchMode::Semantic,
        "title" => SearchMode::Title,
        "permalink" => SearchMode::Permalink,
        other => {
            return Err(EngineError::Invalid(format!(
                "unknown search_type '{other}'; expected hybrid, text, semantic, title or permalink"
            )));
        }
    })
}

fn mode_str(m: SearchMode) -> &'static str {
    match m {
        SearchMode::Hybrid => "hybrid",
        SearchMode::Text => "text",
        SearchMode::Semantic => "semantic",
        SearchMode::Title => "title",
        SearchMode::Permalink => "permalink",
    }
}

fn section_err(e: crystalline_core::emit::EditError) -> EngineError {
    match e {
        crystalline_core::emit::EditError::SectionNotFound { path } => {
            EngineError::NotFound(format!("no section found for heading path: {path}"))
        }
    }
}

fn routing_bullets(root: &Path) -> Vec<String> {
    let manifest = root.join("MANIFEST.md");
    let Ok(source) = std::fs::read_to_string(&manifest) else {
        return Vec::new();
    };
    let Ok(engram) = parse_engram(&source) else {
        return Vec::new();
    };
    Manifest::from_engram(&engram, &source)
        .routing_bullets()
        .to_vec()
}

fn read_engram_file(root: &Path, rel: &str) -> Option<Engram> {
    let abs = join_rel(root, rel);
    let source = std::fs::read_to_string(abs).ok()?;
    parse_engram(&source).ok()
}

/// Every `.md` file under `root` as `(forward-slashed relative path, absolute
/// path)`, skipping dot-directories and dot-files. Mirrors the sync engine's
/// walk so `domain import` sees the same files a file-domain sync would.
fn walk_markdown(root: &Path) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    let walker = walkdir::WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| e.depth() == 0 || !is_hidden(&e.file_name().to_string_lossy()));
    for entry in walker.filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }
        let fname = entry.file_name().to_string_lossy();
        if is_hidden(&fname) || !fname.to_lowercase().ends_with(".md") {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(root)
            .unwrap_or(entry.path())
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        out.push((rel, entry.path().to_path_buf()));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn is_hidden(name: &str) -> bool {
    name.starts_with('.') && name != "." && name != ".."
}

/// Whether a directory exists and contains at least one entry.
fn dir_is_nonempty(dir: &Path) -> bool {
    std::fs::read_dir(dir)
        .map(|mut it| it.next().is_some())
        .unwrap_or(false)
}

/// Join a forward-slashed domain-relative path onto a root, per-segment so it is
/// correct on every platform.
fn join_rel(root: &Path, rel: &str) -> PathBuf {
    let mut p = root.to_path_buf();
    for seg in rel.split('/').filter(|s| !s.is_empty()) {
        p.push(seg);
    }
    p
}

/// Normalize a destination into a forward-slashed `.md` path.
fn normalize_md(dest: &str) -> String {
    let trimmed = dest.trim_start_matches("./").trim_matches('/');
    let joined = trimmed
        .split('/')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("/");
    if joined.is_empty() {
        String::new()
    } else if joined.to_lowercase().ends_with(".md") {
        joined
    } else {
        format!("{joined}.md")
    }
}

fn write_file(abs: &Path, contents: &str) -> Result<()> {
    write_bytes(abs, contents.as_bytes())
}

fn write_bytes(abs: &Path, contents: &[u8]) -> Result<()> {
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent).map_err(|source| EngineError::Io {
            path: parent.display().to_string(),
            source,
        })?;
    }
    // Write to a sibling temp then rename so the watcher never sees a partial file.
    let tmp = abs.with_extension(format!("md.tmp.{}", std::process::id()));
    std::fs::write(&tmp, contents).map_err(|source| EngineError::Io {
        path: tmp.display().to_string(),
        source,
    })?;
    std::fs::rename(&tmp, abs).map_err(|source| EngineError::Io {
        path: abs.display().to_string(),
        source,
    })?;
    Ok(())
}

/// A synthesized file stamp for a virtual write: the current epoch seconds, the
/// content byte length and its SHA-256. The sha doubles as the CAS token, so a
/// virtual engram gets the same `(mtime, size, sha256)` shape a file write would
/// without ever touching a filesystem.
fn virtual_stamp(content: &str) -> FileStamp {
    FileStamp {
        mtime: chrono::Utc::now().timestamp(),
        size: content.len() as u64,
        sha256: sha256_hex(content.as_bytes()),
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut s = String::with_capacity(digest.len() * 2);
    for b in digest {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn mtime_secs(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn now_offset() -> DateTime<FixedOffset> {
    chrono::Utc::now().fixed_offset()
}

/// The ISO date `spec` before today, for `timeframe` windows like `7d`, `24h`,
/// `2w`, `3m`, `1y`. Falls back to seven days on a parse failure.
fn timeframe_cutoff(spec: &str) -> Option<String> {
    let spec = spec.trim();
    let (num, unit) = spec.split_at(spec.find(|c: char| c.is_alphabetic()).unwrap_or(spec.len()));
    let n: i64 = num.trim().parse().unwrap_or(7);
    let days = match unit.trim() {
        "h" => (n + 23) / 24,
        "d" | "" => n,
        "w" => n * 7,
        "m" => n * 30,
        "y" => n * 365,
        _ => 7,
    };
    let cutoff = chrono::Utc::now().date_naive() - Duration::days(days.max(0));
    Some(cutoff.format("%Y-%m-%d").to_string())
}

/// Build engram markdown with auto-filled frontmatter via the core emitter.
/// `valid_from` and `valid_to` are never written.
#[allow(clippy::too_many_arguments)]
fn build_markdown(
    engram_type: &str,
    title: &str,
    permalink: &str,
    tags: &[String],
    status: &str,
    recorded_at: &str,
    timestamp: &str,
    metadata: Option<&Value>,
    body: &str,
) -> Result<String> {
    let mut fm = Frontmatter {
        engram_type: engram_type.to_string(),
        title: title.to_string(),
        permalink: Some(permalink.to_string()),
        tags: tags.to_vec(),
        status: Some(status.to_string()),
        ..Frontmatter::default()
    };
    fm.recorded_at = chrono::NaiveDate::parse_from_str(recorded_at, "%Y-%m-%d").ok();
    fm.timestamp = DateTime::parse_from_rfc3339(timestamp).ok();
    if let Some(Value::Object(map)) = metadata {
        for (k, v) in map {
            if is_reserved_key(k) {
                continue;
            }
            fm.extra.insert(k.clone(), json_to_yaml(v));
        }
    } else if let Some(other) = metadata
        && !other.is_null()
    {
        return Err(EngineError::Invalid("metadata must be an object".into()));
    }

    let engram = Engram {
        frontmatter: fm,
        body: format!("\n{}\n", body.trim_matches('\n')),
        observations: Vec::new(),
        relations: Vec::new(),
        links: Vec::new(),
        headings: Vec::new(),
    };
    Ok(crystalline_core::emit_engram(&engram))
}

/// Frontmatter keys the write tool owns; a caller cannot override them through
/// `metadata`.
fn is_reserved_key(key: &str) -> bool {
    matches!(
        key,
        "type" | "title" | "permalink" | "tags" | "status" | "recorded_at" | "timestamp"
    )
}

fn json_to_yaml(v: &Value) -> YamlValue {
    match v {
        Value::Null => YamlValue::Null,
        Value::Bool(b) => YamlValue::Bool(*b),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                YamlValue::Int(i)
            } else {
                YamlValue::Float(n.as_f64().unwrap_or(0.0))
            }
        }
        Value::String(s) => YamlValue::String(s.clone()),
        Value::Array(a) => YamlValue::Sequence(a.iter().map(json_to_yaml).collect()),
        Value::Object(o) => YamlValue::Mapping(
            o.iter()
                .map(|(k, v)| (k.clone(), json_to_yaml(v)))
                .collect(),
        ),
    }
}
