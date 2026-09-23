//! String-surgical reference rewriting for an engram whose address changed.
//!
//! A move is a refactoring: when an engram leaves its domain, changes its
//! permalink or both, every reference that named it by its old address is
//! rewritten to the new one, the way a rename in an editor rewrites every
//! call site. [`relink`] does that for one referencing engram's markdown
//! source, and nothing else: finding the referencing engrams is the index's
//! job, and deciding who may rewrite what is the engine's.
//!
//! Three reference shapes are rewritten, all in the body only:
//!
//! 1. a wikilink, bare or prefixed - `[[old]]`, `[[domain:old]]` - which
//!    covers prose links and relation bullets alike, since a relation's target
//!    is written in the same brackets;
//! 2. a bare wikilink that named the engram by **title** and now has to carry
//!    a domain prefix, because the engram left the domain the link resolves
//!    in. A title link whose domain did not change is left exactly as it was:
//!    the title did not change, so the link still resolves, and rewriting it
//!    to the permalink would replace the author's words with an identifier;
//! 3. a `crystalline://domain/old` URL, with or without a `#fragment` after
//!    it. The fragment is kept.
//!
//! The scan mirrors the indexer's own reading of a body, so what is rewritten
//! is exactly what the index counted as a reference: a fenced code block and
//! an inline code span are never touched, since `[[x]]` inside code is an
//! example rather than a link. Every byte outside a rewritten reference is
//! kept verbatim, and a source whose frontmatter does not parse comes back
//! unchanged - the index never read a reference out of it either.

use crate::address::SCHEME;
use crate::engram::LinkTarget;
use crate::parse::{body_lines, mask_inline_code, parse_engram_lossless};

/// The address change a set of references has to follow.
#[derive(Debug, Clone, Copy)]
pub struct Relink<'a> {
    /// The domain the engram lived in.
    pub from_domain: &'a str,
    /// The permalink it answered to.
    pub from_permalink: &'a str,
    /// Its title, which a wikilink may name it by. Unchanged by a move.
    pub title: &'a str,
    /// The domain it lives in now.
    pub to_domain: &'a str,
    /// The permalink it answers to now.
    pub to_permalink: &'a str,
}

impl Relink<'_> {
    /// Whether the address changed at all. A relink that changes nothing is a
    /// caller's no-op, and [`relink`] answers it without scanning.
    pub fn changes_address(&self) -> bool {
        self.from_domain != self.to_domain || self.from_permalink != self.to_permalink
    }
}

/// How a wikilink's target text named the engram.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Named {
    Permalink,
    Title,
}

/// Rewrite every reference to the engram `spec` describes inside `source`,
/// answering the new source and how many references were rewritten.
///
/// `read_in` is the domain the source's bare links resolve in as it stands,
/// and `written_in` the domain it will live in once written. The two are the
/// same for every referencing engram; they differ only for the moved engram's
/// own self-references, which were written in the domain it is leaving and
/// have to read right in the one it lands in.
///
/// The resolution rule is the index's: a permalink match wins, then a
/// case-insensitive title match. One deliberate gap follows from rewriting
/// text rather than rows: a bare target that equals this engram's title and,
/// at the same time, some other engram's permalink resolved to that other
/// engram, and it is rewritten here only on a domain change. The index's
/// inbound query already narrowed the sources to engrams that reference this
/// one, so the case needs one engram to reference both under one spelling.
pub fn relink(source: &str, read_in: &str, written_in: &str, spec: &Relink<'_>) -> (String, usize) {
    if !spec.changes_address() && read_in == written_in {
        return (source.to_string(), 0);
    }
    let Ok(lossless) = parse_engram_lossless(source) else {
        return (source.to_string(), 0);
    };
    let body_start = lossless.body_span.start;
    let body = &source[body_start..];
    let lines = body_lines(body, lossless.body_line_start);
    let mut out = String::with_capacity(source.len());
    out.push_str(&source[..body_start]);
    let mut count = 0usize;
    // `body_lines` splits on '\n' exactly as this loop does, so the two walk
    // the same lines in the same order; the carriage return it trims from its
    // text is put back from the raw segment here.
    for (index, raw) in body.split('\n').enumerate() {
        if index > 0 {
            out.push('\n');
        }
        let in_fence = lines.get(index).is_some_and(|line| line.in_fence);
        if in_fence || !(raw.contains("[[") || raw.contains(SCHEME)) {
            out.push_str(raw);
            continue;
        }
        let (rewritten, n) = relink_line(raw, read_in, written_in, spec);
        out.push_str(&rewritten);
        count += n;
    }
    (out, count)
}

