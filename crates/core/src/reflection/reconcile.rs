//! What a reconcile of a Reflection page records about what the page
//! follows, and the pages one page owns.
//!
//! The pact hash ([`pact_hash`]) is what a reconcile saw of the pacts the
//! page follows and of its own settings: the pact texts by their content,
//! never their paths, with the `reflection:` block and the section id lines
//! left out (Crystalline's bookkeeping, which a settle and a first write
//! change) and line endings as LF, then the page's `freshness` and
//! `status`. `follows` as written is left out: a move rewrites it, and a
//! page that follows another page already has that page's pact in its
//! chain. A link inside a pact is content, so a link retargeted on disk or
//! by a pull changes the hash; a move that rewrites the links of a pact
//! updates the stored hashes it leaves current in its own commit instead.
//!
//! Ownership is recorded on the owned page, in its page record
//! ([`PageRecord::owner`]); a page's set of owned pages is a scan of the
//! pages ([`owned_pages`]).

use std::collections::{BTreeMap, BTreeSet};

use sha2::Digest as _;

use super::layout::{ReflectionFile, classify, is_pact_file};
use super::page::{PageLinks, PageSettings, pact_chain, read_page_settings, would_cycle};
use super::provenance::{PageRecord, Provenance, read_provenance, write_provenance};
use super::sections::parse_id_line;

/// The text of a pact file as the pact hash reads it: LF line endings, no
/// `reflection:` block and no section id lines.
fn pact_content(text: &str) -> String {
    let text = crate::to_lf(text);
    let text = write_provenance(&text, &Provenance::new()).unwrap_or_else(|_| text.to_string());
    text.split_inclusive('\n')
        .filter(|line| parse_id_line(line).is_none())
        .collect()
}

/// The pact hash of a page: the texts of the pacts it follows, in the order
/// they apply (`pacts`, as [`pact_chain`] names them), and its own
/// `freshness` and `status` as read ([`read_page_settings`]). `sha256:` and
/// the lowercase hex digest.
pub fn pact_hash(pacts: &[&str], settings: &PageSettings) -> String {
    let mut hasher = sha2::Sha256::new();
    let mut part = |tag: &str, text: &str| {
        // Each part with its tag and length, so no two lists of texts can
        // read as the same bytes.
        hasher.update(format!("{tag} {}\n", text.len()).as_bytes());
        hasher.update(text.as_bytes());
        hasher.update(b"\n");
    };
    for pact in pacts {
        part("pact", &pact_content(pact));
    }
    let freshness = settings.freshness.as_ref().map(|f| f.written.as_str());
    part("freshness", freshness.unwrap_or(""));
    part(
        "status",
        settings.status.as_deref().map(str::trim).unwrap_or(""),
    );
    let digest = hasher.finalize();
    let mut out = String::from("sha256:");
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// A Reflection's files read once for the pact hashes of its pages.
struct Pages<'a> {
    files: BTreeMap<&'a str, &'a str>,
    links: BTreeMap<String, PageLinks>,
    settings: BTreeMap<&'a str, PageSettings>,
    pact_files: BTreeSet<String>,
}

impl<'a> Pages<'a> {
    fn read(files: impl IntoIterator<Item = (&'a str, &'a str)>) -> Pages<'a> {
        let files: BTreeMap<&str, &str> = files.into_iter().collect();
        let mut links = BTreeMap::new();
        let mut settings = BTreeMap::new();
        let mut pact_files = BTreeSet::new();
        for (&path, &text) in &files {
            if is_pact_file(path) {
                pact_files.insert(path.to_string());
                continue;
            }
            if !matches!(classify(path), ReflectionFile::Page(_)) {
                continue;
            }
            let record = read_provenance(text).page;
            let (read, _) = read_page_settings(text, path, &record);
            links.insert(path.to_string(), read.links(&record));
            settings.insert(path, read);
        }
        Pages {
            files,
            links,
            settings,
            pact_files,
        }
    }

    fn hash(&self, page: &str) -> Option<String> {
        let settings = self.settings.get(page)?;
        let chain = pact_chain(page, &self.links, &self.pact_files);
        let texts: Vec<&str> = chain
            .pacts
            .iter()
            .filter_map(|pact| self.files.get(pact.as_str()).copied())
            .collect();
        Some(pact_hash(&texts, settings))
    }
}

