//! Relative markdown links between reflections, rewritten when one moves.
//!
//! The reflection dialect over the shared body walker
//! ([`crate::relink`]'s `rewrite_body_lines`, the body after the frontmatter
//! when there is one, fences untouched, CR kept): only inline links
//! `[text](target)` whose target is relative (no scheme, no leading `/`, not
//! a bare `#fragment`) are touched, outside inline code, and a title after
//! the target and a `#fragment` stay as written. A target is read in either
//! markdown form, bare or in angle brackets, percent-decoded, and written
//! back in angle brackets when it holds a space or a parenthesis (the form
//! the folder indexes write). A target that climbs out of the domain is left
//! alone.

use crate::attachment::{canonical_asset_path, percent_decode_target};
use crate::index::escape_link_url;
use crate::relink::{BodyGate, LinkDialect, rewrite_body_lines};

use super::assets::{AssetTarget, classify_target};
use super::layout::domain_rel;

/// The folder a reflection path sits in, `""` at the root.
fn folder_of(path: &str) -> &str {
    path.rsplit_once('/').map(|(dir, _)| dir).unwrap_or("")
}

/// `target` resolved against the folder of `from_file`, or `None` when it
/// climbs out of the folder `from_file` is relative to. Handed a
/// domain-relative `from_file`, it resolves inside the domain, which is how
/// a link that leaves `reflections/` is followed to where it lands.
pub(crate) fn resolve(from_file: &str, target: &str) -> Option<String> {
    let mut parts: Vec<&str> = folder_of(from_file)
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    for segment in target.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            other => parts.push(other),
        }
    }
    Some(parts.join("/"))
}

/// The domain-relative path a relative link `target` in the engram at the
/// domain-relative `from_file` lands on, or `None` when it climbs out of the
/// domain or lands on its root.
pub fn resolve_in_domain(from_file: &str, target: &str) -> Option<String> {
    resolve(from_file, target).filter(|path| !path.is_empty())
}

/// The relative path from the reflection at `from_file` to `target`.
pub fn relative_to(from_file: &str, target: &str) -> String {
    let from: Vec<&str> = folder_of(from_file)
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    let to: Vec<&str> = target.split('/').collect();
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let mut out: Vec<String> = std::iter::repeat_n("..".to_string(), from.len() - common).collect();
    out.extend(to[common..].iter().map(|s| s.to_string()));
    out.join("/")
}

pub(crate) fn is_relative(target: &str) -> bool {
    !target.is_empty()
        && !target.starts_with('#')
        && !target.starts_with('/')
        && !target.contains("://")
        && !target.starts_with("mailto:")
}

/// Byte ranges of the inline code spans on one line (backticks included),
/// matched the way `crate::parse::mask_inline_code` matches them. That
/// function swaps characters for spaces, which shifts byte offsets when a
/// span holds a multi-byte character, so this dialect keeps its own ranges.
pub(crate) fn code_spans(line: &str) -> Vec<(usize, usize)> {
    let bytes = line.as_bytes();
    let mut spans = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'`' {
            i += 1;
            continue;
        }
        let run_end = i + bytes[i..].iter().take_while(|b| **b == b'`').count();
        let run = run_end - i;
        let mut k = run_end;
        let mut closed = None;
        while k < bytes.len() {
            if bytes[k] == b'`' {
                let m = k + bytes[k..].iter().take_while(|b| **b == b'`').count();
                if m - k == run {
                    closed = Some(m);
                    break;
                }
                k = m;
            } else {
                k += 1;
            }
        }
        match closed {
            Some(end) => {
                spans.push((i, end));
                i = end;
            }
            None => i = run_end,
        }
    }
    spans
}

/// One link destination, read from just after a `](`.
pub(crate) struct Destination<'a> {
    /// The path, unescaped and percent-decoded, without a `#fragment` or a title.
    pub(crate) path: String,
    /// The `#fragment` as written, without the `#`.
    pub(crate) fragment: Option<&'a str>,
    /// What follows the target up to the closing parenthesis: a title.
    pub(crate) rest: &'a str,
    /// The byte offset of the closing parenthesis.
    pub(crate) close: usize,
}

