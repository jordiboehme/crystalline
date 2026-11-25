//! The reflect queue's items about pages that keep themselves current
//! (design 2.1 to 2.4 and 3.4): a page whose freshness ran out (`stale`), a
//! changed pact a page follows (`page_pact_changed`), a page whose owner is
//! gone or retired (`owner_gone`), and links to a retired or removed page
//! (`link_retired`, `link_broken`). Everything here is read from the rows
//! [`Engine::detect_reflection`] already loaded and the reconcile states
//! read once for the whole domain.

use std::collections::{BTreeMap, BTreeSet};

use crystalline_core::reflection as r;
use serde_json::{Value, json};

use super::{Item, item_basis};
use crate::engine::mark_stale::{Mark, page_mark};
use crate::engine::reconcile::PageReconcile;

/// What an `owner_gone` item says to do.
pub const OWNER_GONE_HINT: &str = "The page that owned this page is gone or retired, so no reconcile keeps it current any more. Retire it (write_reflection action=retire), or keep it as a page of its own: retire it, then restore it without owner.";

/// What the queue knows about the pages of a Reflection before it walks
/// them: each page's settings and record, which are retired, what each
/// follows and owns, and the pact files there are.
pub(super) struct PageView {
    /// Every page (not `PACT.md`, no page pact) by path: its settings, as
    /// read, and its page record.
    pub(super) pages: BTreeMap<String, (r::PageSettings, r::PageRecord)>,
    /// The pages whose `status` is a retired word: never reconciled, never
    /// covering an engram, exempt from the structure.
    pub(super) retired: BTreeSet<String>,
    /// What each page follows, for the pact chains.
    links: BTreeMap<String, r::PageLinks>,
    /// The pact files there are, `PACT.md` among them when it is there.
    pact_files: BTreeSet<String>,
    /// The pages each page owns.
    owned: BTreeMap<String, Vec<String>>,
    /// The pages marked not fresh (worked pages only), with their marks.
    marks: BTreeMap<String, Mark>,
}

impl PageView {
    /// Read the pages of `rows` (path, text).
    pub(super) fn read(rows: &[(&str, &str)]) -> PageView {
        let mut pages = BTreeMap::new();
        let mut retired = BTreeSet::new();
        let mut links = BTreeMap::new();
        let mut pact_files = BTreeSet::new();
        for &(path, text) in rows {
            if r::is_pact_file(path) {
                pact_files.insert(path.to_string());
                continue;
            }
            if !matches!(r::classify(path), r::ReflectionFile::Page(_)) {
                continue;
            }
            let record = r::read_provenance(text).page;
            let (settings, _) = r::read_page_settings(text, path, &record);
            if settings.retired() {
                retired.insert(path.to_string());
            }
            links.insert(path.to_string(), settings.links(&record));
            pages.insert(path.to_string(), (settings, record));
        }
        let marks = pages
            .iter()
            .filter(|(path, _)| !retired.contains(*path))
            .filter_map(|(path, (_, record))| Some((path.clone(), page_mark(record)?)))
            .collect();
        PageView {
            pages,
            retired,
            links,
            pact_files,
            owned: r::owned_pages(rows.iter().copied()),
            marks,
        }
    }

    /// The not-fresh mark of the worked page at `path`, when it carries one.
    pub(super) fn mark(&self, path: &str) -> Option<&Mark> {
        self.marks.get(path)
    }

    /// Whether the worked page at `path` is due at `now` (seconds since the
    /// epoch): its freshness ran out, or it is marked not fresh.
    fn due(&self, states: &BTreeMap<String, PageReconcile>, path: &str, now: i64) -> bool {
        self.worked(path)
            && (self.marks.contains_key(path) || freshness_ran_out(self, states, path, now))
    }

