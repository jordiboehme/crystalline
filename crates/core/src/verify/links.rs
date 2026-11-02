//! L-family rules: link quality.
//!
//! Resolution reuses [`crate::address::resolve`] against a [`LookupTable`]
//! built once across every scanned Domain, so a `[[domain:Target]]` link can
//! resolve against a domain other than the one it was written in. A target
//! whose named domain is outside the scan set cannot be judged broken or
//! sound, so it is only ever informational (`L006`), never a warning. A bare
//! target that does not resolve in its own domain is looked up in the rest of
//! the scan set, and L001 names the prefixed link when one holds it.

use std::borrow::Cow;
use std::collections::{BTreeSet, HashMap};

use crate::address::{self, CrystallineUrl, LinkResolver, LookupTable, Resolution};
use crate::engram::{Engram, LinkForm, LinkTarget};
use crate::parse::{BodyLine, mask_inline_code};

use super::scanner::{Domain, ScannedFile};
use super::{Severity, Sink};

/// Build the cross-domain permalink/title lookup used to resolve every
/// wikilink and relation target in the scan set.
pub(crate) fn build_lookup(domains: &[Domain]) -> LookupTable {
    let mut table = LookupTable::new();
    for domain in domains {
        // Declared whether or not it holds anything a link can land on, so
        // `[[name:Target]]` is judged against the scan set rather than against
        // whichever domains happened to parse.
        table.register_domain(&domain.name);
        for file in &domain.files {
            table.insert_path(&domain.name, &rel_path_of(file));
            let Ok(engram) = &file.parsed else { continue };
            let permalink = effective_permalink(file, engram);
            table.insert(&domain.name, &permalink, &engram.frontmatter.title);
        }
    }
    table
}

/// The permalink a link resolves against: the frontmatter value when
/// present, otherwise the domain-relative path slugified the same way a
/// write tool would auto-generate one.
pub(crate) fn effective_permalink(file: &ScannedFile, engram: &Engram) -> String {
    engram
        .frontmatter
        .permalink
        .clone()
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| address::path_permalink(&file.rel_path.to_string_lossy()))
}

/// `file_lines` holds each file's engram body tokenized once by the verify
/// entry point (`run_rules` in `mod.rs`), aligned by index with
/// `domain.files`, and shared here rather than retokenized per file.
pub(crate) fn check(
    domain: &Domain,
    domain_names: &BTreeSet<&str>,
    lookup: &LookupTable,
    file_lines: &[Vec<BodyLine>],
    sink: &mut Sink,
) {
    check_duplicates(domain, sink);
    // What a relative link can land on: the domain's files, by the path a link
    // spells them with.
    let paths: BTreeSet<String> = domain.files.iter().map(rel_path_of).collect();

    for (file, lines) in domain.files.iter().zip(file_lines) {
        let Ok(engram) = &file.parsed else { continue };
        let own_permalink = effective_permalink(file, engram);
        let own_title = engram.frontmatter.title.trim().to_lowercase();
        let own_path = rel_path_of(file);
        // A reference that binds by path: a relative link, or a bare wikilink
        // spelled as one of the domain's files (`[[pumps/p-12.md]]`), which
        // the index's path arm binds to that file the same way. A permalink
        // never ends in `.md`, so such a spelling names nothing else.
        let by_path = |t: &LinkTarget| {
            t.form == LinkForm::Relative || (t.domain.is_none() && paths.contains(&t.target))
        };

        for (target, line) in engram
            .links
            .iter()
            .map(|l| (&l.target, l.line))
            .chain(engram.relations.iter().map(|r| (&r.target, r.line)))
            .filter(|(t, _)| by_path(t))
        {
            check_relative(file, target, line, &own_path, &paths, sink);
        }

        // A wikilink spelled as a file path of a named domain in the scan set
        // (`[[ops:pumps/p.md]]`) binds by that path there, as the index's
        // path arm binds it; a permalink never ends in `.md`.
        let by_named_path = |t: &LinkTarget| {
            t.form == LinkForm::Wiki
                && t.target.to_ascii_lowercase().ends_with(".md")
                && t.domain
                    .as_deref()
                    .is_some_and(|d| domain_names.contains(d))
        };
        for (target, line) in engram
            .links
            .iter()
            .map(|l| (&l.target, l.line))
            .chain(engram.relations.iter().map(|r| (&r.target, r.line)))
            .filter(|(t, _)| !by_path(t) && by_named_path(t))
        {
            let named = target.domain.as_deref().unwrap_or_default();
            if !lookup.has_path(named, &target.target) {
                sink.emit(
                    &file.path,
                    Some(line),
                    "L001",
                    Severity::Warning,
                    format!("broken wikilink to `{named}:{}`", target.target),
                    None,
                );
            }
        }

        for link in engram
            .links
            .iter()
            .filter(|l| !by_path(&l.target) && !by_named_path(&l.target))
        {
            check_target(
                domain,
                file,
                &link.target,
                Some(link.line),
                domain_names,
                lookup,
                &own_permalink,
                &own_title,
                sink,
            );
        }
        for rel in engram
            .relations
            .iter()
            .filter(|r| !by_path(&r.target) && !by_named_path(&r.target))
        {
            check_target(
                domain,
                file,
                &rel.target,
                Some(rel.line),
                domain_names,
                lookup,
                &own_permalink,
                &own_title,
                sink,
            );
        }

        check_crystalline_urls(file, lines, domain_names, lookup, sink);
        check_file_links(domain, file, lines, &own_path, sink);
    }
}