/// The byte offset of the depth-zero `)` at or after `from`. Balanced
/// parentheses inside a destination are read past, and a backslash-escaped
/// `\(` or `\)` neither opens nor closes, so `[x](R\).png)` reads `R).png`.
fn closing_paren(line: &str, from: usize) -> Option<usize> {
    let bytes = line.as_bytes();
    let mut depth = 1usize;
    let mut i = from;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if matches!(bytes.get(i + 1), Some(b'(' | b')' | b'\\')) => i += 1,
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// `path` with every CommonMark backslash escape removed: a backslash before
/// an ASCII punctuation character drops the backslash, any other backslash
/// stays.
fn unescape_backslashes(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    let mut chars = path.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' && chars.peek().is_some_and(char::is_ascii_punctuation) {
            continue;
        }
        out.push(c);
    }
    out
}

/// Whether `c` ends a bare link destination (one not in angle brackets): an
/// ASCII space or an ASCII control character, as CommonMark reads it. Any
/// other space, an ideographic U+3000 or a no-break U+00A0, is part of the
/// path, so a file named with one is linked by its whole name. Every reader
/// of a destination asks here, so the parser, the move rewrites and the
/// attachment scan cut a target at the same place.
pub fn ends_destination(c: char) -> bool {
    c == ' ' || c.is_ascii_control()
}

/// The destination that starts at `open` (just after `](`), or `None` when
/// none closes on this line. The `#` fragment is split off the raw target
/// first, then backslash escapes are removed and the rest is percent-decoded,
/// so `a%23b.png` keeps its `#` in the path and `R\&D.png` reads `R&D.png`.
pub(crate) fn read_destination(line: &str, open: usize) -> Option<Destination<'_>> {
    let (target, rest, close) = if line[open..].starts_with('<') {
        // `[a](<my page.md> "Title")`: the target runs to the first `>`.
        let gt = open + 1 + line[open + 1..].find('>')?;
        let close = closing_paren(line, gt + 1)?;
        (&line[open + 1..gt], &line[gt + 1..close], close)
    } else {
        let close = closing_paren(line, open)?;
        let inside = &line[open..close];
        // A title after the target (`[a](b.md "Title")`) is kept as written.
        let target_len = inside.find(ends_destination).unwrap_or(inside.len());
        let (target, rest) = inside.split_at(target_len);
        (target, rest, close)
    };
    let (path, fragment) = decode_target(target);
    Some(Destination {
        path,
        fragment,
        rest,
        close,
    })
}

/// One raw link target read into its path and `#fragment`: the fragment is
/// split off on the raw `#` first, then backslash escapes are removed from the
/// path and it is percent-decoded. The reading every destination takes, inline
/// or in a reference definition.
pub(crate) fn decode_target(target: &str) -> (String, Option<&str>) {
    let (path, fragment) = match target.split_once('#') {
        Some((p, f)) => (p, Some(f)),
        None => (target, None),
    };
    (percent_decode_target(&unescape_backslashes(path)), fragment)
}

/// Which relative links a line rewrite hands to its map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Targets {
    /// Every relative link target: a reflection's links to pages and files.
    All,
    /// Only the links the engram parser reads as links to another engram's
    /// file (`crate::parse`'s `note_link_start`): a target ending in `.md`,
    /// never an image.
    Notes,
}

