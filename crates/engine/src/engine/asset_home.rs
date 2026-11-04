//! The asset substrate: the files a kind of document carries beside it, one
//! body of code for every kind. A home names the folder, the index table and
//! what a change owes afterwards; everything else (the path rules, the
//! allowlist, the size ceiling, containment, the per-file lock, the row heal)
//! is the same for every home. The overlay projection of the engram
//! attachments sits above this and is engram only.

use super::fence::{DiskUndo, OnRefusal, RowOp, is_fence_refusal};
use super::*;
use crystalline_core::ContentKind;
use crystalline_index::AssetTable;

/// The end of the sentence a file move onto an occupied path is refused
/// with, after the quoted path. Public so a surface that answers this
/// refusal with its own status matches exactly this one.
pub const MOVE_ONTO_A_FILE: &str = "already holds a file; remove it first or pick another name";

/// The end of the sentence a create-only upload onto an occupied path is
/// refused with, after the quoted path. Public so a surface can answer this
/// refusal with its own status (REST: 412).
pub const ASSET_EXISTS: &str = "already holds a file; this upload only creates";

/// Where a domain keeps the files of one kind of document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AssetHome {
    /// `assets/` at the domain root: the engram attachments.
    Engram,
    /// `reflections/assets/`: a Reflection's files. Rows in
    /// `reflection_asset`, a change marks the backup branch, never the evolve
    /// sweep, and a virtual domain has none (a Reflection needs files).
    Reflection,
}

impl AssetHome {
    /// The index table the home's rows live in.
    pub(crate) fn table(self) -> AssetTable {
        match self {
            AssetHome::Engram => AssetTable::Engram,
            AssetHome::Reflection => AssetTable::Reflection,
        }
    }

    /// The kind of document the home serves.
    pub(crate) fn kind(self) -> ContentKind {
        match self {
            AssetHome::Engram => ContentKind::Engram,
            AssetHome::Reflection => ContentKind::Reflection,
        }
    }

    /// The domain-relative path of the file the home names `path`.
    pub(crate) fn domain_rel(self, path: &str) -> String {
        format!("{}{path}", self.kind().root())
    }

    /// The domain-relative path of the file the home names `path`, as it
    /// stands on disk under `root`: a Reflection's files live in the
    /// Reflection's folder in its spelling on disk, as its pages do, so a
    /// write never puts a second spelling of the folder beside the first.
    pub(crate) fn on_disk_rel(self, root: &Path, path: &str) -> String {
        match self {
            AssetHome::Engram => self.domain_rel(path),
            AssetHome::Reflection => {
                let folder = super::reflection::reflection_dir(root);
                let name = folder
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(crystalline_core::reflection::REFLECTIONS_DIR);
                format!("{name}/{path}")
            }
        }
    }

    /// The miss every verb of this home reports, one spelling per home.
    fn missing(self, domain: &str, path: &str) -> String {
        match self {
            AssetHome::Engram => missing_attachment(domain, path),
            AssetHome::Reflection => {
                format!("no file '{path}' in the Reflection of '{domain}'")
            }
        }
    }

    /// What one file of the home is called in a log line.
    fn noun(self) -> &'static str {
        match self {
            AssetHome::Engram => "attachment",
            AssetHome::Reflection => "Reflection file",
        }
    }

    /// The answer for a virtual domain, which keeps no Reflection.
    fn no_virtual(self) -> EngineError {
        EngineError::Invalid(
            "a Reflection lives in a file domain; a virtual domain has no reflections/".into(),
        )
    }
}

/// The failure a test arms for the next asset move: the rows always fail
/// after the rename; what the undo meets differs.
#[cfg(any(test, feature = "testing"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AssetMoveFault {
    /// The undo runs as it would.
    RowsFail,
    /// The undo fails.
    UndoFails,
    /// A new file appears at the old path before the undo.
    FileAppears,
}

/// Whether a write may replace a file that is already there.
#[derive(Debug)]
pub(crate) enum ReplaceRule {
    /// It may.
    Allow,
    /// It may not, and the caller is told this sentence (`Forbidden`).
    Refuse(String),
    /// The caller asked to only create: a file already there is refused with
    /// [`ASSET_EXISTS`] (`Conflict`), whatever the caller's right.
    CreateOnly,
}

impl ReplaceRule {
    /// The refusal of a write onto `path`, which already holds a file, or
    /// `None` when the write may replace it.
    fn refusal(&self, path: &str) -> Option<EngineError> {
        match self {
            ReplaceRule::Allow => None,
            ReplaceRule::Refuse(sentence) => Some(EngineError::Forbidden(sentence.clone())),
            ReplaceRule::CreateOnly => {
                Some(EngineError::Conflict(format!("'{path}' {ASSET_EXISTS}")))
            }
        }
    }
}

/// What a write landed as.
#[derive(Debug, Clone)]
pub(crate) struct WrittenAsset {
    /// The row that now describes the file.
    pub row: AttachmentRow,
    /// Whether the path held no file before.
    pub created: bool,
    /// Whether the path already held exactly these bytes, so nothing was
    /// written.
    pub unchanged: bool,
}

