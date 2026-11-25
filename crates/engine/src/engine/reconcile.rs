//! The reconcile state of a Reflection page: when it was last reconciled,
//! what that did, the hash of the pact chain and page settings it saw and
//! the version of the queue item it settled.
//!
//! Kept in the index beside the page row at the Reflection's home, never in
//! the page file, so a check that changes nothing makes no file change and
//! no backup commit. The state is written behind the host-lock fence
//! ([`Engine::commit_reflection_rows`]), moves with its page in the move's
//! own commit ([`RowOp::MoveReconcile`]) and goes with the page row
//! ([`crystalline_index::Store::delete_reflection`], which the engine's
//! delete and the sync both reach). It belongs to the domain's index row,
//! so a domain rename keeps it.
//!
//! A page with no state (a new home, a restore, `reindex --wipe`) reads as
//! reconciled when its row last changed, with no outcome. That is computed
//! on every read and never written, so a wiped index does not make every
//! page due at once.
//!
//! Reads go through the engine's reader (the read pool on PostgreSQL), so
//! the automation status that reads them never waits for the writer.

use std::collections::BTreeMap;

use crystalline_core::reflection as r;
use crystalline_index::ReflectionReconcile;

use super::fence::{DiskUndo, OnRefusal, RowOp};
use super::*;
use crate::scope::{ReflectionCaller, ReflectionRight};

/// What a reconcile did to its page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconcileOutcome {
    /// The reconcile changed the page.
    Updated,
    /// The reconcile found nothing to change.
    Unchanged,
}

impl ReconcileOutcome {
    /// The word stored and shown.
    pub fn as_str(self) -> &'static str {
        match self {
            ReconcileOutcome::Updated => "updated",
            ReconcileOutcome::Unchanged => "unchanged",
        }
    }

    /// The outcome a stored word names, or `None` for a word this binary
    /// does not know.
    pub fn parse(word: &str) -> Option<ReconcileOutcome> {
        match word {
            "updated" => Some(ReconcileOutcome::Updated),
            "unchanged" => Some(ReconcileOutcome::Unchanged),
            _ => None,
        }
    }
}

/// A page's reconcile state as the engine reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageReconcile {
    /// When the page was last reconciled, RFC 3339. For a page with no
    /// recorded reconcile, the modified time of its index row.
    pub last_reconciled: String,
    /// What the last reconcile did; `None` when none is recorded.
    pub outcome: Option<ReconcileOutcome>,
    /// The hash of the pact chain and page settings the last reconcile saw;
    /// `None` when none is recorded.
    pub pact_hash: Option<String>,
    /// The version of the queue item the last reconcile settled; `None` when
    /// none is recorded.
    pub version: Option<String>,
}

impl PageReconcile {
    /// The state of a page with none recorded: reconciled when its row last
    /// changed.
    fn seeded(modified: &str) -> PageReconcile {
        PageReconcile {
            last_reconciled: modified.to_string(),
            outcome: None,
            pact_hash: None,
            version: None,
        }
    }

    /// The state `stored` records, or the seed from `modified` when its
    /// outcome is a word this binary does not know (a newer one wrote it).
    fn read(stored: ReflectionReconcile, modified: &str) -> PageReconcile {
        match ReconcileOutcome::parse(&stored.outcome) {
            Some(outcome) => PageReconcile {
                last_reconciled: stored.last_reconciled,
                outcome: Some(outcome),
                pact_hash: Some(stored.pact_hash),
                version: Some(stored.version),
            },
            None => {
                tracing::warn!(
                    path = %stored.path,
                    outcome = %stored.outcome,
                    "a reconcile record was not read: its outcome is unknown here"
                );
                PageReconcile::seeded(modified)
            }
        }
    }
}

/// Whether `path` is a page, the only thing that has a reconcile state:
/// not `PACT.md`, not a page pact.
fn is_page(path: &str) -> bool {
    matches!(r::classify(path), r::ReflectionFile::Page(_))
}