/// One line of the reflection dialect: every relative link target outside
/// inline code (of the kind `targets` names) goes through `map`, which is
/// handed the path (decoded, no title, no `#fragment`) and answers the
/// replacement or `None` to keep it. A `](` that opens no destination is
/// read past, so a later link on the line is still found.
fn map_line(line: &str, targets: Targets, map: &dyn Fn(&str) -> Option<String>) -> (String, usize) {
    let spans = code_spans(line);
    let in_code = |at: usize| spans.iter().any(|(s, e)| (*s..*e).contains(&at));
    let mut out = String::with_capacity(line.len());
    let mut changed = 0;
    let mut cursor = 0;
    while let Some(found) = line[cursor..].find("](") {
        let open = cursor + found + 2;
        out.push_str(&line[cursor..open]);
        cursor = open;
        let Some(dest) = read_destination(line, open) else {
            continue;
        };
        let wanted = match targets {
            Targets::All => is_relative(&dest.path),
            Targets::Notes => crate::parse::note_link_start(line, open - 2, &dest.path).is_some(),
        };
        let replacement = if in_code(open - 2) || !wanted {
            None
        } else {
            map(&dest.path)
        };
        match replacement {
            Some(mut target) => {
                if let Some(f) = dest.fragment {
                    target.push('#');
                    target.push_str(f);
                }
                out.push_str(&escape_link_url(&target));
                out.push_str(dest.rest);
                changed += 1;
            }
            None => out.push_str(&line[open..dest.close]),
        }
        cursor = dest.close;
    }
    out.push_str(&line[cursor..]);
    (out, changed)
}

/// Rewrite every relative link target of a reflection through `map`. The
/// one walk every reflection link rewrite takes: the shared body walker with
/// the reflection gate and dialect.
pub(crate) fn map_links(source: &str, map: &dyn Fn(&str) -> Option<String>) -> (String, usize) {
    map_body(source, BodyGate::Split, Targets::All, map)
}

/// Rewrite the relative links of an engram that point at another engram's
/// file through `map`: the body as the lossless engram parser cuts it, and
/// only the links the parser reads as edges, so what changes is exactly what
/// the index counted.
pub(crate) fn map_note_links(
    source: &str,
    map: &dyn Fn(&str) -> Option<String>,
) -> (String, usize) {
    map_body(source, BodyGate::Lossless, Targets::Notes, map)
}

/// What [`map_note_links_whole`] puts in place of one link to another
/// engram's file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoteLinkTo {
    /// The destination only: the text, the `#fragment` and the title stay.
    Target(String),
    /// The whole link, as the wikilink `[[address|text]]`.
    Wikilink(String),
}

/// What [`map_note_links_whole`] did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct WholeLinks {
    /// The rewritten source.
    pub text: String,
    /// How many links changed.
    pub changed: usize,
    /// Links written as a wikilink, as they were written before, whose title
    /// a wikilink cannot carry, or whose `#fragment` holds `|`, `[`, `]` or
    /// `#` once percent-decoded and so cannot follow the address. Any other
    /// fragment is carried as `[[address#fragment|text]]`.
    pub dropped: Vec<String>,
    /// Links that should have become a wikilink, left as written because
    /// their text cannot sit inside `[[...]]` (it holds `[[` or `]]`).
    pub unwritable: Vec<String>,
}

/// `map_note_links` that may also replace a whole link: `map` is handed
/// each decoded target and answers a new destination, a wikilink address, or
/// `None` to keep the link. A wikilink keeps the link text as its display
/// text and the `#fragment`, percent-decoded, after the address; a title, and
/// a fragment a wikilink cannot hold, cannot follow it and are named in
/// [`WholeLinks::dropped`].
pub fn map_note_links_whole(source: &str, map: &dyn Fn(&str) -> Option<NoteLinkTo>) -> WholeLinks {
    use std::cell::RefCell;
    let dropped: RefCell<Vec<String>> = RefCell::new(Vec::new());
    let unwritable: RefCell<Vec<String>> = RefCell::new(Vec::new());
    let (text, changed) = rewrite_body_lines(source, BodyGate::Lossless, &|line| {
        if !LinkDialect::Relative.may_hold(line) {
            return (line.to_string(), 0);
        }
        whole_line(line, map, &dropped, &unwritable)
    });
    WholeLinks {
        text,
        changed,
        dropped: dropped.into_inner(),
        unwritable: unwritable.into_inner(),
    }
}