    /// Whether a page up the owners of `path` is due: then `path` is part
    /// of that page's `stale` item, not one of its own. The walk stops at a
    /// missing or retired owner (it owns nothing) and where a loop comes
    /// back.
    fn owner_due(&self, states: &BTreeMap<String, PageReconcile>, path: &str, now: i64) -> bool {
        let mut seen = BTreeSet::from([path.to_string()]);
        let mut next = self
            .pages
            .get(path)
            .and_then(|(_, record)| record.owner.clone());
        while let Some(owner) = next.take() {
            if !seen.insert(owner.clone()) || !self.worked(&owner) {
                return false;
            }
            if self.due(states, &owner, now) {
                return true;
            }
            next = self
                .pages
                .get(&owner)
                .and_then(|(_, record)| record.owner.clone());
        }
        false
    }

    /// Whether the page at `path` has a `stale` item of its own at `now`:
    /// it is due and no page up its owners is.
    pub(super) fn own_item(
        &self,
        states: &BTreeMap<String, PageReconcile>,
        path: &str,
        now: i64,
    ) -> bool {
        self.due(states, path, now) && !self.owner_due(states, path, now)
    }

    /// Whether `path` is a page that is not retired: one that is worked.
    pub(super) fn worked(&self, path: &str) -> bool {
        self.pages.contains_key(path) && !self.retired.contains(path)
    }

    /// Whether the page at `path` is owned by another page.
    pub(super) fn is_owned(&self, path: &str) -> bool {
        self.pages
            .get(path)
            .is_some_and(|(_, record)| record.owner.is_some())
    }

    /// The pact files the page at `path` follows, in the order they apply.
    fn chain(&self, path: &str) -> Vec<String> {
        r::pact_chain(path, &self.links, &self.pact_files).pacts
    }

    /// Every page `path` owns, its owned pages' owned pages included, in
    /// the order they are reached.
    fn owned_tree(&self, path: &str) -> Vec<String> {
        owned_tree(&self.owned, path)
    }
}

/// Every page `path` owns by `owned` (the pages each page owns, as
/// [`r::owned_pages`] reads them: a retired page is in no set), its owned
/// pages' owned pages included, in the order they are reached.
pub(crate) fn owned_tree(owned: &BTreeMap<String, Vec<String>>, path: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut next = vec![path.to_string()];
    while let Some(owner) = next.pop() {
        for child in owned.get(&owner).into_iter().flatten() {
            if child != path && !out.contains(child) {
                out.push(child.clone());
                next.push(child.clone());
            }
        }
    }
    out
}

/// A pact file's change against its baseline: the diff the item shows and
/// how long it has waited.
pub(super) struct PactChange {
    pub(super) diff: String,
    pub(super) text: String,
    pub(super) age_secs: i64,
}

/// What the page items are built from beside the [`PageView`].
pub(super) struct PageInputs<'a> {
    pub(super) domain: &'a str,
    /// Every row (path, text).
    pub(super) texts: &'a BTreeMap<&'a str, &'a str>,
    /// The age of each row, by path.
    pub(super) ages: &'a BTreeMap<&'a str, i64>,
    /// The reconcile state of every page.
    pub(super) states: &'a BTreeMap<String, PageReconcile>,
    /// The pact hash every page has now.
    pub(super) hashes: &'a BTreeMap<String, String>,
    /// The page pacts a person changed, by path.
    pub(super) pact_changes: &'a BTreeMap<String, PactChange>,
    /// The section ids of each page a person changed and nobody settled.
    pub(super) unsettled: &'a BTreeMap<String, Vec<String>>,
    /// Whether diffs are rendered.
    pub(super) full: bool,
}

