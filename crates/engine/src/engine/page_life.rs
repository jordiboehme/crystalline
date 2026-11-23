//! Owned and retired Reflection pages (spec 2.3 and 2.4).
//!
//! A page an agent creates with `owner` during a reconcile is owned by that
//! page: the owned page's page record names it ([`r::PageRecord::owner`]),
//! and an owner's set is a scan of the pages ([`Engine::reflection_owned_pages`]).
//! An owner is named on create only, by a curator, and must be another
//! existing page that is not retired and leads nowhere back.
//!
//! A page is retired, never deleted, by an unattended session: its `status`
//! becomes a retired word, its owner leaves the record and is kept as the
//! page it was owned by, so a restore can put it back under that page. Both
//! change only the frontmatter and the block, which no section and no
//! baseline holds, so neither owes feedback; a live room keeps both on its
//! next save (it puts the stored keys and block back).

use std::collections::BTreeMap;

use crystalline_core::reflection as r;

use super::fence::{DiskUndo, OnRefusal};
use super::reflection::ReflectionAccess;
use super::*;
use crate::collab::session::RoomDoc;
use crate::reflection_backup::BackupCause;
use crate::scope::{ReflectionCaller, ReflectionRight};

/// The status a retire writes: a word of the retired set, as an engram's.
pub const RETIRED_STATUS: &str = "archived";

impl Engine {
    /// The pages each page of `domain`'s Reflection owns, by owner, read
    /// through the reader: a scan of the page records ([`r::owned_pages`]).
    /// A retired page is in no set.
    pub async fn reflection_owned_pages(
        &self,
        domain: &str,
    ) -> Result<BTreeMap<String, Vec<String>>> {
        let reader = self.reader();
        let Some(id) = reader.domain_id(domain).await? else {
            return Ok(BTreeMap::new());
        };
        let rows = reader.list_reflections(id).await?;
        Ok(r::owned_pages(
            rows.iter()
                .map(|row| (row.path.as_str(), row.content.as_str())),
        ))
    }

    /// Refuse `owner` as the owner of the page at `page` of the Reflection
    /// of `domain_id`: it must be another existing page that is not
    /// retired, and neither its owners nor the page's pact chain (with the
    /// page's `follows`, as it is written) may lead back to `page`
    /// ([`r::owner_refusal`]). `current` is the page as stored, `None` when
    /// it is created.
    pub(super) async fn refuse_owner(
        &self,
        domain_id: DomainId,
        page: &str,
        owner: &str,
        current: Option<&str>,
        follows: Option<String>,
    ) -> Result<()> {
        let row = self
            .store
            .lock()
            .await
            .get_reflection(domain_id, owner)
            .await?;
        if let Some(row) = row
            && is_retired(&row.content, owner)
        {
            return Err(EngineError::Invalid(format!(
                "owner '{owner}' is retired; restore it first or name another page"
            )));
        }
        let mut pages = self.page_links(domain_id, page, current).await?;
        pages.insert(
            page.to_string(),
            r::PageLinks {
                follows,
                owner: None,
            },
        );
        match r::owner_refusal(page, owner, &pages) {
            Some(refusal) => Err(EngineError::Invalid(refusal)),
            None => Ok(()),
        }
    }