/// One line of [`map_note_links_whole`].
fn whole_line(
    line: &str,
    map: &dyn Fn(&str) -> Option<NoteLinkTo>,
    dropped: &std::cell::RefCell<Vec<String>>,
    unwritable: &std::cell::RefCell<Vec<String>>,
) -> (String, usize) {
    let spans = code_spans(line);
    let in_code = |at: usize| spans.iter().any(|(s, e)| (*s..*e).contains(&at));
    let mut out = String::with_capacity(line.len());
    let mut changed = 0;
    let mut cursor = 0;
    while let Some(found) = line[cursor..].find("](") {
        let close_text = cursor + found;
        let open = close_text + 2;
        let Some(dest) = read_destination(line, open) else {
            out.push_str(&line[cursor..open]);
            cursor = open;
            continue;
        };
        let start = crate::parse::note_link_start(line, close_text, &dest.path)
            .filter(|start| *start >= cursor && !in_code(close_text));
        let replacement = start.and_then(|_| map(&dest.path));
        match (replacement, start) {
            (Some(NoteLinkTo::Target(mut target)), _) => {
                out.push_str(&line[cursor..open]);
                if let Some(f) = dest.fragment {
                    target.push('#');
                    target.push_str(f);
                }
                out.push_str(&escape_link_url(&target));
                out.push_str(dest.rest);
                changed += 1;
            }
            (Some(NoteLinkTo::Wikilink(address)), Some(start)) => {
                let text = &line[start + 1..close_text];
                let written = &line[start..=dest.close];
                if text.contains("[[") || text.contains("]]") {
                    unwritable.borrow_mut().push(written.to_string());
                    out.push_str(&line[cursor..open]);
                    out.push_str(&line[open..dest.close]);
                } else {
                    // A `#fragment` follows the address, percent-decoded, when
                    // a wikilink can carry it; a title never can.
                    let fragment = dest.fragment.map(percent_decode_target);
                    let carried = fragment
                        .as_deref()
                        .filter(|f| !f.contains(['|', '[', ']', '#']));
                    let lost = fragment
                        .as_deref()
                        .is_some_and(|f| carried.is_none() && !f.is_empty());
                    if lost || !dest.rest.trim().is_empty() {
                        dropped.borrow_mut().push(written.to_string());
                    }
                    out.push_str(&line[cursor..start]);
                    out.push_str("[[");
                    out.push_str(&address);
                    if let Some(fragment) = carried.filter(|f| !f.trim().is_empty()) {
                        out.push('#');
                        out.push_str(fragment);
                    }
                    if !text.trim().is_empty() {
                        out.push('|');
                        out.push_str(text);
                    }
                    out.push_str("]]");
                    changed += 1;
                    // Past the closing parenthesis, which the wikilink
                    // replaced.
                    cursor = dest.close + 1;
                    continue;
                }
            }
            _ => {
                out.push_str(&line[cursor..open]);
                out.push_str(&line[open..dest.close]);
            }
        }
        cursor = dest.close;
    }
    out.push_str(&line[cursor..]);
    (out, changed)
}

fn map_body(
    source: &str,
    gate: BodyGate,
    targets: Targets,
    map: &dyn Fn(&str) -> Option<String>,
) -> (String, usize) {
    rewrite_body_lines(source, gate, &|line| {
        if LinkDialect::Relative.may_hold(line) {
            map_line(line, targets, map)
        } else {
            (line.to_string(), 0)
        }
    })
}

/// Rewrite the links in the reflection at `source_path` that point at `from`
/// so they point at `to`.
pub fn rewrite_links_to(source: &str, source_path: &str, from: &str, to: &str) -> (String, usize) {
    map_links(source, &|target| {
        (resolve(source_path, target).as_deref() == Some(from))
            .then(|| relative_to(source_path, to))
    })
}