/// The `page_pact_changed`, `owner_gone`, `link_retired` and `link_broken`
/// items of the worked pages.
pub(super) fn page_items(view: &PageView, inputs: &PageInputs<'_>) -> Vec<Item> {
    let mut items = Vec::new();
    for (path, (_, record)) in &view.pages {
        if view.retired.contains(path) {
            continue;
        }
        let age = inputs.ages.get(path.as_str()).copied().unwrap_or(0);
        let item = |what: &'static str, section, heading, age_secs, extra, basis| Item {
            kind: "update",
            what,
            domain: inputs.domain.to_string(),
            path: path.clone(),
            section,
            heading,
            age_secs,
            extra,
            basis,
        };
        // A changed pact: a person's change to a page pact on the way up
        // (settled with the pact file), or a stored hash the pacts and
        // settings no longer match (settled with a reconcile). A page with
        // no stored hash has nothing to compare.
        let chain = view.chain(path);
        let changed: Vec<&String> = chain
            .iter()
            .filter(|pact| inputs.pact_changes.contains_key(pact.as_str()))
            .collect();
        let fresh = inputs.hashes.get(path).map(String::as_str).unwrap_or("");
        let stored = inputs
            .states
            .get(path)
            .and_then(|state| state.pact_hash.as_deref())
            .filter(|hash| !hash.is_empty());
        let moved = stored.is_some_and(|stored| stored != fresh);
        if !changed.is_empty() || moved {
            let mut extra = json!({
                "pacts": changed,
                "chain": chain,
                "pact_hash_changed": moved,
            });
            if inputs.full && !changed.is_empty() {
                let diff: String = changed
                    .iter()
                    .map(|pact| inputs.pact_changes[pact.as_str()].diff.as_str())
                    .collect();
                extra["diff"] = json!(diff);
            }
            let waited = changed
                .iter()
                .map(|pact| inputs.pact_changes[pact.as_str()].age_secs)
                .max()
                .unwrap_or(age);
            let basis = item_basis([fresh, stored.unwrap_or("")].into_iter().chain(
                changed.iter().flat_map(|pact| {
                    [
                        pact.as_str(),
                        inputs.pact_changes[pact.as_str()].text.as_str(),
                    ]
                }),
            ));
            items.push(item(
                "page_pact_changed",
                None,
                String::new(),
                waited,
                extra,
                basis,
            ));
        }
        // An owner that is gone or retired reconciles nothing any more.
        if let Some(owner) = &record.owner {
            let owner_is = if !view.pages.contains_key(owner) {
                Some("missing")
            } else if view.retired.contains(owner) {
                Some("retired")
            } else {
                None
            };
            if let Some(owner_is) = owner_is {
                items.push(item(
                    "owner_gone",
                    None,
                    String::new(),
                    age,
                    json!({ "owner": owner, "owner_is": owner_is, "hint": OWNER_GONE_HINT }),
                    item_basis([owner.as_str(), owner_is]),
                ));
            }
        }
        // Links to a page that is retired or not there, per section.
        let Some(text) = inputs.texts.get(path.as_str()) else {
            continue;
        };
        for section in r::split_sections(text) {
            let mut retired = Vec::new();
            let mut broken = Vec::new();
            for target in r::reflection_page_refs(&section.text, path) {
                if !view.pages.contains_key(&target) {
                    broken.push(target);
                } else if view.retired.contains(&target) {
                    retired.push(target);
                }
            }
            for (what, list) in [("link_retired", retired), ("link_broken", broken)] {
                if list.is_empty() {
                    continue;
                }
                let basis = item_basis(list.iter().map(String::as_str));
                items.push(item(
                    what,
                    section.id.clone(),
                    section.heading.clone(),
                    age,
                    json!({ "links": list }),
                    basis,
                ));
            }
        }
    }
    items
}

/// Whether the freshness of the page at `path` ran out at `now` (seconds
/// since the epoch). An owned page's freshness is a finding and is never
/// read, so it never runs out.
fn freshness_ran_out(
    view: &PageView,
    states: &BTreeMap<String, PageReconcile>,
    path: &str,
    now: i64,
) -> bool {
    let Some((settings, _)) = view.pages.get(path) else {
        return false;
    };
    let (Some(freshness), Some(state)) = (settings.freshness.as_ref(), states.get(path)) else {
        return false;
    };
    crystalline_core::time::parse_rfc3339(&state.last_reconciled).is_some_and(|at| {
        freshness
            .timing(at.stamp().unix_seconds(), now, &[])
            .stale()
    })
}