    /// `write_reflection` with `action: retire`: the page at `inner` gets
    /// the retired status, leaves its owner's set and keeps that owner as
    /// the page it was owned by. A curator's (an agent of the reflect loop
    /// is one); `PACT.md` and page pacts are no pages to retire.
    pub(crate) async fn retire_reflection(
        &self,
        domain: &str,
        inner: &str,
        caller: &ReflectionCaller,
    ) -> Result<Value> {
        let mut was_owned_by = None;
        self.change_page_record(domain, inner, caller, |text| {
            let mut prov = r::read_provenance(text);
            if prov.page.retired.is_some() || is_retired(text, inner) {
                return Err(EngineError::Conflict(format!(
                    "'{inner}' is already retired; restore it to bring it back"
                )));
            }
            was_owned_by = prov.page.owner.take();
            // The status it has now comes back with a restore.
            let was_status = r::read_page_settings(text, inner, &prov.page).0.status;
            prov.page.retired = Some(r::Retirement {
                at: time::now().rfc3339_secs_z(),
                by: caller.author.name.clone(),
                was_owned_by: was_owned_by.clone(),
                was_status,
                other: Default::default(),
            });
            let text = r::write_provenance(text, &prov).map_err(EngineError::Invalid)?;
            set_status(&text, Some(RETIRED_STATUS))
        })
        .await?;
        Ok(json!({
            "domain": domain,
            "path": inner,
            "retired": true,
            "was_owned_by": was_owned_by,
        }))
    }

    /// `write_reflection` with `action: restore`: the retired page at
    /// `inner` loses its retired status and record. With `owner`, which
    /// must be the page it was owned by and still a page that may own it,
    /// it goes back under that page; without, it stands on its own. A
    /// curator's.
    pub(crate) async fn restore_reflection(
        &self,
        domain: &str,
        inner: &str,
        owner: Option<&str>,
        caller: &ReflectionCaller,
    ) -> Result<Value> {
        let owner = owner.map(str::trim);
        if owner == Some("") {
            return Err(EngineError::Invalid(EMPTY_OWNER.into()));
        }
        let access = self.page_record_gate(domain, inner, caller).await?;
        let abs = access.abs(inner);
        let _lock = self.lock_file(&abs).await;
        let text = read_opt(&abs)?.ok_or_else(|| no_page(domain, inner))?;
        let mut prov = r::read_provenance(&text);
        if prov.page.retired.is_none() && !is_retired(&text, inner) {
            return Err(EngineError::Invalid(format!(
                "'{inner}' is not retired; there is nothing to restore"
            )));
        }
        if let Some(owner) = owner {
            let was = prov
                .page
                .retired
                .as_ref()
                .and_then(|retired| retired.was_owned_by.as_deref());
            if was != Some(owner) {
                return Err(EngineError::Invalid(match was {
                    Some(was) => format!(
                        "'{inner}' was owned by '{was}', so it goes back under that page or stands on its own; restore it with owner '{was}' or leave owner out"
                    ),
                    None => format!(
                        "'{inner}' was owned by no page; leave owner out to restore it on its own"
                    ),
                }));
            }
            // Checked under the page's lock, against the pages as they are.
            let follows = r::read_page_settings(&text, inner, &prov.page).0.follows;
            self.refuse_owner(access.domain_id, inner, owner, Some(&text), follows)
                .await?;
        }
        // The status the page had before it was retired comes back; none
        // stays none.
        let was_status = prov
            .page
            .retired
            .take()
            .and_then(|retired| retired.was_status);
        prov.page.owner = owner.map(str::to_string);
        let changed = r::write_provenance(&text, &prov).map_err(EngineError::Invalid)?;
        let changed = set_status(&changed, was_status.as_deref())?;
        self.land_page_record(domain, &access, inner, &abs, &changed, caller)
            .await?;
        drop(_lock);
        self.absorb_into_room(domain, inner).await;
        Ok(json!({
            "domain": domain,
            "path": inner,
            "restored": true,
            "owner": owner,
        }))
    }

    /// Change the frontmatter or the block of the page at `inner` through
    /// `change`, which is handed the stored text, under the page's lock.
    async fn change_page_record(
        &self,
        domain: &str,
        inner: &str,
        caller: &ReflectionCaller,
        change: impl FnOnce(&str) -> Result<String>,
    ) -> Result<()> {
        let access = self.page_record_gate(domain, inner, caller).await?;
        let abs = access.abs(inner);
        let _lock = self.lock_file(&abs).await;
        let text = read_opt(&abs)?.ok_or_else(|| no_page(domain, inner))?;
        let changed = change(&text)?;
        self.land_page_record(domain, &access, inner, &abs, &changed, caller)
            .await?;
        drop(_lock);
        self.absorb_into_room(domain, inner).await;
        Ok(())
    }

