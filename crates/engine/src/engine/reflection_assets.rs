//! A Reflection's files: `reflections/assets/`, under the Reflection's own
//! rights. A reader reads them, a contributor adds a new one, a
//! curator replaces, moves and removes. Knowledge rights play no part.
//!
//! The byte rules, the write, the read and its row heal are the asset
//! substrate's, the engram attachments' own code, with the Reflection's
//! home: its folder, its table, and a change that marks the backup branch.

use crystalline_core::reflection as r;

use super::asset_home::refuse_extension_change;
use super::reflection::rooms_to_rewrite;
use super::*;
use crate::scope::{ReflectionCaller, ReflectionRight, reflection_right_word};

/// What an upload into a Reflection landed as.
#[derive(Debug, Clone)]
pub struct WrittenReflectionAsset {
    /// The row that now describes the file.
    pub row: AttachmentRow,
    /// Whether the path held no file before.
    pub created: bool,
    /// Whether the path already held exactly these bytes, so nothing was
    /// written.
    pub unchanged: bool,
}

impl Engine {
    /// Every file of a domain's Reflection, metadata only, ordered by path.
    pub async fn reflection_asset_list(
        &self,
        domain: &str,
        caller: &ReflectionCaller,
    ) -> Result<Vec<AttachmentRow>> {
        self.require_reflection(domain, caller, ReflectionRight::Read)
            .await?;
        self.asset_list_in(AssetHome::Reflection, domain).await
    }

    /// One file's bytes and its row, the row healed from the file when they
    /// disagree (into the Reflection's own table).
    pub async fn reflection_asset_read(
        &self,
        domain: &str,
        path: &str,
        caller: &ReflectionCaller,
    ) -> Result<(Vec<u8>, AttachmentRow)> {
        self.require_reflection(domain, caller, ReflectionRight::Read)
            .await?;
        self.asset_read_in(AssetHome::Reflection, domain, path)
            .await
    }

    /// Add a file, or replace one (replacing destroys the old bytes,
    /// so it needs curate; the check runs under the file's lock).
    pub async fn reflection_asset_write(
        &self,
        domain: &str,
        path: &str,
        bytes: Vec<u8>,
        caller: &ReflectionCaller,
    ) -> Result<WrittenReflectionAsset> {
        self.reflection_asset_put(domain, path, bytes, caller, false)
            .await
    }

    /// Add a file only where none is: a file already at `path` is refused
    /// with [`EngineError::Conflict`] ending in
    /// [`crate::engine::ASSET_EXISTS`], for a curator too. The check runs
    /// under the file's lock, so of two racing uploads to one free path
    /// exactly one lands.
    pub async fn reflection_asset_create(
        &self,
        domain: &str,
        path: &str,
        bytes: Vec<u8>,
        caller: &ReflectionCaller,
    ) -> Result<WrittenReflectionAsset> {
        self.reflection_asset_put(domain, path, bytes, caller, true)
            .await
    }

    /// The two verbs above. Create-only wins over the curate refusal (R90):
    /// a contributor who asked to only create is told that, not that
    /// replacing needs curate.
    async fn reflection_asset_put(
        &self,
        domain: &str,
        path: &str,
        bytes: Vec<u8>,
        caller: &ReflectionCaller,
        create_only: bool,
    ) -> Result<WrittenReflectionAsset> {
        let access = self
            .require_reflection_to_write(domain, caller, ReflectionRight::Edit)
            .await?;
        let replace = if create_only {
            ReplaceRule::CreateOnly
        } else if access.right >= ReflectionRight::Curate {
            ReplaceRule::Allow
        } else {
            ReplaceRule::Refuse(format!(
                "'{path}' already holds a file, and replacing it removes that file: your access to the Reflection of '{domain}' is {}, and curate is needed; upload it under a new name instead",
                reflection_right_word(access.right)
            ))
        };
        let written: WrittenAsset = self
            .asset_write_in(AssetHome::Reflection, domain, path, bytes, replace)
            .await?;
        // The stored spelling, the one the file listing answers. The same
        // bytes again changed nothing, so there is nothing to announce.
        if !written.unchanged {
            self.announce_reflection(
                domain,
                &written.row.path,
                ReflectionChangeKind::Files,
                None,
                change_label(&caller.scope),
            );
        }
        Ok(WrittenReflectionAsset {
            row: written.row,
            created: written.created,
            unchanged: written.unchanged,
        })
    }