/// A file's domain-relative path, forward-slashed: what a relative link to it
/// resolves to.
fn rel_path_of(file: &ScannedFile) -> String {
    file.rel_path.to_string_lossy().replace('\\', "/")
}

/// A reference that binds by path is judged by the file it lands on: one of
/// the domain's files is sound (the file itself is a self-link, `L004`),
/// anything else is broken (`L001`), including a path that climbs out of the
/// domain.
fn check_relative(
    file: &ScannedFile,
    target: &LinkTarget,
    line: usize,
    own_path: &str,
    paths: &BTreeSet<String>,
    sink: &mut Sink,
) {
    let landed = target.stored_target(own_path);
    if landed == own_path {
        sink.emit(
            &file.path,
            Some(line),
            "L004",
            Severity::Warning,
            format!("self-link to `{}`", target.target),
            None,
        );
    } else if !paths.contains(&landed) {
        sink.emit(
            &file.path,
            Some(line),
            "L001",
            Severity::Warning,
            format!("broken link to `{}`", target.target),
            None,
        );
    }
}

fn check_duplicates(domain: &Domain, sink: &mut Sink) {
    let mut by_permalink: HashMap<String, Vec<&ScannedFile>> = HashMap::new();
    let mut by_title: HashMap<String, Vec<&ScannedFile>> = HashMap::new();

    for file in &domain.files {
        let Ok(engram) = &file.parsed else { continue };
        by_permalink
            .entry(effective_permalink(file, engram))
            .or_default()
            .push(file);
        let title = engram.frontmatter.title.trim().to_lowercase();
        if !title.is_empty() {
            by_title.entry(title).or_default().push(file);
        }
    }

    for (permalink, files) in &by_permalink {
        if files.len() > 1 {
            for file in &files[1..] {
                let others = other_paths(files, file);
                sink.emit(
                    &file.path,
                    None,
                    "L002",
                    Severity::Error,
                    format!("permalink `{permalink}` is also used by {others}"),
                    None,
                );
            }
        }
    }
    for (title, files) in &by_title {
        if files.len() > 1 {
            for file in &files[1..] {
                let others = other_paths(files, file);
                sink.emit(
                    &file.path,
                    None,
                    "L003",
                    Severity::Warning,
                    format!("title `{title}` is also used by {others}"),
                    None,
                );
            }
        }
    }
}

