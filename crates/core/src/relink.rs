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
//! unchanged - the index never read a reference out of it either. The walker
//! itself (`rewrite_body_lines`) is shared with every other kind of document
//! that rewrites links; a dialect decides which lines can hold its links.
//!
//! A relative Markdown link to another engram's file (`[p](../pumps/p.md)`)
//! names a path, not an address, so it follows the file rather than the
//! address: [`respell_relative_links`] rewrites the links to a file that
//! moved inside its domain, and [`rebase_relative_links`] rebases the moved
//! engram's own relative links onto its new folder. A wikilink spelled as the
//! file's path (`[[pumps/p.md]]`) binds by path too, so
//! [`respell_path_wikilinks`] makes it follow the file the same way.

use crate::address::SCHEME;
use crate::engram::LinkTarget;
use crate::parse::{body_lines, mask_inline_code, mask_link_destinations, parse_engram_lossless};

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
    /// Whole link texts with a `#` ([`LinkTarget::whole`]) that the caller
    /// found as the title of another engram where the link is read
    /// (`[[C# Basics]]` beside a moved `C`). Such a link points there, not
    /// at the moved engram, so it is left as written. Compared in any letter
    /// case. Empty when the caller looked nothing up.
    pub shadowed: &'a [String],
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
    rewrite_body_lines(source, BodyGate::Lossless, &|line| {
        through(LinkDialect::Addressed, line, |l| {
            relink_line(l, read_in, written_in, spec)
        })
    })
}

/// Rewrite the domain part of every `[[x:target]]` wikilink and every
/// `crystalline://x...` URL in the body where `respell(x)` answers a
/// different name. Same scan as [`relink`]: body only, fences and inline
/// code untouched, every other byte kept.
pub fn respell_domains(source: &str, respell: &dyn Fn(&str) -> Option<String>) -> (String, usize) {
    respell_domains_with(source, BodyGate::Lossless, respell)
}

/// [`respell_domains`] over a body the gate names: `BodyGate::Split` for a
/// document that is never an engram.
pub fn respell_domains_with(
    source: &str,
    gate: BodyGate,
    respell: &dyn Fn(&str) -> Option<String>,
) -> (String, usize) {
    rewrite_body_lines(source, gate, &|line| {
        through(LinkDialect::Addressed, line, |l| respell_line(l, respell))
    })
}

/// How a rewrite finds the body of the source it walks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyGate {
    /// An engram: the body as the lossless engram parser cuts it, and a
    /// source whose frontmatter does not parse comes back untouched (the
    /// index never read a reference out of it either).
    Lossless,
    /// A document that is never an engram: the body after a frontmatter
    /// block when there is one (never parsed), else the whole source.
    Split,
}

/// The links a rewrite follows. Each dialect knows which lines can hold one,
/// and its line rewrite does the rest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkDialect {
    /// Engram references: wikilinks and `crystalline://` addresses.
    Addressed,
    /// A reflection's relative markdown links.
    Relative,
}

impl LinkDialect {
    /// Whether `line` can hold a link of this dialect at all: the cheap test
    /// before a line is handed to the rewrite.
    pub fn may_hold(self, line: &str) -> bool {
        match self {
            LinkDialect::Addressed => line.contains("[[") || line.contains(SCHEME),
            LinkDialect::Relative => line.contains("]("),
        }
    }
}

/// Walk the body one line at a time: frontmatter kept verbatim, fenced lines
/// kept, every other line handed to `line`, which answers the line and how
/// many links it changed. Every byte outside a changed line is kept, CR
/// included. The one walker every link rewrite runs on.
pub(crate) fn rewrite_body_lines(
    source: &str,
    gate: BodyGate,
    line: &dyn Fn(&str) -> (String, usize),
) -> (String, usize) {
    let (body_start, body_line_start) = match gate {
        BodyGate::Lossless => match parse_engram_lossless(source) {
            Ok(lossless) => (lossless.body_span.start, lossless.body_line_start),
            Err(_) => return (source.to_string(), 0),
        },
        BodyGate::Split => match crate::frontmatter::split_frontmatter(source) {
            Some(parts) => {
                let start = source.len() - parts.body.len();
                (start, source[..start].matches('\n').count() + 1)
            }
            None => (0, 1),
        },
    };
    let body = &source[body_start..];
    let lines = body_lines(body, body_line_start);
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
        if lines.get(index).is_some_and(|l| l.in_fence) {
            out.push_str(raw);
            continue;
        }
        let (rewritten, n) = line(raw);
        out.push_str(&rewritten);
        count += n;
    }
    (out, count)
}