    /// Remove a file and its row. A reflection that still links it keeps
    /// the link; the reflect queue names it as missing.
    pub async fn reflection_asset_delete(
        &self,
        domain: &str,
        path: &str,
        caller: &ReflectionCaller,
    ) -> Result<()> {
        self.require_reflection_to_write(domain, caller, ReflectionRight::Curate)
            .await?;
        self.asset_delete_in(AssetHome::Reflection, domain, path)
            .await?;
        self.announce_reflection(
            domain,
            path,
            ReflectionChangeKind::Files,
            None,
            change_label(&caller.scope),
        );
        Ok(())
    }

    /// Move or rename a file inside `reflections/assets/` and rewrite every
    /// reflection (PACT.md included) that links it. The rewrite is not a
    /// person's change, so it owes no feedback.
    pub async fn move_reflection_asset(
        &self,
        domain: &str,
        from: &str,
        to: &str,
        caller: &ReflectionCaller,
    ) -> Result<Value> {
        let access = self
            .require_reflection_to_write(domain, caller, ReflectionRight::Curate)
            .await?;
        // The paths first, so a move that cannot happen says why rather
        // than that a page is open.
        validate_attachment_path(from)?;
        validate_attachment_path(to)?;
        refuse_extension_change(from, to)?;
        if from == to {
            return Err(EngineError::Invalid(format!(
                "'{from}' already is at '{to}'"
            )));
        }
        // Every page the move will rewrite, read from disk and checked
        // against open rooms before anything moves: a refused move changes
        // nothing.
        let pages = self
            .store
            .lock()
            .await
            .list_reflection_stamps(access.domain_id)
            .await?;
        let linking = rooms_to_rewrite(
            &access,
            pages.iter().map(|p| p.path.as_str()),
            &|text, inner| r::rewrite_asset_links(text, inner, from, to).1,
        );
        self.refuse_under_live_pages(domain, &linking).await?;
        let row_behind = self
            .asset_move_in(AssetHome::Reflection, domain, from, to)
            .await?;
        self.announce_reflection(
            domain,
            to,
            ReflectionChangeKind::Files,
            Some(from),
            change_label(&caller.scope),
        );
        // The file has moved by now, so a page that cannot be relinked is
        // named in the receipt (and the log) instead of failing a move that
        // happened, as a page move does.
        let mut links = 0;
        let mut failed: Vec<Value> = Vec::new();
        for page in &pages {
            let inner = page.path.as_str();
            match self
                .relink_reflection(domain, &access, inner, &|s| {
                    r::rewrite_asset_links(s, inner, from, to)
                })
                .await
            {
                Ok(n) => {
                    links += n;
                    if n > 0 {
                        // A link that followed the file: nobody's change.
                        self.announce_reflection(
                            domain,
                            inner,
                            ReflectionChangeKind::Changed,
                            None,
                            None,
                        );
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        domain,
                        from,
                        to,
                        page = inner,
                        "a link to the moved file could not be rewritten: {e}"
                    );
                    super::move_::note_link_failure(&mut failed, domain, inner, &e);
                }
            }
        }
        if links > 0 {
            // The pages the move relinked, after the file's own mark.
            self.mark_reflection_dirty(domain, crate::reflection_backup::BackupCause::Settled);
        }
        let mut receipt =
            json!({ "domain": domain, "from": from, "to": to, "links_rewritten": links });
        if !failed.is_empty() {
            receipt["links_failed"] = json!(failed);
        }
        // The file moved and its row did not: said plainly, the sync heals it.
        if let Some(note) = row_behind {
            receipt["note"] = json!(note);
        }
        Ok(receipt)
    }
}
