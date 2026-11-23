//! What a Reflection page says about itself in its frontmatter (`freshness`,
//! `follows`, `status`) and the chain of pacts it follows.
//!
//! A page follows `PACT.md` plus every page pact on its way up: an explicit
//! `follows: <page>` in its frontmatter (a page of the same Reflection), else
//! its owner (the `owner` of its [`PageRecord`]), else its folder parent
//! (`arch/overview.md` for the pages in `arch/overview/`). A page pact is the
//! sibling file `<page>.pact.md` ([`page_pact_path`]). Everything here is
//! pure: the caller hands in what exists, nothing is read from disk.

use std::collections::{BTreeMap, BTreeSet};

use super::layout::{PACT_FILE, validate_reflection_path};
use super::provenance::PageRecord;
use crate::frontmatter::split_frontmatter;

/// The suffix of a page pact, beside its page: `arch/overview.pact.md` for
/// `arch/overview.md`.
pub const PAGE_PACT_SUFFIX: &str = ".pact.md";

/// The frontmatter key that says how often a page is brought in line.
pub const FRESHNESS_KEY: &str = "freshness";
/// The frontmatter key that names the page whose pact a page follows.
pub const FOLLOWS_KEY: &str = "follows";
/// The frontmatter key that retires a page with a word of the retired set.
pub const STATUS_KEY: &str = "status";
/// The key a finding names when the step it is about came from the page's
/// owner (the `owner` of its [`PageRecord`]).
pub const OWNER_KEY: &str = "owner";
/// The key a cycle finding names when the step that closed the cycle was a
/// folder parent, which no key spells.
pub const FOLDER_PARENT: &str = "folder parent";

/// The shortest freshness a page may have: one hour.
pub const MIN_FRESHNESS_SECONDS: i64 = 3600;

/// A `freshness` that does not read as a timeframe.
pub const FRESHNESS_UNREADABLE: &str = "is not a duration like 1h, 6h, 1d, 7d or 30d (a whole number and h, d, w, m for months or y), so it is ignored";
/// A `freshness` under [`MIN_FRESHNESS_SECONDS`].
pub const FRESHNESS_TOO_SHORT: &str = "is shorter than the minimum of 1 hour, so it is ignored";
/// A `freshness` on a page another page owns: the owner's reconcile keeps
/// it current.
pub const FRESHNESS_ON_OWNED_PAGE: &str =
    "is ignored on an owned page: reconciling its owner keeps it current";
/// A `follows` that is not a page path of this Reflection: no text, not a
/// valid page path, `PACT.md` or a pact file.
pub const FOLLOWS_NO_PAGE: &str =
    "names no page of this Reflection (a page path such as arch/overview.md), so it is ignored";
/// A `follows` that names a page of another domain.
pub const FOLLOWS_OTHER_DOMAIN: &str = "names a page of another domain; a page follows only pages of its own Reflection, so it is ignored";
/// A `follows` whose page does not exist.
pub const FOLLOWS_MISSING: &str = "names a page that does not exist, so it is ignored";
/// An `owner` whose page does not exist.
pub const OWNER_MISSING: &str = "names an owner page that does not exist, so it is ignored";
/// A step that leads back to a page already on the way; the chain stops
/// before it.
pub const FOLLOWS_CYCLE: &str = "leads back to a page already on the way, so the chain stops here";

/// Something a page says about itself that is not read the way its writer
/// likely meant, in the shape of the pact's
/// [`StructureFinding`](super::StructureFinding).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageFinding {
    /// The page the finding is about, by its path inside the Reflection. A
    /// finding of the pact chain may be about a page on the way up.
    pub page: String,
    /// The key: [`FRESHNESS_KEY`], [`FOLLOWS_KEY`], [`OWNER_KEY`] or
    /// [`FOLDER_PARENT`].
    pub key: &'static str,
    /// The value as written (YAML for a value that is no string).
    pub value: String,
    /// Why: one of the reason constants of this module.
    pub reason: &'static str,
}

/// The page pact beside the page at `page`.
pub fn page_pact_path(page: &str) -> String {
    let stem = page.strip_suffix(".md").unwrap_or(page);
    format!("{stem}{PAGE_PACT_SUFFIX}")
}