/// One line through a dialect's rewrite, skipped when the dialect says the
/// line cannot hold a link.
fn through(
    dialect: LinkDialect,
    raw: &str,
    rewrite: impl Fn(&str) -> (String, usize),
) -> (String, usize) {
    if dialect.may_hold(raw) {
        rewrite(raw)
    } else {
        (raw.to_string(), 0)
    }
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
    let wiki = wikilink_mask(line, &masked);
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
        if wiki[i] == '['
            && wiki.get(i + 1) == Some(&'[')
            && let Some(close) = find_close(&wiki, i + 2)
        {
            let end = target_end(&wiki, i + 2, close);
            let inner: String = wiki[i + 2..end].iter().collect();
            if let Some(new_inner) = relink_wikilink(&inner, read_in, written_in, spec) {
                out.push_str("[[");
                out.push_str(&new_inner);
                // The `#fragment` and the `|display text`, as written.
                out.extend(&original[fragment_start(&inner, i + 2, end)..close]);
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

/// The chars a line's wikilinks are looked for in: `code_masked` (the line
/// with inline code masked) with every inline link destination masked too,
/// as the parser's `mask_for_wikilinks` reads it, so a move never rewrites a
/// `[[...]]` the parser reads as part of a path. Both masks keep the line's
/// char count, so an index into one is an index into the other and into the
/// original. A URL is still looked for in `code_masked`: a
/// `crystalline://` address in a destination is a reference that follows.
fn wikilink_mask(line: &str, code_masked: &[char]) -> Vec<char> {
    if line.contains("](") {
        let code: String = code_masked.iter().collect();
        mask_link_destinations(&code).chars().collect()
    } else {
        code_masked.to_vec()
    }
}

/// The index of the first `]]` at or after `from`.
fn find_close(chars: &[char], from: usize) -> Option<usize> {
    (from..chars.len().saturating_sub(1)).find(|&k| chars[k] == ']' && chars[k + 1] == ']')
}

/// Where the `#fragment` of the wikilink whose target part `inner` runs from
/// `from` to `end` starts, as a char index: at the whitespace before its first
/// `#`, the split [`crate::engram::split_fragment`] makes, or at `end` when it
/// has none. A rewrite replaces only the trimmed target before it and keeps
/// the fragment, and the spacing before its `#`, exactly as written.
fn fragment_start(inner: &str, from: usize, end: usize) -> usize {
    match crate::engram::split_fragment(inner) {
        (before, Some(_)) => from + before.trim_end().chars().count(),
        _ => end,
    }
}

/// Where the target part of the wikilink whose text runs from `from` to
/// `close` ends: at its first `|`, the split [`crate::engram::split_alias`]
/// makes, or at `close` when it has no display text. A rewrite replaces only
/// the target part and keeps `|display text` exactly as it was written.
fn target_end(chars: &[char], from: usize, close: usize) -> usize {
    (from..close).find(|&k| chars[k] == '|').unwrap_or(close)
}

fn starts_with(chars: &[char], at: usize, needle: &[char]) -> bool {
    chars.len() >= at + needle.len() && chars[at..at + needle.len()] == *needle
}

/// The new target part for one wikilink, before its `#fragment`, or `None`
/// when it does not name the moved engram or already reads right. The whole
/// text of a link whose title holds a `#` ([`LinkTarget::whole`]) is tried as
/// the title first, as the resolver tries it; written back with the fragment
/// as the caller keeps it, it reads exactly as before.
fn relink_wikilink(
    inner: &str,
    read_in: &str,
    written_in: &str,
    spec: &Relink<'_>,
) -> Option<String> {
    let target = LinkTarget::parse(inner);
    if let Some(whole) = &target.whole
        && spec
            .shadowed
            .iter()
            .any(|other| other.to_lowercase() == whole.to_lowercase())
    {
        return None;
    }
    let named = |text: &str| -> Option<Named> {
        let title = spec.title.trim();
        if let Some(whole) = &target.whole
            && !title.is_empty()
            && whole.to_lowercase() == title.to_lowercase()
        {
            return Some(Named::Title);
        }
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
    // One exception: when the engram changed domain into the very domain the
    // link is written in, the prefix would name the link's own domain, so the
    // link is written bare (`[[ops:x]]` in an engram of `ops` reads `[[x]]`),
    // unless the bare text would itself read as a prefix (a title such as
    // `Log: Weekly`). A move inside one domain keeps a prefix the author
    // wrote.
    let new_domain = match &target.domain {
        Some(_)
            if spec.from_domain != spec.to_domain
                && written_in == spec.to_domain
                && LinkTarget::parse(new_target).domain.is_none() =>
        {
            None
        }
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

/// One body line, outside any fence, for [`respell_domains`]. Same char-based
/// walk as [`relink_line`], over the same masked text, but the only question
/// asked of each construct is whether its domain part reads differently now.
fn respell_line(line: &str, respell: &dyn Fn(&str) -> Option<String>) -> (String, usize) {
    let original: Vec<char> = line.chars().collect();
    let masked: Vec<char> = if line.contains('`') {
        mask_inline_code(line).chars().collect()
    } else {
        original.clone()
    };
    let wiki = wikilink_mask(line, &masked);
    let scheme: Vec<char> = SCHEME.chars().collect();
    let mut out = String::with_capacity(line.len());
    let mut count = 0usize;
    let mut i = 0usize;
    let n = masked.len();
    while i < n {
        if wiki[i] == '['
            && wiki.get(i + 1) == Some(&'[')
            && let Some(close) = find_close(&wiki, i + 2)
        {
            let end = target_end(&wiki, i + 2, close);
            let inner: String = wiki[i + 2..end].iter().collect();
            let raw_inner: String = original[i + 2..end].iter().collect();
            if let Some(new_inner) = respell_wikilink(&inner, &raw_inner, respell) {
                out.push_str("[[");
                out.push_str(&new_inner);
                out.extend(&original[end..close]);
                out.push_str("]]");
                count += 1;
            } else {
                out.extend(&original[i..close + 2]);
            }
            i = close + 2;
            continue;
        }
        if masked[i] == 'c' && starts_with(&masked, i, &scheme) {
            let run_start = i + scheme.len();
            let mut end = run_start;
            while end < n && is_domain_char(masked[end]) {
                end += 1;
            }
            let run: String = original[run_start..end].iter().collect();
            let trimmed = run.trim_end_matches('.');
            if !trimmed.is_empty()
                && let Some(new_name) = respell(trimmed)
                && new_name != trimmed
            {
                out.push_str(SCHEME);
                out.push_str(&new_name);
                count += 1;
                i = run_start + trimmed.chars().count();
                continue;
            }
        }
        out.push(original[i]);
        i += 1;
    }
    (out, count)
}

/// A char that can be part of the bare domain run right after `crystalline://`.
/// A run of them, with any trailing `.` trimmed, is the address's authority.
pub fn is_domain_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '-' | '_' | '.')
}

/// The new bracket text for one wikilink under [`respell_domains`], or `None`
/// when it carries no domain prefix or `respell` leaves it as it reads.
///
/// `masked_inner` (the same masked text the caller walked to find the target)
/// is parsed for structure; `raw_inner` is the original bracket text, whose
/// first colon and surrounding whitespace are kept exactly as written, with
/// only the trimmed domain segment before it replaced.
fn respell_wikilink(
    masked_inner: &str,
    raw_inner: &str,
    respell: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    let target = LinkTarget::parse(masked_inner);
    let domain = target.domain.as_deref()?;
    let new_domain = respell(domain)?;
    if new_domain == domain {
        return None;
    }
    let colon = raw_inner.find(':')?;
    let (left, rest) = raw_inner.split_at(colon);
    let lead_len = left.len() - left.trim_start().len();
    let trail_len = left.trim_end().len();
    let leading = &left[..lead_len];
    let trailing = &left[trail_len..];
    Some(format!("{leading}{new_domain}{trailing}{rest}"))
}

/// Rewrite the relative links in the engram at the domain-relative
/// `source_path` that land on the file `from` so they land on `to`, both
/// domain-relative paths in the same domain. Each new target is spelled from
/// the folder of `source_path`; a `#fragment` and a title stay as written.
/// Only the links the parser reads as links to another engram's file are
/// touched (a target ending in `.md`, not an image, outside code), in the
/// body as the lossless engram parser cuts it.
pub fn respell_relative_links(
    source: &str,
    source_path: &str,
    from: &str,
    to: &str,
) -> (String, usize) {
    use crate::reflection::links::{map_note_links, relative_to, resolve};
    map_note_links(source, &|target| {
        (resolve(source_path, target).as_deref() == Some(from))
            .then(|| relative_to(source_path, to))
    })
}

/// Rebase the relative links of an engram that moved from the
/// domain-relative path `from` to `to` inside its domain, so each lands on the
/// file it landed on before. A link to the engram itself follows it to `to`.
/// A link that climbs out of the domain, and an `assets/` path (domain-root
/// relative, so a move inside the domain leaves it valid), stay as written,
/// as does every link that is not a link to another engram's file.
pub fn rebase_relative_links(source: &str, from: &str, to: &str) -> (String, usize) {
    use crate::reflection::links::{map_note_links, relative_to, resolve};
    map_note_links(source, &|target| {
        if crate::is_under_assets(target) {
            return None;
        }
        let landed = resolve(from, target).filter(|path| !path.is_empty())?;
        let landed = if landed == from { to } else { landed.as_str() };
        let rebased = relative_to(to, landed);
        (rebased != target.trim_start_matches("./")).then_some(rebased)
    })
}

pub use crate::reflection::links::{NoteLinkTo, WholeLinks};

/// Rewrite the relative links to another engram's file in `source` through
/// `map`, which is handed each decoded target as written (no `#fragment`, no
/// title) and answers a new destination, a wikilink address that replaces
/// the whole link, or `None` to keep it. A move into another domain needs
/// the address: a relative path never reaches into another domain, so a link
/// that crosses has to name the domain. The link text stays as the
/// wikilink's display text. Same body walk as [`rebase_relative_links`].
pub fn readdress_relative_links(
    source: &str,
    map: &dyn Fn(&str) -> Option<NoteLinkTo>,
) -> WholeLinks {
    crate::reflection::links::map_note_links_whole(source, map)
}

/// Rewrite every bare wikilink whose target text `map` answers, so it names
/// the answer instead (`[[pumps/b.md]]` to `[[ops:pumps/b]]`); the display
/// text after a `|` stays as written. A prefixed wikilink and one inside
/// code are never handed to `map`.
pub fn readdress_path_wikilinks(
    source: &str,
    map: &dyn Fn(&str) -> Option<String>,
) -> (String, usize) {
    readdress_wikilinks(source, &|domain, target, _| {
        domain.is_none().then(|| map(target)).flatten()
    })
}

/// Rewrite every prefixed wikilink whose domain prefix and target `map`
/// answers (`[[notes:guides/a.md]]` to `[[notes:archive/a.md]]`), so it names
/// the answer instead; the display text after a `|` stays as written. A bare
/// wikilink and one inside code are never handed to `map`.
pub fn readdress_prefixed_path_wikilinks(
    source: &str,
    map: &dyn Fn(&str, &str) -> Option<String>,
) -> (String, usize) {
    readdress_wikilinks(source, &|domain, target, _| {
        domain.and_then(|d| map(d, target))
    })
}

/// What [`readdress_wikilinks`] asks of each wikilink: its domain prefix
/// (none for a bare one), its target, and whether it shows a display text;
/// the answer is the new target part, or `None` to keep the link.
pub type WikilinkMap<'a> = dyn Fn(Option<&str>, &str, bool) -> Option<String> + 'a;

/// Rewrite every wikilink, bare or prefixed, whose domain prefix (none for a
/// bare one) and target `map` answers, so its target part reads the answer;
/// the third argument says whether the link shows a display text after a
/// `|`, which stays as written. A link inside code is never handed to `map`.
/// The one walk behind every wikilink readdress here.
pub fn readdress_wikilinks(source: &str, map: &WikilinkMap<'_>) -> (String, usize) {
    rewrite_body_lines(source, BodyGate::Lossless, &|line| {
        through(LinkDialect::Addressed, line, |l| path_wikilink_line(l, map))
    })
}

/// Rewrite every bare wikilink spelled as the domain-relative file path `from`
/// (`[[pumps/b.md]]`) so it names `to`. The index binds such a link by path,
/// and the path is read from the domain root, so it needs no respelling from
/// the folder of the engram that holds it. A prefixed wikilink, one inside
/// code and one naming any other text stay as written.
pub fn respell_path_wikilinks(source: &str, from: &str, to: &str) -> (String, usize) {
    readdress_wikilinks(source, &|domain, target, _| {
        (domain.is_none() && target == from).then(|| to.to_string())
    })
}

/// One body line under [`readdress_wikilinks`]: the wikilinks found the way
/// [`relink_line`] finds them, over the line with inline code masked, and
/// every one whose domain prefix (none for a bare one), target and display
/// text flag `map` answers renamed to the answer.
fn path_wikilink_line(line: &str, map: &WikilinkMap<'_>) -> (String, usize) {
    let original: Vec<char> = line.chars().collect();
    let code: Vec<char> = if line.contains('`') {
        mask_inline_code(line).chars().collect()
    } else {
        original.clone()
    };
    let masked = wikilink_mask(line, &code);
    let mut out = String::with_capacity(line.len());
    let mut count = 0usize;
    let mut i = 0usize;
    while i < masked.len() {
        if masked[i] == '['
            && masked.get(i + 1) == Some(&'[')
            && let Some(close) = find_close(&masked, i + 2)
        {
            let end = target_end(&masked, i + 2, close);
            let inner: String = masked[i + 2..end].iter().collect();
            let target = LinkTarget::parse(&inner);
            let domain = target.domain.as_deref();
            let shown = end < close;
            // The whole text first, as the resolver reads it: a title that
            // holds a `#` is replaced as a whole, fragment and all.
            let whole = target
                .whole
                .as_deref()
                .and_then(|whole| map(domain, whole, shown));
            if let Some(to) = whole {
                out.push_str("[[");
                out.push_str(&to);
                out.extend(&original[end..close]);
                out.push_str("]]");
                count += 1;
            } else if let Some(to) = map(domain, &target.target, shown) {
                // The answer replaces the text before the `#fragment`, which
                // stays as written, before any display text the answer adds.
                let (to_target, to_alias) = match to.find('|') {
                    Some(bar) => to.split_at(bar),
                    None => (to.as_str(), ""),
                };
                out.push_str("[[");
                out.push_str(to_target);
                out.extend(&original[fragment_start(&inner, i + 2, end)..end]);
                out.push_str(to_alias);
                out.extend(&original[end..close]);
                out.push_str("]]");
                count += 1;
            } else {
                out.extend(&original[i..close + 2]);
            }
            i = close + 2;
            continue;
        }
        out.push(original[i]);
        i += 1;
    }
    (out, count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wikilink_spelled_as_the_moved_path_follows_it() {
        let source = format!(
            "{ENGRAM_HEAD}- part_of [[pumps/b.md]]\n\nSee [[pumps/b.md]], [[ops:pumps/b.md]], \
             [[Pumps/B.md]], [[pumps/b]] and `[[pumps/b.md]]`.\n\n```\n[[pumps/b.md]]\n```\n"
        );
        let (out, n) = respell_path_wikilinks(&source, "pumps/b.md", "archive/b.md");
        assert_eq!(n, 2, "{out}");
        assert!(out.contains("- part_of [[archive/b.md]]\n"), "{out}");
        assert!(
            out.contains(
                "See [[archive/b.md]], [[ops:pumps/b.md]], [[Pumps/B.md]], [[pumps/b]] and `[[pumps/b.md]]`."
            ),
            "{out}"
        );
        assert!(out.contains("```\n[[pumps/b.md]]\n```"), "{out}");
    }

    #[test]
    fn readdress_wikilinks_hands_the_map_every_link_and_whether_it_shows_a_text() {
        let source = format!(
            "{ENGRAM_HEAD}- relates_to [[b]]\n\nSee [[b|bee]], [[ops:x]], [[Cee]] and `[[b]]`.\n\n\
             ```\n[[b]]\n```\n"
        );
        let (out, n) = readdress_wikilinks(&source, &|domain, target, shown| match (
            domain, target, shown,
        ) {
            (None, "b", _) => Some("notes:b".to_string()),
            (Some("ops"), "x", false) => Some("x".to_string()),
            (None, "Cee", false) => Some("notes:c|Cee".to_string()),
            _ => None,
        });
        assert_eq!(n, 4, "{out}");
        assert!(out.contains("- relates_to [[notes:b]]\n"), "{out}");
        assert!(
            out.contains("See [[notes:b|bee]], [[x]], [[notes:c|Cee]] and `[[b]]`."),
            "{out}"
        );
        assert!(out.contains("```\n[[b]]\n```"), "{out}");
    }

    #[test]
    fn a_path_wikilink_with_a_display_text_follows_the_move_and_keeps_it() {
        let source = format!("{ENGRAM_HEAD}See [[pumps/b.md|the pump]] and [[pumps/b.md|]].\n");
        let (out, n) = respell_path_wikilinks(&source, "pumps/b.md", "archive/b.md");
        assert_eq!(n, 2, "{out}");
        assert!(
            out.contains("See [[archive/b.md|the pump]] and [[archive/b.md|]]."),
            "{out}"
        );
    }

    #[test]
    fn relink_keeps_the_alias() {
        let moved = Relink {
            from_domain: "eng",
            from_permalink: "pumps/p-12",
            title: "Pump Twelve",
            to_domain: "eng",
            to_permalink: "pumps/p-13",
            shadowed: &[],
        };
        let source = doc("See [[pumps/p-12|the pump]] and [[eng:pumps/p-12|it]].\n\
             - part_of [[pumps/p-12|the pump]]\n\
             By title [[Pump Twelve|twelve]] and code `[[pumps/p-12|x]]`.\n");
        let (out, n) = relink(&source, "eng", "eng", &moved);
        assert_eq!(n, 3, "{out}");
        assert!(
            out.contains("See [[pumps/p-13|the pump]] and [[eng:pumps/p-13|it]]."),
            "{out}"
        );
        assert!(out.contains("- part_of [[pumps/p-13|the pump]]"), "{out}");
        assert!(
            out.contains("By title [[Pump Twelve|twelve]] and code `[[pumps/p-12|x]]`."),
            "{out}"
        );
        // A move to another domain prefixes a title link and keeps its words.
        let (out, n) = relink(&doc("[[Alpha Notes|the notes]]\n"), "eng", "eng", &CROSS);
        assert_eq!(n, 1, "{out}");
        assert!(out.contains("[[ops:Alpha Notes|the notes]]"), "{out}");
    }

    #[test]
    fn a_domain_respell_keeps_the_alias() {
        let (out, n) = respell_domains(
            &spell_doc("See [[old:runbook|the runbook]] and [[runbook|old:x]].\n"),
            &old_to_new,
        );
        assert_eq!(n, 1, "{out}");
        assert!(
            out.ends_with("See [[new:runbook|the runbook]] and [[runbook|old:x]].\n"),
            "{out}"
        );
    }

    const ENGRAM_HEAD: &str =
        "---\ntype: engram\ntitle: B\npermalink: b\nstatus: stable\n---\n\n# B\n\n";

    #[test]
    fn relative_links_to_a_moved_file_are_respelled_from_the_linker() {
        let source = format!(
            "{ENGRAM_HEAD}- part_of [p](../pumps/p.md \"P\")\n\nSee [p](../pumps/p.md#top), \
             ![p](../pumps/p.md), `[p](../pumps/p.md)` and [q](../pumps/q.md).\n\n\
             ```\n[p](../pumps/p.md)\n```\n"
        );
        let (out, n) = respell_relative_links(&source, "deep/a.md", "pumps/p.md", "archive/p.md");
        assert_eq!(n, 2, "{out}");
        assert!(
            out.contains("- part_of [p](../archive/p.md \"P\")\n"),
            "{out}"
        );
        assert!(out.contains("See [p](../archive/p.md#top), ![p](../pumps/p.md), `[p](../pumps/p.md)` and [q](../pumps/q.md)."), "{out}");
        assert!(out.contains("```\n[p](../pumps/p.md)\n```"), "{out}");
        assert!(out.starts_with(ENGRAM_HEAD), "{out}");
    }

    /// A file name with an ideographic space (U+3000) or a no-break space
    /// (U+00A0) is one destination, so a move rewrites the whole link and
    /// what it writes is read back whole.
    #[test]
    fn a_link_to_a_file_name_with_a_unicode_space_follows_a_move() {
        let from = "pumps/\u{65e5}\u{672c}\u{3000}\u{5831}\u{544a}.md";
        let to = "archive/new\u{a0}name.md";
        let source = format!(
            "{ENGRAM_HEAD}- part_of [R](../pumps/\u{65e5}\u{672c}\u{3000}\u{5831}\u{544a}.md)\n\n\
             See [r](../pumps/%E6%97%A5%E6%9C%AC%E3%80%80%E5%A0%B1%E5%91%8A.md#top \"R\").\n"
        );
        let (out, n) = respell_relative_links(&source, "deep/a.md", from, to);
        assert_eq!(n, 2, "{out}");
        assert!(
            out.contains("- part_of [R](../archive/new\u{a0}name.md)\n"),
            "{out}"
        );
        assert!(
            out.contains("See [r](../archive/new\u{a0}name.md#top \"R\")."),
            "{out}"
        );
        let parsed = parse_engram_lossless(&out).expect("parses");
        assert_eq!(
            parsed.engram.relations[0].target.target,
            "../archive/new\u{a0}name.md"
        );
        // And back, from the written form.
        let (back, n) = respell_relative_links(&out, "deep/a.md", to, from);
        assert_eq!(n, 2, "{back}");
        assert!(
            back.contains("- part_of [R](../pumps/\u{65e5}\u{672c}\u{3000}\u{5831}\u{544a}.md)\n"),
            "{back}"
        );

        // The moved engram's own links are rebased whole too.
        let own = format!("{ENGRAM_HEAD}[a](../x\u{3000}y.md)\n");
        let (out, n) = rebase_relative_links(&own, "pumps/b.md", "archive/old/b.md");
        assert_eq!(n, 1, "{out}");
        assert!(out.ends_with("[a](../../x\u{3000}y.md)\n"), "{out}");
    }

    #[test]
    fn a_moved_engrams_own_relative_links_are_rebased() {
        let source = format!(
            "{ENGRAM_HEAD}[a](../a.md) [peer](./peer.md) [me](b.md) [out](../../x.md) \
             [file](assets/n.md) [web](https://x.test/a.md) ![pic](../pic.md)\n"
        );
        let (out, n) = rebase_relative_links(&source, "pumps/b.md", "archive/old/b.md");
        assert_eq!(n, 2, "{out}");
        assert!(
            out.ends_with(
                "[a](../../a.md) [peer](../../pumps/peer.md) [me](b.md) [out](../../x.md) \
                 [file](assets/n.md) [web](https://x.test/a.md) ![pic](../pic.md)\n"
            ),
            "{out}"
        );
        // Back to the root: the climb shortens, a same-folder link gains its folder.
        let (out, n) = rebase_relative_links(&source, "pumps/b.md", "b.md");
        assert_eq!(n, 2, "{out}");
        assert!(
            out.contains("[a](a.md) [peer](pumps/peer.md) [me](b.md)"),
            "{out}"
        );
    }

    #[test]
    fn relative_links_that_cross_into_another_domain_become_prefixed_wikilinks() {
        let source = format!(
            "{ENGRAM_HEAD}- part_of [Overview](../overview.md)\n\n\
             See [B](b.md), [me](a.md#top), [frag](b.md#part), [titled](b.md \"T\"), \
             [](b.md), ![pic](b.md), `[B](b.md)`, [odd [[x]]](b.md) and [left](c.md).\n"
        );
        let map = |target: &str| match target {
            "b.md" => Some(NoteLinkTo::Wikilink("dom:guides/b".to_string())),
            "../overview.md" => Some(NoteLinkTo::Wikilink("dom:overview".to_string())),
            "a.md" => Some(NoteLinkTo::Target("x.md".to_string())),
            _ => None,
        };
        let out = readdress_relative_links(&source, &map);
        assert_eq!(out.changed, 6, "{}", out.text);
        assert!(
            out.text.contains("- part_of [[dom:overview|Overview]]\n"),
            "{}",
            out.text
        );
        assert!(
            out.text.contains(
                "See [[dom:guides/b|B]], [me](x.md#top), [[dom:guides/b#part|frag]], \
                 [[dom:guides/b|titled]], [[dom:guides/b]], ![pic](b.md), `[B](b.md)`, \
                 [odd [[x]]](b.md) and [left](c.md)."
            ),
            "{}",
            out.text
        );
        assert_eq!(out.dropped, vec!["[titled](b.md \"T\")".to_string()]);
        assert_eq!(out.unwritable, vec!["[odd [[x]]](b.md)".to_string()]);
        assert!(out.text.starts_with(ENGRAM_HEAD));
    }

    #[test]
    fn map_note_links_whole_carries_a_fragment_a_wikilink_can_hold() {
        let source = format!(
            "{ENGRAM_HEAD}See [p](p.md#setup), [q](p.md#a%20b \"t\") and [r](p.md#a%7Cb).\n"
        );
        let out = readdress_relative_links(&source, &|target| {
            (target == "p.md").then(|| NoteLinkTo::Wikilink("notes:p".to_string()))
        });
        assert_eq!(out.changed, 3, "{}", out.text);
        assert!(
            out.text
                .contains("See [[notes:p#setup|p]], [[notes:p#a b|q]] and [[notes:p|r]]."),
            "{}",
            out.text
        );
        assert_eq!(
            out.dropped,
            vec![
                "[q](p.md#a%20b \"t\")".to_string(),
                "[r](p.md#a%7Cb)".to_string()
            ]
        );

        let only =
            readdress_relative_links(&format!("{ENGRAM_HEAD}See [p](p.md#setup).\n"), &|_| {
                Some(NoteLinkTo::Wikilink("notes:p".to_string()))
            });
        assert!(
            only.text.contains("See [[notes:p#setup|p]]."),
            "{}",
            only.text
        );
        assert!(only.dropped.is_empty(), "{:?}", only.dropped);
    }

    #[test]
    fn a_prefixed_path_wikilink_can_be_readdressed() {
        let source =
            format!("{ENGRAM_HEAD}[[notes:guides/a.md|A]] [[guides/a.md]] [[ops:guides/a.md]]\n");
        let (out, n) = readdress_prefixed_path_wikilinks(&source, &|domain, target| {
            (domain == "notes" && target == "guides/a.md").then(|| "notes:archive/a.md".to_string())
        });
        assert_eq!(n, 1, "{out}");
        assert!(
            out.ends_with("[[notes:archive/a.md|A]] [[guides/a.md]] [[ops:guides/a.md]]\n"),
            "{out}"
        );
    }

    #[test]
    fn a_bare_path_wikilink_can_be_given_a_domain() {
        let source =
            format!("{ENGRAM_HEAD}[[guides/b.md]] [[guides/b.md|B]] [[dom:guides/b.md]] [[B]]\n");
        let (out, n) = readdress_path_wikilinks(&source, &|target| {
            (target == "guides/b.md").then(|| "dom:guides/b".to_string())
        });
        assert_eq!(n, 2, "{out}");
        assert!(
            out.ends_with("[[dom:guides/b]] [[dom:guides/b|B]] [[dom:guides/b.md]] [[B]]\n"),
            "{out}"
        );
    }

    const SAME: Relink<'static> = Relink {
        from_domain: "eng",
        from_permalink: "velog/alpha",
        title: "Alpha Notes",
        to_domain: "eng",
        to_permalink: "projects/velog/alpha",
        shadowed: &[],
    };

    const CROSS: Relink<'static> = Relink {
        from_domain: "eng",
        from_permalink: "alpha",
        title: "Alpha Notes",
        to_domain: "ops",
        to_permalink: "alpha",
        shadowed: &[],
    };

    fn doc(body: &str) -> String {
        format!("---\ntitle: Linker\ntype: engram\nstatus: stable\n---\n\n{body}")
    }

    #[test]
    fn a_link_written_in_the_destination_loses_the_prefix_naming_its_own_domain() {
        let source = doc(
            "See [[eng:alpha]], [[eng:Alpha Notes|the notes]] and [[eng:alpha|it]].\n\
             - relates_to [[eng:alpha]]\n",
        );
        let (out, n) = relink(&source, "ops", "ops", &CROSS);
        assert_eq!(n, 4, "{out}");
        assert!(
            out.contains(
                "See [[alpha]], [[Alpha Notes|the notes]] and [[alpha|it]].\n\
                 - relates_to [[alpha]]\n"
            ),
            "{out}"
        );
        // Written anywhere else, the prefix names the new domain.
        let (out, _) = relink(&source, "dev", "dev", &CROSS);
        assert!(
            out.contains("See [[ops:alpha]], [[ops:Alpha Notes|the notes]]"),
            "{out}"
        );
        // A title whose head would read as a prefix once bare keeps one.
        let colon = Relink {
            title: "Log: Weekly",
            ..CROSS
        };
        let (out, n) = relink(&doc("[[eng:Log: Weekly]]\n"), "ops", "ops", &colon);
        assert_eq!(n, 1, "{out}");
        assert!(out.contains("[[ops:Log: Weekly]]"), "{out}");
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

    /// A `[[...]]` inside an inline link's destination is part of the path,
    /// as the parser reads it, so a move never rewrites it. A URL in a
    /// destination is still an address and follows.
    #[test]
    fn a_wikilink_inside_a_link_destination_is_never_rewritten() {
        let source = doc("- part_of [Doc](a[[velog/alpha]]b.md)\n\
             See [t](https://x.test/[[velog/alpha]]) and [[velog/alpha]].\n\
             [odd [[velog/alpha]]](b.md) and [u](crystalline://eng/velog/alpha).\n");
        let (out, n) = relink(&source, "eng", "eng", &SAME);
        assert_eq!(n, 3, "{out}");
        assert!(
            out.contains("- part_of [Doc](a[[velog/alpha]]b.md)\n"),
            "{out}"
        );
        assert!(
            out.contains("See [t](https://x.test/[[velog/alpha]]) and [[projects/velog/alpha]]."),
            "{out}"
        );
        assert!(
            out.contains("[odd [[projects/velog/alpha]]](b.md) and [u](crystalline://eng/projects/velog/alpha)."),
            "{out}"
        );

        let (out, n) = respell_domains(
            &spell_doc("[d](a[[old:x]].md) [[old:x]] [u](crystalline://old/x)\n"),
            &old_to_new,
        );
        assert_eq!(n, 2, "{out}");
        assert!(
            out.ends_with("[d](a[[old:x]].md) [[new:x]] [u](crystalline://new/x)\n"),
            "{out}"
        );

        let source = format!("{ENGRAM_HEAD}[d](a[[pumps/b.md]].md) [[pumps/b.md]]\n");
        let (out, n) = respell_path_wikilinks(&source, "pumps/b.md", "archive/b.md");
        assert_eq!(n, 1, "{out}");
        assert!(
            out.ends_with("[d](a[[pumps/b.md]].md) [[archive/b.md]]\n"),
            "{out}"
        );
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
    fn a_linker_in_the_destination_domain_drops_the_prefix_form() {
        let source = doc("From ops: [[eng:alpha]]\n");
        let (out, n) = relink(&source, "ops", "ops", &CROSS);
        assert_eq!(n, 1);
        assert!(out.contains("From ops: [[alpha]]"), "{out}");
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
        // link needs no prefix to find its own engram there, and a prefixed
        // one loses the prefix that would name its own domain.
        let source = doc("Back to the top: [[alpha]] and [[eng:alpha]]\n");
        let (out, n) = relink(&source, "eng", "ops", &CROSS);
        assert_eq!(n, 1, "{out}");
        assert!(
            out.contains("Back to the top: [[alpha]] and [[alpha]]"),
            "{out}"
        );
        let renamed = Relink {
            to_permalink: "guides/alpha",
            ..CROSS
        };
        let (out, n) = relink(&source, "eng", "ops", &renamed);
        assert_eq!(n, 2, "{out}");
        assert!(
            out.contains("[[guides/alpha]] and [[guides/alpha]]"),
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

    fn spell_doc(body: &str) -> String {
        format!(
            "---\ntype: note\ntitle: T\npermalink: t\nstatus: stable\nrecorded_at: 2026-01-01\nsource: crystalline://old/x\n---\n\n{body}"
        )
    }

    fn old_to_new(x: &str) -> Option<String> {
        (x == "old").then(|| "new".to_string())
    }

    #[test]
    fn a_prefixed_wikilink_and_a_url_are_respelled() {
        let (out, n) = respell_domains(
            &spell_doc("See [[old:runbook]] and crystalline://old/runbook#steps.\n"),
            &old_to_new,
        );
        assert_eq!(n, 2);
        assert!(
            out.ends_with("See [[new:runbook]] and crystalline://new/runbook#steps.\n"),
            "{out}"
        );
        assert!(
            out.contains("source: crystalline://old/x"),
            "frontmatter is not touched"
        );
    }

    #[test]
    fn spacing_inside_the_brackets_is_kept() {
        let (out, n) = respell_domains(&spell_doc("[[ old : Runbook ]]\n"), &old_to_new);
        assert_eq!(n, 1);
        assert!(out.ends_with("[[ new : Runbook ]]\n"), "{out}");
    }

    #[test]
    fn globs_bare_domain_urls_and_sentence_dots_work() {
        let (out, n) = respell_domains(
            &spell_doc("crystalline://old/* and crystalline://old. And crystalline://older/x\n"),
            &old_to_new,
        );
        assert_eq!(n, 2);
        assert!(
            out.ends_with("crystalline://new/* and crystalline://new. And crystalline://older/x\n"),
            "{out}"
        );
    }

    #[test]
    fn code_bare_links_and_other_domains_are_left_alone() {
        let body = "`[[old:x]]`\n\n```\n[[old:x]]\n```\n\n[[old]] [[other:x]] [[Log: old notes]]\n";
        let (out, n) = respell_domains(&spell_doc(body), &old_to_new);
        assert_eq!(n, 0);
        assert_eq!(out, spell_doc(body));
    }

    #[test]
    fn an_unparseable_source_comes_back_unchanged() {
        let src = "---\nnot: [valid\n---\n[[old:x]]\n";
        assert_eq!(respell_domains(src, &old_to_new), (src.to_string(), 0));
    }

    #[test]
    fn the_split_gate_rewrites_a_body_the_engram_parser_refuses() {
        // Frontmatter that is no engram's (a list where a mapping goes) and
        // no frontmatter at all: Lossless leaves both alone, Split walks
        // their bodies, fences and inline code untouched as ever.
        let respell = |d: &str| (d == "old").then(|| "new".to_string());
        let odd = "---\n- \"[[old:z]]\"\n---\n\nSee [[old:a]].\n\n```\n[[old:b]]\n```\n";
        assert_eq!(
            respell_domains_with(odd, BodyGate::Lossless, &respell),
            (odd.to_string(), 0)
        );
        let (out, n) = respell_domains_with(odd, BodyGate::Split, &respell);
        assert_eq!(
            n, 1,
            "the frontmatter is kept verbatim, links and all: {out}"
        );
        assert!(
            out.starts_with("---\n- \"[[old:z]]\"\n---\n")
                && out.contains("[[new:a]]")
                && out.contains("[[old:b]]")
        );
        let bare = "# Page\r\n\r\nSee `[[old:c]]` and crystalline://old/x.\r\n";
        let (out, n) = respell_domains_with(bare, BodyGate::Split, &respell);
        assert_eq!(n, 1);
        assert_eq!(
            out, "# Page\r\n\r\nSee `[[old:c]]` and crystalline://new/x.\r\n",
            "CRLF and inline code kept"
        );
    }

    #[test]
    fn the_addressed_dialect_names_the_lines_it_may_rewrite() {
        assert!(LinkDialect::Addressed.may_hold("see [[a]]"));
        assert!(LinkDialect::Addressed.may_hold("see crystalline://eng/a"));
        assert!(!LinkDialect::Addressed.may_hold("see [a](b.md)"));
    }

    #[test]
    fn a_relink_keeps_the_fragment_as_written() {
        let renamed = Relink {
            from_domain: "eng",
            from_permalink: "b",
            title: "B",
            to_domain: "eng",
            to_permalink: "newb",
            shadowed: &[],
        };
        let source = doc("See [[b#setup|t]], [[b # Setup Guide ]] and [[eng:b#x:y]].\n");
        let (out, n) = relink(&source, "eng", "eng", &renamed);
        assert_eq!(n, 3, "{out}");
        assert!(
            out.contains("See [[newb#setup|t]], [[newb # Setup Guide ]] and [[eng:newb#x:y]]."),
            "{out}"
        );

        let across = Relink {
            from_domain: "eng",
            from_permalink: "b",
            title: "B",
            to_domain: "notes",
            to_permalink: "b",
            shadowed: &[],
        };
        let (out, n) = relink(&doc("See [[b#setup|t]].\n"), "eng", "eng", &across);
        assert_eq!(n, 1, "{out}");
        assert!(out.contains("See [[notes:b#setup|t]]."), "{out}");

        // The spacing before the `#` stays as written.
        let spaced = Relink {
            from_permalink: "c",
            title: "C",
            to_permalink: "c",
            ..across
        };
        let (out, n) = relink(
            &doc("See [[C # Basics]] and [[c #intro|i]].\n"),
            "eng",
            "eng",
            &spaced,
        );
        assert_eq!(n, 2, "{out}");
        assert!(
            out.contains("See [[notes:C # Basics]] and [[notes:c #intro|i]]."),
            "{out}"
        );
    }

    #[test]
    fn a_title_holding_a_hash_still_follows_a_move() {
        let across = Relink {
            from_domain: "eng",
            from_permalink: "c-basics",
            title: "C# Basics",
            to_domain: "notes",
            to_permalink: "c-basics",
            shadowed: &[],
        };
        let (out, n) = relink(
            &doc("See [[C# Basics]] and [[c#intro]].\n"),
            "eng",
            "eng",
            &across,
        );
        assert_eq!(n, 1, "{out}");
        assert!(
            out.contains("See [[notes:C# Basics]] and [[c#intro]]."),
            "{out}"
        );

        // Moving `c` (title `C`): a `[[C# Basics]]` the caller found as the
        // title of another engram points there and is left alone.
        let shadowed = vec!["C# Basics".to_string()];
        let moved_c = Relink {
            from_domain: "eng",
            from_permalink: "c",
            title: "C",
            to_domain: "eng",
            to_permalink: "newc",
            shadowed: &shadowed,
        };
        let (out, n) = relink(
            &doc("See [[C]], [[c#intro]], [[C# Basics]] and [[c# basics]].\n"),
            "eng",
            "eng",
            &moved_c,
        );
        assert_eq!(n, 1, "{out}");
        assert!(
            out.contains("See [[C]], [[newc#intro]], [[C# Basics]] and [[c# basics]]."),
            "{out}"
        );
    }

    #[test]
    fn a_domain_respell_and_a_path_respell_keep_the_fragment() {
        let (out, n) = respell_domains(&spell_doc("See [[old:b#x]].\n"), &old_to_new);
        assert_eq!(n, 1, "{out}");
        assert!(out.ends_with("See [[new:b#x]].\n"), "{out}");

        let source = format!("{ENGRAM_HEAD}See [[pumps/p.md#x]] and [[pumps/p.md#x|the pump]].\n");
        let (out, n) = respell_path_wikilinks(&source, "pumps/p.md", "archive/p.md");
        assert_eq!(n, 2, "{out}");
        assert!(
            out.contains("See [[archive/p.md#x]] and [[archive/p.md#x|the pump]]."),
            "{out}"
        );
    }

    #[test]
    fn a_readdress_puts_the_fragment_before_the_display_text_it_adds() {
        let source = format!("{ENGRAM_HEAD}See [[B#setup]], [[C# Basics]] and [[C#intro|c]].\n");
        let (out, n) = readdress_wikilinks(&source, &|domain, target, shown| match (
            domain, target, shown,
        ) {
            (None, "B", false) => Some("notes:b|B".to_string()),
            (None, "C# Basics", false) => Some("notes:c-basics|C# Basics".to_string()),
            (None, "C", true) => Some("notes:c".to_string()),
            _ => None,
        });
        assert_eq!(n, 3, "{out}");
        assert!(
            out.contains(
                "See [[notes:b#setup|B]], [[notes:c-basics|C# Basics]] and [[notes:c#intro|c]]."
            ),
            "{out}"
        );
    }
}
