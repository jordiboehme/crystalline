//! The freshness clocks of a Reflection's pages (design 2.2 and 3.5): when
//! each page that keeps itself current was last checked, when it falls due
//! and whether it is overdue, on the automation clock. On the page it is
//! overdue one whole period after it fell due; a reflect run missed it once
//! a run of its domain started after it fell due and ended without
//! reconciling it (Jordi, 2026-11-23). Only a missed page is warned about.
//!
//! Read through the reader and never the writer lock, and without the due
//! work, so a status asked with `counts: false` (the poll a runner makes to
//! know when to come back) answers them too. A page's freshness and status
//! live in its text; the page stamps carry its checksum, so the text is
//! fetched only for a page whose checksum moved since the last read
//! ([`FactsMemo`]), and in one bulk read when nothing is remembered.

use std::collections::{BTreeMap, HashMap};

use crystalline_core::automation::{Loop, RUNS_KEPT};
use crystalline_core::reflection as r;
use crystalline_index::{AutomationRun, DomainId, ReadStore, ReflectionRow, ReflectionStamp};

use super::reconcile::{PageReconcile, page_reconciles};
use super::*;
use crate::scope::{ReflectionCaller, ReflectionRight};

/// What the clocks read of one page's text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PageFacts {
    /// Its freshness; `None` when it has none, it does not read or the page
    /// is owned (an owned page is reconciled with its owner).
    freshness: Option<r::Freshness>,
    /// Whether its status is a retired word: never reconciled.
    retired: bool,
}

impl PageFacts {
    fn read(path: &str, text: &str) -> PageFacts {
        let record = r::read_provenance(text).page;
        let (settings, _) = r::read_page_settings(text, path, &record);
        PageFacts {
            retired: settings.retired(),
            freshness: settings.freshness,
        }
    }
}

/// [`PageFacts`] by domain, then page path, each with the checksum of the
/// text it was read from.
pub(crate) type FactsMemo = std::sync::Mutex<HashMap<String, HashMap<String, (String, PageFacts)>>>;

/// One page's clock: a page with a freshness that is not retired.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageClock {
    /// The page's path inside `reflections/`.
    pub path: String,
    /// Its freshness as read.
    pub freshness: r::Freshness,
    /// When it was last reconciled, RFC 3339 (the row's modified time when
    /// none is recorded).
    pub last_reconciled: String,
    /// Where it stands on the automation clock.
    pub timing: r::PageTiming,
}

/// More pages whose checksum moved than this, and the texts come in one
/// bulk read instead of one read per page.
const BULK_AFTER: usize = 8;

/// The finished reflect runs of a domain as [`r::RunSpan`]s; a run whose
/// times do not read is left out.
fn run_spans(runs: &[AutomationRun]) -> Vec<r::RunSpan> {
    let secs =
        |at: &str| crystalline_core::time::parse_rfc3339(at).map(|t| t.stamp().unix_seconds());
    runs.iter()
        .filter_map(|run| {
            Some(r::RunSpan {
                started: secs(&run.started_at)?,
                ended: secs(&run.ended_at)?,
            })
        })
        .collect()
}

/// The finished reflect runs of the domain `id` in `store` (a run still
/// going holds a lease and has no record yet).
async fn reflect_runs(
    store: &dyn ReadStore,
    id: DomainId,
) -> crystalline_index::Result<Vec<r::RunSpan>> {
    Ok(run_spans(
        &store
            .automation_runs(id, Loop::Reflect.as_str(), RUNS_KEPT)
            .await?,
    ))
}

/// A row without its text.
fn stamp_of(row: &ReflectionRow) -> ReflectionStamp {
    ReflectionStamp {
        path: row.path.clone(),
        title: row.title.clone(),
        sha256: row.sha256.clone(),
        size: row.size,
        modified: row.modified.clone(),
    }
}

/// The clocks of `stamps` read with `facts` and their reconcile `states` at
/// `now` (seconds since the epoch), with the domain's finished reflect
/// `runs`, by path. A page whose last reconcile time does not read has
/// none.
fn clocks(
    stamps: &[ReflectionStamp],
    facts: &HashMap<String, (String, PageFacts)>,
    states: &BTreeMap<String, PageReconcile>,
    now: i64,
    runs: &[r::RunSpan],
) -> Vec<PageClock> {
    let mut out = Vec::new();
    for stamp in stamps {
        let Some((_, page)) = facts.get(&stamp.path) else {
            continue;
        };
        let (Some(freshness), false) = (&page.freshness, page.retired) else {
            continue;
        };
        let Some(state) = states.get(&stamp.path) else {
            continue;
        };
        let Some(at) = crystalline_core::time::parse_rfc3339(&state.last_reconciled) else {
            continue;
        };
        out.push(PageClock {
            path: stamp.path.clone(),
            freshness: freshness.clone(),
            last_reconciled: state.last_reconciled.clone(),
            timing: freshness.timing(at.stamp().unix_seconds(), now, runs),
        });
    }
    out
}

