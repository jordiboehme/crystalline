//! The host-lock fence of a Reflection write that is not a walk.
//!
//! On a shared database only the instance that holds a domain's host lock
//! writes its Reflection. Every such write passes the gate first
//! ([`Engine::confirm_reflection_host`]), then writes its files on this
//! disk, then its rows. A host that pauses (a sleeping laptop, a stopped
//! process) between the gate and the rows can lose the lock in between, and
//! its rows would then land in the shared database behind the new host's
//! back. So the rows commit in one transaction that first reads the lock
//! row the way a walk's apply does (`FOR SHARE` on Postgres) and goes on
//! only while this instance still holds it.
//!
//! A refused write keeps what the person or agent sent in the kept folder
//! (as a room keeps a text it could not save), puts every file it already
//! changed on this disk back to what it was, and answers the
//! `hosted_elsewhere` refusal naming the new host. A file is put back only
//! while it still holds exactly what this write left there: on a volume
//! several hosts share, another host may have written it since, and that
//! newer file stays. A file that cannot be put back stays as the write left
//! it and is logged. The file that holds the sent text is put back only
//! once that text is kept, so nothing is ever deleted that is the only copy
//! of anything.

use std::path::{Path, PathBuf};

use crystalline_core::reflection as r;
use crystalline_index::AssetTable;

use super::reflection::{REFUSED_HOSTED_START, hosted_refusal};
use super::*;

/// One file a Reflection write changed: what it held before the write and
/// what the write left there (`None`: no file).
struct Undone {
    abs: PathBuf,
    before: Option<Vec<u8>>,
    /// Read when the write is done ([`DiskUndo::seal`]); `None` until then.
    after: Option<Option<Vec<u8>>>,
}

/// The files a Reflection write changed on this disk, each with what it
/// held before, recorded before the write, and what the write left there,
/// recorded once it is done, so a refused write can put them back without
/// undoing somebody else's newer file.
#[derive(Default)]
pub(crate) struct DiskUndo {
    files: Vec<Undone>,
    /// Letter-case renames to turn back, `(new spelling, old spelling)`,
    /// after the file's content is back.
    renamed: Vec<(PathBuf, PathBuf)>,
}

/// What `abs` holds now, `None` for no file.
fn bytes_at(abs: &Path) -> Result<Option<Vec<u8>>> {
    match std::fs::read(abs) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(EngineError::Io {
            path: abs.display().to_string(),
            source,
        }),
    }
}

impl DiskUndo {
    /// Record what `abs` holds now, once per path: the first record is the
    /// state before the write.
    pub(crate) fn record(&mut self, abs: &Path) -> Result<()> {
        if self.files.iter().any(|f| f.abs == abs) {
            return Ok(());
        }
        let before = bytes_at(abs)?;
        self.files.push(Undone {
            abs: abs.to_path_buf(),
            before,
            after: None,
        });
        Ok(())
    }

    /// Record a letter-case rename of one file (`from` and `to` name the
    /// same file on a case-insensitive disk): its content is recorded under
    /// `to`, and a put-back gives it its old spelling again.
    pub(crate) fn record_case_rename(&mut self, from: &Path, to: &Path) -> Result<()> {
        self.record(to)?;
        self.renamed.push((to.to_path_buf(), from.to_path_buf()));
        Ok(())
    }

    /// Record what the write left in every recorded file. Called once the
    /// write is done, still under its file lock, before the rows commit.
    pub(crate) fn seal(&mut self) {
        for file in &mut self.files {
            file.after = Some(bytes_at(&file.abs).unwrap_or(None));
        }
    }