/// The page whose page pact `path` would be, when `path` ends in
/// [`PAGE_PACT_SUFFIX`] and that page could be a page: a valid reflection
/// path that is neither `PACT.md` (which has no page pact) nor a pact file
/// itself. Whether the page exists is not asked.
pub fn pact_page_of(path: &str) -> Option<String> {
    let stem = path.strip_suffix(PAGE_PACT_SUFFIX)?;
    let of = format!("{stem}.md");
    if of.ends_with(PAGE_PACT_SUFFIX) || of.eq_ignore_ascii_case(PACT_FILE) {
        return None;
    }
    validate_reflection_path(&of).is_ok().then_some(of)
}

/// Whether the pact file at `pact` has no page beside it. `pages` holds the
/// pages that exist.
pub fn is_loose_pact(pact: &str, pages: &BTreeMap<String, PageLinks>) -> bool {
    pact_page_of(pact).is_none_or(|of| !pages.contains_key(&of))
}

/// A page's freshness: how long a reconcile holds before the page is due
/// again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Freshness {
    /// The value as written (`6h`, `1d`), trimmed.
    pub written: String,
    /// The duration in seconds, at least [`MIN_FRESHNESS_SECONDS`].
    pub seconds: i64,
}

/// Read a freshness: any span of an hour or more, in `build_context`'s
/// timeframe grammar exactly ([`crate::time::timeframe_seconds`]), so a
/// person who knows one knows the other: `6h`, `1d`, `7d`, `2w`, `2m` (months
/// of 30 days), `1y`, and a bare number of days (`7`). There is no minute
/// unit. `Err` holds the reason: [`FRESHNESS_UNREADABLE`] when it does not
/// read, [`FRESHNESS_TOO_SHORT`] when it reads to less than
/// [`MIN_FRESHNESS_SECONDS`] (`0h`, `0d`, a negative span).
pub fn parse_freshness(spec: &str) -> Result<Freshness, &'static str> {
    let written = spec.trim();
    let seconds = crate::time::timeframe_seconds(written).ok_or(FRESHNESS_UNREADABLE)?;
    if seconds < MIN_FRESHNESS_SECONDS {
        return Err(FRESHNESS_TOO_SHORT);
    }
    Ok(Freshness {
        written: written.to_string(),
        seconds,
    })
}

/// What a page's frontmatter says about the page, as read. A value that does
/// not read is left out here and named in the findings
/// [`read_page_settings`] answers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PageSettings {
    /// How often the page is brought in line; `None` when absent, when it
    /// does not read, is under an hour or the page is owned.
    pub freshness: Option<Freshness>,
    /// The page whose pact this page follows, as written. Whether it names
    /// a page of this Reflection is asked where the pages are known
    /// ([`pact_chain`]).
    pub follows: Option<String>,
    /// The page's `status`, as written.
    pub status: Option<String>,
}

impl PageSettings {
    /// Whether the page is retired: its `status` is a word of the retired
    /// set ([`crate::RETIRED_STATUSES`]), case folded, as an engram's is.
    pub fn retired(&self) -> bool {
        self.status
            .as_deref()
            .is_some_and(|status| crate::is_retired_status(&status.trim().to_lowercase()))
    }

    /// The links [`pact_chain`] walks, with the owner of `record`.
    pub fn links(&self, record: &PageRecord) -> PageLinks {
        PageLinks {
            follows: self.follows.clone(),
            owner: record.owner.clone(),
        }
    }
}