/// The version of a `stale` item: the recorded reconcile it was handed out
/// after (its time in milliseconds since the epoch, 0 for a page with none
/// recorded), the index of the period since then (0 for a page due only by
/// its mark), the newest not-fresh mark of the item's set when it has one,
/// and a hash of the freshness as written (empty for none), the pact hash,
/// that reconcile (or never), the period and the mark. It stays the same
/// for one period and moves with the next and with every re-mark; a
/// reconcile recorded after it was handed out makes it older
/// ([`stale_base`]). An item with no mark in its set has the version 0.36
/// gave it, byte for byte (`base:period:hash`); one with a mark gets
/// `base:period:mark:hash`.
fn stale_version(
    freshness: &str,
    state: &PageReconcile,
    recorded_at: Option<i64>,
    period: i64,
    pact_hash: &str,
    mark: Option<i64>,
) -> String {
    let period = period.to_string();
    let reconciled = if recorded_at.is_some() {
        state.last_reconciled.as_str()
    } else {
        "never"
    };
    let base = recorded_at.unwrap_or(0);
    match mark {
        None => {
            let hash = item_basis([freshness, pact_hash, reconciled, period.as_str()]);
            format!("{base}:{period}:{hash}")
        }
        Some(mark) => {
            let mark = mark.to_string();
            let hash = item_basis([
                freshness,
                pact_hash,
                reconciled,
                period.as_str(),
                mark.as_str(),
            ]);
            format!("{base}:{period}:{mark}:{hash}")
        }
    }
}

/// The parts of a `stale` item's `version`: the recorded reconcile it was
/// handed out after and the mark part, when it has one. `None` when
/// `version` is no stale item's version. Reads both forms: 3 parts with no
/// mark, 4 parts with one (decimal milliseconds).
fn stale_parts(version: &str) -> Option<(i64, Option<i64>)> {
    let parts: Vec<&str> = version.split(':').collect();
    let (base, period, mark, hash) = match parts.as_slice() {
        [base, period, hash] => (*base, *period, None, *hash),
        [base, period, mark, hash] => (*base, *period, Some(*mark), *hash),
        _ => return None,
    };
    let base = base.parse::<i64>().ok()?;
    period.parse::<u64>().ok()?;
    let mark = match mark {
        Some(mark) if mark.bytes().all(|b| b.is_ascii_digit()) => Some(mark.parse::<i64>().ok()?),
        Some(_) => return None,
        None => None,
    };
    (hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit())).then_some((base, mark))
}

/// The recorded reconcile a `stale` item's `version` was handed out after,
/// in milliseconds since the epoch (0 for none), or `None` when `version`
/// is no stale item's version.
pub(crate) fn stale_base(version: &str) -> Option<i64> {
    stale_parts(version).map(|(base, _)| base)
}

/// The mark part of a `stale` item's `version`: the newest not-fresh mark
/// of the item's set when it was handed out, in milliseconds since the
/// epoch. `None` for a version without one (3 parts) and for what is no
/// stale item's version.
pub(crate) fn stale_mark(version: &str) -> Option<i64> {
    stale_parts(version)?.1
}

/// Whether the owned page `child` counts as reconciled for its owner's
/// item: it has a recorded reconcile after `since` (the later of the
/// owner's last reconcile and the owner's own mark) and after its own mark.
pub(crate) fn owned_done(
    state: Option<&PageReconcile>,
    since_ms: i64,
    mark_ms: Option<i64>,
) -> bool {
    let after = mark_ms.map_or(since_ms, |mark| mark.max(since_ms));
    state.is_some_and(|state| {
        state.outcome.is_some()
            && crystalline_core::time::parse_rfc3339(&state.last_reconciled)
                .is_some_and(|at| at.stamp().unix_millis() > after)
    })
}

/// Whether the page at `path` of the Reflection `rows` (path, text) has a
/// `stale` item of its own at `now` (seconds since the epoch), by
/// `states`: it is due (its freshness ran out, or it is marked not fresh)
/// and no page up its owners is. What a `reconciled` without a version is
/// refused for.
pub(crate) fn has_own_stale_item(
    rows: &[(&str, &str)],
    states: &BTreeMap<String, PageReconcile>,
    path: &str,
    now: i64,
) -> bool {
    PageView::read(rows).own_item(states, path, now)
}