/// One body line, outside any fence.
///
/// Worked on chars rather than bytes because [`mask_inline_code`] preserves
/// the char count of the line and not its byte count: a multibyte char inside
/// a code span becomes one space, so a byte offset read off the mask would
/// land in the wrong place of the original.
fn relink_line(line: &str, read_in: &str, written_in: &str, spec: &Relink<'_>) -> (String, usize) {
    let original: Vec<char> = line.chars().collect();
    let masked: Vec<char> = if line.contains('`') {
        mask_inline_code(line).chars().collect()
    } else {
        original.clone()
    };
    let url_old: Vec<char> = format!("{SCHEME}{}/{}", spec.from_domain, spec.from_permalink)
        .chars()
        .collect();
    let url_new = format!("{SCHEME}{}/{}", spec.to_domain, spec.to_permalink);
    let mut out = String::with_capacity(line.len());
    let mut count = 0usize;
    let mut i = 0usize;
    let n = masked.len();
    while i < n {
        // A wikilink: the same `[[` ... first `]]` pairing the parser's
        // `find_wikilinks` reads, over the same masked text. An opening pair
        // with no closing one anywhere on the rest of the line is no wikilink,
        // and neither is anything after it, but a URL still can be.
        if masked[i] == '['
            && masked.get(i + 1) == Some(&'[')
            && let Some(close) = find_close(&masked, i + 2)
        {
            let inner: String = masked[i + 2..close].iter().collect();
            if let Some(new_inner) = relink_wikilink(&inner, read_in, written_in, spec) {
                out.push_str("[[");
                out.push_str(&new_inner);
                out.push_str("]]");
                count += 1;
            } else {
                out.extend(&original[i..close + 2]);
            }
            i = close + 2;
            continue;
        }
        if spec.changes_address() && masked[i] == 'c' && starts_with(&masked, i, &url_old) {
            let end = i + url_old.len();
            // The permalink has to end here. A following permalink char means
            // the URL names a longer permalink (`old-notes`) or a folder glob
            // under it (`old/*`), neither of which is this engram.
            let bounded = masked
                .get(end)
                .is_none_or(|c| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '/' | '_')));
            if bounded {
                out.push_str(&url_new);
                count += 1;
                i = end;
                continue;
            }
        }
        out.push(original[i]);
        i += 1;
    }
    (out, count)
}

/// The index of the first `]]` at or after `from`.
fn find_close(chars: &[char], from: usize) -> Option<usize> {
    (from..chars.len().saturating_sub(1)).find(|&k| chars[k] == ']' && chars[k + 1] == ']')
}

fn starts_with(chars: &[char], at: usize, needle: &[char]) -> bool {
    chars.len() >= at + needle.len() && chars[at..at + needle.len()] == *needle
}