/// The pact hash ([`pact_hash`]) of every page of a Reflection whose files
/// (path inside the Reflection, text) are `files`: pages only, not `PACT.md`
/// and no page pact. Threads and baselines among `files` are passed over.
pub fn pact_hashes<'a>(
    files: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> BTreeMap<String, String> {
    let pages = Pages::read(files);
    pages
        .settings
        .keys()
        .filter_map(|page| Some((page.to_string(), pages.hash(page)?)))
        .collect()
}

/// The pact hash of the page at `page` among `files`, as [`pact_hashes`]
/// answers it; `None` when `page` is no page among them.
pub fn page_pact_hash<'a>(
    page: &str,
    files: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Option<String> {
    Pages::read(files).hash(page)
}

/// The pages each page owns, by owner, among the pages of `files`: a page
/// whose page record names an `owner` is that owner's, whether the owner
/// exists or not. A retired page names no owner (its record keeps it as
/// `was_owned_by`), so it is in no owner's set. Sorted.
pub fn owned_pages<'a>(
    files: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> BTreeMap<String, Vec<String>> {
    let mut owned: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (path, text) in files {
        if !matches!(classify(path), ReflectionFile::Page(_)) || is_pact_file(path) {
            continue;
        }
        if let Some(owner) = read_provenance(text).page.owner {
            owned.entry(owner).or_default().push(path.to_string());
        }
    }
    for pages in owned.values_mut() {
        pages.sort();
    }
    owned
}

/// What a write of a page changes in the engrams it keeps covered after it
/// dropped them (rule A of the reflect queue): [`cover_change`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CoverChange {
    /// The engrams of the page's own domain the write dropped from a page
    /// with a freshness (or an owned page), each with the checksum the page
    /// recorded when it cited it. The index keeps each with its checksum at
    /// the drop instead ([`cover_change`]); it stays covered while that
    /// checksum holds.
    pub add: Vec<(String, String)>,
    /// The engrams the page cites again: it covers them by citing them.
    pub remove: Vec<String>,
}

impl CoverChange {
    /// Whether the write changes nothing.
    pub fn is_empty(&self) -> bool {
        self.add.is_empty() && self.remove.is_empty()
    }
}

/// The engrams of the page's own domain a page cites, each with the
/// checksum its first citing source recorded.
fn own_cited(provenance: &Provenance) -> BTreeMap<String, String> {
    let mut cited = BTreeMap::new();
    for source in provenance.values().flat_map(|record| record.sources.iter()) {
        if source.domain.is_none() {
            cited
                .entry(source.engram.clone())
                .or_insert_with(|| source.checksum.clone());
        }
    }
    cited
}