/// The clocks of every page of the Reflection `id` in `store` at `now`
/// (seconds since the epoch), reading every page's text and the domain's
/// reflect runs: what `crystalline doctor` asks of an index it opened
/// itself.
pub async fn page_clocks_in(
    store: &dyn ReadStore,
    id: DomainId,
    now: i64,
) -> crystalline_index::Result<Vec<PageClock>> {
    let rows = store.list_reflections(id).await?;
    let stamps: Vec<ReflectionStamp> = rows.iter().map(stamp_of).collect();
    let facts = rows
        .iter()
        .map(|row| {
            (
                row.path.clone(),
                (row.sha256.clone(), PageFacts::read(&row.path, &row.content)),
            )
        })
        .collect();
    let states = page_reconciles(&stamps, store.reflection_reconciles(id).await?);
    let runs = reflect_runs(store, id).await?;
    Ok(clocks(&stamps, &facts, &states, now, &runs))
}

impl Engine {
    /// The clocks of every page of `domain`'s Reflection that keeps itself
    /// current (a freshness, not retired), on the automation clock, read
    /// through the reader. With `runs` the domain's reflect runs are read
    /// too, so each clock says whether its page is overdue; without, no
    /// page is. Empty for a domain with no index row. No rights are checked
    /// here.
    pub(crate) async fn page_clocks(&self, domain: &str, runs: bool) -> Result<Vec<PageClock>> {
        let reader = self.reader();
        let Some(id) = reader.domain_id(domain).await? else {
            return Ok(Vec::new());
        };
        let stamps = reader.list_reflection_stamps(id).await?;
        let known: Option<HashMap<String, (String, PageFacts)>> =
            self.page_facts.lock().unwrap().get(domain).cloned();
        let page = |stamp: &&ReflectionStamp| {
            matches!(r::classify(&stamp.path), r::ReflectionFile::Page(_))
        };
        let moved: Vec<&ReflectionStamp> = stamps
            .iter()
            .filter(page)
            .filter(|stamp| {
                known
                    .as_ref()
                    .and_then(|known| known.get(&stamp.path))
                    .is_none_or(|seen| seen.0 != stamp.sha256)
            })
            .collect();
        let mut facts: HashMap<String, (String, PageFacts)> = known.unwrap_or_default();
        facts.retain(|path, _| stamps.iter().any(|stamp| &stamp.path == path));
        if moved.len() > BULK_AFTER || (facts.is_empty() && !moved.is_empty()) {
            // Nothing remembered, or much moved: one read of every text.
            self.count_page_text_read();
            for row in reader.list_reflections(id).await? {
                if matches!(r::classify(&row.path), r::ReflectionFile::Page(_)) {
                    let page = PageFacts::read(&row.path, &row.content);
                    facts.insert(row.path, (row.sha256, page));
                }
            }
        } else {
            for stamp in moved {
                // A page gone between the two reads is left out.
                self.count_page_text_read();
                if let Some(row) = reader.get_reflection(id, &stamp.path).await? {
                    let page = PageFacts::read(&row.path, &row.content);
                    facts.insert(stamp.path.clone(), (row.sha256, page));
                }
            }
        }
        let states = page_reconciles(&stamps, reader.reflection_reconciles(id).await?);
        let runs = if runs {
            reflect_runs(reader, id).await?
        } else {
            Vec::new()
        };
        let now = self.automation_clock().stamp().unix_seconds();
        let out = clocks(&stamps, &facts, &states, now, &runs);
        self.page_facts
            .lock()
            .unwrap()
            .insert(domain.to_string(), facts);
        Ok(out)
    }

