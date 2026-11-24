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
            // Retired by its status word, as the page view reads it: a
            // record left under a live status is replaced.
            if is_retired(text, inner) {
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
        // Retired by its status word only: a record left under a live
        // status (changed by hand) must not put an old status back.
        if !is_retired(&text, inner) {
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
        self.land_page_record(domain, &access, inner, &abs, &text, &changed, caller)
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

    /// A curator's change to the settings of the page at `change.path`
    /// (`PATCH /domains/{domain}/reflection/settings`, design 3.2): each key
    /// left out keeps its value, an empty value takes the key off and any
    /// other value sets it. The new text is the stored one with only those
    /// frontmatter keys changed, written as a whole-page write against the
    /// stored checksum, so a live room takes it in, the index and the
    /// backup follow and `follows` is checked as on every write. Answers
    /// the write's receipt with the page as a read shows it (`page`).
    ///
    /// Refused: below curate; a pact file or a missing page; nothing to
    /// change; a `freshness` that does not read, under an hour, or on an
    /// owned page (it is reconciled with its owner); a retired word in
    /// `status` (retire the page instead) and any `status` change on a
    /// retired page (restore it instead).
    pub async fn set_reflection_page_settings(
        &self,
        domain: &str,
        change: &crate::params::PageSettingsChange,
        caller: &ReflectionCaller,
    ) -> Result<Value> {
        let inner = change.path.as_str();
        let access = self.page_settings_gate(domain, inner, caller).await?;
        let wanted: Vec<(&str, Option<&str>)> = [
            (r::FRESHNESS_KEY, change.freshness.as_deref()),
            (r::FOLLOWS_KEY, change.follows.as_deref()),
            (r::STATUS_KEY, change.status.as_deref()),
        ]
        .into_iter()
        .filter_map(|(key, value)| value.map(|v| (key, Some(v.trim()).filter(|v| !v.is_empty()))))
        .collect();
        if wanted.is_empty() {
            return Err(EngineError::Invalid(
                "a settings change names freshness, follows or status; send at least one".into(),
            ));
        }
        let stored = read_opt(&access.abs(inner))?.ok_or_else(|| no_page(domain, inner))?;
        let record = r::read_provenance(&stored).page;
        // Retired while the status is a retired word, as the page view
        // reads it; a record left under a live status is no retirement.
        let retired = is_retired(&stored, inner);
        let mut text = stored.clone();
        for (key, value) in &wanted {
            match (*key, *value) {
                (r::FRESHNESS_KEY, Some(spec)) => {
                    r::parse_freshness(spec).map_err(|reason| {
                        EngineError::Invalid(format!("freshness '{spec}' {reason}"))
                    })?;
                    if let Some(owner) = &record.owner {
                        return Err(EngineError::Invalid(format!(
                            "'{inner}' is owned by '{owner}' and is reconciled with it, so it takes no freshness of its own; give '{owner}' the freshness instead"
                        )));
                    }
                }
                (r::STATUS_KEY, _) if retired => {
                    return Err(EngineError::Invalid(format!(
                        "'{inner}' is retired; restore it (write_reflection action=restore) to change its status"
                    )));
                }
                (r::STATUS_KEY, Some(status))
                    if crystalline_core::is_retired_status(&status.to_lowercase()) =>
                {
                    return Err(EngineError::Invalid(format!(
                        "status '{status}' retires the page; retire '{inner}' instead (write_reflection action=retire), which keeps the page it was owned by so a restore can put it back"
                    )));
                }
                _ => {}
            }
            // A cleared key is sent with no value: a curator's whole-page
            // write takes it off (a key left out keeps its stored value).
            text = set_page_key(&text, key, *value)?;
        }
        let receipt = self
            .write_reflection(
                &crate::params::WriteReflectionParams {
                    domain: domain.to_string(),
                    path: inner.to_string(),
                    markdown: Some(text),
                    expected_checksum: Some(sha256_hex(stored.as_bytes())),
                    ..Default::default()
                },
                caller,
            )
            .await?;
        let written = read_opt(&access.abs(inner))?.ok_or_else(|| no_page(domain, inner))?;
        let mut receipt = receipt;
        receipt["page"] = self.page_view(access.domain_id, inner, &written).await?;
        Ok(receipt)
    }

    /// The stored pact hash a write of the page at `inner` carries along
    /// when it changes only the page's `freshness` or `status` (design 3.2):
    /// `(seen_hash, seen_at, hash)` when the stored state held the hash of
    /// the page as it was (`current`) and the new text (`written`) differs
    /// from it only by those two keys. A change of `follows` that brings
    /// other pacts is no such change: the page then really follows another
    /// pact, and its `page_pact_changed` item stays. `None` otherwise,
    /// including for a page with no stored hash; a change on disk or by a
    /// pull never comes here, so it still raises the item.
    pub(super) async fn settings_rehash(
        &self,
        domain_id: DomainId,
        inner: &str,
        current: Option<&str>,
        written: &str,
    ) -> Result<Option<(String, String, String)>> {
        let Some(current) = current else {
            return Ok(None);
        };
        if !matches!(r::classify(inner), r::ReflectionFile::Page(_)) {
            return Ok(None);
        }
        let keys = [r::FRESHNESS_KEY, r::STATUS_KEY];
        let (before, after) = (r::page_key_values(current), r::page_key_values(written));
        if keys.iter().all(|key| before.get(key) == after.get(key)) {
            return Ok(None);
        }
        let (rows, state) = {
            let store = self.store.lock().await;
            let state = store.reflection_reconcile(domain_id, inner).await?;
            let Some(state) = state.filter(|state| !state.pact_hash.is_empty()) else {
                return Ok(None);
            };
            (store.list_reflections(domain_id).await?, state)
        };
        let hash_with = |text: &str| {
            r::page_pact_hash(
                inner,
                rows.iter()
                    .filter(|row| row.path != inner)
                    .map(|row| (row.path.as_str(), row.content.as_str()))
                    .chain([(inner, text)]),
            )
        };
        let (Some(was), Some(now)) = (hash_with(current), hash_with(written)) else {
            return Ok(None);
        };
        if state.pact_hash != was || now == was {
            return Ok(None);
        }
        // The new text with the old freshness and status: the hash it would
        // have when nothing but those two keys changed.
        let only_settings = r::carry_page_keys(written, Some(current), &keys)
            .ok()
            .and_then(|text| hash_with(&text))
            .is_some_and(|hash| hash == was);
        Ok(only_settings.then_some((state.pact_hash, state.last_reconciled, now)))
    }

    /// The gate of a settings change: curate, a writable domain, and a page
    /// (no pact file).
    async fn page_settings_gate(
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
                "'{inner}' is a pact file, no page: only a page has settings"
            )));
        }
        Ok(access)
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
        self.land_page_record(domain, &access, inner, &abs, &text, &changed, caller)
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

    /// Write `text` as the page at `inner` (at `abs`, under its lock), which
    /// held `current`, and index it behind the fence; a refusal puts the
    /// file back. Its sections are as they were, so its baseline and threads
    /// stay. A change of only its `status` (and `freshness`) carries the
    /// stored pact hash along in the same commit, as a write's does
    /// ([`Engine::settings_rehash`]), so a retire or a restore asks for no
    /// reconcile.
    #[allow(clippy::too_many_arguments)]
    async fn land_page_record(
        &self,
        domain: &str,
        access: &ReflectionAccess,
        inner: &str,
        abs: &Path,
        current: &str,
        text: &str,
        caller: &ReflectionCaller,
    ) -> Result<()> {
        let text = crystalline_core::to_lf(text).into_owned();
        let rehash = self
            .settings_rehash(access.domain_id, inner, Some(current), &text)
            .await?;
        let mut undo = DiskUndo::default();
        undo.record(abs)?;
        write_file(abs, &text)?;
        let mut ops = vec![super::fence::RowOp::Page {
            inner,
            text: &text,
            abs,
        }];
        if let Some((seen_hash, seen_at, hash)) = &rehash {
            ops.push(super::fence::RowOp::RehashReconcile {
                path: inner,
                seen_hash,
                seen_at,
                hash,
            });
        }
        self.commit_reflection_rows(access.domain_id, &ops, OnRefusal::put_back(domain, undo))
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
        .map(|status| key_block(r::STATUS_KEY, serde_yaml_ng::Value::from(status)))
        .transpose()?;
    r::carry_page_keys(text, stored.as_deref(), &[r::STATUS_KEY]).map_err(EngineError::Invalid)
}