fn other_paths(files: &[&ScannedFile], excluding: &ScannedFile) -> String {
    files
        .iter()
        .filter(|f| f.path != excluding.path)
        .map(|f| f.path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

#[allow(clippy::too_many_arguments)]
fn check_target(
    domain: &Domain,
    file: &ScannedFile,
    target: &LinkTarget,
    line: Option<usize>,
    domain_names: &BTreeSet<&str>,
    lookup: &LookupTable,
    own_permalink: &str,
    own_title: &str,
    sink: &mut Sink,
) {
    if target.domain.is_none() {
        let text = target.target.to_lowercase();
        if text == own_permalink.to_lowercase() || text == own_title {
            sink.emit(
                &file.path,
                line,
                "L004",
                Severity::Warning,
                format!("self-link to `{}`", target.target),
                None,
            );
            return;
        }
    }

    match address::resolve(target, &domain.name, lookup) {
        Resolution::Resolved(r) => {
            if r.domain == domain.name && r.permalink.eq_ignore_ascii_case(own_permalink) {
                sink.emit(
                    &file.path,
                    line,
                    "L004",
                    Severity::Warning,
                    format!("self-link to `{}`", target.target),
                    None,
                );
            }
        }
        Resolution::Unresolved => {
            // A bare target resolves only in its own domain. When the scan set
            // holds it elsewhere, the fix is one prefix away, so name it.
            let mut found = found_elsewhere(&target.target, &domain.name, domain_names, lookup);
            if let Some(whole) = &target.whole {
                for elsewhere in found_elsewhere(whole, &domain.name, domain_names, lookup) {
                    if !found.contains(&elsewhere) {
                        found.push(elsewhere);
                    }
                }
                found.sort_unstable();
            }
            // Quoted with its fragment, as it is written.
            sink.emit(
                &file.path,
                line,
                "L001",
                Severity::Warning,
                broken_bare_message(&target.written(), &found),
                None,
            );
        }
        Resolution::CrossDomainUnresolved {
            domain: target_domain,
        } => {
            if domain_names.contains(target_domain.as_str()) {
                sink.emit(
                    &file.path,
                    line,
                    "L001",
                    Severity::Warning,
                    format!("broken wikilink to `{}`", target.written()),
                    None,
                );
            } else {
                sink.emit(
                    &file.path,
                    line,
                    "L006",
                    Severity::Info,
                    format!(
                        "link references domain `{target_domain}`, which is outside the scan set"
                    ),
                    None,
                );
            }
        }
    }
}

/// The other domains of the scan set that hold `target` as a permalink or a
/// title, in the scan set's sorted order. `own` is left out: a bare target
/// was already looked up there and missed.
fn found_elsewhere<'a>(
    target: &str,
    own: &str,
    domain_names: &BTreeSet<&'a str>,
    lookup: &LookupTable,
) -> Vec<&'a str> {
    domain_names
        .iter()
        .copied()
        .filter(|d| *d != own)
        .filter(|d| {
            lookup.by_permalink(d, target).is_some() || lookup.by_title(d, target).is_some()
        })
        .collect()
}

/// L001's text for a bare link: the plain message, plus the domains that hold
/// the target and the prefixed link to write when there are any.
fn broken_bare_message(target: &str, found: &[&str]) -> String {
    if found.is_empty() {
        return format!("broken wikilink to `{target}`");
    }
    let names: Vec<String> = found.iter().map(|d| format!("`{d}`")).collect();
    let links: Vec<String> = found
        .iter()
        .map(|d| format!("`[[{d}:{target}]]`"))
        .collect();
    format!(
        "broken wikilink to `{target}` (found in {}, link it as {})",
        join_list(&names, "and"),
        join_list(&links, "or")
    )
}

/// `a`, `a and b`, `a, b and c`: no comma before the last item.
fn join_list(items: &[String], last: &str) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [head @ .., tail] => format!("{} {last} {tail}", head.join(", ")),
    }
}

/// L005: a literal `crystalline://` URL in prose (not `[[...]]` syntax)
/// naming a domain that is in the scan set but a permalink that is not.
fn check_crystalline_urls(
    file: &ScannedFile,
    lines: &[BodyLine],
    domain_names: &BTreeSet<&str>,
    lookup: &LookupTable,
    sink: &mut Sink,
) {
    for bl in lines {
        if bl.in_fence {
            continue;
        }
        let masked: Cow<'_, str> = if bl.text.contains('`') {
            Cow::Owned(mask_inline_code(bl.text))
        } else {
            Cow::Borrowed(bl.text)
        };
        for url in find_urls(&masked) {
            // A `#fragment` names a place inside the engram, never which
            // engram: `crystalline://d/b#setup` is checked as
            // `crystalline://d/b`, and quoted as written.
            let address = url.split_once('#').map_or(url.as_str(), |(a, _)| a);
            let Some(parsed) = CrystallineUrl::parse(address) else {
                continue;
            };
            if parsed.glob {
                continue;
            }
            if !domain_names.contains(parsed.domain.as_str()) {
                continue;
            }
            if lookup
                .by_permalink(&parsed.domain, &parsed.permalink)
                .is_none()
            {
                sink.emit(
                    &file.path,
                    Some(bl.line_no),
                    "L005",
                    Severity::Warning,
                    format!("broken crystalline:// URL: `{url}`"),
                    None,
                );
            }
        }
    }
}