/// One page's `stale` item, when it has one of its own at `now` (seconds
/// since the epoch): its freshness ran out or it is marked not fresh, and
/// no page up its owners is due (then it is part of that page's item). Its `changed_engrams` is filled in later
/// (`Detail::Full` only), from the engrams changed since `last_reconciled`,
/// which comes back beside the item: how many engrams changed since then in
/// the domains the caller may read, or null when that is unknown. It is
/// unknown when a readable domain lives on a connected server, or when a
/// configured file domain has not been walked (its folder is missing or
/// unreadable, or the first sync has not reached it), never 0.
pub(super) fn stale_item(
    view: &PageView,
    inputs: &PageInputs<'_>,
    path: &str,
    now: i64,
) -> Option<(Item, i64)> {
    if view.retired.contains(path) || !view.own_item(inputs.states, path, now) {
        return None;
    }
    let (settings, _) = view.pages.get(path)?;
    // An owned page's freshness is a finding and was not read.
    let freshness = settings.freshness.as_ref();
    let state = inputs.states.get(path)?;
    let at = crystalline_core::time::parse_rfc3339(&state.last_reconciled)?.stamp();
    let timing = freshness
        .map(|freshness| (freshness, freshness.timing(at.unix_seconds(), now, &[])))
        .filter(|(_, timing)| timing.stale());
    let mark = view.mark(path);
    let period = timing.as_ref().map_or(0, |(_, timing)| timing.period);
    let recorded_at = state.outcome.is_some().then(|| at.unix_millis());
    let pact_hash = inputs.hashes.get(path).map(String::as_str).unwrap_or("");
    let owned = view.owned_tree(path);
    let owned_marked: Vec<&String> = owned
        .iter()
        .filter(|child| view.mark(child).is_some())
        .collect();
    // The newest mark of the item's set: what its record may take off.
    let set_mark = mark
        .map(|mark| mark.at_ms)
        .into_iter()
        .chain(
            owned_marked
                .iter()
                .filter_map(|child| view.mark(child).map(|m| m.at_ms)),
        )
        .max();
    let version = stale_version(
        freshness.map_or("", |freshness| freshness.written.as_str()),
        state,
        recorded_at,
        period,
        pact_hash,
        set_mark,
    );
    let chain = view.chain(path);
    let mut texts = serde_json::Map::new();
    for pact in &chain {
        if let Some(text) = inputs.texts.get(pact.as_str()) {
            texts.insert(
                pact.clone(),
                json!(super::super::reflection::without_provenance(text)),
            );
        }
    }
    // The owned pages reconciled since the owner was (and since the owner's
    // mark and their own): a run that stopped at its cap part way through
    // them resumes after them (design 3.5). Only a recorded reconcile
    // counts, never the seed of a page with none.
    let since = mark.map_or(at.unix_millis(), |mark| mark.at_ms.max(at.unix_millis()));
    let owned_reconciled: Vec<&String> = owned
        .iter()
        .filter(|child| {
            owned_done(
                inputs.states.get(child.as_str()),
                since,
                view.mark(child).map(|m| m.at_ms),
            )
        })
        .collect();
    let freshness_age = timing
        .as_ref()
        .map(|(freshness, timing)| timing.last_checked_secs - freshness.seconds);
    let mark_age = mark.map(|mark| (now - mark.at_ms.div_euclid(1000)).max(0));
    let age_secs = freshness_age.into_iter().chain(mark_age).max().unwrap_or(0);
    let extra = json!({
        "freshness": timing.as_ref().map(|(freshness, _)| freshness.written.clone()),
        "refresh_requested": mark.map(Mark::to_json),
        "owned_marked": owned_marked,
        "last_reconciled": state.last_reconciled,
        "outcome": state.outcome.map(|o| o.as_str()),
        "pacts": chain,
        "pact_hash": pact_hash,
        "owned": owned,
        "owned_reconciled": owned_reconciled,
        "unsettled": inputs.unsettled.get(path).cloned().unwrap_or_default(),
        "version": version,
        PACT_TEXTS: Value::Object(texts),
    });
    Some((
        Item {
            kind: "update",
            what: "stale",
            domain: inputs.domain.to_string(),
            path: path.to_string(),
            section: None,
            heading: String::new(),
            age_secs,
            extra,
            basis: version,
        },
        at.unix_seconds(),
    ))
}