/// `text` with the page key `key` set to `value`, or written with no value
/// for `None`: how a curator's whole-page write takes a key off.
fn set_page_key(text: &str, key: &str, value: Option<&str>) -> Result<String> {
    let value = value.map_or(serde_yaml_ng::Value::Null, serde_yaml_ng::Value::from);
    let stored = key_block(key, value)?;
    r::carry_page_keys(text, Some(&stored), &[key]).map_err(EngineError::Invalid)
}

/// A frontmatter holding only `key: value`, YAML safe.
fn key_block(key: &str, value: serde_yaml_ng::Value) -> Result<String> {
    let mut map = serde_yaml_ng::Mapping::new();
    map.insert(key.into(), value);
    serde_yaml_ng::to_string(&map)
        .map(|yaml| format!("---\n{yaml}---\n"))
        .map_err(|e| EngineError::Internal(e.to_string()))
}

/// Whether the page `text` at `inner` is retired by its `status`.
pub(super) fn is_retired(text: &str, inner: &str) -> bool {
    let record = r::read_provenance(text).page;
    r::read_page_settings(text, inner, &record).0.retired()
}

fn no_page(domain: &str, inner: &str) -> EngineError {
    EngineError::NotFound(format!("no reflection '{inner}' in domain '{domain}'"))
}