impl Engine {
    /// Every file of `home` in `domain`, metadata only, ordered by path.
    pub(crate) async fn asset_list_in(
        &self,
        home: AssetHome,
        domain: &str,
    ) -> Result<Vec<AttachmentRow>> {
        let (domain_id, _) = self.domain_source(domain).await?;
        let store = self.store.lock().await;
        Ok(store.list_assets(home.table(), domain_id).await?)
    }

    /// One file of `home`: its bytes and its metadata row.
    ///
    /// A file domain reads the file under its root; a virtual domain reads the
    /// stored blob. Either way an absent file is [`EngineError::NotFound`], the
    /// same miss an absent engram reports.
    ///
    /// The file arm heals the row it serves. A file can arrive behind the index
    /// (a `git pull`, an editor, a domain whose first sync has not run) and can
    /// change behind it the same way, so when the recorded row does not match
    /// the file's own size and modification instant the bytes just read are
    /// hashed and the row is refreshed through the same upsert the walker uses,
    /// in the home's own table. That keeps the sha a caller caches on
    /// describing exactly what it received. The match itself is the walker's
    /// stat prefilter, so the common case costs no hashing at all.
    pub(crate) async fn asset_read_in(
        &self,
        home: AssetHome,
        domain: &str,
        path: &str,
    ) -> Result<(Vec<u8>, AttachmentRow)> {
        validate_attachment_path(path)?;
        let (domain_id, source) = self.domain_source(domain).await?;
        match (home, &source) {
            (AssetHome::Engram | AssetHome::Reflection, ContentSource::File { root }) => {
                let abs = contained_asset_path(root, &home.on_disk_rel(root, path))?;
                // The stat comes first so an over-cap file is refused without
                // ever being read: the ceiling is enforced by the walker (which
                // skips such a file, so it has no row) and by the write, and a
                // read that hashed one anyway would both spend the memory and
                // mint a row the next full scan deletes again.
                let meta = match std::fs::metadata(&abs) {
                    Ok(meta) => meta,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        return Err(EngineError::NotFound(home.missing(domain, path)));
                    }
                    Err(source) => {
                        return Err(EngineError::Io {
                            path: abs.display().to_string(),
                            source,
                        });
                    }
                };
                if meta.len() > crystalline_core::MAX_ATTACHMENT_BYTES {
                    return Err(EngineError::Invalid(over_cap_error(path, meta.len())));
                }
                let bytes = match std::fs::read(&abs) {
                    Ok(bytes) => bytes,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        return Err(EngineError::NotFound(home.missing(domain, path)));
                    }
                    Err(source) => {
                        return Err(EngineError::Io {
                            path: abs.display().to_string(),
                            source,
                        });
                    }
                };
                // The bytes that were actually read decide, like the walker's
                // post-read check: a file that grew past the ceiling between
                // the stat and the read is caught here rather than served.
                if bytes.len() as u64 > crystalline_core::MAX_ATTACHMENT_BYTES {
                    return Err(EngineError::Invalid(over_cap_error(
                        path,
                        bytes.len() as u64,
                    )));
                }
                let modified = asset_modified(&abs);
                let store = self.store.lock().await;
                let seen = store.get_asset(home.table(), domain_id, path).await?;
                if let Some(row) = &seen
                    && row.size == bytes.len() as u64
                    && row.modified == modified
                {
                    return Ok((bytes, row.clone()));
                }
                let row = attachment_row(path, &bytes, modified)?;
                match home {
                    // A Reflection file's row heals behind the host-lock
                    // fence like every other write of one; an instance that
                    // no longer hosts the domain serves the bytes and leaves
                    // the row to the host.
                    AssetHome::Reflection => {
                        drop(store);
                        match self
                            .commit_reflection_rows(
                                domain_id,
                                &[RowOp::HealAsset {
                                    row: &row,
                                    seen: seen.as_ref(),
                                }],
                                OnRefusal::read_heal(domain),
                            )
                            .await
                        {
                            Err(e) if is_fence_refusal(&e) => {}
                            other => other?,
                        }
                    }
                    // Only the domain's host heals an attachment's row; any
                    // other instance serves the bytes and leaves the row.
                    AssetHome::Engram => {
                        if self.instance_id.is_empty()
                            || self.hosted.read().unwrap().contains_key(domain)
                            || !store.shares_database()
                        {
                            store.upsert_asset(home.table(), domain_id, &row).await?;
                        }
                    }
                }
                Ok((bytes, row))
            }
            (AssetHome::Engram, ContentSource::Virtual) => {
                let store = self.store.lock().await;
                let row = store
                    .get_asset(home.table(), domain_id, path)
                    .await?
                    .ok_or_else(|| EngineError::NotFound(home.missing(domain, path)))?;
                let bytes = store
                    .read_attachment_blob(domain_id, path)
                    .await?
                    .ok_or_else(|| EngineError::NotFound(home.missing(domain, path)))?;
                Ok((bytes, row))
            }
            (AssetHome::Reflection, ContentSource::Virtual) => Err(home.no_virtual()),
        }
    }

    /// Create or replace one file of `home`, returning the row that now
    /// describes it and whether the path held no file before.
    ///
    /// Every gate runs before a byte is stored: the path rules and the
    /// extension allowlist ([`crystalline_core::validate_asset_path`]) and the
    /// size ceiling. A file domain then writes the bytes atomically under its
    /// root, with the joined path proven to stay inside it, and takes the same
    /// per-file lock every other file write takes so a concurrent replace
    /// cannot leave the row describing the loser's bytes. A virtual domain
    /// writes the row and then the blob, in that order, since the store keeps a
    /// blob without a row an error rather than an orphan; a blob write that
    /// fails takes the row back out with it, so a failure leaves the domain
    /// exactly as it found it rather than listing a path with no bytes.
    ///
    /// `replace` decides whether a file already at the path may be replaced;
    /// [`ReplaceRule::Refuse`] answers [`EngineError::Forbidden`] with its
    /// sentence and [`ReplaceRule::CreateOnly`] answers
    /// [`EngineError::Conflict`] ending in [`ASSET_EXISTS`]. The check runs
    /// under the lock that serialises the write, so two racing writes cannot
    /// both find the path empty.
    ///
    /// A path that already holds exactly these bytes (same sha256) is
    /// nothing to write, under any `replace`: the answer is the row with
    /// `unchanged` set, and no file, row, change or after-change step
    /// follows. That is what makes a retry after a timeout safe.
    ///
    /// Both kinds owe the home's after-change step afterwards
    /// (`Engine::after_asset_change`).
    pub(crate) async fn asset_write_in(
        &self,
        home: AssetHome,
        domain: &str,
        path: &str,
        bytes: Vec<u8>,
        replace: ReplaceRule,
    ) -> Result<WrittenAsset> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        self.refuse_read_only_domain(domain, &crate::scope::Scope::Unrestricted)
            .await?;
        validate_attachment_path(path)?;
        if bytes.len() as u64 > crystalline_core::MAX_ATTACHMENT_BYTES {
            return Err(EngineError::Invalid(over_cap_error(
                path,
                bytes.len() as u64,
            )));
        }
        let (domain_id, source) = self.domain_source(domain).await?;
        let written = match (home, &source) {
            (AssetHome::Engram | AssetHome::Reflection, ContentSource::File { root }) => {
                let abs = contained_asset_path(root, &home.on_disk_rel(root, path))?;
                // Held across the write and the row upsert, the file lock
                // before the store lock like every other writer here. See
                // `Engine::write_lock`.
                let lock = self.write_lock(&abs);
                let _guard = lock.lock().await;
                // The disk decides whether the path holds a file, not the
                // row: a file that arrived with no sync yet has none, and a
                // refused replace must still see it. The atomic write's rename
                // cannot report what it replaced, so this one stat stays, under
                // the lock so no racing write can slip in after it.
                let existed = abs.is_file();
                // **The same bytes again are nothing to write**, whatever
                // `replace` says: nothing would be replaced. This is what
                // makes a retry after a timeout safe. The disk decides here
                // too, and the comparison is the one a move makes
                // (`Engine::free_asset_destination`): same sha256, same file.
                if existed && holds_these_bytes(&abs, &bytes)? {
                    return Ok(WrittenAsset {
                        row: attachment_row(path, &bytes, asset_modified(&abs))?,
                        created: false,
                        unchanged: true,
                    });
                }
                if existed && let Some(refusal) = replace.refusal(path) {
                    return Err(refusal);
                }
                if !existed {
                    self.refuse_only_elsewhere(
                        domain,
                        domain_id,
                        ElsewhereRow::Asset(home.table(), path),
                    )
                    .await?;
                }
                let mut undo = DiskUndo::default();
                undo.record(&abs)?;
                write_bytes(&abs, &bytes)?;
                // The modification instant is read back off the file rather
                // than taken from the clock, so it is the same value the sync
                // walker's stat prefilter compares against and an upload costs
                // no re-hash on the next scan.
                let row = attachment_row(path, &bytes, asset_modified(&abs))?;
                match home {
                    // A Reflection file's row commits behind the host-lock
                    // fence; a refusal puts the file back. The bytes are not
                    // kept: the uploader still has them.
                    AssetHome::Reflection => {
                        self.commit_reflection_rows(
                            domain_id,
                            &[RowOp::PutAsset(&row)],
                            OnRefusal::put_back(domain, undo),
                        )
                        .await?;
                    }
                    // An attachment's row commits behind the file domain's
                    // host-lock fence; a refusal puts the file back.
                    AssetHome::Engram => {
                        let store = self.store.lock().await;
                        store.begin().await?;
                        let upserted = async {
                            self.file_row_fence(&*store, domain_id, root).await?;
                            store.upsert_asset(home.table(), domain_id, &row).await?;
                            Ok::<(), EngineError>(())
                        }
                        .await;
                        match upserted {
                            Ok(()) => store.commit().await?,
                            Err(e) => {
                                let _ = store.rollback().await;
                                drop(store);
                                if super::fence::is_file_host_refusal(&e) {
                                    undo.seal();
                                    undo.put_back(domain, None);
                                }
                                return Err(e);
                            }
                        }
                    }
                }
                WrittenAsset {
                    row,
                    created: !existed,
                    unchanged: false,
                }
            }
            (AssetHome::Engram, ContentSource::Virtual) => {
                let row = attachment_row(path, &bytes, Utc::now().to_rfc3339())?;
                let store = self.store.lock().await;
                // What was there before this write, so a failure can put it
                // back rather than approximate it.
                let previous = store.get_asset(home.table(), domain_id, path).await?;
                // The same bytes again are nothing to write, as in a folder.
                if let Some(prior) = &previous
                    && prior.sha256 == row.sha256
                {
                    return Ok(WrittenAsset {
                        row: prior.clone(),
                        created: false,
                        unchanged: true,
                    });
                }
                if previous.is_some()
                    && let Some(refusal) = replace.refusal(path)
                {
                    return Err(refusal);
                }
                store.upsert_asset(home.table(), domain_id, &row).await?;
                // A failed blob write must leave the domain exactly as it found
                // it, which is two different things depending on what was
                // there. A replace: the upsert above moved the row's metadata
                // only - the blob belongs to the row and still holds the older
                // bytes - so restoring the recorded row restores the whole
                // attachment, and deleting instead would destroy an attachment
                // this write never got to replace. A create: nothing was there,
                // so the row this write inserted goes with it, or a listing
                // would advertise bytes that were never stored. Best effort
                // either way (the next write, or a scan of the file domain's
                // twin, reconciles), and the original error is what the caller
                // sees. A file domain needs none of this: the temp file is
                // renamed into place only on success, so a failed write leaves
                // the old file untouched.
                if let Err(e) = store.write_attachment_blob(domain_id, path, &bytes).await {
                    let undone = match &previous {
                        Some(prior) => store
                            .upsert_asset(home.table(), domain_id, prior)
                            .await
                            .err(),
                        None => store
                            .delete_asset(home.table(), domain_id, path)
                            .await
                            .err(),
                    };
                    if let Some(failed) = undone {
                        tracing::warn!(
                            "{} '{path}' in '{domain}' could not be rolled back after a failed blob write: {failed}",
                            home.noun()
                        );
                    }
                    return Err(e.into());
                }
                WrittenAsset {
                    row,
                    created: previous.is_none(),
                    unchanged: false,
                }
            }
            (AssetHome::Reflection, ContentSource::Virtual) => return Err(home.no_virtual()),
        };
        self.after_asset_change(home, domain);
        Ok(written)
    }

    /// Remove one file of `home`: the file or the blob, and the row.
    ///
    /// [`EngineError::NotFound`] when neither was there, so a caller can answer
    /// a miss. A file that is gone while its row stands (or the reverse, after
    /// a hand-edited domain) still counts as a delete: whichever half existed
    /// is removed and the pair ends up consistent.
    pub(crate) async fn asset_delete_in(
        &self,
        home: AssetHome,
        domain: &str,
        path: &str,
    ) -> Result<()> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        self.refuse_read_only_domain(domain, &crate::scope::Scope::Unrestricted)
            .await?;
        validate_attachment_path(path)?;
        let (domain_id, source) = self.domain_source(domain).await?;
        let mut file_removed = false;
        let mut undo = DiskUndo::default();
        // Held until the row follows, so the fence's refusal puts the file
        // back under the lock.
        let mut _held = None;
        match (home, &source) {
            (AssetHome::Engram | AssetHome::Reflection, ContentSource::File { root }) => {
                let abs = contained_asset_path(root, &home.on_disk_rel(root, path))?;
                _held = Some(self.write_lock(&abs).lock_owned().await);
                undo.record(&abs)?;
                match std::fs::remove_file(&abs) {
                    Ok(()) => file_removed = true,
                    // The row of a file only the previous host of a host
                    // move has is the only shared copy of it: a delete here
                    // must not drop it unseen. `--accept-disk` is the way.
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        self.refuse_only_elsewhere(
                            domain,
                            domain_id,
                            ElsewhereRow::Asset(home.table(), path),
                        )
                        .await?;
                    }
                    Err(source) => {
                        return Err(EngineError::Io {
                            path: abs.display().to_string(),
                            source,
                        });
                    }
                }
            }
            (AssetHome::Engram, ContentSource::Virtual) => {}
            (AssetHome::Reflection, ContentSource::Virtual) => return Err(home.no_virtual()),
        }
        let row_removed = match home {
            // Behind the host-lock fence; a refusal puts the file back.
            AssetHome::Reflection => {
                let had_row = self
                    .store
                    .lock()
                    .await
                    .get_asset(home.table(), domain_id, path)
                    .await?
                    .is_some();
                self.commit_reflection_rows(
                    domain_id,
                    &[RowOp::DeleteAsset(path)],
                    OnRefusal::put_back(domain, undo),
                )
                .await?;
                had_row
            }
            // Behind the file domain's host-lock fence; a refusal puts the
            // file back. A virtual domain's blob has no lock.
            AssetHome::Engram => match &source {
                ContentSource::File { root } => {
                    let store = self.store.lock().await;
                    store.begin().await?;
                    let removed = async {
                        self.file_row_fence(&*store, domain_id, root).await?;
                        Ok::<bool, EngineError>(
                            store.delete_asset(home.table(), domain_id, path).await?,
                        )
                    }
                    .await;
                    match removed {
                        Ok(removed) => {
                            store.commit().await?;
                            removed
                        }
                        Err(e) => {
                            let _ = store.rollback().await;
                            drop(store);
                            if super::fence::is_file_host_refusal(&e) {
                                undo.seal();
                                undo.put_back(domain, None);
                            }
                            return Err(e);
                        }
                    }
                }
                ContentSource::Virtual => {
                    let store = self.store.lock().await;
                    store.delete_asset(home.table(), domain_id, path).await?
                }
            },
        };
        drop(_held);
        if !file_removed && !row_removed {
            return Err(EngineError::NotFound(home.missing(domain, path)));
        }
        self.after_asset_change(home, domain);
        Ok(())
    }

    /// Move a file of `home` from `from` to `to` (paths inside the home's
    /// root), and its row with it. Refuses a destination that holds a file,
    /// so a move never overwrites. Both file locks are taken in byte order,
    /// so two crossing moves cannot wait on each other, and held until the
    /// rows follow, so no write to either path slips in between. Links to
    /// the file are the caller's: the substrate moves bytes and rows only.
    ///
    /// All or nothing where it can be: the row is built before the rename
    /// (taken over from the stored row when it still describes the file, so
    /// unchanged bytes are not hashed again), and when the rows cannot follow
    /// the rename it is undone and the error answered. Only when the undo
    /// fails too does the file stay at `to` with its row behind: the answer
    /// is then `Some` sentence saying so (the next sync brings the row up to
    /// date), and the move still counts as a change of the home.
    pub(crate) async fn asset_move_in(
        &self,
        home: AssetHome,
        domain: &str,
        from: &str,
        to: &str,
    ) -> Result<Option<String>> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        self.refuse_read_only_domain(domain, &crate::scope::Scope::Unrestricted)
            .await?;
        validate_attachment_path(from)?;
        validate_attachment_path(to)?;
        refuse_extension_change(from, to)?;
        let (domain_id, source) = self.domain_source(domain).await?;
        let ContentSource::File { root } = &source else {
            return Err(match home {
                AssetHome::Reflection => home.no_virtual(),
                AssetHome::Engram => {
                    EngineError::Invalid(format!("a move of '{from}' needs a file domain"))
                }
            });
        };
        let src = contained_asset_path(root, &home.on_disk_rel(root, from))?;
        let dst = contained_asset_path(root, &home.on_disk_rel(root, to))?;
        if src == dst {
            return Err(EngineError::Invalid(format!(
                "'{from}' already is at '{to}'"
            )));
        }
        // Both locks in lock-key order; two spellings of one file (a
        // letter-case rename on a case-insensitive disk) take its lock once.
        let _locks = self.lock_files(&[&src, &dst]).await;
        let meta = match std::fs::metadata(&src) {
            Ok(meta) if meta.is_file() => meta,
            Ok(_) => return Err(EngineError::NotFound(home.missing(domain, from))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(EngineError::NotFound(home.missing(domain, from)));
            }
            Err(source) => {
                return Err(EngineError::Io {
                    path: src.display().to_string(),
                    source,
                });
            }
        };
        // A letter-case rename on a case-insensitive disk finds the file
        // itself at `to`: the same file, not another one to move onto.
        let case_only = is_case_only_rename(&src, &dst, from, to);
        if dst.exists() && !case_only {
            return Err(EngineError::Conflict(format!("'{to}' {MOVE_ONTO_A_FILE}")));
        }
        if !case_only {
            self.refuse_only_elsewhere(domain, domain_id, ElsewhereRow::Asset(home.table(), to))
                .await?;
        }
        // A rename keeps the modification instant, so a stored row that
        // matches the file's stat still describes the bytes at `to`.
        let modified = asset_modified(&src);
        let stored = self
            .store
            .lock()
            .await
            .get_asset(home.table(), domain_id, from)
            .await?;
        let row = match stored {
            Some(prior) if prior.size == meta.len() && prior.modified == modified => {
                moved_row(prior, to)?
            }
            _ => {
                if meta.len() > crystalline_core::MAX_ATTACHMENT_BYTES {
                    return Err(EngineError::Invalid(over_cap_error(from, meta.len())));
                }
                let bytes = std::fs::read(&src).map_err(|source| EngineError::Io {
                    path: src.display().to_string(),
                    source,
                })?;
                attachment_row(to, &bytes, modified)?
            }
        };
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent).map_err(|source| EngineError::Io {
                path: parent.display().to_string(),
                source,
            })?;
        }
        // On a case-insensitive disk this one rename also changes the spelling
        // of a case-only move, atomically.
        std::fs::rename(&src, &dst).map_err(|source| EngineError::Io {
            path: dst.display().to_string(),
            source,
        })?;
        let rows_followed = self
            .move_asset_rows(home, domain, domain_id, from, &row)
            .await;
        let Err(e) = rows_followed else {
            self.after_asset_change(home, domain);
            return Ok(None);
        };
        // The rows of a move commit in one transaction or not at all, so a
        // failed one wrote no row at `to`, and a row there (the new host's,
        // after a fence refusal) stays.
        let fenced = (home == AssetHome::Reflection && is_fence_refusal(&e))
            || super::fence::is_file_host_refusal(&e);
        match self.undo_asset_rename(&dst, &src, case_only) {
            Ok(()) => Err(e),
            Err(undo) if fenced => {
                tracing::error!(
                    "{} '{from}' in '{domain}' moved to '{to}' on this instance's disk, the move was refused because this instance no longer holds the domain's host lock ({e}), and the file could not be moved back ({undo})",
                    home.noun()
                );
                Ok(Some(format!(
                    "the file moved to '{to}' on this instance's disk and could not be moved back ({undo}); no row changed, since {e}"
                )))
            }
            Err(undo) => {
                tracing::warn!(
                    "{} '{from}' in '{domain}' moved to '{to}', its row could not follow ({e}) and the move could not be undone ({undo})",
                    home.noun()
                );
                self.after_asset_change(home, domain);
                let why = if undo.kind() == std::io::ErrorKind::AlreadyExists {
                    format!("a new file appeared at '{from}' meanwhile, so the move was not undone")
                } else {
                    "the move could not be undone".to_string()
                };
                Ok(Some(format!(
                    "the file moved to '{to}', and its index row could not follow ({e}); {why}; the next sync brings the rows up to date"
                )))
            }
        }
    }

    /// The rows of a move: the new one in, the old one out.
    async fn move_asset_rows(
        &self,
        home: AssetHome,
        domain: &str,
        domain_id: DomainId,
        from: &str,
        row: &AttachmentRow,
    ) -> Result<()> {
        #[cfg(any(test, feature = "testing"))]
        if self.asset_move_fault.lock().unwrap().is_some() {
            return Err(EngineError::Internal(
                "the rows of the move were not written (test failpoint)".into(),
            ));
        }
        // A Reflection file's rows commit behind the host-lock fence. The
        // caller undoes the rename on a refusal, so nothing is put back
        // here.
        if home == AssetHome::Reflection {
            return self
                .commit_reflection_rows(
                    domain_id,
                    &[RowOp::PutAsset(row), RowOp::DeleteAsset(from)],
                    OnRefusal::put_back(domain, DiskUndo::default()),
                )
                .await;
        }
        let root = match self.content_source(domain)? {
            ContentSource::File { root } => Some(root),
            ContentSource::Virtual => None,
        };
        let store = self.store.lock().await;
        store.begin().await?;
        let moved = async {
            // Behind the file domain's host-lock fence.
            if let Some(root) = &root {
                self.file_row_fence(&*store, domain_id, root).await?;
            }
            store.upsert_asset(home.table(), domain_id, row).await?;
            store.delete_asset(home.table(), domain_id, from).await?;
            Ok::<(), EngineError>(())
        }
        .await;
        match moved {
            Ok(()) => Ok(store.commit().await?),
            Err(e) => {
                let _ = store.rollback().await;
                Err(e)
            }
        }
    }

    /// Put a moved file back where it was, never over a file that appeared
    /// at the old path meanwhile (an editor or a pull: this process holds
    /// the path's lock, others do not): that answers `AlreadyExists` and
    /// leaves the moved file where it is. A narrow window between the check
    /// and the rename stays, as no portable rename refuses to replace.
    fn undo_asset_rename(&self, dst: &Path, src: &Path, case_only: bool) -> std::io::Result<()> {
        #[cfg(any(test, feature = "testing"))]
        match self.asset_move_fault.lock().unwrap().take() {
            Some(AssetMoveFault::UndoFails) => {
                return Err(std::io::Error::other(
                    "the undo was refused (test failpoint)",
                ));
            }
            // Somebody else writes the old path before the undo runs.
            Some(AssetMoveFault::FileAppears) => std::fs::write(src, b"new")?,
            _ => {}
        }
        // After a case-only move the old spelling names the moved file itself.
        if !case_only && src.symlink_metadata().is_ok() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("'{}' holds a new file", src.display()),
            ));
        }
        std::fs::rename(dst, src)
    }

    /// Arm a failure of the rows of the next asset move, after its rename;
    /// with `undo_fails` the rename back fails too. Disarmed by the move it
    /// hits.
    #[cfg(any(test, feature = "testing"))]
    pub fn fail_next_asset_move(&self, undo_fails: bool) {
        *self.asset_move_fault.lock().unwrap() = Some(if undo_fails {
            AssetMoveFault::UndoFails
        } else {
            AssetMoveFault::RowsFail
        });
    }

    /// Arm a failure of the rows of the next asset move, after its rename,
    /// with a new file written at the old path before the undo runs.
    #[cfg(any(test, feature = "testing"))]
    pub fn fail_next_asset_move_with_a_file_at_the_old_path(&self) {
        *self.asset_move_fault.lock().unwrap() = Some(AssetMoveFault::FileAppears);
    }

    /// What a change to a file of `home` owes afterwards.
    fn after_asset_change(&self, home: AssetHome, domain: &str) {
        match home {
            // A human just added something the agent has not read yet, which
            // is exactly what a consolidation sweep is for.
            AssetHome::Engram => crate::maintenance::record_pending(domain),
            // A Reflection's file is the backup branch's business, never the
            // evolve sweep's (D4).
            AssetHome::Reflection => {
                self.mark_reflection_dirty(domain, crate::reflection_backup::BackupCause::Settled)
            }
        }
    }
}

