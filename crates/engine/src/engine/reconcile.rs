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
        let mut stored: BTreeMap<String, ReflectionReconcile> = reader
            .reflection_reconciles(id)
            .await?
            .into_iter()
            .map(|state| (state.path.clone(), state))
            .collect();
        Ok(stamps
            .into_iter()
            .filter(|stamp| is_page(&stamp.path))
            .map(|stamp| {
                let state = match stored.remove(&stamp.path) {
                    Some(state) => PageReconcile::read(state, &stamp.modified),
                    None => PageReconcile::seeded(&stamp.modified),
                };
                (stamp.path, state)
            })
            .collect())
    }

    /// Record a reconcile of the page at `page` now, behind the host-lock
    /// fence. No rights are checked here: the caller has checked them, and
    /// that the page exists.
    // Only the tests call it until the `reconciled` action of
    // `write_reflection` does.
    #[cfg_attr(not(any(test, feature = "testing")), allow(dead_code))]
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