/// What the write of the page at `path` from `before` (its text as stored,
/// `None` for a new page) to `after` changes in the engrams it keeps
/// covered. An engram of the page's own domain that `before` cites and
/// `after` does not is kept when `after` is a page with a freshness or an
/// owned page that is not
/// retired: a page that keeps itself current drops last week's change, and
/// that change is still reflected. An engram `after` cites and `before` did
/// not leaves the kept set. Citing another domain's engram covers nothing of
/// this one, so only sources of the page's own domain count. Nothing for
/// `PACT.md`, a page pact or anything that is no page.
///
/// The checksum in [`CoverChange::add`] is the one `before` recorded; the
/// index does not store it. It keeps each dropped engram with the engram's
/// own checksum at the drop, read from its engram rows
/// (`crystalline_index::reflection::index_reflection_row`): a reconcile that
/// saw the engram change and then dropped it has reflected that change.
pub fn cover_change(path: &str, before: Option<&str>, after: &str) -> CoverChange {
    if is_pact_file(path) || !matches!(classify(path), ReflectionFile::Page(_)) {
        return CoverChange::default();
    }
    let Some(before) = before else {
        return CoverChange::default();
    };
    let was = own_cited(&read_provenance(before));
    let provenance = read_provenance(after);
    let now = own_cited(&provenance);
    let (settings, _) = read_page_settings(after, path, &provenance.page);
    let rolling =
        !settings.retired() && (settings.freshness.is_some() || provenance.page.owner.is_some());
    CoverChange {
        add: if rolling {
            was.iter()
                .filter(|(engram, _)| !now.contains_key(*engram))
                .map(|(engram, checksum)| (engram.clone(), checksum.clone()))
                .collect()
        } else {
            Vec::new()
        },
        remove: now
            .into_keys()
            .filter(|engram| !was.contains_key(engram))
            .collect(),
    }
}

/// `source` with an `owner` or a `was_owned_by` in its page record that
/// names the page `from` naming `to` instead, and how many it changed: the
/// page `from` moved. Nothing else of the record or the page changes; a
/// page that names `from` nowhere is answered as it is. `Err` when the
/// frontmatter cannot be cut into key blocks.
pub fn rewrite_owner(source: &str, from: &str, to: &str) -> Result<(String, usize), String> {
    let mut provenance = read_provenance(source);
    let mut changed = 0;
    let page: &mut PageRecord = &mut provenance.page;
    if page.owner.as_deref() == Some(from) {
        page.owner = Some(to.to_string());
        changed += 1;
    }
    if let Some(retired) = &mut page.retired
        && retired.was_owned_by.as_deref() == Some(from)
    {
        retired.was_owned_by = Some(to.to_string());
        changed += 1;
    }
    if changed == 0 {
        return Ok((source.to_string(), 0));
    }
    Ok((write_provenance(source, &provenance)?, changed))
}