    /// The clock of the one page at `path` of `domain`'s Reflection, as
    /// [`Engine::page_clocks`] reads it with runs, without reading the
    /// other pages. `None` when the page has no clock or is missing.
    async fn page_clock(&self, domain: &str, path: &str) -> Result<Option<PageClock>> {
        if !matches!(r::classify(path), r::ReflectionFile::Page(_)) {
            return Ok(None);
        }
        let reader = self.reader();
        let Some(id) = reader.domain_id(domain).await? else {
            return Ok(None);
        };
        let Some(row) = reader.get_reflection(id, path).await? else {
            return Ok(None);
        };
        let stamps = [stamp_of(&row)];
        let facts = HashMap::from([(
            row.path.clone(),
            (row.sha256.clone(), PageFacts::read(&row.path, &row.content)),
        )]);
        let stored = reader.reflection_reconcile(id, path).await?;
        let states = page_reconciles(&stamps, stored.into_iter().collect());
        let runs = reflect_runs(reader, id).await?;
        let now = self.automation_clock().stamp().unix_seconds();
        Ok(clocks(&stamps, &facts, &states, now, &runs).pop())
    }

    /// Count one read of page texts for the tests
    /// (`Engine::page_text_reads`).
    fn count_page_text_read(&self) {
        #[cfg(any(test, feature = "testing"))]
        self.page_text_reads
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    /// When the next page of `domain`'s Reflection falls due or enters its
    /// next period, in seconds from the automation clock (always more than
    /// 0), with that instant in RFC 3339; `None` when no page keeps itself
    /// current. A page already due names the start of its next period, so
    /// a page left for a person or open in a live room never makes the
    /// answer now.
    pub(crate) async fn next_stale(&self, domain: &str) -> Result<Option<(i64, String)>> {
        let soonest = self
            .page_clocks(domain, false)
            .await?
            .iter()
            .map(|clock| clock.timing.next_due_in_secs)
            .min();
        Ok(soonest.map(|secs| {
            let at = self
                .automation_clock()
                .stamp()
                .saturating_add(time::saturating_seconds(secs));
            (secs, at.rfc3339_secs_z())
        }))
    }

    /// Whether a person has the page at `path` of `domain` open in a live
    /// editor: its update items wait for the room, so a due page shows
    /// "Waiting: open in the editor" instead of its clock (design 2.2).
    async fn waits_for_the_room(&self, domain: &str, path: &str) -> bool {
        match self.collab_rooms() {
            Some(rooms) => {
                rooms
                    .has_live_doc(
                        domain,
                        &crate::collab::session::RoomDoc::Reflection(path.to_string()),
                        None,
                    )
                    .await
            }
            None => false,
        }
    }

    /// The clock of the page at `path` of `domain`'s Reflection for its
    /// status line, in seconds relative to the automation clock (design
    /// 2.2): `{path, freshness, last_reconciled, last_checked_secs,
    /// due_in_secs, stale, overdue_secs, missed_secs,
    /// loop_slower_than_freshness, waiting}`.
    ///
    /// - `due_in_secs`: the time until a fresh page falls due, null once it
    ///   is due.
    /// - `overdue_secs`: the time since it turned overdue, one whole period
    ///   after it fell due; null before.
    /// - `missed_secs`: the time since the end of the first reflect run of
    ///   the domain that started after the page fell due and left it; null
    ///   while no such run has ended.
    /// - `loop_slower_than_freshness`: overdue, runs are known, and none
    ///   missed it: the loop runs less often than the page's freshness.
    /// - `waiting`: a due page waits for a live editor that has it open.
    ///
    /// - `part_of`: `{path, title}` of the page that owns this one, null
    ///   for a page on its own (design 2.3).
    /// - `clock_of`: the page whose freshness keeps this one current: the
    ///   page itself, or for an owned page the first page up its owners
    ///   that has a freshness; null when none has.
    ///
    /// An owned page is reconciled with its owner and never due on its own:
    /// it answers that page's `freshness`, `last_reconciled` and
    /// `last_checked_secs` (null when no owner up the way has a freshness),
    /// `due_in_secs`, `overdue_secs` and `missed_secs` null and `stale`,
    /// `loop_slower_than_freshness` and `waiting` false.
    ///
    /// Null for a page on its own with no freshness, a retired page and a
    /// missing one. Needs the right to read the Reflection.
    pub async fn reflection_page_clock(
        &self,
        domain: &str,
        path: &str,
        caller: &ReflectionCaller,
    ) -> Result<Value> {
        self.require_reflection(domain, caller, ReflectionRight::Read)
            .await?;
        if let Some(clock) = self.page_clock(domain, path).await? {
            let waiting = clock.timing.stale() && self.waits_for_the_room(domain, path).await;
            return Ok(json!({
                "path": clock.path,
                "freshness": clock.freshness.written,
                "last_reconciled": clock.last_reconciled,
                "last_checked_secs": clock.timing.last_checked_secs,
                "due_in_secs": (!clock.timing.stale()).then_some(clock.timing.next_due_in_secs),
                "stale": clock.timing.stale(),
                "overdue_secs": clock.timing.overdue_secs,
                "missed_secs": clock.timing.missed_secs,
                "loop_slower_than_freshness": clock.timing.loop_slower_than_freshness,
                "waiting": waiting,
                "part_of": null,
                "clock_of": clock.path,
            }));
        }
        let Some((part_of, kept_by)) = self.owners_clock(domain, path).await? else {
            return Ok(Value::Null);
        };
        Ok(json!({
            "path": path,
            "freshness": kept_by.as_ref().map(|c| c.freshness.written.clone()),
            "last_reconciled": kept_by.as_ref().map(|c| c.last_reconciled.clone()),
            "last_checked_secs": kept_by.as_ref().map(|c| c.timing.last_checked_secs),
            "due_in_secs": null,
            "stale": false,
            "overdue_secs": null,
            "missed_secs": null,
            "loop_slower_than_freshness": false,
            "waiting": false,
            "part_of": part_of,
            "clock_of": kept_by.as_ref().map(|c| c.path.clone()),
        }))
    }

    /// For the owned page at `path` of `domain`'s Reflection: `{path,
    /// title}` of its owner, and the clock of the first page up its owners
    /// that has one. `None` for a page that is missing, retired or owned by
    /// no page.
    async fn owners_clock(
        &self,
        domain: &str,
        path: &str,
    ) -> Result<Option<(Value, Option<PageClock>)>> {
        let reader = self.reader();
        let Some(id) = reader.domain_id(domain).await? else {
            return Ok(None);
        };
        let owner_of = |text: &str| r::read_provenance(text).page.owner;
        let Some(row) = reader.get_reflection(id, path).await? else {
            return Ok(None);
        };
        // A retired page is never reconciled, whatever its record says.
        if PageFacts::read(path, &row.content).retired {
            return Ok(None);
        }
        let Some(owner) = owner_of(&row.content) else {
            return Ok(None);
        };
        let title = match reader.get_reflection(id, &owner).await? {
            Some(row) => row.title,
            None => owner.clone(),
        };
        let part_of = json!({ "path": owner, "title": title });
        // Up the owners until one keeps a clock; a loop written by hand
        // ends where it comes back.
        let mut seen = std::collections::BTreeSet::from([path.to_string()]);
        let mut next = Some(owner);
        while let Some(page) = next.take() {
            if !seen.insert(page.clone()) {
                break;
            }
            if let Some(clock) = self.page_clock(domain, &page).await? {
                return Ok(Some((part_of, Some(clock))));
            }
            next = match reader.get_reflection(id, &page).await? {
                Some(row) => owner_of(&row.content),
                None => None,
            };
        }
        Ok(Some((part_of, None)))
    }

    /// Every page of the Reflections served here that a reflect run missed
    /// (it started after the page fell due and ended without reconciling
    /// it) and that `caller` may read, longest missed first, for the admin
    /// banner, the maintenance view and `crystalline doctor`: `{pages:
    /// [{domain, path, freshness, last_checked_secs, missed_secs,
    /// waiting}]}`, the times in seconds relative to the automation clock.
    /// A page only overdue because the loop runs less often than its
    /// freshness is not here: the loop did not fail it. A domain whose
    /// pages cannot be read is left out with a warning: this is a report,
    /// never a failure.
    pub async fn overdue_reflection_pages(&self, caller: &ReflectionCaller) -> Result<Value> {
        let mut pages = Vec::new();
        for domain in self.reflections_here().await? {
            if self
                .require_reflection(&domain, caller, ReflectionRight::Read)
                .await
                .is_err()
            {
                continue;
            }
            let clocks = match self.page_clocks(&domain, true).await {
                Ok(clocks) => clocks,
                Err(e) => {
                    tracing::warn!("the page clocks of '{domain}' could not be read: {e}");
                    continue;
                }
            };
            for clock in clocks {
                let Some(missed) = clock.timing.missed_secs else {
                    continue;
                };
                let waiting = self.waits_for_the_room(&domain, &clock.path).await;
                pages.push((
                    missed,
                    json!({
                        "domain": domain,
                        "path": clock.path,
                        "freshness": clock.freshness.written,
                        "last_checked_secs": clock.timing.last_checked_secs,
                        "missed_secs": missed,
                        "waiting": waiting,
                    }),
                ));
            }
        }
        pages.sort_by_key(|page| std::cmp::Reverse(page.0));
        Ok(json!({ "pages": pages.into_iter().map(|(_, page)| page).collect::<Vec<_>>() }))
    }
}
