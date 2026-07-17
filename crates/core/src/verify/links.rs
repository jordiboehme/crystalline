//! L-family rules: link quality.
//!
//! Resolution reuses [`crate::address::resolve`] against a [`LookupTable`]
//! built once across every scanned Domain, so a `[[domain:Target]]` link can
//! resolve against a domain other than the one it was written in. A target
//! whose named domain is outside the scan set cannot be judged broken or
//! sound, so it is only ever informational (`L006`), never a warning.

use std::borrow::Cow;
use std::collections::{BTreeSet, HashMap};

use crate::address::{self, CrystallineUrl, LinkResolver, LookupTable, Resolution};
use crate::engram::{Engram, LinkTarget};
use crate::parse::{body_lines, mask_inline_code};

use super::scanner::{Domain, ScannedFile};
use super::util::body_line_start;
use super::{Severity, Sink};

/// Build the cross-domain permalink/title lookup used to resolve every
/// wikilink and relation target in the scan set.
pub(crate) fn build_lookup(domains: &[Domain]) -> LookupTable {
    let mut table = LookupTable::new();
    for domain in domains {
        for file in &domain.files {
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
        .unwrap_or_else(|| address::slugify(&file.rel_path.to_string_lossy()))
}

pub(crate) fn check(
    domain: &Domain,
    domain_names: &BTreeSet<&str>,
    lookup: &LookupTable,
    sink: &mut Sink,
) {
    check_duplicates(domain, sink);

    for file in &domain.files {
        let Ok(engram) = &file.parsed else { continue };
        let own_permalink = effective_permalink(file, engram);
        let own_title = engram.frontmatter.title.trim().to_lowercase();

        for link in &engram.links {
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
        for rel in &engram.relations {
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

        check_crystalline_urls(file, engram, domain_names, lookup, sink);
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
            sink.emit(
                &file.path,
                line,
                "L001",
                Severity::Warning,
                format!("broken wikilink to `{}`", target.target),
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
                    format!("broken wikilink to `{target_domain}:{}`", target.target),
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

/// L005: a literal `crystalline://` URL in prose (not `[[...]]` syntax)
/// naming a domain that is in the scan set but a permalink that is not.
fn check_crystalline_urls(
    file: &ScannedFile,
    engram: &Engram,
    domain_names: &BTreeSet<&str>,
    lookup: &LookupTable,
    sink: &mut Sink,
) {
    let start = body_line_start(&file.source);
    for bl in body_lines(&engram.body, start) {
        if bl.in_fence {
            continue;
        }
        let masked: Cow<'_, str> = if bl.text.contains('`') {
            Cow::Owned(mask_inline_code(bl.text))
        } else {
            Cow::Borrowed(bl.text)
        };
        for url in find_urls(&masked) {
            let Some(parsed) = CrystallineUrl::parse(&url) else {
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
