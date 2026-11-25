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
        PageView {
            pages,
            retired,
            links,
            pact_files,
            owned: r::owned_pages(rows.iter().copied()),
        }
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

/// The version of a `stale` item: the recorded reconcile it was handed out
/// after (its time in milliseconds since the epoch, 0 for a page with none
/// recorded), the index of the period since then and a hash of the
/// freshness, the pact hash, that reconcile (or never) and the period. It
/// stays the same for one period and moves with the next; a reconcile
/// recorded after it was handed out makes it older ([`stale_base`]).
fn stale_version(
    freshness: &r::Freshness,
    state: &PageReconcile,
    recorded_at: Option<i64>,
    period: i64,
    pact_hash: &str,
) -> String {
    let period = period.to_string();
    let reconciled = if recorded_at.is_some() {
        state.last_reconciled.as_str()
    } else {
        "never"
    };
    let hash = item_basis([
        freshness.written.as_str(),
        pact_hash,
        reconciled,
        period.as_str(),
    ]);
    format!("{}:{period}:{hash}", recorded_at.unwrap_or(0))
}

/// The recorded reconcile a `stale` item's `version` was handed out after,
/// in milliseconds since the epoch (0 for none), or `None` when `version`
/// is no stale item's version.
pub(crate) fn stale_base(version: &str) -> Option<i64> {
    let mut parts = version.splitn(3, ':');
    let base = parts.next()?.parse::<i64>().ok()?;
    parts.next()?.parse::<u64>().ok()?;
    let hash = parts.next()?;
    (hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit())).then_some(base)
}

/// One page's `stale` item, when its freshness ran out at `now` (seconds
/// since the epoch). Its `changed_engrams` is filled in later
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
    if view.retired.contains(path) {
        return None;
    }
    let (settings, _) = view.pages.get(path)?;
    // An owned page's freshness is a finding and was not read.
    let freshness = settings.freshness.as_ref()?;
    let state = inputs.states.get(path)?;
    let at = crystalline_core::time::parse_rfc3339(&state.last_reconciled)?.stamp();
    let timing = freshness.timing(at.unix_seconds(), now, &[]);
    if !timing.stale() {
        return None;
    }
    let period = timing.period;
    let recorded_at = state.outcome.is_some().then(|| at.unix_millis());
    let pact_hash = inputs.hashes.get(path).map(String::as_str).unwrap_or("");
    let version = stale_version(freshness, state, recorded_at, period, pact_hash);
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
    // The owned pages reconciled since the owner was: a run that stopped at
    // its cap part way through them resumes after them (design 3.5). Only a
    // recorded reconcile counts, never the seed of a page with none.
    let owned = view.owned_tree(path);
    let owned_reconciled: Vec<&String> = owned
        .iter()
        .filter(|child| {
            inputs.states.get(child.as_str()).is_some_and(|child| {
                child.outcome.is_some()
                    && crystalline_core::time::parse_rfc3339(&child.last_reconciled)
                        .is_some_and(|t| t.stamp() > at)
            })
        })
        .collect();
    let extra = json!({
        "freshness": freshness.written,
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
            age_secs: timing.last_checked_secs - freshness.seconds,
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

    use super::counted_elsewhere;

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