/// The settings of the page at `page`, read from its `source`, and what did
/// not read. `record` is the page record of its provenance block: a
/// `freshness` on a page it names an owner for is a finding and is ignored.
/// A page without frontmatter, or whose frontmatter is no mapping, has no
/// settings and no findings.
pub fn read_page_settings(
    source: &str,
    page: &str,
    record: &PageRecord,
) -> (PageSettings, Vec<PageFinding>) {
    let mut settings = PageSettings::default();
    let mut findings = Vec::new();
    let Some(parts) = split_frontmatter(source) else {
        return (settings, findings);
    };
    let Ok(serde_yaml_ng::Value::Mapping(map)) = serde_yaml_ng::from_str(parts.yaml) else {
        return (settings, findings);
    };
    let finding = |key, value: String, reason| PageFinding {
        page: page.to_string(),
        key,
        value,
        reason,
    };
    if let Some(value) = map.get(FRESHNESS_KEY).filter(|v| !v.is_null()) {
        let read = match value {
            serde_yaml_ng::Value::String(spec) => parse_freshness(spec),
            // `freshness: 7` is the timeframe's bare number of days.
            serde_yaml_ng::Value::Number(n) => parse_freshness(&n.to_string()),
            _ => Err(FRESHNESS_UNREADABLE),
        };
        match read {
            Ok(_) if record.owner.is_some() => {
                findings.push(finding(
                    FRESHNESS_KEY,
                    as_written(value),
                    FRESHNESS_ON_OWNED_PAGE,
                ));
            }
            Ok(freshness) => settings.freshness = Some(freshness),
            Err(reason) => findings.push(finding(FRESHNESS_KEY, as_written(value), reason)),
        }
    }
    if let Some(value) = map.get(FOLLOWS_KEY).filter(|v| !v.is_null()) {
        match value {
            serde_yaml_ng::Value::String(target) => {
                settings.follows = Some(target.trim().to_string());
            }
            _ => findings.push(finding(FOLLOWS_KEY, as_written(value), FOLLOWS_NO_PAGE)),
        }
    }
    if let Some(serde_yaml_ng::Value::String(status)) = map.get(STATUS_KEY) {
        settings.status = Some(status.clone());
    }
    (settings, findings)
}

/// A YAML value as its writer would recognise it.
fn as_written(value: &serde_yaml_ng::Value) -> String {
    match value {
        serde_yaml_ng::Value::String(text) => text.clone(),
        other => serde_yaml_ng::to_string(other)
            .map(|text| text.trim().to_string())
            .unwrap_or_default(),
    }
}

/// What one page says about the page it follows: its `follows` as written
/// and its owner.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PageLinks {
    /// The `follows` of its frontmatter, as written.
    pub follows: Option<String>,
    /// The `owner` of its page record.
    pub owner: Option<String>,
}

/// The pacts a page follows, in the order they apply.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PactChain {
    /// The pages the page follows, from the top down, the page itself left
    /// out: the last one is the page it follows directly.
    pub followed: Vec<String>,
    /// The pact files, in the order they apply: `PACT.md` first (when it
    /// exists), then the page pact of each followed page that has one, from
    /// the top down, then the page's own page pact when it has one.
    pub pacts: Vec<String>,
    /// What was not read on the way: a `follows` that names no page, another
    /// domain's page or a missing one, a missing owner, a cycle.
    pub findings: Vec<PageFinding>,
}

/// The pact chain of the page at `page`. `pages` holds every page of the
/// Reflection that exists (not `PACT.md`, not a pact file) with its links;
/// `pact_files` holds the pact files that exist, `PACT.md` among them when
/// it does.
///
/// Each page on the way follows, in this order: its `follows` when that
/// names an existing page of this Reflection, else its owner when that
/// exists, else its nearest folder parent that exists (`a/b/c.md` follows
/// `a/b.md`, else `a.md`), else nothing. A `follows` or an owner that is not
/// taken is a finding, and the next rule is asked. A step back to a page
/// already on the way is a finding, and the chain stops there, so a cycle
/// written by hand or by a pull still resolves the same way every time.
pub fn pact_chain(
    page: &str,
    pages: &BTreeMap<String, PageLinks>,
    pact_files: &BTreeSet<String>,
) -> PactChain {
    let mut findings = Vec::new();
    let mut on_the_way = vec![page.to_string()];
    let mut current = page.to_string();
    while let Some((next, via)) = step(&current, pages, None, &mut findings) {
        if on_the_way.contains(&next) {
            findings.push(PageFinding {
                page: current.clone(),
                key: via,
                value: next,
                reason: FOLLOWS_CYCLE,
            });
            break;
        }
        on_the_way.push(next.clone());
        current = next;
    }
    let mut followed: Vec<String> = on_the_way.split_off(1);
    followed.reverse();
    let mut pacts = Vec::new();
    if pact_files.contains(PACT_FILE) {
        pacts.push(PACT_FILE.to_string());
    }
    for each in followed.iter().map(String::as_str).chain([page]) {
        let pact = page_pact_path(each);
        if pact_files.contains(&pact) {
            pacts.push(pact);
        }
    }
    PactChain {
        followed,
        pacts,
        findings,
    }
}