/// The reconcile state of every page among `stamps` (the Reflection's rows
/// without their text): the state `stored` records for it, or the seed from
/// its row's modified time. A state recorded for a path with no page row is
/// left out. What [`Engine::reflection_reconciles`] answers, for a caller
/// that read the rows itself.
pub(crate) fn page_reconciles(
    stamps: &[crystalline_index::ReflectionStamp],
    stored: Vec<ReflectionReconcile>,
) -> BTreeMap<String, PageReconcile> {
    let mut stored: BTreeMap<String, ReflectionReconcile> = stored
        .into_iter()
        .map(|state| (state.path.clone(), state))
        .collect();
    stamps
        .iter()
        .filter(|stamp| is_page(&stamp.path))
        .map(|stamp| {
            let state = match stored.remove(&stamp.path) {
                Some(state) => PageReconcile::read(state, &stamp.modified),
                None => PageReconcile::seeded(&stamp.modified),
            };
            (stamp.path.clone(), state)
        })
        .collect()
}

impl Engine {
    /// The reconcile state of the page at `page` of `domain`'s Reflection,
    /// read through the reader. `None` when there is no such page (or the
    /// domain has no index row); a page with no recorded reconcile reads as
    /// reconciled when its row last changed.
    pub async fn reflection_reconcile(
        &self,
        domain: &str,
        page: &str,
    ) -> Result<Option<PageReconcile>> {
        if !is_page(page) {
            return Ok(None);
        }
        let reader = self.reader();
        let Some(id) = reader.domain_id(domain).await? else {
            return Ok(None);
        };
        let Some(row) = reader.get_reflection(id, page).await? else {
            return Ok(None);
        };
        Ok(Some(match reader.reflection_reconcile(id, page).await? {
            Some(stored) => PageReconcile::read(stored, &row.modified),
            None => PageReconcile::seeded(&row.modified),
        }))
    }

    /// The reconcile state of every page of `domain`'s Reflection, by path,
    /// read through the reader as [`Engine::reflection_reconcile`] reads one.
    /// The page rows are read without their text. A state recorded for a
    /// path that has no page row is left out.
    pub async fn reflection_reconciles(
        &self,
        domain: &str,
    ) -> Result<BTreeMap<String, PageReconcile>> {
        let reader = self.reader();
        let Some(id) = reader.domain_id(domain).await? else {
            return Ok(BTreeMap::new());
        };
        // Two reads, not one snapshot: a page written or moved between them
        // reads as seeded or without its new state until the next read,
        // which is no worse than the read before the write.
        let stamps = reader.list_reflection_stamps(id).await?;
        let stored = reader.reflection_reconciles(id).await?;
        Ok(page_reconciles(&stamps, stored))
    }

