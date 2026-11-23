//! The freshness clocks of a Reflection's pages (design 2.2 and 3.5): when
//! each page that keeps itself current was last checked, when it falls due
//! and whether it is overdue, on the automation clock.
//!
//! Read through the reader and never the writer lock, and without the due
//! work, so a status asked with `counts: false` (the poll a runner makes to
//! know when to come back) answers them too. A page's freshness and status
//! live in its text; the page stamps carry its checksum, so the text is
//! fetched only for a page whose checksum moved since the last read
//! ([`FactsMemo`]).

use std::collections::{BTreeMap, HashMap};

use crystalline_core::reflection as r;
use crystalline_index::{DomainId, ReadStore, ReflectionStamp};

use super::reconcile::{PageReconcile, page_reconciles};
use super::*;

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

/// The clocks of `stamps` read with `facts` and their reconcile `states` at
/// `now` (seconds since the epoch), by path. A page whose last reconcile
/// time does not read has none.
fn clocks(
    stamps: &[ReflectionStamp],
    facts: &HashMap<String, (String, PageFacts)>,
    states: &BTreeMap<String, PageReconcile>,
    now: i64,
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
            timing: freshness.timing(at.stamp().unix_seconds(), now),
        });
    }
    out
}

/// The clocks of every page of the Reflection `id` in `store` at `now`
/// (seconds since the epoch), reading every page's text: what `crystalline
/// doctor` asks of an index it opened itself.
pub async fn page_clocks_in(
    store: &dyn ReadStore,
    id: DomainId,
    now: i64,
) -> crystalline_index::Result<Vec<PageClock>> {
    let rows = store.list_reflections(id).await?;
    let stamps: Vec<ReflectionStamp> = rows
        .iter()
        .map(|row| ReflectionStamp {
            path: row.path.clone(),
            title: row.title.clone(),
            sha256: row.sha256.clone(),
            size: row.size,
            modified: row.modified.clone(),
        })
        .collect();
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
    Ok(clocks(&stamps, &facts, &states, now))
}

impl Engine {
    /// The clocks of every page of `domain`'s Reflection that keeps itself
    /// current (a freshness, not retired), on the automation clock, read
    /// through the reader. Empty for a domain with no index row. No rights
    /// are checked here.
    pub(crate) async fn page_clocks(&self, domain: &str) -> Result<Vec<PageClock>> {
        let reader = self.reader();
        let Some(id) = reader.domain_id(domain).await? else {
            return Ok(Vec::new());
        };
        let stamps = reader.list_reflection_stamps(id).await?;
        let known: HashMap<String, (String, PageFacts)> = self
            .page_facts
            .lock()
            .unwrap()
            .get(domain)
            .cloned()
            .unwrap_or_default();
        let mut facts = HashMap::with_capacity(stamps.len());
        for stamp in &stamps {
            if !matches!(r::classify(&stamp.path), r::ReflectionFile::Page(_)) {
                continue;
            }
            match known.get(&stamp.path) {
                Some(seen) if seen.0 == stamp.sha256 => {
                    facts.insert(stamp.path.clone(), seen.clone());
                }
                _ => {
                    // A page gone between the two reads is left out.
                    if let Some(row) = reader.get_reflection(id, &stamp.path).await? {
                        let page = PageFacts::read(&row.path, &row.content);
                        facts.insert(stamp.path.clone(), (row.sha256, page));
                    }
                }
            }
        }
        let states = page_reconciles(&stamps, reader.reflection_reconciles(id).await?);
        let now = self.automation_clock().stamp().unix_seconds();
        let out = clocks(&stamps, &facts, &states, now);
        self.page_facts
            .lock()
            .unwrap()
            .insert(domain.to_string(), facts);
        Ok(out)
    }

    /// When the next page of `domain`'s Reflection falls due or enters its
    /// next period, in seconds from the automation clock (always more than
    /// 0), with that instant in RFC 3339; `None` when no page keeps itself
    /// current. A page already due names the start of its next period, so
    /// a page left for a person or open in a live room never makes the
    /// answer now.
    pub(crate) async fn next_stale(&self, domain: &str) -> Result<Option<(i64, String)>> {
        let soonest = self
            .page_clocks(domain)
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
}