/// Rewrite the links in the reflection at `source_path` that reach the
/// Reflection file `from` (a path inside `reflections/`, `assets/...`) so
/// they reach `to`. A target is matched by [`classify_target`], the reading
/// the references use: the dialect hands it over percent-decoded, and the
/// classifier resolves it inside the domain and folds the folder's case, so
/// every link counted as a reference (an encoded one, one spelled `Assets/`,
/// one that climbs to the domain root and back) follows its file. A title
/// after the target and a formatting fragment are kept as written.
pub fn rewrite_asset_links(
    source: &str,
    source_path: &str,
    from: &str,
    to: &str,
) -> (String, usize) {
    let from = canonical_asset_path(from).unwrap_or_else(|| from.to_string());
    map_links(source, &|target| {
        matches!(
            classify_target(target, source_path),
            AssetTarget::Reflection(path) if path == from
        )
        .then(|| relative_to(source_path, to))
    })
}

/// Rebase the relative links inside a reflection that moved from `from` to
/// `to`, so each still points at the file it pointed at. The links resolve
/// inside the domain, so one that leaves `reflections/` for another file of
/// the domain still lands on it after the move changes the depth.
pub fn rebase_links(source: &str, from: &str, to: &str) -> (String, usize) {
    map_links(source, &|target| {
        let resolved = resolve(&domain_rel(from), target)?;
        let rebased = relative_to(&domain_rel(to), &resolved);
        (rebased != target.trim_start_matches("./")).then_some(rebased)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(line: &str) -> (String, Option<String>) {
        let open = line.find("](").unwrap() + 2;
        let d = read_destination(line, open).unwrap();
        (d.path, d.fragment.map(str::to_string))
    }

    #[test]
    fn read_destination_decodes_unescapes_and_splits() {
        assert_eq!(
            read("](Q3%20report%20%28final%29.pdf.assets/page-1.png)").0,
            "Q3 report (final).pdf.assets/page-1.png"
        );
        assert_eq!(read("](<a b.png>)").0, "a b.png");
        assert_eq!(read("](R\\&D.png)").0, "R&D.png");
        assert_eq!(
            read("](a%23b.png#right)"),
            ("a#b.png".to_string(), Some("right".to_string()))
        );
        // A backslash before a letter is not an escape.
        assert_eq!(read("](a\\b.png)").0, "a\\b.png");
    }

    /// CommonMark ends a bare destination at an ASCII space or an ASCII
    /// control character only, so a file name with an ideographic space
    /// (U+3000) or a no-break space (U+00A0) is read whole.
    #[test]
    fn a_bare_destination_ends_only_at_an_ascii_space_or_control() {
        assert_eq!(
            read("](../\u{65e5}\u{672c}\u{3000}\u{5831}\u{544a}.md)").0,
            "../\u{65e5}\u{672c}\u{3000}\u{5831}\u{544a}.md"
        );
        assert_eq!(read("](a\u{a0}b.md \"T\")").0, "a\u{a0}b.md");
        assert_eq!(read("](a.md\t\"T\")").0, "a.md");
        assert_eq!(read("](<a\u{3000}b c.md>)").0, "a\u{3000}b c.md");
    }

    #[test]
    fn an_escaped_parenthesis_neither_opens_nor_closes() {
        assert_eq!(read("[x](R\\).png)").0, "R).png");
        assert_eq!(read("[x](R\\(.png) after").0, "R(.png");
        // An escaped backslash before `)` leaves the `)` to close.
        assert_eq!(read("[x](a\\\\) b)").0, "a\\");
    }

    #[test]
    fn relative_paths_between_reflections() {
        assert_eq!(relative_to("guides/a.md", "guides/b.md"), "b.md");
        assert_eq!(relative_to("guides/a.md", "howto/c.md"), "../howto/c.md");
        assert_eq!(relative_to("a.md", "guides/b.md"), "guides/b.md");
        assert_eq!(relative_to("x/y/a.md", "b.md"), "../../b.md");
    }

    #[test]
    fn links_to_a_moved_reflection_follow_it() {
        let source = "# Home\n\nSee [deploying](guides/deploying.md#night) and [again](./guides/deploying.md).\n\n```\n[not a link](guides/deploying.md)\n```\n\nAnd `[code](guides/deploying.md)` and [elsewhere](https://x.test/guides/deploying.md).\n";
        let (out, n) =
            rewrite_links_to(source, "home.md", "guides/deploying.md", "ops/deploying.md");
        assert_eq!(n, 2);
        assert!(out.contains("[deploying](ops/deploying.md#night)"));
        assert!(out.contains("[again](ops/deploying.md)"));
        assert!(
            out.contains("```\n[not a link](guides/deploying.md)\n```"),
            "fences untouched"
        );
        assert!(
            out.contains("`[code](guides/deploying.md)`"),
            "inline code untouched"
        );
        assert!(
            out.contains("https://x.test/guides/deploying.md"),
            "absolute urls untouched"
        );
    }

    #[test]
    fn links_inside_the_moved_reflection_are_rebased() {
        let source = "# Deploying\n\nSee [home](../home.md) and [peer](peer.md \"The peer\").\n";
        let (out, n) = rebase_links(source, "guides/deploying.md", "ops/night/deploying.md");
        assert_eq!(n, 2);
        assert!(out.contains("[home](../../home.md)"));
        assert!(
            out.contains("[peer](../../guides/peer.md \"The peer\")"),
            "a title stays: {out}"
        );
    }

    /// The rebase maps a title through unchanged (it holds no `/` a path
    /// mapping would move), so only a rewrite that compares the target can
    /// tell whether the title and the parentheses were read around.
    #[test]
    fn a_title_and_parentheses_in_the_target_are_read_around() {
        let source = "See [a](guides/a_(draft).md \"The draft\") and [b](b.md 'B').\n";
        let (out, n) = rewrite_links_to(source, "home.md", "guides/a_(draft).md", "ops/a.md");
        assert_eq!(n, 1, "{out}");
        assert_eq!(out, "See [a](ops/a.md \"The draft\") and [b](b.md 'B').\n");
    }

    #[test]
    fn a_link_out_to_the_domain_keeps_its_file_when_the_depth_changes() {
        let source = "[m](../MANIFEST.md) and [n](../notes/a.md#top)\n";
        let (out, n) = rebase_links(source, "a.md", "b/a.md");
        assert_eq!(n, 2, "{out}");
        assert_eq!(
            out,
            "[m](../../MANIFEST.md) and [n](../../notes/a.md#top)\n"
        );
    }

    #[test]
    fn a_target_with_a_space_is_written_in_angle_brackets_and_read_back() {
        let source = "See [a](guides/a.md#top \"A\").\n";
        let (out, n) = rewrite_links_to(source, "home.md", "guides/a.md", "ops/my page.md");
        assert_eq!(n, 1);
        assert_eq!(out, "See [a](<ops/my page.md#top> \"A\").\n");
        let (back, n) = rewrite_links_to(&out, "home.md", "ops/my page.md", "ops/b.md");
        assert_eq!(n, 1);
        assert_eq!(back, "See [a](ops/b.md#top \"A\").\n");
        let encoded = "See [c](ops/my%20page.md).\n";
        let (out, n) = rewrite_links_to(encoded, "home.md", "ops/my page.md", "c.md");
        assert_eq!(
            (out.as_str(), n),
            ("See [c](c.md).\n", 1),
            "a percent-encoded target is read"
        );
    }

    #[test]
    fn a_bracket_that_opens_no_link_does_not_hide_a_later_one() {
        let source = "x ](y and [a](b.md)\n";
        let (out, n) = rewrite_links_to(source, "home.md", "b.md", "c.md");
        assert_eq!((out.as_str(), n), ("x ](y and [a](c.md)\n", 1));
    }

    #[test]
    fn a_link_climbing_out_of_the_folder_is_left_alone() {
        let source = "[out](../../MANIFEST.md)\n";
        let (out, n) = rebase_links(source, "a.md", "b/a.md");
        assert_eq!((out.as_str(), n), (source, 0));
    }

    #[test]
    fn asset_links_follow_a_moved_file() {
        let page = "![a](../assets/gr%C3%B6%C3%9Fe.png \"Size\") ![b](../assets/größe.png#left) [c](./../assets/größe.png) ![d](../assets/other.png)\n```\n![e](../assets/größe.png)\n```\n";
        let (out, n) = rewrite_asset_links(
            page,
            "guides/a.md",
            "assets/größe.png",
            "assets/diagrams/size.png",
        );
        assert_eq!(n, 3, "{out}");
        assert!(
            out.contains("![a](../assets/diagrams/size.png \"Size\")"),
            "decoded match, title kept: {out}"
        );
        assert!(
            out.contains("![b](../assets/diagrams/size.png#left)"),
            "fragment kept: {out}"
        );
        assert!(out.contains("[c](../assets/diagrams/size.png)"));
        assert!(out.contains("![d](../assets/other.png)"));
        assert!(
            out.contains("```\n![e](../assets/größe.png)\n```"),
            "fences untouched"
        );
    }

    /// The file half of a link rewrite only: an asset link with a fragment
    /// follows the file in the page and in its baseline alike, through
    /// `relink` (whose hash carry has its own test).
    #[test]
    fn an_asset_link_rewrite_reaches_the_baseline_too() {
        use crate::reflection::{advance_baseline, feedback_changes, relink, split_sections};
        let page =
            "# Deploying\n\n## Flow\n<!-- r:7f3a9c2e -->\n\n![flow](../assets/a.png#right)\n";
        let baseline = advance_baseline("", page, &["7f3a9c2e".to_string()]);
        let out = relink(page, &baseline, &|s| {
            rewrite_asset_links(s, "guides/deploying.md", "assets/a.png", "assets/b.png")
        })
        .unwrap();
        assert_eq!(out.links, 1);
        assert!(
            out.page.contains("](../assets/b.png#right)")
                && out.baseline.contains("](../assets/b.png#right)")
        );
        assert!(
            feedback_changes(&out.page, &out.baseline)
                .iter()
                .all(|c| c.id.as_deref() != Some("7f3a9c2e"))
        );
        assert_eq!(split_sections(&out.page).len(), 2);
        let cased = rewrite_asset_links(
            "![a](../Assets/a.png)\n",
            "guides/x.md",
            "assets/a.png",
            "assets/b.png",
        );
        assert_eq!(
            cased.1, 1,
            "the folder's case is folded as the references fold it: {}",
            cased.0
        );
    }

    /// The rewriter reads a link exactly as the references do: one that
    /// climbs to the domain root and back into `reflections/assets/` is a
    /// reference, so it follows the file too.
    #[test]
    fn a_domain_rooted_asset_link_follows_its_file() {
        for (path, link, moved) in [
            (
                "guides/a.md",
                "../../reflections/assets/x.png",
                "../assets/y.png",
            ),
            ("a.md", "../reflections/assets/x.png", "assets/y.png"),
        ] {
            let page = format!("![x]({link})\n");
            assert_eq!(
                crate::reflection::reflection_asset_refs(&page, path),
                ["assets/x.png"],
                "{link} is a reference"
            );
            let (out, n) = rewrite_asset_links(&page, path, "assets/x.png", "assets/y.png");
            assert_eq!((out, n), (format!("![x]({moved})\n"), 1), "{link}");
        }
    }

    #[test]
    fn the_frontmatter_and_the_line_endings_are_kept() {
        let source = "---\ntitle: See [x](a.md)\n---\r\n\r\n# Home\r\n\r\nSee [a](a.md).\r\n";
        let (out, n) = rewrite_links_to(source, "home.md", "a.md", "b.md");
        assert_eq!(n, 1);
        assert_eq!(
            out,
            "---\ntitle: See [x](a.md)\n---\r\n\r\n# Home\r\n\r\nSee [a](b.md).\r\n"
        );
    }
}