    /// `write_reflection` with `action: reconciled`: record that the page
    /// at `inner` was brought in line with its pacts now, with `outcome`
    /// (`updated` or `unchanged`), the version of the queue item it settled
    /// and the pact hash the page has now ([`r::page_pact_hash`], computed
    /// here, never sent). A curator's (an agent of the reflect loop is one;
    /// an unattended run writes only into the domain it works on). The page
    /// must exist and not be retired: a retired page is never reconciled.
    /// The record lives beside the file; open tabs hear a change event and
    /// read the new state.
    ///
    /// A `version` of a `stale` item handed out before the reconcile
    /// recorded now is refused: that period was already reconciled.
    ///
    /// A version with a mark part (the item's set held a page marked not
    /// fresh) takes the marks it saw off: the page's and those of the pages
    /// it owns whose mark is at or before that part, each under its own
    /// lock (that write keeps the file's modified time). A later mark stays
    /// and is named in the receipt's `still_marked`. Such a version must be
    /// the one the queue hands out for the page now (its hash is rebuilt
    /// with its own period and mark part), or it is refused. A mark part
    /// newer than every mark left in the set (a marked page was retired or
    /// moved while the run worked) clears up to the newest mark still
    /// there, and nothing when none is left.
    pub(crate) async fn reconciled_reflection(
        &self,
        domain: &str,
        inner: &str,
        outcome: Option<&str>,
        version: Option<&str>,
        caller: &ReflectionCaller,
    ) -> Result<Value> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        let access = self
            .require_reflection_to_write(domain, caller, ReflectionRight::Curate)
            .await?;
        self.refuse_read_only_domain(domain, &crate::scope::Scope::Unrestricted)
            .await?;
        self.validate_named_reflection_path(domain, inner).await?;
        if !is_page(inner) {
            return Err(EngineError::Invalid(format!(
                "'{inner}' is no page: only a page is reconciled, never PACT.md or a page pact"
            )));
        }
        let outcome = outcome.and_then(ReconcileOutcome::parse).ok_or_else(|| {
            EngineError::Invalid(
                "reconciled needs outcome: updated (the reconcile changed the page) or unchanged"
                    .into(),
            )
        })?;
        let version = version.unwrap_or_default();
        // Under the page's lock, so the page checked is the page recorded.
        let abs = access.abs(inner);
        let _lock = self.lock_file(&abs).await;
        let text = read_opt(&abs)?.ok_or_else(|| {
            EngineError::NotFound(format!("no reflection '{inner}' in domain '{domain}'"))
        })?;
        if super::page_life::is_retired(&text, inner) {
            return Err(EngineError::Invalid(format!(
                "'{inner}' is retired, and a retired page is never reconciled; restore it first"
            )));
        }
        let rows = self
            .store
            .lock()
            .await
            .list_reflections(access.domain_id)
            .await?;
        // The page as it is on disk, which its row follows.
        let texts: Vec<(&str, &str)> = rows
            .iter()
            .filter(|row| row.path != inner)
            .map(|row| (row.path.as_str(), row.content.as_str()))
            .chain([(inner, text.as_str())])
            .collect();
        // Without a version a late call cannot be told from a current one:
        // while the page has a stale item of its own, its version is
        // required.
        if version.is_empty() {
            let states = self.reflection_reconciles(domain).await?;
            let now = self.automation_clock().stamp().unix_seconds();
            if super::reflect::has_own_stale_item(&texts, &states, inner, now) {
                let marked = super::mark_stale::text_mark(&text, inner).is_some();
                return Err(EngineError::Invalid(if marked {
                    format!(
                        "'{inner}' is marked not fresh and has a stale item in the reflect queue: send its version with reconciled, so the mark it saw is recorded"
                    )
                } else {
                    format!(
                        "'{inner}' has a stale item in the reflect queue: send its version with reconciled, so a late call for an older period can be told apart"
                    )
                }));
            }
        }
        // A late call for an older period: the `stale` item it answers was
        // handed out before the reconcile recorded now, so another run (or
        // this one, again) already brought the page in line. A version that
        // is no stale item's, and a page with no recorded reconcile, are
        // taken as sent.
        if let Some(base) = super::reflect::stale_base(version)
            && let Some(state) = self.reflection_reconcile(domain, inner).await?
            && state.outcome.is_some()
            && let Some(at) = time::parse_rfc3339(&state.last_reconciled)
            && base < at.stamp().unix_millis()
        {
            return Err(EngineError::Conflict(format!(
                "this reconcile answers an older period of '{inner}': the page was reconciled again at {} after that stale item was handed out, so nothing is recorded; read the reflect queue again for the page's current item",
                state.last_reconciled
            )));
        }
        self.refuse_owner_before_owned(domain, inner, &text, &rows)
            .await?;
        let pact_hash = r::page_pact_hash(inner, texts.iter().copied()).unwrap_or_default();
        // A version with a mark part clears marks, so it must be one the
        // queue handed out: rebuilt from the page as it is, with the
        // version's own period and mark part.
        if super::reflect::stale_mark(version).is_some() {
            let record = r::read_provenance(&text).page;
            let (settings, _) = r::read_page_settings(&text, inner, &record);
            let freshness = settings
                .freshness
                .as_ref()
                .map_or("", |freshness| freshness.written.as_str());
            let handed_out = self
                .reflection_reconcile(domain, inner)
                .await?
                .is_some_and(|state| {
                    super::reflect::marked_version_matches(version, freshness, &state, &pact_hash)
                });
            if !handed_out {
                return Err(EngineError::Invalid(format!(
                    "the version {version} is no version the reflect queue handed out for '{inner}' as it is now (its parts do not match the page's item, or the page's pacts or freshness changed since), so nothing is recorded; read the reflect queue again for the page's current item"
                )));
            }
        }
        // The page and the pages it owns: the set a stale item of the page
        // names, whose marks its version saw.
        let owned = r::owned_pages(texts.iter().copied());
        let set: Vec<String> = std::iter::once(inner.to_string())
            .chain(super::reflect::owned_tree(&owned, inner))
            .collect();
        let by_path: BTreeMap<&str, &str> = texts.iter().copied().collect();
        let newest = set
            .iter()
            .filter_map(|path| by_path.get(path.as_str()))
            .zip(set.iter())
            .filter_map(|(text, path)| super::mark_stale::text_mark(text, path))
            .map(|mark| mark.at_ms)
            .max();
        // The version is one the queue handed out (checked above), so a mark
        // part newer than every mark left in the set means marks left it
        // while the run worked (a page retired or moved): clamp to the
        // newest mark still there, and clear nothing when none is left. A
        // mark set after the set was read is later than that and stays.
        let seen = super::reflect::stale_mark(version)
            .and_then(|sent| newest.map(|newest| sent.min(newest)));
        let state = self
            .record_reflection_reconcile(
                domain,
                access.domain_id,
                inner,
                outcome,
                &pact_hash,
                version,
            )
            .await?;
        // The marks the item saw come off: never one later than the mark
        // part, which the check above holds at or below the newest mark.
        let mut cleared_here = false;
        if let Some(up_to) = seen {
            match self
                .clear_seen_mark(domain, &access, inner, &abs, &text, up_to, caller)
                .await
            {
                Ok(cleared) => cleared_here = cleared,
                Err(e) => tracing::warn!(
                    domain,
                    path = inner,
                    "the not-fresh mark the reconcile saw was not taken off: {e}"
                ),
            }
        }
        // A row left behind its file (a bookkeeping write whose row commit
        // was lost) is landed here, or the queue would hand out the old
        // mark for ever.
        if !cleared_here && let Ok(Some(now)) = read_opt(&abs) {
            self.land_row_from_file(domain, &access, inner, &abs, &now)
                .await;
        }
        drop(_lock);
        if cleared_here {
            self.absorb_into_room(domain, inner).await;
        }
        for child in set.iter().skip(1) {
            let abs = access.abs(child);
            let lock = self.lock_file(&abs).await;
            let cleared = match (seen, read_opt(&abs)) {
                (Some(up_to), Ok(Some(text)))
                    if !super::page_life::is_retired(&text, child) =>
                {
                    self.clear_seen_mark(domain, &access, child, &abs, &text, up_to, caller)
                        .await
                }
                (_, Ok(_)) => Ok(false),
                (_, Err(e)) => Err(e),
            };
            if matches!(cleared, Ok(false))
                && let Ok(Some(now)) = read_opt(&abs)
            {
                self.land_row_from_file(domain, &access, child, &abs, &now)
                    .await;
            }
            drop(lock);
            match cleared {
                Ok(true) => self.absorb_into_room(domain, child).await,
                Ok(false) => {}
                Err(e) => tracing::warn!(
                    domain,
                    path = %child,
                    "the not-fresh mark the owner's reconcile saw was not taken off: {e}"
                ),
            }
        }
        self.announce_reflection(
            domain,
            inner,
            ReflectionChangeKind::Changed,
            None,
            crate::changes::change_label(&caller.scope),
        );
        // What is still marked after this record: marks set while the run
        // worked, or ones a version without a mark part never saw.
        let mut still_marked = Vec::new();
        let mut own_mark = Value::Null;
        if !version.is_empty() {
            for path in &set {
                let Ok(Some(text)) = read_opt(&access.abs(path)) else {
                    continue;
                };
                if let Some(mark) = super::mark_stale::text_mark(&text, path) {
                    if path == inner {
                        own_mark = mark.to_json();
                    }
                    let mut entry = mark.to_json();
                    entry["path"] = json!(path);
                    still_marked.push(entry);
                }
            }
        } else if let Ok(Some(text)) = read_opt(&abs) {
            own_mark = super::mark_stale::text_mark(&text, inner)
                .map_or(Value::Null, |mark| mark.to_json());
        }
        Ok(json!({
            "domain": domain,
            "path": inner,
            "outcome": outcome.as_str(),
            "last_reconciled": state.last_reconciled,
            "pact_hash": state.pact_hash,
            "version": state.version,
            "refresh_requested": own_mark,
            "still_marked": still_marked,
        }))
    }

    /// Land the row of the page at `inner` from its file text `text` (read
    /// under its lock, which the caller holds) when the stored row holds
    /// other text. A bookkeeping write keeps the file's modified time, so
    /// when its row commit was lost and the text kept its length, the
    /// sync's stat check never reads the file again; the reconcile, which
    /// reads the file anyway, puts the row right. A failure is logged: the
    /// next reconcile tries again.
    async fn land_row_from_file(
        &self,
        domain: &str,
        access: &super::reflection::ReflectionAccess,
        inner: &str,
        abs: &std::path::Path,
        text: &str,
    ) {
        let stored = self
            .store
            .lock()
            .await
            .get_reflection(access.domain_id, inner)
            .await;
        let behind = match stored {
            Ok(row) => row.is_none_or(|row| row.content != text),
            Err(e) => {
                tracing::warn!(domain, path = inner, "the page row was not read: {e}");
                return;
            }
        };
        if !behind {
            return;
        }
        let ops = [super::fence::RowOp::Page { inner, text, abs }];
        match self
            .commit_reflection_rows(
                access.domain_id,
                &ops,
                super::fence::OnRefusal::read_heal(domain),
            )
            .await
        {
            Ok(()) => tracing::debug!(domain, path = inner, "landed a page row behind its file"),
            Err(e) => tracing::warn!(
                domain,
                path = inner,
                "the page row behind its file was not landed: {e}"
            ),
        }
    }

    /// Take the not-fresh mark off the page at `inner` (its text `text`,
    /// under its lock, which the caller holds) when it is at or before
    /// `up_to` (milliseconds since the epoch): the reconcile that saw it
    /// was recorded. The file keeps its modified time. Answers whether
    /// anything was written.
    #[allow(clippy::too_many_arguments)]
    async fn clear_seen_mark(
        &self,
        domain: &str,
        access: &super::reflection::ReflectionAccess,
        inner: &str,
        abs: &std::path::Path,
        text: &str,
        up_to: i64,
        caller: &ReflectionCaller,
    ) -> Result<bool> {
        let mut prov = r::read_provenance(text);
        if !prov.page.mark_millis().is_some_and(|at| at <= up_to) {
            return Ok(false);
        }
        prov.page.clear_mark();
        let changed = r::write_provenance(text, &prov).map_err(EngineError::Invalid)?;
        self.land_page_record(domain, access, inner, abs, text, &changed, caller, true)
            .await?;
        Ok(true)
    }

    /// Refuse the `reconciled` of the page at `inner` (its text `text`, the
    /// Reflection's rows `rows`) while it is due (its freshness ran out, or
    /// it is marked not fresh) and a page it owns (its owned pages' owned
    /// pages included, a retired one left out) has no reconcile recorded
    /// since the later of the owner's last one, the owner's mark and its
    /// own mark: owned pages first, the owner last, so a run that stops at
    /// its cap never leaves owned pages behind for a whole period (design
    /// 3.5). An owner that is not due is reconciled whatever its owned
    /// pages are.
    async fn refuse_owner_before_owned(
        &self,
        domain: &str,
        inner: &str,
        text: &str,
        rows: &[crystalline_index::ReflectionRow],
    ) -> Result<()> {
        let record = r::read_provenance(text).page;
        let (settings, _) = r::read_page_settings(text, inner, &record);
        let mark = super::mark_stale::text_mark(text, inner).map(|mark| mark.at_ms);
        if settings.freshness.is_none() && mark.is_none() {
            return Ok(());
        }
        let states = self.reflection_reconciles(domain).await?;
        let Some(since) = states
            .get(inner)
            .and_then(|state| time::parse_rfc3339(&state.last_reconciled))
            .map(|at| at.stamp())
        else {
            return Ok(());
        };
        let now = self.automation_clock().stamp().unix_seconds();
        let ran_out = settings
            .freshness
            .is_some_and(|freshness| freshness.timing(since.unix_seconds(), now, &[]).stale());
        if !ran_out && mark.is_none() {
            return Ok(());
        }
        let since_ms = mark.map_or(since.unix_millis(), |mark| mark.max(since.unix_millis()));
        let texts: BTreeMap<&str, &str> = rows
            .iter()
            .map(|row| (row.path.as_str(), row.content.as_str()))
            .collect();
        let owned = r::owned_pages(texts.iter().map(|(p, t)| (*p, *t)));
        let mut waiting: Vec<String> = Vec::new();
        let mut next = vec![inner.to_string()];
        let mut seen: Vec<String> = vec![inner.to_string()];
        while let Some(owner) = next.pop() {
            for child in owned.get(&owner).into_iter().flatten() {
                if seen.contains(child) {
                    continue;
                }
                seen.push(child.clone());
                next.push(child.clone());
                if texts
                    .get(child.as_str())
                    .is_some_and(|text| super::page_life::is_retired(text, child))
                {
                    continue;
                }
                let child_mark = texts
                    .get(child.as_str())
                    .and_then(|text| super::mark_stale::text_mark(text, child))
                    .map(|mark| mark.at_ms);
                let done = super::reflect::owned_done(states.get(child), since_ms, child_mark);
                if !done {
                    waiting.push(child.clone());
                }
            }
        }
        if waiting.is_empty() {
            return Ok(());
        }
        waiting.sort();
        Err(EngineError::Invalid(format!(
            "'{inner}' is due and owns pages not reconciled in this period: {}. Reconcile each of them first (write_reflection action=reconciled), then '{inner}'",
            waiting.join(", ")
        )))
    }

    /// Record a reconcile of the page at `page` now, behind the host-lock
    /// fence. No rights are checked here: the caller has checked them, and
    /// that the page exists.
    pub(crate) async fn record_reflection_reconcile(
        &self,
        domain: &str,
        domain_id: DomainId,
        page: &str,
        outcome: ReconcileOutcome,
        pact_hash: &str,
        version: &str,
    ) -> Result<PageReconcile> {
        let state = ReflectionReconcile {
            path: page.to_string(),
            last_reconciled: time::now().rfc3339(),
            outcome: outcome.as_str().to_string(),
            pact_hash: pact_hash.to_string(),
            version: version.to_string(),
        };
        self.commit_reflection_rows(
            domain_id,
            &[RowOp::Reconcile(&state)],
            OnRefusal::put_back(domain, DiskUndo::default()),
        )
        .await?;
        Ok(PageReconcile {
            last_reconciled: state.last_reconciled,
            outcome: Some(outcome),
            pact_hash: Some(state.pact_hash),
            version: Some(state.version),
        })
    }

    /// [`Engine::record_reflection_reconcile`] by domain name, for tests:
    /// the action that records a reconcile comes with its own gates.
    #[cfg(any(test, feature = "testing"))]
    pub async fn record_reflection_reconcile_for_tests(
        &self,
        domain: &str,
        page: &str,
        outcome: ReconcileOutcome,
        pact_hash: &str,
        version: &str,
    ) -> Result<PageReconcile> {
        let (domain_id, _) = self.domain_source(domain).await?;
        self.record_reflection_reconcile(domain, domain_id, page, outcome, pact_hash, version)
            .await
    }
}