    /// The gate of a retire and a restore: curate, a writable domain, and a
    /// page (no pact file).
    async fn page_record_gate(
        &self,
        domain: &str,
        inner: &str,
        caller: &ReflectionCaller,
    ) -> Result<ReflectionAccess> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        let access = self
            .require_reflection_to_write(domain, caller, ReflectionRight::Curate)
            .await?;
        self.refuse_read_only_domain(domain, &crate::scope::Scope::Unrestricted)
            .await?;
        r::validate_reflection_path(inner).map_err(EngineError::Invalid)?;
        if r::is_pact_file(inner) {
            return Err(EngineError::Invalid(format!(
                "'{inner}' is a pact file, no page: only a page is retired or restored"
            )));
        }
        Ok(access)
    }

    /// Write `text` as the page at `inner` (at `abs`, under its lock) and
    /// index it behind the fence; a refusal puts the file back. Its sections
    /// are as they were, so its baseline and threads stay.
    async fn land_page_record(
        &self,
        domain: &str,
        access: &ReflectionAccess,
        inner: &str,
        abs: &Path,
        text: &str,
        caller: &ReflectionCaller,
    ) -> Result<()> {
        let text = crystalline_core::to_lf(text).into_owned();
        let mut undo = DiskUndo::default();
        undo.record(abs)?;
        write_file(abs, &text)?;
        self.index_reflection(
            access.domain_id,
            inner,
            &text,
            abs,
            OnRefusal::put_back(domain, undo),
        )
        .await?;
        self.mark_reflection_dirty(domain, BackupCause::Settled);
        self.announce_reflection(
            domain,
            inner,
            ReflectionChangeKind::Changed,
            None,
            crate::changes::change_label(&caller.scope),
        );
        Ok(())
    }

    /// Let a live room over the page at `inner` take the file in, as a
    /// write's does; a room that closed meanwhile is no refusal.
    async fn absorb_into_room(&self, domain: &str, inner: &str) {
        let doc = RoomDoc::Reflection(inner.to_string());
        if let Some(rooms) = self.collab_rooms()
            && rooms.has_live_doc(domain, &doc, None).await
            && let Err(e) = rooms.absorb_doc_change(domain, &doc, None).await
        {
            tracing::debug!(
                domain,
                path = inner,
                "the open room did not take the change in: {e}"
            );
        }
    }
}

/// The refusal of an empty `owner`, on a write and on a restore alike.
pub(super) const EMPTY_OWNER: &str =
    "owner names no page; name a page path such as arch/overview.md, or leave owner out";

/// `text` with its `status` set to `status`, or taken out with `None`.
fn set_status(text: &str, status: Option<&str>) -> Result<String> {
    let stored = status
        .map(|status| {
            let mut map = serde_yaml_ng::Mapping::new();
            map.insert(r::STATUS_KEY.into(), status.into());
            serde_yaml_ng::to_string(&map)
                .map(|yaml| format!("---\n{yaml}---\n"))
                .map_err(|e| EngineError::Internal(e.to_string()))
        })
        .transpose()?;
    r::carry_page_keys(text, stored.as_deref(), &[r::STATUS_KEY]).map_err(EngineError::Invalid)
}

/// Whether the page `text` at `inner` is retired by its `status`.
pub(super) fn is_retired(text: &str, inner: &str) -> bool {
    let record = r::read_provenance(text).page;
    r::read_page_settings(text, inner, &record).0.retired()
}

fn no_page(domain: &str, inner: &str) -> EngineError {
    EngineError::NotFound(format!("no reflection '{inner}' in domain '{domain}'"))
}