    /// Put every recorded file back, the last change first, except `keep`
    /// (the file holding a text that could not be kept). A file that no
    /// longer holds what the write left, or that cannot be put back, stays
    /// as it is, and that is said loudly.
    pub(crate) fn put_back(self, domain: &str, keep: Option<&Path>) {
        let mut left: Vec<PathBuf> = Vec::new();
        for file in self.files.into_iter().rev() {
            let abs = &file.abs;
            if keep == Some(abs.as_path()) {
                left.push(abs.clone());
                tracing::error!(
                    domain,
                    file = %abs.display(),
                    "a refused Reflection write leaves this file as it wrote it: the text sent could not be kept anywhere else"
                );
                continue;
            }
            let now = bytes_at(abs);
            let unchanged = matches!((&now, &file.after), (Ok(now), Some(after)) if now == after);
            if !unchanged {
                left.push(abs.clone());
                tracing::error!(
                    domain,
                    file = %abs.display(),
                    "a refused Reflection write leaves this file as it is: it changed since the write, so it is not put back"
                );
                continue;
            }
            if now.ok().flatten() == file.before {
                continue;
            }
            let done = match &file.before {
                Some(bytes) => write_bytes(abs, bytes),
                None => remove_if_there(abs).map(|_| ()),
            };
            if let Err(e) = done {
                left.push(abs.clone());
                tracing::error!(
                    domain,
                    file = %abs.display(),
                    "a refused Reflection write could not put this file back, so it stays as the write left it: {e}"
                );
            }
        }
        for (to, from) in self.renamed.into_iter().rev() {
            if left.contains(&to) {
                continue;
            }
            if let Err(e) = std::fs::rename(&to, &from) {
                tracing::error!(
                    domain,
                    file = %to.display(),
                    "a refused Reflection write could not give this file its old spelling back: {e}"
                );
            }
        }
    }
}

/// What a refused write keeps in the kept folder: the text the person or
/// agent sent, under the page path inside `reflections/`.
pub(crate) struct KeepText<'a> {
    /// A page text, or a thread comment on the page.
    pub kind: crystalline_core::kept::KeptKind,
    /// The page's path inside `reflections/`.
    pub path: &'a str,
    /// The text as sent.
    pub text: &'a str,
    /// Who sent it: the caller, whose account is recorded beside the kept
    /// text (an agent's is the account it acts for) with its name.
    pub caller: &'a crate::scope::ReflectionCaller,
    /// The file on this disk that holds the text the write wrote: left as
    /// written when the text cannot be kept.
    pub holds: &'a Path,
}

/// What a fenced write does when the fence refuses it.
pub(crate) struct OnRefusal<'a> {
    /// The domain, for the kept folder, the hosts map and the log.
    pub domain: &'a str,
    /// The files to put back.
    pub undo: DiskUndo,
    /// The text to keep, if the write carried one a person or agent sent.
    pub keep: Option<KeepText<'a>>,
    /// Whether this is a read healing a row, not a write: its refusal is
    /// no failure, and is logged as such.
    pub read: bool,
}