/// L007: a relative link or image whose destination is not a note and names
/// no file in the domain, such as an attachment an import left behind. A note
/// link (`.md`) is L001's, a URL, `crystalline://` link, mail link, absolute
/// path and bare `#anchor` are not files of this domain, and a destination
/// with a `?query` is a request rather than a path, so none of them is
/// judged here. The path is resolved against the engram's folder and looked
/// up under the domain root only: one that climbs out of the domain is
/// reported without ever being read.
fn check_file_links(
    domain: &Domain,
    file: &ScannedFile,
    lines: &[BodyLine],
    own_path: &str,
    sink: &mut Sink,
) {
    use crate::reflection::links::{code_spans, is_relative, read_destination, resolve_in_domain};

    for bl in lines.iter().filter(|bl| !bl.in_fence) {
        if !bl.text.contains("](") {
            continue;
        }
        let spans = code_spans(bl.text);
        let in_code = |at: usize| spans.iter().any(|(s, e)| (*s..*e).contains(&at));
        let mut seen: Vec<String> = Vec::new();
        let mut cursor = 0;
        while let Some(found) = bl.text[cursor..].find("](") {
            let open = cursor + found + 2;
            cursor = open;
            let Some(dest) = read_destination(bl.text, open) else {
                continue;
            };
            cursor = dest.close;
            let path = dest.path;
            if in_code(open)
                || !is_relative(&path)
                || path.contains('?')
                || path.to_ascii_lowercase().ends_with(".md")
                || seen.contains(&path)
            {
                continue;
            }
            let present = resolve_in_domain(own_path, &path)
                .is_some_and(|landed| domain.root.join(landed).exists());
            if !present {
                sink.emit(
                    &file.path,
                    Some(bl.line_no),
                    "L007",
                    Severity::Warning,
                    format!(
                        "`{own_path}` links to `{path}`, but the domain has no such file. Add the file, or fix or remove the link."
                    ),
                    None,
                );
            }
            seen.push(path);
        }
    }
}

fn find_urls(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = line;
    while let Some(pos) = rest.find(address::SCHEME) {
        let tail = &rest[pos..];
        let end = tail
            .find(|c: char| c.is_whitespace() || c == ')' || c == ']' || c == '"' || c == '\'')
            .unwrap_or(tail.len());
        out.push(
            tail[..end]
                .trim_end_matches(['.', ',', ';', ':'])
                .to_string(),
        );
        rest = &tail[end..];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_link_found_nowhere_keeps_the_plain_message() {
        assert_eq!(
            broken_bare_message("Some Title", &[]),
            "broken wikilink to `Some Title`"
        );
    }

    #[test]
    fn a_bare_link_found_in_one_other_domain_names_the_prefixed_form() {
        assert_eq!(
            broken_bare_message("Some Title", &["other-domain"]),
            "broken wikilink to `Some Title` (found in `other-domain`, link it as `[[other-domain:Some Title]]`)"
        );
    }

    #[test]
    fn several_domains_are_listed_in_order_without_an_oxford_comma() {
        assert_eq!(
            broken_bare_message("X", &["a", "b"]),
            "broken wikilink to `X` (found in `a` and `b`, link it as `[[a:X]]` or `[[b:X]]`)"
        );
        assert_eq!(
            broken_bare_message("X", &["a", "b", "c"]),
            "broken wikilink to `X` (found in `a`, `b` and `c`, link it as `[[a:X]]`, `[[b:X]]` or `[[c:X]]`)"
        );
    }

    #[test]
    fn found_elsewhere_matches_title_or_permalink_and_skips_the_own_domain() {
        let mut lookup = LookupTable::new();
        lookup.insert("own", "local", "Local");
        lookup.insert("ops", "runbook", "Some Title");
        lookup.insert("pay", "some-title", "Other");
        lookup.insert("zzz", "unrelated", "Unrelated");
        let names: BTreeSet<&str> = ["own", "ops", "pay", "zzz"].into_iter().collect();
        assert_eq!(
            found_elsewhere("some title", "own", &names, &lookup),
            vec!["ops"]
        );
        assert_eq!(
            found_elsewhere("some-title", "own", &names, &lookup),
            vec!["pay"]
        );
        lookup.insert("own", "dup", "Some Title");
        assert_eq!(
            found_elsewhere("Some Title", "own", &names, &lookup),
            vec!["ops"]
        );
    }

    #[test]
    fn found_elsewhere_lists_several_domains_in_the_scan_sets_sorted_order() {
        // Registered in reverse order, so the result's order comes from the
        // scan set and not from the lookup table.
        let mut lookup = LookupTable::new();
        lookup.insert("zeta", "shared", "Shared Title");
        lookup.insert("mid", "other", "Shared Title");
        lookup.insert("alpha", "shared", "Alpha");
        let names: BTreeSet<&str> = ["own", "zeta", "mid", "alpha"].into_iter().collect();
        assert_eq!(
            found_elsewhere("Shared Title", "own", &names, &lookup),
            vec!["mid", "zeta"]
        );
        assert_eq!(
            found_elsewhere("shared", "own", &names, &lookup),
            vec!["alpha", "zeta"]
        );
    }
}