/// Whether giving the page at `page` the `follows: <target>` would close a
/// cycle: the walk up from `target`, with that `follows` in place, comes
/// back to `page`. A `target` that would not be taken (it names no existing
/// page) closes none. What a write that sets `follows` asks before it is
/// accepted.
pub fn would_cycle(page: &str, target: &str, pages: &BTreeMap<String, PageLinks>) -> bool {
    if target == page {
        return true;
    }
    let mut ignored = Vec::new();
    let replaced = Some((page, target));
    let Some((first, _)) = step(page, pages, replaced, &mut ignored) else {
        return false;
    };
    if first != target {
        return false;
    }
    let mut seen = BTreeSet::from([page.to_string()]);
    let mut current = first;
    loop {
        if current == page {
            return true;
        }
        if !seen.insert(current.clone()) {
            return false;
        }
        match step(&current, pages, replaced, &mut ignored) {
            Some((next, _)) => current = next,
            None => return false,
        }
    }
}

/// The page that `page` follows and the key that chose it, with a finding
/// for each rule that was asked and not taken. `replaced` stands in a
/// `follows` for one page (what [`would_cycle`] tries).
fn step(
    page: &str,
    pages: &BTreeMap<String, PageLinks>,
    replaced: Option<(&str, &str)>,
    findings: &mut Vec<PageFinding>,
) -> Option<(String, &'static str)> {
    let links = pages.get(page).cloned().unwrap_or_default();
    let follows = match replaced {
        Some((that, target)) if that == page => Some(target.to_string()),
        _ => links.follows,
    };
    let finding = |key, value: &str, reason| PageFinding {
        page: page.to_string(),
        key,
        value: value.to_string(),
        reason,
    };
    if let Some(target) = follows.as_deref() {
        match follows_target(target) {
            Err(reason) => findings.push(finding(FOLLOWS_KEY, target, reason)),
            Ok(()) if !pages.contains_key(target) => {
                findings.push(finding(FOLLOWS_KEY, target, FOLLOWS_MISSING));
            }
            Ok(()) => return Some((target.to_string(), FOLLOWS_KEY)),
        }
    }
    if let Some(owner) = links.owner.as_deref() {
        if pages.contains_key(owner) {
            return Some((owner.to_string(), OWNER_KEY));
        }
        findings.push(finding(OWNER_KEY, owner, OWNER_MISSING));
    }
    folder_parent(page, pages).map(|parent| (parent, FOLDER_PARENT))
}

/// Whether a `follows` value can name a page of this Reflection, before
/// asking whether that page exists.
fn follows_target(target: &str) -> Result<(), &'static str> {
    if target.starts_with(crate::address::SCHEME) {
        return Err(FOLLOWS_OTHER_DOMAIN);
    }
    let a_page = validate_reflection_path(target).is_ok()
        && !target.eq_ignore_ascii_case(PACT_FILE)
        && !target.ends_with(PAGE_PACT_SUFFIX)
        && !super::layout::is_under_reflections(target);
    if a_page { Ok(()) } else { Err(FOLLOWS_NO_PAGE) }
}