/// The new bracket text for one wikilink, or `None` when it does not name the
/// moved engram or already reads right.
fn relink_wikilink(
    inner: &str,
    read_in: &str,
    written_in: &str,
    spec: &Relink<'_>,
) -> Option<String> {
    let target = LinkTarget::parse(inner);
    let named = |text: &str| -> Option<Named> {
        let title = spec.title.trim();
        if text == spec.from_permalink {
            Some(Named::Permalink)
        } else if !title.is_empty() && text.to_lowercase() == title.to_lowercase() {
            Some(Named::Title)
        } else {
            None
        }
    };
    let how = match &target.domain {
        // A prefix that names the domain the engram left: the link points here
        // wherever it was written.
        Some(domain) if domain == spec.from_domain => named(&target.target)?,
        // A prefix naming any other domain points somewhere else. A colon
        // title (`[[Log: Weekly Notes]]`) parses the same way; whether its
        // head is a registered domain is a question only the registry can
        // answer, so it is left alone rather than guessed at.
        Some(_) => return None,
        // Bare: it resolves in the domain the source is read in.
        None if read_in == spec.from_domain => named(&target.target)?,
        None => return None,
    };
    let new_target = match how {
        Named::Permalink => spec.to_permalink,
        // The author's own words: a title link keeps its text and only gains,
        // or changes, the prefix that says where it resolves.
        Named::Title => target.target.as_str(),
    };
    // A link that carried a prefix keeps one, now naming the new domain; a
    // bare link gains one only when it no longer resolves where it is written.
    let new_domain = match &target.domain {
        Some(_) => Some(spec.to_domain),
        None if written_in != spec.to_domain => Some(spec.to_domain),
        None => None,
    };
    // Unchanged text is not a rewrite: a title link inside a domain the engram
    // never left, or a link the move did not reach at all.
    if new_domain == target.domain.as_deref() && new_target == target.target {
        return None;
    }
    Some(match new_domain {
        Some(domain) => format!("{domain}:{new_target}"),
        None => new_target.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAME: Relink<'static> = Relink {
        from_domain: "eng",
        from_permalink: "velog/alpha",
        title: "Alpha Notes",
        to_domain: "eng",
        to_permalink: "projects/velog/alpha",
    };

    const CROSS: Relink<'static> = Relink {
        from_domain: "eng",
        from_permalink: "alpha",
        title: "Alpha Notes",
        to_domain: "ops",
        to_permalink: "alpha",
    };

    fn doc(body: &str) -> String {
        format!("---\ntitle: Linker\ntype: engram\nstatus: stable\n---\n\n{body}")
    }

    #[test]
    fn a_permalink_change_rewrites_bare_prefixed_relation_and_url_references() {
        let source = doc("See [[velog/alpha]] and [[eng:velog/alpha]].\n\
             - relates_to [[velog/alpha]]\n\
             Anchor crystalline://eng/velog/alpha#setup and crystalline://eng/velog/alpha.\n");
        let (out, n) = relink(&source, "eng", "eng", &SAME);
        assert_eq!(n, 5, "{out}");
        assert!(out.contains("See [[projects/velog/alpha]] and [[eng:projects/velog/alpha]]."));
        assert!(out.contains("- relates_to [[projects/velog/alpha]]"));
        assert!(out.contains("crystalline://eng/projects/velog/alpha#setup"));
        assert!(out.contains("crystalline://eng/projects/velog/alpha."));
        assert!(out.starts_with("---\ntitle: Linker\n"), "frontmatter kept");
    }

    #[test]
    fn a_title_link_stays_when_the_domain_did_not_change() {
        let source = doc("By title [[Alpha Notes]] and [[alpha notes]].\n");
        let (out, n) = relink(&source, "eng", "eng", &SAME);
        assert_eq!(n, 0);
        assert_eq!(out, source);
    }

    #[test]
    fn a_cross_domain_move_prefixes_bare_links_and_repoints_prefixed_ones() {
        let source = doc("[[alpha]], [[Alpha Notes]], [[eng:alpha]], crystalline://eng/alpha\n");
        let (out, n) = relink(&source, "eng", "eng", &CROSS);
        assert_eq!(n, 4, "{out}");
        assert!(out.contains(
            "[[ops:alpha]], [[ops:Alpha Notes]], [[ops:alpha]], crystalline://ops/alpha"
        ));
    }

    #[test]
    fn a_linker_in_the_destination_domain_keeps_its_prefix_form() {
        let source = doc("From ops: [[eng:alpha]]\n");
        let (out, n) = relink(&source, "ops", "ops", &CROSS);
        assert_eq!(n, 1);
        assert!(out.contains("From ops: [[ops:alpha]]"));
    }

    #[test]
    fn a_bare_link_in_another_domain_names_something_else() {
        let source = doc("Their own [[alpha]] and [[Alpha Notes]].\n");
        let (out, n) = relink(&source, "ops", "ops", &CROSS);
        assert_eq!(n, 0);
        assert_eq!(out, source);
    }

    #[test]
    fn self_references_read_in_the_old_domain_and_write_for_the_new_one() {
        // Bare in the domain it leaves, bare again in the one it lands in: the
        // link needs no prefix to find its own engram there.
        let source = doc("Back to the top: [[alpha]] and [[eng:alpha]]\n");
        let (out, n) = relink(&source, "eng", "ops", &CROSS);
        assert_eq!(n, 1, "{out}");
        assert!(
            out.contains("Back to the top: [[alpha]] and [[ops:alpha]]"),
            "{out}"
        );
        let renamed = Relink {
            to_permalink: "guides/alpha",
            ..CROSS
        };
        let (out, n) = relink(&source, "eng", "ops", &renamed);
        assert_eq!(n, 2, "{out}");
        assert!(
            out.contains("[[guides/alpha]] and [[ops:guides/alpha]]"),
            "{out}"
        );
    }

    #[test]
    fn code_is_never_rewritten() {
        let source = doc("Inline `[[velog/alpha]]` stays.\n\
             ```\n[[velog/alpha]] crystalline://eng/velog/alpha\n```\n\
             Real [[velog/alpha]].\n");
        let (out, n) = relink(&source, "eng", "eng", &SAME);
        assert_eq!(n, 1, "{out}");
        assert!(out.contains("Inline `[[velog/alpha]]` stays."));
        assert!(out.contains("```\n[[velog/alpha]] crystalline://eng/velog/alpha\n```"));
        assert!(out.contains("Real [[projects/velog/alpha]]."));
    }

    #[test]
    fn a_longer_permalink_or_a_folder_glob_is_not_this_engram() {
        let source = doc(
            "[[velog/alpha-two]] crystalline://eng/velog/alpha-two crystalline://eng/velog/alpha/*\n",
        );
        let (out, n) = relink(&source, "eng", "eng", &SAME);
        assert_eq!(n, 0);
        assert_eq!(out, source);
    }

    #[test]
    fn multibyte_text_around_a_code_span_keeps_its_bytes() {
        let source = doc("日本 `コード` [[velog/alpha]] 語\r\nnext line\n");
        let (out, n) = relink(&source, "eng", "eng", &SAME);
        assert_eq!(n, 1);
        assert!(
            out.contains("日本 `コード` [[projects/velog/alpha]] 語\r\nnext line\n"),
            "{out}"
        );
    }

    #[test]
    fn a_source_that_does_not_parse_comes_back_unchanged() {
        let source = "---\ntitle: [unclosed\n---\n[[velog/alpha]]\n";
        let (out, n) = relink(source, "eng", "eng", &SAME);
        assert_eq!(n, 0);
        assert_eq!(out, source);
    }
}