impl<'a> OnRefusal<'a> {
    /// A refusal that puts `undo` back and keeps nothing.
    pub(crate) fn put_back(domain: &'a str, undo: DiskUndo) -> OnRefusal<'a> {
        OnRefusal {
            domain,
            undo,
            keep: None,
            read: false,
        }
    }

    /// A read that heals a row: nothing to put back or keep.
    pub(crate) fn read_heal(domain: &'a str) -> OnRefusal<'a> {
        OnRefusal {
            domain,
            undo: DiskUndo::default(),
            keep: None,
            read: true,
        }
    }
}

/// One row change of a fenced commit.
pub(crate) enum RowOp<'a> {
    /// A page's row and chunks, from its text as written at `abs`.
    Page {
        inner: &'a str,
        text: &'a str,
        abs: &'a Path,
    },
    /// A page's row goes.
    DeletePage(&'a str),
    /// A Reflection file's row, as written.
    PutAsset(&'a crystalline_index::AttachmentRow),
    /// A Reflection file's row goes.
    DeleteAsset(&'a str),
    /// A read heals a Reflection file's row: written only while the stored
    /// row is still `seen`, the one the read compared, so a row another
    /// writer stored meanwhile stays.
    HealAsset {
        row: &'a crystalline_index::AttachmentRow,
        seen: Option<&'a crystalline_index::AttachmentRow>,
    },
    /// A page's reconcile state, as recorded.
    // Built only by the tests until the `reconciled` action of
    // `write_reflection` records one.
    #[cfg_attr(not(any(test, feature = "testing")), allow(dead_code))]
    Reconcile(&'a crystalline_index::ReflectionReconcile),
    /// A page's reconcile state goes with it from `from` to `to`. Put it
    /// before the [`RowOp::DeletePage`] of `from`, which takes the state
    /// with the row.
    MoveReconcile { from: &'a str, to: &'a str },
}

/// Whether `e` is a refusal of the host-lock fence (or of the gate, whose
/// sentence it shares).
pub(crate) fn is_fence_refusal(e: &EngineError) -> bool {
    matches!(e, EngineError::Refused(text) if text.starts_with(REFUSED_HOSTED_START))
}

/// What a fence refusal adds when it kept the text sent.
pub const KEPT_HERE: &str = "; your text is kept on this instance (crystalline kept list shows it)";

/// The sentence of a fence refusal when no instance holds the lock now.
fn nobody_holds() -> EngineError {
    EngineError::Refused(format!("{REFUSED_HOSTED_START}no instance now; try again"))
}

impl Engine {
    /// Whether this instance may write `domain_id`'s Reflection rows now:
    /// `None` when it holds the domain's host lock (or the database is
    /// its own, or it has no instance id and so holds no locks at all),
    /// else the refusal naming the holder. Read inside the caller's
    /// transaction, so a take-over waits for the commit on Postgres.
    async fn reflection_row_fence(
        &self,
        store: &dyn Store,
        domain_id: DomainId,
    ) -> Result<Option<(EngineError, bool)>> {
        if self.instance_id.is_empty() || !store.shares_database() {
            return Ok(None);
        }
        let holder = store.domain_host_held(domain_id).await?;
        Ok(match holder {
            Some(h) if h.instance_id == self.instance_id => None,
            Some(h) => Some((hosted_refusal(&h.label), true)),
            None => Some((nobody_holds(), false)),
        })
    }

    /// Commit `ops` in one transaction behind the host-lock fence (see the
    /// module). With an empty `ops` it only checks the fence, for a write
    /// that has no rows (a thread entry). On a refusal it keeps the sent
    /// text, puts the files back and answers the refusal; any other failure
    /// is answered as it is, the files left as written (the next sync
    /// brings the rows up to date).
    pub(crate) async fn commit_reflection_rows(
        &self,
        domain_id: DomainId,
        ops: &[RowOp<'_>],
        on_refusal: OnRefusal<'_>,
    ) -> Result<()> {
        let mut on_refusal = on_refusal;
        on_refusal.undo.seal();
        self.take_row_fence_hold().await;
        let store = self.store.lock().await;
        store.begin().await?;
        let written = async {
            if let Some(refused) = self.reflection_row_fence(&*store, domain_id).await? {
                return Ok(Some(refused));
            }
            for op in ops {
                match op {
                    RowOp::Page { inner, text, abs } => {
                        let row = crystalline_index::ReflectionRow {
                            path: inner.to_string(),
                            title: r::title_of(text, inner),
                            content: text.to_string(),
                            sha256: sha256_hex(text.as_bytes()),
                            size: text.len() as u64,
                            modified: asset_modified(abs),
                        };
                        crystalline_index::reflection::index_reflection_row(
                            &*store,
                            domain_id,
                            &row,
                            &self.chunk_params,
                        )
                        .await?;
                    }
                    RowOp::DeletePage(inner) => {
                        store.delete_reflection(domain_id, inner).await?;
                    }
                    RowOp::PutAsset(row) => {
                        store
                            .upsert_asset(AssetTable::Reflection, domain_id, row)
                            .await?;
                    }
                    RowOp::DeleteAsset(path) => {
                        store
                            .delete_asset(AssetTable::Reflection, domain_id, path)
                            .await?;
                    }
                    RowOp::Reconcile(state) => {
                        store.set_reflection_reconcile(domain_id, state).await?;
                    }
                    RowOp::MoveReconcile { from, to } => {
                        store.move_reflection_reconcile(domain_id, from, to).await?;
                    }
                    RowOp::HealAsset { row, seen } => {
                        let now = store
                            .get_asset(AssetTable::Reflection, domain_id, &row.path)
                            .await?;
                        if now.as_ref() == *seen {
                            store
                                .upsert_asset(AssetTable::Reflection, domain_id, row)
                                .await?;
                        }
                    }
                }
            }
            #[cfg(any(test, feature = "testing"))]
            if self
                .reflection_index_fault
                .swap(false, std::sync::atomic::Ordering::SeqCst)
            {
                return Err(EngineError::Invalid(
                    "injected failure after the reflection row and its chunks".into(),
                ));
            }
            Ok::<_, EngineError>(None)
        }
        .await;
        let committed = match written {
            Ok(None) => store.commit().await.map_err(EngineError::from),
            Ok(Some((refusal, foreign))) => {
                let _ = store.rollback().await;
                drop(store);
                return Err(self.refuse_fenced(refusal, foreign, on_refusal));
            }
            Err(e) => {
                let _ = store.rollback().await;
                Err(e)
            }
        };
        drop(store);
        // R56: the new chunks are embedded soon after the write, as an
        // engram's are, rather than at the next tick.
        if committed.is_ok() && ops.iter().any(|op| matches!(op, RowOp::Page { .. })) {
            self.nudge_embed();
        }
        committed
    }

    /// The refusal of a fenced write: keep the sent text, put the files
    /// back, stop hosting a domain another instance took, and answer. The
    /// answer says when the text was kept here.
    fn refuse_fenced(
        &self,
        refusal: EngineError,
        foreign: bool,
        on_refusal: OnRefusal<'_>,
    ) -> EngineError {
        let OnRefusal {
            domain,
            undo,
            keep,
            read,
        } = on_refusal;
        if read {
            tracing::info!(
                domain,
                "a Reflection file was read here, and its index row is left to the host: this instance no longer holds the domain's host lock ({refusal})"
            );
            if foreign && self.hosted.write().unwrap().remove(domain).is_some() {
                self.announce_host_moved(domain);
            }
            return refusal;
        }
        let mut answer = refusal;
        let mut not_kept = None;
        match keep.filter(|k| !k.text.trim().is_empty()) {
            Some(keep) => {
                let kept = self
                    .kept_dir()
                    .ok_or_else(|| "this process knows no state folder".to_string())
                    .and_then(|root| {
                        crystalline_core::kept::write(
                            &root,
                            domain,
                            None,
                            keep.kind,
                            keep.path,
                            crystalline_core::time::now(),
                            keep.text,
                        )
                        .map_err(|e| e.to_string())
                    });
                let actor = keep.caller.author.name.as_str();
                match kept {
                    Ok(file) => {
                        match crate::scope::overlay_actor(&keep.caller.scope) {
                            Some(account) => {
                                let sender = crystalline_core::kept::Sender {
                                    account,
                                    name: actor.to_string(),
                                };
                                if let Err(e) =
                                    crystalline_core::kept::record_sender(&file, &sender)
                                {
                                    tracing::error!(
                                        domain,
                                        kept = %file.display(),
                                        "the sender of this kept text could not be recorded, so only people who edit the Reflection reach it: {e}"
                                    );
                                }
                            }
                            None => tracing::warn!(
                                domain,
                                kept = %file.display(),
                                "this kept text was sent by nobody signed in, so only people who edit the Reflection reach it"
                            ),
                        }
                        tracing::warn!(
                            domain,
                            path = keep.path,
                            actor,
                            kept = %file.display(),
                            "a Reflection write was refused because this instance no longer holds the domain's host lock ({answer}); the text sent is kept here; crystalline kept list shows it"
                        );
                        answer = EngineError::Refused(format!("{answer}{KEPT_HERE}"));
                    }
                    Err(e) => {
                        tracing::error!(
                            domain,
                            path = keep.path,
                            actor,
                            "a Reflection write was refused because this instance no longer holds the domain's host lock ({answer}), and keeping the text sent failed: {e}"
                        );
                        not_kept = Some(keep.holds);
                    }
                }
            }
            None => tracing::warn!(
                domain,
                "a Reflection write was refused because this instance no longer holds the domain's host lock: {answer}"
            ),
        }
        undo.put_back(domain, not_kept);
        if foreign && self.hosted.write().unwrap().remove(domain).is_some() {
            self.announce_host_moved(domain);
        }
        answer
    }
}

/// One file a refused write changed: its path, what it held before, and
/// what the write left there (`None`: no file).
pub(crate) type FileChange = (PathBuf, Option<Vec<u8>>, Option<Vec<u8>>);

/// Whether `e` is the refusal of a write into a file domain on an instance
/// that does not host it (the gate's or the fence's).
pub(crate) fn is_file_host_refusal(e: &EngineError) -> bool {
    matches!(e, EngineError::Conflict(text)
        if text.contains(super::FILE_HOSTED_MARK) || text.contains(super::FILE_NO_HOST_MARK))
}

/// What a refused engram write sent, to keep: its path in the domain, the
/// text, and who sent it.
pub(crate) struct EngramKeep<'a> {
    /// The engram's file path in its domain.
    pub rel: &'a str,
    /// The text the write would have landed.
    pub text: &'a str,
    /// The sender's account (an agent's is the account it acts for), or
    /// `None` when nobody signed in sent it.
    pub account: Option<String>,
    /// The name the write was made under.
    pub actor: &'a str,
}

impl Engine {
    /// The host-lock fence of a file domain's rows: read inside the row
    /// write's transaction, as the Reflection fence and a walk's apply read
    /// it, so a take-over waits for the commit on Postgres. Passes while this
    /// instance holds the lock (any process of this machine under its id),
    /// off a shared database and without an instance id. A refusal names the
    /// holder, and drops a domain another instance took from this
    /// instance's hosts.
    pub(crate) async fn file_row_fence(
        &self,
        store: &dyn Store,
        domain_id: DomainId,
        root: &Path,
    ) -> Result<()> {
        if self.instance_id.is_empty() || !store.shares_database() {
            return Ok(());
        }
        self.take_row_fence_hold().await;
        let holder = store.domain_host_held(domain_id).await?;
        if holder
            .as_ref()
            .is_some_and(|h| h.instance_id == self.instance_id)
        {
            return Ok(());
        }
        let name = self
            .domain_at_root(root)
            .unwrap_or_else(|| "<domain>".to_string());
        let Some(host) = holder else {
            return Err(super::file_no_host_refusal(&name));
        };
        if self.hosted.write().unwrap().remove(&name).is_some() {
            tracing::warn!("lost the host lock for domain '{name}'; another instance took over");
            self.announce_host_moved(&name);
        }
        Err(super::file_host_refusal(&name, &host.label))
    }

    /// The registered file domain whose folder is `root`.
    fn domain_at_root(&self, root: &Path) -> Option<String> {
        self.config()
            .domains
            .iter()
            .find(|(_, entry)| entry.file_path().as_deref() == Some(root))
            .map(|(name, _)| name.clone())
    }

    /// A file-domain write refused after its file was written (the fence
    /// found the lock gone): keep the text sent in the kept folder with its
    /// sender, then put the file back to `before` while it still holds
    /// `written`, and answer the refusal, saying the text was kept. A text
    /// that cannot be kept leaves the file as written. Any other error is
    /// answered as it is.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn refuse_file_write(
        &self,
        e: EngineError,
        domain: &str,
        abs: &Path,
        before: Option<&[u8]>,
        written: &[u8],
        keep: Option<EngramKeep<'_>>,
    ) -> EngineError {
        if !is_file_host_refusal(&e) {
            return e;
        }
        let mut answer = e;
        let mut kept_ok = true;
        if let Some(keep) = keep.filter(|k| !k.text.trim().is_empty()) {
            let kept = self
                .kept_dir()
                .ok_or_else(|| "this process knows no state folder".to_string())
                .and_then(|root| {
                    crystalline_core::kept::write(
                        &root,
                        domain,
                        None,
                        crystalline_core::kept::KeptKind::Engram,
                        keep.rel,
                        crystalline_core::time::now(),
                        keep.text,
                    )
                    .map_err(|e| e.to_string())
                });
            match kept {
                Ok(file) => {
                    if let Some(account) = keep.account.clone() {
                        let sender = crystalline_core::kept::Sender {
                            account,
                            name: keep.actor.to_string(),
                        };
                        if let Err(e) = crystalline_core::kept::record_sender(&file, &sender) {
                            tracing::error!(
                                domain,
                                kept = %file.display(),
                                "the sender of this kept text could not be recorded: {e}"
                            );
                        }
                    }
                    tracing::warn!(
                        domain,
                        path = keep.rel,
                        actor = keep.actor,
                        kept = %file.display(),
                        "an engram write was refused because this instance no longer holds the domain's host lock ({answer}); the text sent is kept here; crystalline kept list shows it"
                    );
                    answer = EngineError::Conflict(format!("{answer}{KEPT_HERE}"));
                }
                Err(err) => {
                    kept_ok = false;
                    tracing::error!(
                        domain,
                        path = keep.rel,
                        "an engram write was refused because this instance no longer holds the domain's host lock ({answer}), keeping the text sent failed ({err}), so the file stays as written"
                    );
                }
            }
        }
        if kept_ok {
            DiskUndo {
                files: vec![Undone {
                    abs: abs.to_path_buf(),
                    before: before.map(<[u8]>::to_vec),
                    after: Some(Some(written.to_vec())),
                }],
                renamed: Vec::new(),
            }
            .put_back(domain, None);
        }
        answer
    }

    /// A file-domain delete or move refused by the fence after its files
    /// changed: put each file back to what it held while it still holds what
    /// the write left there (`after`, `None` for removed), and answer the
    /// refusal.
    pub(crate) fn refuse_file_change(
        &self,
        e: EngineError,
        domain: &str,
        files: Vec<FileChange>,
    ) -> EngineError {
        if is_file_host_refusal(&e) {
            DiskUndo {
                files: files
                    .into_iter()
                    .map(|(abs, before, after)| Undone {
                        abs,
                        before,
                        after: Some(after),
                    })
                    .collect(),
                renamed: Vec::new(),
            }
            .put_back(domain, None);
        }
        e
    }

    /// [`Engine::refuse_file_change`] for one removed file.
    pub(crate) fn refuse_file_delete(
        &self,
        e: EngineError,
        domain: &str,
        abs: &Path,
        before: Option<&[u8]>,
    ) -> EngineError {
        self.refuse_file_change(
            e,
            domain,
            vec![(abs.to_path_buf(), before.map(<[u8]>::to_vec), None)],
        )
    }

    /// One engram attachment row write of a file domain, behind its
    /// host-lock fence, in a transaction of its own.
    pub(crate) async fn fenced_attachment_row(
        &self,
        domain_id: DomainId,
        root: &Path,
        op: AttachmentRowOp<'_>,
    ) -> Result<()> {
        let store = self.store.lock().await;
        store.begin().await?;
        let done = async {
            self.file_row_fence(&*store, domain_id, root).await?;
            match op {
                AttachmentRowOp::Upsert(row) => store.upsert_attachment(domain_id, row).await?,
                AttachmentRowOp::Delete(path) => {
                    store.delete_attachment(domain_id, path).await?;
                }
            }
            Ok::<(), EngineError>(())
        }
        .await;
        match done {
            Ok(()) => Ok(store.commit().await?),
            Err(e) => {
                let _ = store.rollback().await;
                Err(e)
            }
        }
    }
}

/// An engram attachment row change for [`Engine::fenced_attachment_row`].
pub(crate) enum AttachmentRowOp<'a> {
    /// The row as written.
    Upsert(&'a crystalline_index::AttachmentRow),
    /// The row at this path goes.
    Delete(&'a str),
}