/// A move keeps the file's extension, in any letter case: the bytes of a PNG
/// renamed to `.jpg` would be served as `image/jpeg`. `flow.png` to
/// `flow.PNG` is fine.
pub(crate) fn refuse_extension_change(from: &str, to: &str) -> Result<()> {
    let ext = |path: &str| {
        let name = path.rsplit('/').next().unwrap_or(path);
        name.rsplit_once('.')
            .map(|(_, ext)| ext.to_ascii_lowercase())
    };
    match ext(from) {
        Some(kept) if ext(to).as_deref() != Some(kept.as_str()) => Err(EngineError::Invalid(
            format!("'{from}' cannot move to '{to}': a move keeps the .{kept} extension"),
        )),
        _ => Ok(()),
    }
}

/// The row a moved file gets when its stored row still describes it: the
/// same bytes at the new path, the mime read off the new name.
/// Whether the file at `abs` holds exactly `bytes`: the same size, then the
/// same sha256. The size is asked first so a different file of another size
/// costs no read.
fn holds_these_bytes(abs: &Path, bytes: &[u8]) -> Result<bool> {
    let io = |source| EngineError::Io {
        path: abs.display().to_string(),
        source,
    };
    if std::fs::metadata(abs).map_err(io)?.len() != bytes.len() as u64 {
        return Ok(false);
    }
    let held = std::fs::read(abs).map_err(io)?;
    Ok(sha256_hex(&held) == sha256_hex(bytes))
}