/// The nearest folder parent of `page` that exists: `a/b.md`, then `a.md`,
/// for `a/b/c.md`.
fn folder_parent(page: &str, pages: &BTreeMap<String, PageLinks>) -> Option<String> {
    let mut folder = page.rsplit_once('/')?.0;
    loop {
        let candidate = format!("{folder}.md");
        if pages.contains_key(&candidate) {
            return Some(candidate);
        }
        folder = folder.rsplit_once('/')?.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reflection::{ReflectionFile, classify};

    fn pages(list: &[(&str, Option<&str>, Option<&str>)]) -> BTreeMap<String, PageLinks> {
        list.iter()
            .map(|(path, follows, owner)| {
                (
                    path.to_string(),
                    PageLinks {
                        follows: follows.map(str::to_string),
                        owner: owner.map(str::to_string),
                    },
                )
            })
            .collect()
    }

    fn set(list: &[&str]) -> BTreeSet<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn reasons(findings: &[PageFinding]) -> Vec<(&str, &str, &str, &str)> {
        findings
            .iter()
            .map(|f| (f.page.as_str(), f.key, f.value.as_str(), f.reason))
            .collect()
    }

    #[test]
    fn a_page_pact_sits_beside_its_page() {
        assert_eq!(page_pact_path("arch/overview.md"), "arch/overview.pact.md");
        assert_eq!(page_pact_path("a.md"), "a.pact.md");
        assert_eq!(
            pact_page_of("arch/overview.pact.md").as_deref(),
            Some("arch/overview.md")
        );
        for no_page in [
            "PACT.pact.md",
            "pact.pact.md",
            "a.pact.pact.md",
            "a.threads.pact.md",
            "index.pact.md",
            "guides/.pact.md",
            ".pact.md",
            "a.md",
            "a.threads.md",
        ] {
            assert_eq!(pact_page_of(no_page), None, "{no_page}");
        }
    }

    #[test]
    fn classify_tells_a_page_pact_from_a_page() {
        assert_eq!(
            classify("arch/overview.pact.md"),
            ReflectionFile::PagePact {
                of: "arch/overview.md".into()
            }
        );
        assert_eq!(
            classify("arch/overview.pact.threads.md"),
            ReflectionFile::Threads {
                of: "arch/overview.pact.md".into()
            },
            "a page pact has threads of its own"
        );
        assert_eq!(
            classify(".reflected/arch/overview.pact.md"),
            ReflectionFile::Baseline {
                of: "arch/overview.pact.md".into()
            },
            "and a baseline"
        );
        assert_eq!(classify("PACT.md"), ReflectionFile::Pact, "unchanged");
        assert_eq!(
            classify("guides/pact.md"),
            ReflectionFile::Page("guides/pact.md".into()),
            "a page named pact is no page pact"
        );
        assert_eq!(
            classify("a.Pact.md"),
            ReflectionFile::Page("a.Pact.md".into()),
            "the suffix is matched as spelled, as the threads suffix is"
        );
        for stray in [
            "PACT.pact.md",
            "a.pact.pact.md",
            "a.threads.pact.md",
            "index.pact.md",
            ".reflected/PACT.pact.md",
        ] {
            assert_eq!(classify(stray), ReflectionFile::Other, "{stray}");
            if !stray.starts_with('.') {
                assert!(validate_reflection_path(stray).is_err(), "{stray}");
            }
        }
        assert!(validate_reflection_path("arch/overview.pact.md").is_ok());
    }

    #[test]
    fn a_pact_without_its_page_is_loose() {
        let existing = pages(&[("arch/overview.md", None, None)]);
        assert!(!is_loose_pact("arch/overview.pact.md", &existing));
        assert!(is_loose_pact("arch/gone.pact.md", &existing));
        assert!(is_loose_pact("PACT.pact.md", &existing));
    }

    #[test]
    fn freshness_reads_any_duration_of_an_hour_or_more() {
        for (spec, seconds) in [
            ("1h", 3600),
            ("6h", 6 * 3600),
            ("1d", 86_400),
            ("7d", 7 * 86_400),
            ("30d", 30 * 86_400),
            ("2w", 14 * 86_400),
            (" 1d ", 86_400),
            ("60h", 60 * 3600),
            // Any duration of an hour or more, not only the presets.
            ("2h", 2 * 3600),
            ("12h", 12 * 3600),
            ("3d", 3 * 86_400),
            ("14d", 14 * 86_400),
            ("90d", 90 * 86_400),
            ("1y", 365 * 86_400),
            // The timeframe's grammar exactly: a bare number is days, `m`
            // is months of 30 days.
            ("7", 7 * 86_400),
            ("2m", 60 * 86_400),
            ("7 d", 7 * 86_400),
            ("9223372036854775807w", i64::MAX),
        ] {
            let read = parse_freshness(spec).unwrap();
            assert_eq!(read.seconds, seconds, "{spec}");
            assert_eq!(read.written, spec.trim());
        }
    }

    #[test]
    fn a_freshness_that_does_not_read_or_is_too_short_is_refused() {
        for spec in [
            "",
            "d",
            "h",
            "1mo",
            "1s",
            "1min",
            "1.5d",
            "1H",
            "one day",
            "99999999999999999999d",
        ] {
            assert_eq!(parse_freshness(spec), Err(FRESHNESS_UNREADABLE), "{spec:?}");
        }
        // Under an hour: a zero or a negative span.
        for spec in ["0h", "0d", "0", "0m", "-1d"] {
            assert_eq!(parse_freshness(spec), Err(FRESHNESS_TOO_SHORT), "{spec}");
        }
        assert_eq!(MIN_FRESHNESS_SECONDS, 3600);
        assert_eq!(
            parse_freshness("1h").unwrap().seconds,
            MIN_FRESHNESS_SECONDS
        );
    }

    #[test]
    fn page_settings_read_from_the_frontmatter() {
        let source = "---\ntitle: Overview\nfreshness: 1d\nfollows: arch/index-page.md\nstatus: Archived\n---\n\n# Overview\n";
        let (settings, findings) =
            read_page_settings(source, "arch/overview.md", &PageRecord::default());
        assert!(findings.is_empty(), "{findings:?}");
        assert_eq!(settings.freshness.as_ref().map(|f| f.seconds), Some(86_400));
        assert_eq!(settings.follows.as_deref(), Some("arch/index-page.md"));
        assert_eq!(settings.status.as_deref(), Some("Archived"));
        assert!(settings.retired(), "a retired word in any case retires");

        for status in ["deprecated", "superseded", "archived", "legacy", " LEGACY "] {
            let s = PageSettings {
                status: Some(status.into()),
                ..PageSettings::default()
            };
            assert!(s.retired(), "{status}");
        }
        for status in ["stable", "draft", ""] {
            let s = PageSettings {
                status: Some(status.into()),
                ..PageSettings::default()
            };
            assert!(!s.retired(), "{status}");
        }

        let (settings, findings) =
            read_page_settings("# No frontmatter\n", "a.md", &PageRecord::default());
        assert_eq!(settings, PageSettings::default());
        assert!(findings.is_empty());
        let (settings, findings) =
            read_page_settings("---\nfreshness:\n---\n", "a.md", &PageRecord::default());
        assert_eq!(settings, PageSettings::default(), "no value is no setting");
        assert!(findings.is_empty());
    }

    #[test]
    fn a_freshness_that_does_not_read_is_a_finding_and_ignored() {
        for (yaml, value, reason) in [
            ("freshness: 0d", "0d", FRESHNESS_TOO_SHORT),
            ("freshness: 0", "0", FRESHNESS_TOO_SHORT),
            ("freshness: 1mo", "1mo", FRESHNESS_UNREADABLE),
            ("freshness: soon", "soon", FRESHNESS_UNREADABLE),
            ("freshness: true", "true", FRESHNESS_UNREADABLE),
            ("freshness: 0h", "0h", FRESHNESS_TOO_SHORT),
            ("freshness: [1d]", "- 1d", FRESHNESS_UNREADABLE),
        ] {
            let source = format!("---\n{yaml}\n---\n\n# Page\n");
            let (settings, findings) = read_page_settings(&source, "a.md", &PageRecord::default());
            assert_eq!(settings.freshness, None, "{yaml}");
            assert_eq!(
                reasons(&findings),
                vec![("a.md", FRESHNESS_KEY, value, reason)],
                "{yaml}"
            );
        }
    }

    #[test]
    fn a_bare_number_reads_as_days_as_in_the_timeframe() {
        let (settings, findings) =
            read_page_settings("---\nfreshness: 7\n---\n", "a.md", &PageRecord::default());
        assert!(findings.is_empty(), "{findings:?}");
        assert_eq!(
            settings.freshness,
            Some(Freshness {
                written: "7".into(),
                seconds: 7 * 86_400
            })
        );
    }

    #[test]
    fn a_freshness_on_an_owned_page_is_a_finding_and_ignored() {
        let record = PageRecord {
            owner: Some("arch/overview.md".into()),
            ..PageRecord::default()
        };
        let source = "---\nfreshness: 6h\n---\n\n# Child\n";
        let (settings, findings) = read_page_settings(source, "arch/overview/a.md", &record);
        assert_eq!(settings.freshness, None);
        assert_eq!(
            reasons(&findings),
            vec![(
                "arch/overview/a.md",
                FRESHNESS_KEY,
                "6h",
                FRESHNESS_ON_OWNED_PAGE
            )]
        );
        assert_eq!(
            settings.links(&record),
            PageLinks {
                follows: None,
                owner: Some("arch/overview.md".into())
            }
        );
    }

    #[test]
    fn a_follows_that_is_no_text_is_a_finding() {
        let (settings, findings) =
            read_page_settings("---\nfollows: 5\n---\n", "a.md", &PageRecord::default());
        assert_eq!(settings.follows, None);
        assert_eq!(
            reasons(&findings),
            vec![("a.md", FOLLOWS_KEY, "5", FOLLOWS_NO_PAGE)]
        );
    }

    #[test]
    fn a_page_follows_its_folder_parent_by_default() {
        let all = pages(&[
            ("arch.md", None, None),
            ("arch/overview.md", None, None),
            ("arch/overview/api.md", None, None),
            ("arch/deep/far/x.md", None, None),
            ("top.md", None, None),
        ]);
        let pacts = set(&[
            "PACT.md",
            "arch.pact.md",
            "arch/overview.pact.md",
            "arch/overview/api.pact.md",
        ]);
        let chain = pact_chain("arch/overview/api.md", &all, &pacts);
        assert_eq!(chain.followed, vec!["arch.md", "arch/overview.md"]);
        assert_eq!(
            chain.pacts,
            vec![
                "PACT.md",
                "arch.pact.md",
                "arch/overview.pact.md",
                "arch/overview/api.pact.md"
            ]
        );
        assert!(chain.findings.is_empty());

        let chain = pact_chain("arch/deep/far/x.md", &all, &pacts);
        assert_eq!(
            chain.followed,
            vec!["arch.md"],
            "a missing folder parent climbs to the nearest one that exists"
        );
        assert_eq!(chain.pacts, vec!["PACT.md", "arch.pact.md"]);

        let chain = pact_chain("top.md", &all, &pacts);
        assert!(chain.followed.is_empty());
        assert_eq!(chain.pacts, vec!["PACT.md"], "a root page follows PACT.md");

        let chain = pact_chain("top.md", &all, &set(&[]));
        assert!(chain.pacts.is_empty(), "no PACT.md, nothing to follow");
    }

    #[test]
    fn follows_comes_before_the_owner_and_the_owner_before_the_folder() {
        let all = pages(&[
            ("arch.md", None, None),
            ("risks.md", None, None),
            ("weekly.md", None, None),
            ("arch/a.md", Some("risks.md"), Some("weekly.md")),
            ("arch/b.md", None, Some("weekly.md")),
        ]);
        let pacts = set(&["PACT.md", "risks.pact.md", "weekly.pact.md", "arch.pact.md"]);
        let chain = pact_chain("arch/a.md", &all, &pacts);
        assert_eq!(chain.followed, vec!["risks.md"]);
        assert_eq!(chain.pacts, vec!["PACT.md", "risks.pact.md"]);
        let chain = pact_chain("arch/b.md", &all, &pacts);
        assert_eq!(chain.followed, vec!["weekly.md"]);
        assert_eq!(chain.pacts, vec!["PACT.md", "weekly.pact.md"]);
    }

    #[test]
    fn a_follows_that_is_not_taken_is_a_finding_and_the_next_rule_decides() {
        let all = pages(&[
            ("arch.md", None, None),
            ("weekly.md", None, None),
            ("arch/a.md", Some("gone.md"), None),
            ("arch/b.md", Some("crystalline://other/x.md"), None),
            ("arch/c.md", Some("PACT.md"), None),
            ("arch/d.md", Some("weekly.pact.md"), None),
            ("arch/e.md", Some("../weekly.md"), None),
            ("arch/f.md", Some("reflections/weekly.md"), None),
            ("arch/g.md", Some("gone.md"), Some("weekly.md")),
            ("arch/h.md", None, Some("gone.md")),
        ]);
        let pacts = set(&["PACT.md", "arch.pact.md"]);
        for (page, value, reason) in [
            ("arch/a.md", "gone.md", FOLLOWS_MISSING),
            (
                "arch/b.md",
                "crystalline://other/x.md",
                FOLLOWS_OTHER_DOMAIN,
            ),
            ("arch/c.md", "PACT.md", FOLLOWS_NO_PAGE),
            ("arch/d.md", "weekly.pact.md", FOLLOWS_NO_PAGE),
            ("arch/e.md", "../weekly.md", FOLLOWS_NO_PAGE),
            ("arch/f.md", "reflections/weekly.md", FOLLOWS_NO_PAGE),
        ] {
            let chain = pact_chain(page, &all, &pacts);
            assert_eq!(
                reasons(&chain.findings),
                vec![(page, FOLLOWS_KEY, value, reason)],
                "{page}"
            );
            assert_eq!(chain.followed, vec!["arch.md"], "{page}: folder parent");
        }
        let chain = pact_chain("arch/g.md", &all, &pacts);
        assert_eq!(chain.followed, vec!["weekly.md"], "then the owner");
        let chain = pact_chain("arch/h.md", &all, &pacts);
        assert_eq!(
            reasons(&chain.findings),
            vec![("arch/h.md", OWNER_KEY, "gone.md", OWNER_MISSING)]
        );
        assert_eq!(chain.followed, vec!["arch.md"]);
    }

    #[test]
    fn findings_name_the_page_on_the_way_they_are_about() {
        let all = pages(&[
            ("a.md", Some("gone.md"), None),
            ("b.md", Some("a.md"), None),
        ]);
        let chain = pact_chain("b.md", &all, &set(&["PACT.md", "a.pact.md", "b.pact.md"]));
        assert_eq!(chain.followed, vec!["a.md"]);
        assert_eq!(chain.pacts, vec!["PACT.md", "a.pact.md", "b.pact.md"]);
        assert_eq!(
            reasons(&chain.findings),
            vec![("a.md", FOLLOWS_KEY, "gone.md", FOLLOWS_MISSING)]
        );
    }

    #[test]
    fn a_cycle_is_a_finding_and_stops_at_the_first_repeat() {
        let all = pages(&[
            ("a.md", Some("b.md"), None),
            ("b.md", Some("c.md"), None),
            ("c.md", Some("a.md"), None),
        ]);
        let pacts = set(&["PACT.md", "a.pact.md", "b.pact.md", "c.pact.md"]);
        let chain = pact_chain("a.md", &all, &pacts);
        assert_eq!(chain.followed, vec!["c.md", "b.md"]);
        assert_eq!(
            chain.pacts,
            vec!["PACT.md", "c.pact.md", "b.pact.md", "a.pact.md"]
        );
        assert_eq!(
            reasons(&chain.findings),
            vec![("c.md", FOLLOWS_KEY, "a.md", FOLLOWS_CYCLE)]
        );
        assert_eq!(
            chain,
            pact_chain("a.md", &all, &pacts),
            "the same every time"
        );

        // A page that follows itself.
        let own = pages(&[("a.md", Some("a.md"), None)]);
        let chain = pact_chain("a.md", &own, &pacts);
        assert!(chain.followed.is_empty());
        assert_eq!(chain.pacts, vec!["PACT.md", "a.pact.md"]);
        assert_eq!(
            reasons(&chain.findings),
            vec![("a.md", FOLLOWS_KEY, "a.md", FOLLOWS_CYCLE)]
        );

        // A follows down into the page's own folder, which climbs back up.
        let down = pages(&[("a.md", Some("a/b.md"), None), ("a/b.md", None, None)]);
        let chain = pact_chain("a.md", &down, &pacts);
        assert_eq!(chain.followed, vec!["a/b.md"]);
        assert_eq!(
            reasons(&chain.findings),
            vec![("a/b.md", FOLDER_PARENT, "a.md", FOLLOWS_CYCLE)]
        );
    }

    #[test]
    fn would_cycle_answers_before_a_follows_is_written() {
        let all = pages(&[
            ("a.md", None, None),
            ("b.md", Some("a.md"), None),
            ("c.md", None, Some("b.md")),
            ("a/x.md", None, None),
            ("d.md", None, None),
        ]);
        assert!(would_cycle("a.md", "a.md", &all), "itself");
        assert!(would_cycle("a.md", "b.md", &all), "through a follows");
        assert!(would_cycle("a.md", "c.md", &all), "through an owner");
        assert!(
            would_cycle("a.md", "a/x.md", &all),
            "through a folder parent"
        );
        assert!(!would_cycle("a.md", "d.md", &all));
        assert!(!would_cycle("d.md", "c.md", &all));
        assert!(
            !would_cycle("a.md", "gone.md", &all),
            "a follows that is not taken closes nothing"
        );
        let looped = pages(&[
            ("x.md", Some("y.md"), None),
            ("y.md", Some("x.md"), None),
            ("z.md", None, None),
        ]);
        assert!(
            !would_cycle("z.md", "x.md", &looped),
            "a cycle above that does not reach the page is not this write's"
        );
    }
}