/// Whether a domain the caller may read lives on a connected server
/// (`elsewhere`: the local names of the mounted domains and of the local
/// copies a server replaces), so the engrams changed in it cannot be
/// counted here. One hidden from the caller does not count, and a session
/// under a read limit is never routed to a server, so it reads none.
pub(super) fn counted_elsewhere<'a>(
    elsewhere: impl IntoIterator<Item = &'a String>,
    hidden: &std::collections::HashSet<String>,
    limited: bool,
) -> bool {
    !limited && elsewhere.into_iter().any(|name| !hidden.contains(name))
}

/// The key under which a `stale` item carries the texts of its pacts until
/// the answer gathers them once per answer.
pub(super) const PACT_TEXTS: &str = "_pact_texts";

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::{counted_elsewhere, stale_base, stale_mark, stale_version};
    use crate::engine::reconcile::{PageReconcile, ReconcileOutcome};

    fn state() -> PageReconcile {
        PageReconcile {
            last_reconciled: "2026-11-20T10:00:00+00:00".into(),
            outcome: Some(ReconcileOutcome::Updated),
            pact_hash: Some("abc".into()),
            version: None,
        }
    }

    /// An item with no mark in its set has the version 0.36 gave it, byte
    /// for byte, so an item a run left for a person before the upgrade
    /// stays left.
    #[test]
    fn an_unmarked_version_is_the_one_0_36_gave() {
        let version = stale_version("1d", &state(), Some(1_795_600_000_000), 3, "abc", None);
        assert_eq!(
            version,
            "1795600000000:3:8d33ce9b99db50b6480f8e67e7cc632fcf04c7c05d1651d851fff473d3da3499"
        );
        assert_eq!(stale_base(&version), Some(1_795_600_000_000));
        assert_eq!(stale_mark(&version), None);
    }

    /// A version with a mark has four parts; both forms read, and the mark
    /// moves the version.
    #[test]
    fn a_marked_version_carries_its_mark_and_reads_back() {
        let marked = stale_version("", &state(), None, 0, "abc", Some(1_795_615_387_412));
        let parts: Vec<&str> = marked.split(':').collect();
        assert_eq!(parts.len(), 4, "{marked}");
        assert_eq!(&parts[..3], ["0", "0", "1795615387412"]);
        assert_eq!(stale_base(&marked), Some(0));
        assert_eq!(stale_mark(&marked), Some(1_795_615_387_412));
        let later = stale_version("", &state(), None, 0, "abc", Some(1_795_615_387_413));
        assert_ne!(later, marked);
        for wrong in [
            "1:2",
            "x:1:aa",
            "1:2:-5:8d33ce9b99db50b6480f8e67e7cc632fcf04c7c05d1651d851fff473d3da3499",
            "1:2:3:4:8d33ce9b99db50b6480f8e67e7cc632fcf04c7c05d1651d851fff473d3da3499",
            "v1",
        ] {
            assert_eq!(stale_base(wrong), None, "{wrong}");
            assert_eq!(stale_mark(wrong), None, "{wrong}");
        }
    }

    #[test]
    fn a_readable_mount_makes_the_count_unknown() {
        let mounts = ["acme-eng".to_string(), "acme-ops".to_string()];
        let none = HashSet::new();
        assert!(counted_elsewhere(&mounts, &none, false), "readable mounts");
        let one = HashSet::from(["acme-eng".to_string()]);
        assert!(
            counted_elsewhere(&mounts, &one, false),
            "acme-ops is readable"
        );
        let both = HashSet::from(["acme-eng".to_string(), "acme-ops".to_string()]);
        assert!(!counted_elsewhere(&mounts, &both, false), "both hidden");
        assert!(
            !counted_elsewhere(&mounts, &none, true),
            "a limited run reads no mount"
        );
        assert!(!counted_elsewhere(&[], &none, false), "no mount");
    }
}