fn moved_row(prior: AttachmentRow, to: &str) -> Result<AttachmentRow> {
    let name = to.rsplit('/').next().unwrap_or(to);
    let mime = crystalline_core::attachment_mime(name).ok_or_else(|| {
        EngineError::Invalid(format!(
            "attachment path '{to}': an attachment must carry an allowlisted file extension"
        ))
    })?;
    Ok(AttachmentRow {
        path: to.to_string(),
        mime: mime.to_string(),
        ..prior
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\x00flow";

    async fn engine_over(root: &Path) -> Engine {
        let store = crystalline_index::TursoStore::open_in_memory()
            .await
            .unwrap();
        let mut config = GlobalConfig::default();
        config
            .domains
            .insert("eng".to_string(), DomainEntry::file(root));
        let engine = Engine::new(Arc::new(Mutex::new(store)), config, None, None);
        engine.sync(None).await.unwrap();
        engine
    }

    fn folds_case(dir: &Path) -> bool {
        let probe = dir.join("Probe");
        std::fs::create_dir(&probe).unwrap();
        let folds = dir.join("probe").exists();
        std::fs::remove_dir(&probe).unwrap();
        folds
    }

    /// A symlink is another entry, never the same file: moving it onto its
    /// target, or the target onto a symlink spelled in another case, is a
    /// conflict that leaves every byte where it was.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_symlink_is_never_the_same_file_as_its_target() {
        let tmp = tempfile::tempdir().unwrap();
        let engine = engine_over(tmp.path()).await;
        let dir = tmp.path().join("assets");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("real.png"), PNG).unwrap();
        std::os::unix::fs::symlink(dir.join("real.png"), dir.join("link.png")).unwrap();
        std::fs::write(dir.join("b.png"), PNG).unwrap();
        std::os::unix::fs::symlink(dir.join("b.png"), dir.join("B.png")).unwrap_or(());
        let err = engine
            .asset_move_in(
                AssetHome::Engram,
                "eng",
                "assets/link.png",
                "assets/real.png",
            )
            .await
            .unwrap_err();
        assert!(matches!(err, EngineError::Conflict(_)), "{err}");
        assert_eq!(std::fs::read(dir.join("real.png")).unwrap(), PNG);
        assert!(
            dir.join("link.png")
                .symlink_metadata()
                .unwrap()
                .file_type()
                .is_symlink()
        );
        if dir
            .join("B.png")
            .symlink_metadata()
            .is_ok_and(|m| m.file_type().is_symlink())
        {
            let err = engine
                .asset_move_in(AssetHome::Engram, "eng", "assets/b.png", "assets/B.png")
                .await
                .unwrap_err();
            assert!(matches!(err, EngineError::Conflict(_)), "{err}");
            assert_eq!(std::fs::read(dir.join("b.png")).unwrap(), PNG);
        }
    }

    /// Two files that differ in case on a case-sensitive disk are two files.
    #[tokio::test]
    async fn a_move_onto_a_file_of_another_case_is_a_conflict_where_the_disk_tells_them_apart() {
        let tmp = tempfile::tempdir().unwrap();
        if folds_case(tmp.path()) {
            return;
        }
        let engine = engine_over(tmp.path()).await;
        let dir = tmp.path().join("assets");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.png"), PNG).unwrap();
        std::fs::write(dir.join("a.PNG"), b"other").unwrap();
        let err = engine
            .asset_move_in(AssetHome::Engram, "eng", "assets/a.png", "assets/a.PNG")
            .await
            .unwrap_err();
        assert!(matches!(err, EngineError::Conflict(_)), "{err}");
        assert_eq!(std::fs::read(dir.join("a.PNG")).unwrap(), b"other");
    }

    /// Rows that cannot follow a case-only move undo it, whatever the disk.
    #[tokio::test]
    async fn a_case_only_move_whose_rows_fail_is_undone() {
        let tmp = tempfile::tempdir().unwrap();
        let engine = engine_over(tmp.path()).await;
        let dir = tmp.path().join("assets");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("flow.png"), PNG).unwrap();
        engine.sync(None).await.unwrap();
        engine.fail_next_asset_move(false);
        let err = engine
            .asset_move_in(
                AssetHome::Engram,
                "eng",
                "assets/flow.png",
                "assets/flow.PNG",
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("test failpoint"), "{err}");
        let names: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["flow.png"]);
    }

    #[tokio::test]
    async fn a_move_keeps_the_extension_in_both_homes_and_changes_nothing_when_refused() {
        for home in [AssetHome::Engram, AssetHome::Reflection] {
            let tmp = tempfile::tempdir().unwrap();
            let engine = engine_over(tmp.path()).await;
            let rel = |name: &str| home.on_disk_rel(tmp.path(), &format!("assets/{name}"));
            let src = tmp.path().join(rel("flow.png"));
            std::fs::create_dir_all(src.parent().unwrap()).unwrap();
            std::fs::write(&src, PNG).unwrap();
            engine.sync(None).await.unwrap();

            for to in ["assets/flow.jpg", "assets/flow.png.jpg"] {
                let err = engine
                    .asset_move_in(home, "eng", "assets/flow.png", to)
                    .await
                    .unwrap_err();
                assert!(
                    matches!(&err, EngineError::Invalid(m) if m.contains("keeps the .png extension")),
                    "{home:?} {to}: {err}"
                );
                assert_eq!(std::fs::read(&src).unwrap(), PNG, "{home:?} {to}");
                assert!(!tmp.path().join(rel("flow.jpg")).exists());
            }

            // A letter-case rename, in place: works on either kind of disk.
            engine
                .asset_move_in(home, "eng", "assets/flow.png", "assets/flow.PNG")
                .await
                .unwrap();
            let names: Vec<String> = std::fs::read_dir(src.parent().unwrap())
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            assert_eq!(
                names,
                ["flow.PNG"],
                "{home:?}: the file carries the new case"
            );
            engine
                .asset_move_in(home, "eng", "assets/flow.PNG", "assets/sub/flow.PNG")
                .await
                .unwrap();
            assert!(
                !std::fs::read_dir(src.parent().unwrap()).unwrap().any(|e| e
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with("flow")),
                "{home:?}: nothing is left behind"
            );
            assert_eq!(
                std::fs::read(tmp.path().join(rel("sub/flow.PNG"))).unwrap(),
                PNG
            );
        }
    }

    #[test]
    fn the_engram_home_is_assets_at_the_domain_root() {
        assert_eq!(AssetHome::Engram.table(), AssetTable::Engram);
        assert_eq!(
            AssetHome::Engram.domain_rel("assets/flow.png"),
            "assets/flow.png"
        );
        assert_eq!(AssetHome::Engram.kind(), ContentKind::Engram);
    }

    #[test]
    fn the_reflection_home_is_assets_under_reflections() {
        assert_eq!(AssetHome::Reflection.table(), AssetTable::Reflection);
        assert_eq!(
            AssetHome::Reflection.domain_rel("assets/flow.png"),
            "reflections/assets/flow.png"
        );
        assert_eq!(AssetHome::Reflection.kind(), ContentKind::Reflection);
    }

    #[test]
    fn a_reflection_file_lives_in_the_folder_spelling_on_disk() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(
            AssetHome::Reflection.on_disk_rel(tmp.path(), "assets/flow.png"),
            "reflections/assets/flow.png",
            "no folder yet: the canonical spelling"
        );
        std::fs::create_dir_all(tmp.path().join("Reflections")).unwrap();
        assert_eq!(
            AssetHome::Reflection.on_disk_rel(tmp.path(), "assets/flow.png"),
            "Reflections/assets/flow.png"
        );
        assert_eq!(
            AssetHome::Engram.on_disk_rel(tmp.path(), "assets/flow.png"),
            "assets/flow.png"
        );
    }
}
