//! What a reconcile of a Reflection page records about what the page
//! follows, and the pages one page owns.
//!
//! The pact hash ([`pact_hash`]) is what a reconcile saw of the pacts the
//! page follows and of its own settings. Whatever a move rewrites stays out
//! of it, so a curator's move changes no page's hash: the pact texts are
//! hashed by their content, never their paths, with every relative link
//! target left out (a move relinks the pacts that link the moved page and
//! rebases the moved page's own pact), with the `reflection:` block and the
//! section id lines left out (Crystalline's bookkeeping, which a settle and
//! a first write change) and line endings as LF. `follows` as written is
//! left out too: a move rewrites it, and a page that follows another page
//! already has that page's pact in its chain. The cost: a pact edit that
//! changes only a link target does not change the hash.
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
/// `reflection:` block, no section id lines and every relative link target
/// left empty.
fn pact_content(text: &str) -> String {
    let text = crate::to_lf(text);
    let text = write_provenance(&text, &Provenance::new()).unwrap_or_else(|_| text.to_string());
    let text: String = text
        .split_inclusive('\n')
        .filter(|line| parse_id_line(line).is_none())
        .collect();
    super::links::map_links(&text, &|_| Some(String::new())).0
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
    fn the_hash_leaves_out_what_a_move_and_the_bookkeeping_rewrite() {
        let settings = PageSettings::default();
        let a = pact_hash(&[PACT], &settings);
        // A relative link that followed a move.
        let relinked = PACT.replace("arch/overview.md", "design/overview.md");
        assert_eq!(a, pact_hash(&[&relinked], &settings), "a link target");
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
    fn a_follower_keeps_its_hash_when_what_it_follows_moves() {
        let before = [
            ("PACT.md", "# Pact\n\nSee [a](guides/a.md).\n"),
            ("guides/a.md", "# A\n"),
            ("guides/a.pact.md", "# A pact\n\nSee [b](b.md).\n"),
            ("guides/b.md", "---\nfollows: guides/a.md\n---\n# B\n"),
        ];
        let after = [
            ("PACT.md", "# Pact\n\nSee [a](guides/c.md).\n"),
            ("guides/c.md", "# A\n"),
            ("guides/c.pact.md", "# A pact\n\nSee [b](b.md).\n"),
            ("guides/b.md", "---\nfollows: guides/c.md\n---\n# B\n"),
        ];
        let (before, after) = (pact_hashes(before), pact_hashes(after));
        assert_eq!(before["guides/a.md"], after["guides/c.md"]);
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