/// Why the page at `page` may not be owned by `owner`, or `None` when it
/// may: `owner` must be another existing page of this Reflection, and
/// neither the walk up its owners nor the page's pact chain may come back
/// to `page`. `pages` holds every page that exists with its links, `page`
/// itself among them with the `follows` it is written with.
pub fn owner_refusal(
    page: &str,
    owner: &str,
    pages: &BTreeMap<String, PageLinks>,
) -> Option<String> {
    if owner == page {
        return Some(format!(
            "'{page}' cannot own itself; name another page as its owner"
        ));
    }
    if is_pact_file(owner) || !matches!(classify(owner), ReflectionFile::Page(_)) {
        return Some(format!(
            "owner '{owner}' names no page of this Reflection; name a page path such as arch/overview.md"
        ));
    }
    if !pages.contains_key(owner) {
        return Some(format!(
            "owner '{owner}' names a page that does not exist; write that page first or name another"
        ));
    }
    let mut seen = BTreeSet::from([page.to_string()]);
    let mut current = owner.to_string();
    let owners_cycle = loop {
        if current == page {
            break true;
        }
        if !seen.insert(current.clone()) {
            break false;
        }
        match pages.get(&current).and_then(|links| links.owner.clone()) {
            Some(next) => current = next,
            None => break false,
        }
    };
    // With no `follows` of its own the page follows its owner, so the
    // owner may not lead back to it through the pact chain either.
    let follows_owner = pages.get(page).is_none_or(|links| links.follows.is_none());
    if owners_cycle || (follows_owner && would_cycle(page, owner, pages)) {
        return Some(format!(
            "owner '{owner}' leads back to '{page}': a page cannot be owned by a page it owns or that follows it, so choose another owner"
        ));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A page whose one section cites the engrams `cites` of its own domain
    /// (each with the checksum `sum-<permalink>`), with `front` as extra
    /// frontmatter lines.
    fn citing(front: &str, cites: &[&str]) -> String {
        let mut block = String::from("reflection:\n  a1b2:\n    sources:\n");
        for cite in cites {
            block.push_str(&format!(
                "    - engram: {cite}\n      checksum: sum-{cite}\n"
            ));
        }
        if cites.is_empty() {
            block = String::new();
        }
        format!("---\n{front}{block}---\n# Week\n\n## A\n<!-- id: a1b2 -->\n\nText.\n")
    }

    #[test]
    fn a_rolling_page_keeps_what_it_drops_and_a_plain_page_does_not() {
        let before = citing("freshness: 7d\n", &["weekly/a", "weekly/b"]);
        let after = citing("freshness: 7d\n", &["weekly/b", "weekly/c"]);
        assert_eq!(
            cover_change("guides/week.md", Some(&before), &after),
            CoverChange {
                add: vec![("weekly/a".into(), "sum-weekly/a".into())],
                remove: vec!["weekly/c".into()],
            }
        );
        let plain_before = citing("", &["weekly/a", "weekly/b"]);
        let plain_after = citing("", &["weekly/b"]);
        assert!(cover_change("guides/plain.md", Some(&plain_before), &plain_after).is_empty());
    }

    #[test]
    fn an_owned_page_keeps_what_it_drops_and_a_retired_one_does_not() {
        let owned = |cites: &[&str]| {
            citing("", cites).replace(
                "reflection:\n",
                "reflection:\n  _page:\n    owner: guides/week.md\n",
            )
        };
        let change = cover_change(
            "guides/week/a.md",
            Some(&owned(&["weekly/a"])),
            &owned(&["weekly/b"]),
        );
        assert_eq!(change.add, vec![("weekly/a".into(), "sum-weekly/a".into())]);

        let retired = citing("freshness: 7d\nstatus: archived\n", &["weekly/b"]);
        let change = cover_change(
            "guides/week.md",
            Some(&citing("freshness: 7d\n", &["weekly/a", "weekly/b"])),
            &retired,
        );
        assert!(change.add.is_empty(), "{change:?}");
    }

    #[test]
    fn a_new_page_a_pact_and_another_domains_source_change_nothing() {
        let page = citing("freshness: 7d\n", &["weekly/a"]);
        assert!(cover_change("guides/week.md", None, &page).is_empty());
        let bare = citing("freshness: 7d\n", &[]);
        assert!(cover_change("guides/week.pact.md", Some(&page), &bare).is_empty());
        assert!(cover_change("PACT.md", Some(&page), &bare).is_empty());
        let foreign = page.replace("    - engram:", "    - domain: harbor\n      engram:");
        assert!(cover_change("guides/week.md", Some(&foreign), &bare).is_empty());
    }

    fn page(follows: Option<&str>, owner: Option<&str>) -> PageLinks {
        PageLinks {
            follows: follows.map(str::to_string),
            owner: owner.map(str::to_string),
        }
    }

    const PACT: &str = "---\ntitle: Pact\n---\n\n# Pact\n\n## Tone\n<!-- r:aaaa1111 -->\n\nPlain. See [the overview](arch/overview.md).\n";

    #[test]
    fn the_hash_reads_the_pact_texts_and_the_settings() {
        let settings = PageSettings::default();
        let a = pact_hash(&[PACT], &settings);
        assert!(a.starts_with("sha256:") && a.len() == 7 + 64, "{a}");
        assert_eq!(a, pact_hash(&[PACT], &settings), "stable");
        assert_ne!(
            a,
            pact_hash(&[PACT, "# More\n"], &settings),
            "a page pact on the way"
        );
        assert_ne!(
            a,
            pact_hash(&[&PACT.replace("Plain.", "Short.")], &settings),
            "the content"
        );
        let fresh = PageSettings {
            freshness: Some(super::super::page::parse_freshness("1d").unwrap()),
            ..PageSettings::default()
        };
        assert_ne!(a, pact_hash(&[PACT], &fresh), "the freshness");
        let retired = PageSettings {
            status: Some("archived".into()),
            ..PageSettings::default()
        };
        assert_ne!(a, pact_hash(&[PACT], &retired), "the status");
        // Two texts never read as one.
        assert_ne!(
            pact_hash(&["ab", "c"], &settings),
            pact_hash(&["a", "bc"], &settings)
        );
    }

    #[test]
    fn the_hash_reads_link_targets_and_leaves_out_the_bookkeeping() {
        let settings = PageSettings::default();
        let a = pact_hash(&[PACT], &settings);
        // A link retargeted is a change of the pact (a move keeps the
        // stored hash by updating it in its own commit).
        let retargeted = PACT.replace("arch/overview.md", "design/overview.md");
        assert_ne!(a, pact_hash(&[&retargeted], &settings), "a link target");
        // The block and the id lines are Crystalline's.
        let stamped = PACT.replace(
            "title: Pact\n",
            "title: Pact\nreflection:\n  aaaa1111:\n    reflected: abc\n",
        );
        assert_eq!(a, pact_hash(&[&stamped], &settings), "the block");
        let no_id = PACT.replace("<!-- r:aaaa1111 -->\n", "");
        assert_eq!(a, pact_hash(&[&no_id], &settings), "an id line");
        assert_eq!(
            a,
            pact_hash(&[&PACT.replace('\n', "\r\n")], &settings),
            "line endings"
        );
    }

    #[test]
    fn every_page_has_the_hash_of_its_chain() {
        let files = [
            ("PACT.md", "# Pact\n\nPlain.\n"),
            ("arch.md", "# Arch\n"),
            ("arch.pact.md", "# Arch pact\n\nDiagrams.\n"),
            ("arch/overview.md", "---\nfreshness: 1d\n---\n# Overview\n"),
            ("notes.md", "# Notes\n"),
            ("arch.threads.md", "threads"),
        ];
        let hashes = pact_hashes(files);
        assert_eq!(
            hashes.keys().map(String::as_str).collect::<Vec<_>>(),
            vec!["arch.md", "arch/overview.md", "notes.md"]
        );
        let plain = PageSettings::default();
        assert_eq!(
            hashes["notes.md"],
            pact_hash(&["# Pact\n\nPlain.\n"], &plain)
        );
        assert_eq!(
            hashes["arch.md"],
            pact_hash(
                &["# Pact\n\nPlain.\n", "# Arch pact\n\nDiagrams.\n"],
                &plain
            )
        );
        let (fresh, _) = read_page_settings(files[3].1, "arch/overview.md", &PageRecord::default());
        assert_eq!(
            hashes["arch/overview.md"],
            pact_hash(
                &["# Pact\n\nPlain.\n", "# Arch pact\n\nDiagrams.\n"],
                &fresh
            )
        );
        assert_eq!(
            page_pact_hash("arch/overview.md", files).as_deref(),
            Some(hashes["arch/overview.md"].as_str())
        );
        assert_eq!(page_pact_hash("PACT.md", files), None);
        assert_eq!(page_pact_hash("gone.md", files), None);
    }

    #[test]
    fn a_follower_keeps_its_hash_when_only_the_paths_change() {
        let before = [
            ("PACT.md", "# Pact\n\nPlain.\n"),
            ("guides/a.md", "# A\n"),
            ("guides/a.pact.md", "# A pact\n\nShort.\n"),
            ("guides/b.md", "---\nfollows: guides/a.md\n---\n# B\n"),
        ];
        let after = [
            ("PACT.md", "# Pact\n\nPlain.\n"),
            ("design/c.md", "# A\n"),
            ("design/c.pact.md", "# A pact\n\nShort.\n"),
            ("guides/b.md", "---\nfollows: design/c.md\n---\n# B\n"),
        ];
        let (before, after) = (pact_hashes(before), pact_hashes(after));
        assert_eq!(before["guides/a.md"], after["design/c.md"]);
        assert_eq!(before["guides/b.md"], after["guides/b.md"]);
    }

    #[test]
    fn the_owned_pages_are_read_from_their_records() {
        let files = [
            ("arch.md", "# Arch\n"),
            (
                "arch/one.md",
                "---\nreflection:\n  _page:\n    owner: arch.md\n---\n# One\n",
            ),
            (
                "arch/two.md",
                "---\nreflection:\n  _page:\n    owner: arch.md\n---\n# Two\n",
            ),
            (
                "arch/old.md",
                "---\nstatus: archived\nreflection:\n  _page:\n    retired:\n      at: '2026-11-01T00:00:00Z'\n      by: agent\n      was_owned_by: arch.md\n---\n# Old\n",
            ),
            (
                "arch/one.pact.md",
                "---\nreflection:\n  _page:\n    owner: arch.md\n---\n",
            ),
        ];
        let owned = owned_pages(files);
        assert_eq!(owned.len(), 1, "{owned:?}");
        assert_eq!(owned["arch.md"], vec!["arch/one.md", "arch/two.md"]);
    }

    #[test]
    fn a_move_rewrites_the_owner_and_the_former_owner() {
        let owned =
            "---\ntitle: One\nreflection:\n  _page:\n    owner: arch.md\n---\n# One\n\nBody.\n";
        let (text, n) = rewrite_owner(owned, "arch.md", "design.md").unwrap();
        assert_eq!(n, 1);
        assert_eq!(
            read_provenance(&text).page.owner.as_deref(),
            Some("design.md")
        );
        assert!(
            text.contains("title: One\n") && text.ends_with("# One\n\nBody.\n"),
            "{text}"
        );
        let retired = "---\nstatus: archived\nreflection:\n  _page:\n    retired:\n      at: '2026-11-01T00:00:00Z'\n      by: agent\n      was_owned_by: arch.md\n---\n# Old\n";
        let (text, n) = rewrite_owner(retired, "arch.md", "design.md").unwrap();
        assert_eq!(n, 1);
        let record = read_provenance(&text).page;
        assert_eq!(
            record.retired.unwrap().was_owned_by.as_deref(),
            Some("design.md")
        );
        assert_eq!(
            rewrite_owner(owned, "other.md", "x.md").unwrap(),
            (owned.to_string(), 0)
        );
    }

    #[test]
    fn an_owner_must_be_another_page_that_does_not_lead_back() {
        let pages: BTreeMap<String, PageLinks> = [
            ("arch.md", page(None, None)),
            ("arch/one.md", page(None, Some("arch.md"))),
            ("notes.md", page(Some("new.md"), None)),
            ("new.md", page(None, None)),
        ]
        .into_iter()
        .map(|(p, l)| (p.to_string(), l))
        .collect();
        assert_eq!(owner_refusal("new.md", "arch.md", &pages), None);
        assert_eq!(owner_refusal("new.md", "arch/one.md", &pages), None);
        for (owner, says) in [
            ("new.md", "itself"),
            ("PACT.md", "names no page"),
            ("arch.pact.md", "names no page"),
            ("gone.md", "does not exist"),
            // notes.md follows new.md: new.md would follow its owner back.
            ("notes.md", "leads back"),
        ] {
            let refusal = owner_refusal("new.md", owner, &pages).unwrap_or_default();
            assert!(refusal.contains(says), "{owner}: {refusal}");
        }
        // A page owned by one it owns (an old record of a page that went
        // and came back), with its own follows elsewhere.
        let mut back = pages.clone();
        back.insert("arch.md".into(), page(Some("notes.md"), Some("x.md")));
        back.insert("x.md".into(), page(Some("notes.md"), None));
        let refusal = owner_refusal("x.md", "arch.md", &back).unwrap_or_default();
        assert!(refusal.contains("leads back"), "{refusal}");
    }
}
