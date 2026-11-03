//! The external-change merge: three-way over LF text (frontmatter key by key,
//! body line by line, see `crystalline_core::frontmatter`, and a table row
//! both sides changed once more cell by cell), applied into the live document
//! as a minimal edit script. Conflict markers are never output - diffy's
//! marked text is discarded and the room resolves instead.
//!
//! The edit script works per extended grapheme cluster inside a changed line,
//! so it never splits a surrogate pair, a flag or a compound emoji; the cell
//! merge takes whole cells byte for byte.

use crystalline_core::frontmatter::{MergedText, merge_text_with};
use similar::{Algorithm, DiffOp, capture_diff_slices, diff_ratio};
use unicode_segmentation::UnicodeSegmentation;
use yrs::{Text, TextRef, TransactionMut};

use crystalline_core::to_lf;

/// What a three-way merge of an external change against the live session says.
pub enum MergeOutcome {
    /// The merged text, LF only, ready to apply into the doc.
    Clean(String),
    /// Concurrent edits collide: the room decides.
    Conflict,
}

/// base and theirs arrive as the file holds them, mine as the room's LF text;
/// everything is merged as LF (frontmatter key by key, body line by line, see
/// `crystalline_core::frontmatter`), so a CRLF or mixed side merges like any
/// other. diffy's Err carries conflict-marked text - discarded.
///
/// A table row both sides changed (a person typing in one cell, an agent
/// changing another cell of the same row) is merged once more cell by cell in
/// the body (see `body_merge`); the merge is clean only when that is.
/// Frontmatter key blocks keep the plain line merge, so two changes to one
/// value are a conflict rather than a value nobody wrote.
pub fn three_way(base_file: &str, mine_session: &str, theirs_file: &str) -> MergeOutcome {
    let base = to_lf(base_file);
    let theirs = to_lf(theirs_file);
    let mine_session = &*to_lf(mine_session);
    // Frontmatter by key, body by line, and never a result that stops
    // parsing where both sides parsed. diffy's marked text is discarded
    // inside `line_merge` and `body_merge`: conflict markers must never reach
    // an engram file or the live document.
    match merge_text_with(Some(&base), mine_session, &theirs, &line_merge, &body_merge) {
        MergedText::Clean(merged) => MergeOutcome::Clean(merged),
        MergedText::Conflict => MergeOutcome::Conflict,
    }
}

/// The length of the marker runs no input line may start with. diffy marks
/// a conflict with runs of 7; the cell fallback never asks for marked text,
/// and an input or a result with a line that starts like a long run keeps
/// the conflict all the same, so no marker can reach a document through it.
const FALLBACK_MARKER_LEN: usize = 64;

/// The most replaced lines one side may hold for the cell fallback to weigh
/// them: the pairing check compares every replaced line with every other.
const MAX_REPLACED_LINES: usize = 256;

/// diffy's three-way line merge, its conflict-marked `Err` discarded. The
/// frontmatter's merge.
fn line_merge(base: &str, mine: &str, theirs: &str) -> Option<String> {
    diffy::merge(base, mine, theirs).ok()
}

/// The body's merge: [`line_merge`], and when that conflicts, the cell
/// fallback ([`row_merge`]). Anything the fallback does not take stays a
/// conflict, as it was before the fallback existed.
fn body_merge(base: &str, mine: &str, theirs: &str) -> Option<String> {
    line_merge(base, mine, theirs).or_else(|| row_merge(base, mine, theirs))
}

/// The cell fallback, in three gates:
///
/// 1. The whole text, not one hunk: neither side inserts or deletes a line
///    anywhere, and every line a side replaced is mutually and strictly the
///    closest to its own base line among all the lines that side replaced
///    ([`replaces_in_place`]). A row moved, swapped, dropped or added fails
///    here, wherever in the text the other half of the move is.
/// 2. Table rows only: a run of lines both sides changed (diff3's conflicting
///    hunk, lines touching counted together) is taken only when every line
///    of it is a row of a GFM table (opened by a header row and a delimiter
///    row) on all three sides and no delimiter row in it changed
///    ([`table_rows`]). Any other conflicting line stays a conflict.
/// 3. Per cell: each row of such a run is merged cell by cell
///    ([`merge_row`]); a row both sides changed merges only when each side
///    changed one cell and they are different cells, or one side's changes
///    are all made the same way by the other side.
fn row_merge(base: &str, mine: &str, theirs: &str) -> Option<String> {
    let markers = marker_runs();
    if [base, mine, theirs]
        .iter()
        .flat_map(|t| t.lines())
        .any(|line| starts_like_a_marker(line, &markers))
    {
        return None;
    }
    let b: Vec<&str> = base.split_inclusive('\n').collect();
    let m: Vec<&str> = mine.split_inclusive('\n').collect();
    let t: Vec<&str> = theirs.split_inclusive('\n').collect();
    if !replaces_in_place(&b, &m) || !replaces_in_place(&b, &t) {
        return None;
    }
    let kinds = [table_rows(&b), table_rows(&m), table_rows(&t)];
    let changed = |k: usize| m[k] != b[k] || t[k] != b[k];
    let mut merged = String::with_capacity(theirs.len());
    let mut k = 0;
    while k < b.len() {
        if !changed(k) {
            merged.push_str(b[k]);
            k += 1;
            continue;
        }
        let start = k;
        while k < b.len() && changed(k) {
            k += 1;
        }
        let run = start..k;
        let mine_moved = run.clone().any(|i| m[i] != b[i]);
        let theirs_moved = run.clone().any(|i| t[i] != b[i]);
        if !theirs_moved || m[run.clone()] == t[run.clone()] {
            m[run].iter().for_each(|line| merged.push_str(line));
            continue;
        }
        if !mine_moved {
            t[run].iter().for_each(|line| merged.push_str(line));
            continue;
        }
        for i in run {
            for side in &kinds {
                match side[i] {
                    Row::Data => {}
                    Row::Delimiter if m[i] == b[i] && t[i] == b[i] => {}
                    Row::Delimiter | Row::Other => return None,
                }
            }
            merged.push_str(&merge_row(b[i], m[i], t[i])?);
        }
    }
    if merged
        .lines()
        .any(|line| starts_like_a_marker(line, &markers))
    {
        return None;
    }
    Some(merged)
}

/// Whether `side` changes `base` only by replacing lines in place: its line
/// diff holds nothing but equal lines and runs replaced by as many lines,
/// and every replaced line is mutually and strictly closest to its own base
/// line among all the base lines this side replaced (similar's ratio over
/// grapheme clusters). A tie is no evidence and fails, and so does a side
/// replacing more than [`MAX_REPLACED_LINES`] lines.
///
/// Equal line counts alone do not say a side replaced in place: a side that
/// dropped the first row and added a last one, or swapped two rows with an
/// unchanged one between them, has as many lines, each paired with the
/// wrong one. The likeness check sees the whole text, so the two halves of
/// a swap are weighed together however far apart they are.
fn replaces_in_place(base: &[&str], side: &[&str]) -> bool {
    let mut replaced: Vec<usize> = Vec::new();
    for op in capture_diff_slices(Algorithm::Myers, base, side) {
        match op {
            DiffOp::Equal { .. } => {}
            DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            } if old_len == new_len && old_index == new_index => {
                replaced.extend(old_index..old_index + old_len);
            }
            _ => return false,
        }
    }
    if replaced.len() > MAX_REPLACED_LINES {
        return false;
    }
    let likeness = |b: usize, s: usize| grapheme_ratio(base[b], side[s]);
    replaced.iter().all(|&i| {
        let own = likeness(i, i);
        replaced
            .iter()
            .filter(|&&j| j != i)
            .all(|&j| own > likeness(j, i) && own > likeness(i, j))
    })
}

/// similar's diff ratio of two lines over their grapheme clusters: 1.0 for
/// the same line, 0.0 for lines with no cluster in common.
fn grapheme_ratio(a: &str, b: &str) -> f32 {
    let a: Vec<&str> = a.graphemes(true).collect();
    let b: Vec<&str> = b.graphemes(true).collect();
    diff_ratio(
        &capture_diff_slices(Algorithm::Myers, &a, &b),
        a.len(),
        b.len(),
    )
}

/// What a line is to the cell fallback.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Row {
    /// A table row holding cells.
    Data,
    /// A table's delimiter row (`| --- | :-: |`).
    Delimiter,
    /// Anything else: prose, a heading, a fenced or indented code line.
    Other,
}

/// Classify each line of one text the way GFM finds tables: a line is a
/// table row only inside a table, which a header row opens when the next
/// line is a delimiter row with as many cells, piped or pipeless alike. The
/// table runs on up to the first line without an unescaped pipe, or the
/// first line that starts another block (see [`starts_a_block`]). A line
/// that starts or ends with a pipe outside a table is prose. Lines inside a
/// fenced code block, and lines indented four spaces or more, are never
/// rows.
fn table_rows(lines: &[&str]) -> Vec<Row> {
    let mut kinds = vec![Row::Other; lines.len()];
    let mut fence: Option<Fence> = None;
    let mut in_table = false;
    for (k, line) in lines.iter().enumerate() {
        let text = line.trim_end_matches('\n');
        let trimmed = text.trim();
        // A line indented four columns or more (or by a tab) is indented
        // code in GFM: it neither opens nor closes a fence.
        let fence_indent = indent_allows_fence(text);
        if let Some(open) = fence {
            if fence_indent && Fence::closes(open, trimmed) {
                fence = None;
            }
            continue;
        }
        if let Some(open) = Fence::opens(trimmed).filter(|_| fence_indent) {
            fence = Some(open);
            in_table = false;
            continue;
        }
        if !row_shaped(text) {
            in_table = false;
            continue;
        }
        let opens = lines.get(k + 1).is_some_and(|next| {
            let next = next.trim_end_matches('\n');
            row_shaped(next) && {
                let next = next.trim();
                is_delimiter(next) && cells(next).len() == cells(trimmed).len()
            }
        });
        in_table = in_table || opens;
        if in_table {
            kinds[k] = if is_delimiter(trimmed) {
                Row::Delimiter
            } else {
                Row::Data
            };
        }
    }
    kinds
}

/// Whether a line could be a table row by its own shape: indented less than
/// four spaces, holding an unescaped pipe, and not the start of another
/// block.
fn row_shaped(text: &str) -> bool {
    let trimmed = text.trim();
    !(text.starts_with("    ") || text.starts_with('\t'))
        && has_unescaped_pipe(trimmed)
        && !starts_a_block(trimmed)
}

/// Whether a trimmed line starts a block that is not a table row: a block
/// quote, an ATX heading, a list item or an HTML block. Such a line ends a
/// table and never opens one; a row inside a list or a quote is left to the
/// line merge.
fn starts_a_block(trimmed: &str) -> bool {
    let bytes = trimmed.as_bytes();
    let spaced = |at: usize| bytes.get(at).is_none_or(|c| *c == b' ' || *c == b'\t');
    match bytes.first() {
        Some(b'>' | b'<') => true,
        Some(b'#') => {
            let hashes = bytes.iter().take_while(|c| **c == b'#').count();
            hashes <= 6 && spaced(hashes)
        }
        Some(b'-' | b'*' | b'+') => spaced(1),
        Some(c) if c.is_ascii_digit() => {
            let digits = bytes.iter().take_while(|c| c.is_ascii_digit()).count();
            digits <= 9 && matches!(bytes.get(digits), Some(b'.' | b')')) && spaced(digits + 1)
        }
        _ => false,
    }
}

/// An open code fence: its character and how many of it opened the fence.
/// Whether a line's indent still lets it open or close a fence: at most
/// three spaces and no tab before its first character, as GFM reads it.
fn indent_allows_fence(line: &str) -> bool {
    let lead = &line[..line.len() - line.trim_start().len()];
    !lead.contains('\t') && lead.len() < 4
}

#[derive(Clone, Copy)]
struct Fence {
    char: u8,
    len: usize,
}

impl Fence {
    /// The fence a trimmed line opens, if any: three or more backticks or
    /// tildes, and for backticks an info string without a backtick (a line
    /// like `` ```x``` `` is a code span, not a fence).
    fn opens(trimmed: &str) -> Option<Fence> {
        let char = *trimmed
            .as_bytes()
            .first()
            .filter(|c| matches!(c, b'`' | b'~'))?;
        let len = trimmed.bytes().take_while(|c| *c == char).count();
        let info = &trimmed[len..];
        (len >= 3 && !(char == b'`' && info.contains('`'))).then_some(Fence { char, len })
    }

    /// Whether a trimmed line closes `open`: a run of the same character at
    /// least as long, with nothing after it. A shorter run, the other fence
    /// character or a run with an info string is a line of the code.
    fn closes(open: Fence, trimmed: &str) -> bool {
        let len = trimmed.bytes().take_while(|c| *c == open.char).count();
        len >= open.len && len == trimmed.len()
    }
}

/// Whether `text` holds a pipe that is not escaped by a backslash.
fn has_unescaped_pipe(text: &str) -> bool {
    split_cells(text).len() > 1
}

/// The cells of a trimmed row, its edge pipes dropped.
fn cells(trimmed: &str) -> Vec<&str> {
    let mut parts = split_cells(trimmed);
    if trimmed.starts_with('|') {
        parts.remove(0);
    }
    if parts.len() > 1 && trimmed.ends_with('|') && !trimmed.ends_with("\\|") {
        parts.pop();
    }
    parts
}

/// Whether a trimmed row is a delimiter row: every cell dashes with an
/// optional colon at either end.
fn is_delimiter(trimmed: &str) -> bool {
    let parts = cells(trimmed);
    has_unescaped_pipe(trimmed)
        && !parts.is_empty()
        && parts.iter().all(|cell| {
            let cell = cell.trim();
            let cell = cell.strip_prefix(':').unwrap_or(cell);
            let cell = cell.strip_suffix(':').unwrap_or(cell);
            !cell.is_empty() && cell.bytes().all(|c| c == b'-')
        })
}

/// `text` split at every pipe not preceded by a backslash, GFM's rule for
/// cells (inside code spans too). The pieces join back with `|` to `text`
/// byte for byte, edge pieces included: a row written with edge pipes has
/// an empty first and last piece, one written without them has none.
fn split_cells(text: &str) -> Vec<&str> {
    let bytes = text.as_bytes();
    let mut pieces = Vec::new();
    let mut from = 0;
    for (at, &byte) in bytes.iter().enumerate() {
        if byte == b'|' && (at == 0 || bytes[at - 1] != b'\\') {
            pieces.push(&text[from..at]);
            from = at + 1;
        }
    }
    pieces.push(&text[from..]);
    pieces
}

/// Merge one table row three ways, cell by cell. Every version must have the
/// same number of cells and the same line ending; each cell is taken byte
/// for byte from the side that changed it, an identical change from both
/// sides is kept once, and a cell both sides changed differently is a
/// conflict.
///
/// A side that changed more than one cell may have put a new row in the old
/// one's place, keeping some of its cells, so the other side's edit of a
/// kept cell would land on a row it never meant. When both sides changed the
/// row, it therefore merges only when each side changed exactly one cell, or
/// when every cell one side changed the other side changed the same way.
fn merge_row(base: &str, mine: &str, theirs: &str) -> Option<String> {
    let ending = |line: &str| line.ends_with('\n');
    if ending(base) != ending(mine) || ending(base) != ending(theirs) {
        return None;
    }
    let [b, m, t] = [base, mine, theirs].map(|line| split_cells(line.trim_end_matches('\n')));
    if b.len() != m.len() || b.len() != t.len() {
        return None;
    }
    let changed =
        |side: &[&str]| -> Vec<usize> { (0..b.len()).filter(|&i| side[i] != b[i]).collect() };
    let (by_mine, by_theirs) = (changed(&m), changed(&t));
    let covers = |wide: &[usize], narrow: &[usize]| narrow.iter().all(|i| wide.contains(i));
    if (by_mine.len() > 1 && !covers(&by_mine, &by_theirs))
        || (by_theirs.len() > 1 && !covers(&by_theirs, &by_mine))
    {
        return None;
    }
    let mut merged = Vec::with_capacity(b.len());
    for ((b, m), t) in b.iter().zip(&m).zip(&t) {
        merged.push(if m == t || t == b {
            *m
        } else if m == b {
            *t
        } else {
            return None;
        });
    }
    let mut row = merged.join("|");
    if ending(base) {
        row.push('\n');
    }
    Some(row)
}

/// The four marker runs no input or merged line may start with.
fn marker_runs() -> [String; 4] {
    ['<', '|', '=', '>'].map(|c| c.to_string().repeat(FALLBACK_MARKER_LEN))
}

/// Whether `line` starts with one of the four marker runs.
fn starts_like_a_marker(line: &str, markers: &[String; 4]) -> bool {
    markers.iter().any(|m| line.starts_with(m.as_str()))
}

/// Morph the live Y.Text into `target` with a minimal edit script, positions
/// in UTF-16 code units (the doc is `OffsetKind::Utf16`).
///
/// Lines first: an unchanged line is left alone, and a line with no
/// counterpart is removed or inserted whole. Whole-line removals keep the
/// edits clear of the compound-emoji deletion shapes yrs can panic on
/// (y-crdt/y-crdt#386). A run of changed lines replaced by as many lines is
/// edited line against line per extended grapheme cluster, so an agent's
/// change to one cell is an insert or a delete of just those characters, a
/// person's concurrent keystroke elsewhere in the row stays where they typed
/// it, and no removal ever starts or ends inside a cluster.
pub fn apply_target(text: &TextRef, txn: &mut TransactionMut, current: &str, target: &str) {
    let old: Vec<&str> = current.split_inclusive('\n').collect();
    let new: Vec<&str> = target.split_inclusive('\n').collect();
    let mut pos: u32 = 0;
    for op in capture_diff_slices(Algorithm::Myers, &old, &new) {
        match op {
            DiffOp::Equal { old_index, len, .. } => {
                pos += units(&old[old_index..old_index + len]);
            }
            DiffOp::Delete {
                old_index, old_len, ..
            } => text.remove_range(txn, pos, units(&old[old_index..old_index + old_len])),
            DiffOp::Insert {
                new_index, new_len, ..
            } => {
                for line in &new[new_index..new_index + new_len] {
                    text.insert(txn, pos, line);
                    pos += utf16(line);
                }
            }
            DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            } if old_len == new_len => {
                for (from, to) in old[old_index..old_index + old_len]
                    .iter()
                    .zip(&new[new_index..new_index + new_len])
                {
                    pos = apply_line(text, txn, pos, from, to);
                }
            }
            DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            } => {
                text.remove_range(txn, pos, units(&old[old_index..old_index + old_len]));
                for line in &new[new_index..new_index + new_len] {
                    text.insert(txn, pos, line);
                    pos += utf16(line);
                }
            }
        }
    }
}

/// Edit one line at `pos` from `from` into `to` per grapheme cluster, and
/// return the position just past it.
fn apply_line(text: &TextRef, txn: &mut TransactionMut, mut pos: u32, from: &str, to: &str) -> u32 {
    let old: Vec<&str> = from.graphemes(true).collect();
    let new: Vec<&str> = to.graphemes(true).collect();
    for op in capture_diff_slices(Algorithm::Myers, &old, &new) {
        if let DiffOp::Equal { .. } = op {
            pos += units(&old[op.old_range()]);
            continue;
        }
        let removed = units(&old[op.old_range()]);
        if removed > 0 {
            text.remove_range(txn, pos, removed);
        }
        let inserted = new[op.new_range()].concat();
        if !inserted.is_empty() {
            text.insert(txn, pos, &inserted);
            pos += utf16(&inserted);
        }
    }
    pos
}

/// The UTF-16 length of a run of slices, never bytes: the doc is
/// `OffsetKind::Utf16`, so its indexes are the ones a JS client counts.
fn units(slices: &[&str]) -> u32 {
    slices.iter().map(|s| utf16(s)).sum()
}

fn utf16(s: &str) -> u32 {
    s.encode_utf16().count() as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use yrs::{Doc, GetString, Options, Text, Transact};

    fn doc_with(content: &str) -> (Doc, yrs::TextRef) {
        let doc = Doc::with_options(Options {
            offset_kind: yrs::OffsetKind::Utf16,
            ..Options::default()
        });
        let text = doc.get_or_insert_text("content");
        {
            let mut txn = doc.transact_mut();
            text.insert(&mut txn, 0, content);
        }
        (doc, text)
    }

    #[test]
    fn disjoint_edits_merge_clean_across_spaces() {
        let base = "# T\r\n\r\nSection A: original\r\n\r\nSection B: original\r\n";
        let mine = "# T\n\nSection A: mine\n\nSection B: original\n"; // the room's LF text
        let theirs = "# T\r\n\r\nSection A: original\r\n\r\nSection B: theirs\r\n";
        let MergeOutcome::Clean(merged) = three_way(base, mine, theirs) else {
            panic!("disjoint edits merge");
        };
        assert_eq!(merged, "# T\n\nSection A: mine\n\nSection B: theirs\n");
    }

    #[test]
    fn same_line_edits_conflict_and_markers_never_leak() {
        let outcome = three_way("line\n", "line MINE\n", "line THEIRS\n");
        assert!(matches!(outcome, MergeOutcome::Conflict));
    }

    #[test]
    fn an_edit_touching_an_external_append_is_a_conflict() {
        // diff3 semantics, git merge-file's included: two hunks with no
        // unchanged line between them collide even though they do not
        // overlap. An append right behind the line being edited is therefore
        // the room's decision rather than a silent merge - safe, never lossy.
        let outcome = three_way("a\nlast\n", "a\nlast mine\n", "a\nlast\nappended\n");
        assert!(matches!(outcome, MergeOutcome::Conflict));
        // One unchanged line of separation is all it takes to merge cleanly.
        let MergeOutcome::Clean(merged) =
            three_way("a\nlast\n", "a mine\nlast\n", "a\nlast\nappended\n")
        else {
            panic!("separated hunks merge");
        };
        assert_eq!(merged, "a mine\nlast\nappended\n");
    }

    #[test]
    fn a_mixed_endings_theirs_merges_as_lf() {
        let MergeOutcome::Clean(merged) = three_way("a\r\n", "a\n", "a\r\nb\nmixed\r\n") else {
            panic!("a mixed theirs merges as LF");
        };
        assert_eq!(merged, "a\nb\nmixed\n");
    }

    #[test]
    fn a_crlf_base_with_lf_sides_merges_clean_as_lf() {
        let base = "# T\r\n\r\nSection A: original\r\n\r\nSection B: original\r\n";
        let mine = "# T\n\nSection A: mine\n\nSection B: original\n";
        let theirs = "# T\n\nSection A: original\n\nSection B: theirs\n";
        let MergeOutcome::Clean(merged) = three_way(base, mine, theirs) else {
            panic!("a CRLF base never makes LF sides conflict");
        };
        assert_eq!(merged, "# T\n\nSection A: mine\n\nSection B: theirs\n");
    }

    #[test]
    fn apply_target_morphs_the_doc_in_utf16_units() {
        let (doc, text) = doc_with("alpha\n😀 beta\ngamma\n");
        let target = "alpha\n😀 beta edited\ndelta\ngamma\n";
        {
            let mut txn = doc.transact_mut();
            let current = text.get_string(&txn);
            apply_target(&text, &mut txn, &current, target);
        }
        assert_eq!(
            text.get_string(&doc.transact()),
            target,
            "astral-plane emoji ahead of the edit does not shift positions"
        );
    }

    #[test]
    fn apply_target_handles_pure_insertion_and_deletion() {
        let (doc, text) = doc_with("a\nb\nc\n");
        {
            let mut txn = doc.transact_mut();
            let current = text.get_string(&txn);
            apply_target(&text, &mut txn, &current, "b\n");
        }
        assert_eq!(text.get_string(&doc.transact()), "b\n");
    }

    /// The three-person family (man, woman, girl): one extended grapheme
    /// cluster of five code points joined by two ZWJs.
    const FAMILY_GIRL: &str = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}";
    /// The same family with a boy, sharing every code point but the last.
    const FAMILY_BOY: &str = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F466}";
    /// The same family with a woman first, sharing every code point but the first.
    const FAMILY_WOMAN_FIRST: &str = "\u{1F469}\u{200D}\u{1F469}\u{200D}\u{1F467}";
    /// The German flag, two regional indicators.
    const FLAG_DE: &str = "\u{1F1E9}\u{1F1EA}";
    /// The Danish flag, sharing the first regional indicator with the German one.
    const FLAG_DK: &str = "\u{1F1E9}\u{1F1F0}";

    /// The header row and delimiter row of a table as wide as `row`.
    fn header_for(row: &str) -> String {
        let width = cells(row.lines().next().unwrap_or_default().trim()).len();
        format!("|{}\n|{}\n", " h |".repeat(width), " - |".repeat(width))
    }

    /// Merge three versions of a table body under one header, as wide as
    /// the base's first row: the merged body, or None for a conflict.
    fn merge_table(base: &str, mine: &str, theirs: &str) -> Option<String> {
        let header = header_for(base);
        let [base, mine, theirs] = [base, mine, theirs].map(|body| format!("{header}{body}"));
        match three_way(&base, &mine, &theirs) {
            MergeOutcome::Clean(merged) => Some(
                merged
                    .strip_prefix(header.as_str())
                    .expect("the header stays")
                    .to_string(),
            ),
            MergeOutcome::Conflict => None,
        }
    }

    #[test]
    fn two_cells_of_one_row_changed_on_each_side_merge_per_cell() {
        let merged = merge_table("| a | b |\n", "| aX | b |\n", "| a | bY |\n");
        assert_eq!(merged.as_deref(), Some("| aX | bY |\n"));
        // The header row is a row of the table too.
        let MergeOutcome::Clean(merged) = three_way(
            "| a | b |\n| - | - |\n",
            "| aX | b |\n| - | - |\n",
            "| a | bY |\n| - | - |\n",
        ) else {
            panic!("two cells of a header row merge");
        };
        assert_eq!(merged, "| aX | bY |\n| - | - |\n");
    }

    #[test]
    fn two_cells_of_one_row_merge_in_an_engram_body_and_in_a_crlf_file() {
        let base = "---\ntype: engram\ntitle: T\npermalink: t\nstatus: stable\n---\n\n# T\n\n| a | b |\n| - | - |\n| 1 | 2 |\n";
        let mine = base.replace("| 1 | 2 |", "| 1x | 2 |");
        let theirs = base
            .replace("| 1 | 2 |", "| 1 | 2Z |")
            .replace('\n', "\r\n");
        let MergeOutcome::Clean(merged) = three_way(&base.replace('\n', "\r\n"), &mine, &theirs)
        else {
            panic!("a same-row merge in the body is clean");
        };
        assert_eq!(merged, base.replace("| 1 | 2 |", "| 1x | 2Z |"));
    }

    #[test]
    fn one_cell_changed_two_ways_is_still_a_conflict() {
        assert_eq!(
            merge_table("| a | b |\n", "| aX | b |\n", "| aY | b |\n"),
            None
        );
    }

    #[test]
    fn a_cell_merge_needs_every_conflicting_row_to_merge() {
        // The first row merges per cell, the second does not: the hunk
        // is a conflict, never half merged.
        let base = "| a | b |\n| c | d |\n";
        let mine = "| aX | b |\n| cX | d |\n";
        let theirs = "| a | bY |\n| cY | d |\n";
        assert_eq!(merge_table(base, mine, theirs), None);
    }

    #[test]
    fn a_compound_emoji_beside_the_merge_survives_byte_identical() {
        let base = format!("| {FAMILY_GIRL} | {FLAG_DE} | c |\n");
        let mine = format!("| {FAMILY_GIRL} | {FLAG_DE} | cX |\n");
        let theirs = format!("| {FAMILY_BOY} | {FLAG_DE} | c |\n");
        let merged = merge_table(&base, &mine, &theirs);
        assert_eq!(
            merged,
            Some(format!("| {FAMILY_BOY} | {FLAG_DE} | cX |\n")),
            "a change to an emoji cell and one to the third cell merge"
        );
        let theirs = format!("| {FAMILY_GIRL} | {FLAG_DK} | c |\n");
        let merged = merge_table(&base, &mine, &theirs);
        assert_eq!(
            merged,
            Some(format!("| {FAMILY_GIRL} | {FLAG_DK} | cX |\n")),
            "a change to a flag cell and one to the third cell merge"
        );
    }

    #[test]
    fn two_changes_inside_one_compound_emoji_are_a_conflict() {
        // Code point by code point these would merge into a family nobody
        // typed; a cluster is one token, so both sides replaced the same one.
        let base = format!("| {FAMILY_GIRL} | b |\n");
        let mine = format!("| {FAMILY_BOY} | b |\n");
        let theirs = format!("| {FAMILY_WOMAN_FIRST} | b |\n");
        assert_eq!(merge_table(&base, &mine, &theirs), None);
        // Two flags in one cell are one cell both sides changed.
        let base = format!("| {FLAG_DE} x {FLAG_DE} | b |\n");
        let mine = format!("| {FLAG_DK} x {FLAG_DE} | b |\n");
        let theirs = format!("| {FLAG_DE} x {FLAG_DK} | b |\n");
        assert_eq!(merge_table(&base, &mine, &theirs), None);
        // In two cells they merge, each taken byte for byte.
        let base = format!("| {FLAG_DE} | {FLAG_DE} |\n");
        let mine = format!("| {FLAG_DK} | {FLAG_DE} |\n");
        let theirs = format!("| {FLAG_DE} | {FLAG_DK} |\n");
        let merged = merge_table(&base, &mine, &theirs);
        assert_eq!(
            merged,
            Some(format!("| {FLAG_DK} | {FLAG_DK} |\n")),
            "flags in separate cells merge"
        );
    }

    #[test]
    fn a_row_without_a_final_line_ending_merges_per_cell() {
        let merged = merge_table("| a | b |", "| aX | b |", "| a | bY |");
        assert_eq!(merged.as_deref(), Some("| aX | bY |"));
        // A line ending only one side has is a line change, decided by the
        // line merge alone.
        assert_eq!(merge_table("| a | b |", "| aX | b |\n", "| a | bY |"), None);
    }

    #[test]
    fn an_input_line_shaped_like_the_fallback_marker_keeps_the_conflict() {
        let marker = "<".repeat(FALLBACK_MARKER_LEN);
        let table = header_for("| a | b |");
        let base = format!("{marker} ours\n{table}| a | b |\n");
        let mine = format!("{marker} ours\n{table}| aX | b |\n");
        let theirs = format!("{marker} ours\n{table}| a | bY |\n");
        assert!(matches!(
            three_way(&base, &mine, &theirs),
            MergeOutcome::Conflict
        ));
    }

    /// Every change event the text sees while `edit` runs, as `=n` for a
    /// retain, `-n` for a delete and `+text` for an insert.
    fn deltas_of(content: &str, edit: impl FnOnce(&Doc, &yrs::TextRef)) -> Vec<String> {
        use std::sync::{Arc, Mutex};
        use yrs::Observable;
        use yrs::types::Delta;
        let (doc, text) = doc_with(content);
        let seen: Arc<Mutex<Vec<String>>> = Arc::default();
        let sink = seen.clone();
        text.observe("deltas", move |txn, event| {
            for delta in event.delta(txn) {
                sink.lock().unwrap().push(match delta {
                    Delta::Retain(n, _) => format!("={n}"),
                    Delta::Deleted(n) => format!("-{n}"),
                    Delta::Inserted(value, _) => format!("+{}", value.clone().to_string(txn)),
                });
            }
        });
        edit(&doc, &text);
        seen.lock().unwrap().clone()
    }

    fn morph(doc: &Doc, text: &yrs::TextRef, target: &str) {
        let mut txn = doc.transact_mut();
        let current = text.get_string(&txn);
        apply_target(text, &mut txn, &current, target);
    }

    #[test]
    fn apply_target_inserts_only_the_character_a_cell_gained() {
        let deltas = deltas_of("| 1 | 2 |\n", |doc, text| morph(doc, text, "| 1 | 2Z |\n"));
        assert_eq!(deltas, vec!["=7".to_string(), "+Z".to_string()]);
    }

    #[test]
    fn apply_target_keeps_unchanged_lines_and_edits_inside_the_changed_one() {
        let content = "| a | b |\n| - | - |\n| 1 | 2 |\n| 3 | 4 |\n";
        let target = "| a | b |\n| - | - |\n| 1 | two |\n| 3 | 4 |\n";
        let deltas = deltas_of(content, |doc, text| morph(doc, text, target));
        // "| a | b |\n| - | - |\n| 1 | " is 26 units; the "2" goes, "two" comes.
        let mut sorted = deltas.clone();
        sorted.sort();
        assert_eq!(sorted, vec!["+two", "-1", "=26"], "{deltas:?}");
    }

    #[test]
    fn apply_target_replaces_a_compound_emoji_as_one_cluster() {
        let content = format!("| {FAMILY_GIRL} | {FLAG_DE} |\n");
        let target = format!("| {FAMILY_BOY} | {FLAG_DK} |\n");
        let deltas = deltas_of(&content, |doc, text| morph(doc, text, &target));
        let removed: u32 = deltas
            .iter()
            .filter_map(|d| d.strip_prefix('-'))
            .map(|n| n.parse::<u32>().unwrap())
            .sum();
        let inserted: Vec<&str> = deltas.iter().filter_map(|d| d.strip_prefix('+')).collect();
        let whole = (FAMILY_GIRL.encode_utf16().count() + FLAG_DE.encode_utf16().count()) as u32;
        assert_eq!(removed, whole, "whole clusters go: {deltas:?}");
        assert_eq!(inserted, vec![FAMILY_BOY, FLAG_DK], "{deltas:?}");
        let (doc, text) = doc_with(&content);
        morph(&doc, &text, &target);
        assert_eq!(text.get_string(&doc.transact()), target);
    }

    #[test]
    fn apply_target_removes_a_line_without_a_counterpart_whole() {
        let deltas = deltas_of("a\nb\nc\n", |doc, text| morph(doc, text, "a\nc\n"));
        assert_eq!(deltas, vec!["=2".to_string(), "-2".to_string()]);
    }

    #[test]
    fn a_keystroke_concurrent_with_apply_target_stays_in_its_cell() {
        use yrs::updates::decoder::Decode;
        use yrs::{ReadTxn, Update};
        let (server, text) = doc_with("| 1 | 2 |\n");
        let person = Doc::with_options(Options {
            offset_kind: yrs::OffsetKind::Utf16,
            ..Options::default()
        });
        let typed = person.get_or_insert_text("content");
        let sync = |from: &Doc, to: &Doc| {
            let sv = to.transact().state_vector();
            let update = from.transact().encode_state_as_update_v1(&sv);
            to.transact_mut()
                .apply_update(Update::decode_v1(&update).unwrap())
                .unwrap();
        };
        sync(&server, &person);
        // The person types "x" after "1" before the agent's change reaches them.
        typed.insert(&mut person.transact_mut(), 3, "x");
        morph(&server, &text, "| 1 | 2Z |\n");
        sync(&person, &server);
        sync(&server, &person);
        assert_eq!(text.get_string(&server.transact()), "| 1x | 2Z |\n");
        assert_eq!(typed.get_string(&person.transact()), "| 1x | 2Z |\n");
    }

    #[test]
    fn a_hunk_of_equal_line_counts_whose_lines_moved_is_a_conflict() {
        // Theirs dropped the first row and added a last one: as many lines,
        // but none of them pairs with its own counterpart.
        let base = "| 1 | 2 | 3 |\n| 4 | 2 | 3 |\n";
        let mine = "| 1 | 2 | 3x |\n| 4 | 2y | 3 |\n";
        let theirs = "| 4 | 2 | 3 |\n| 7 | 2 | 3 |\n";
        assert_eq!(merge_table(base, mine, theirs), None);
    }

    #[test]
    fn two_rows_one_side_swapped_are_a_conflict() {
        let base = "| 1 | same text |\n| 2 | same text |\n";
        let mine = "| 2 | same text. |\n| 1 | same text. |\n";
        let theirs = "| 1 | same text |\n| 2 | sameY text |\n";
        assert_eq!(merge_table(base, mine, theirs), None);
    }

    #[test]
    fn two_rows_one_side_dropped_at_the_top_and_added_at_the_bottom_are_a_conflict() {
        let base = "| 1 | aa | bb |\n| 4 | aa | bb |\n";
        let mine = "| 4 | aa | bbx |\n| 7 | aa | bb |\n";
        let theirs = "| 1 | aaY | bb |\n| 4 | aa | bb |\n";
        assert_eq!(merge_table(base, mine, theirs), None);
    }

    #[test]
    fn a_tie_in_likeness_pairs_nothing() {
        // Both base lines are equally like both side lines: no evidence.
        assert!(!replaces_in_place(&["ab\n", "ab\n"], &["abx\n", "aby\n"]));
        assert!(replaces_in_place(
            &["| 1 | 2 |\n", "| 3 | 4 |\n"],
            &["| 1x | 2 |\n", "| 3x | 4 |\n"]
        ));
        // An inserted or deleted line anywhere fails the whole text.
        assert!(!replaces_in_place(&["a\n", "b\n"], &["a\n", "b\n", "c\n"]));
    }

    #[test]
    fn a_swap_split_by_an_unchanged_row_is_a_conflict() {
        let base = "| 1 | aa bb |\n| 2 | aa bb |\n| 3 | aa bb |\n";
        let mine = "| 1 | Qaa bb |\n| 2 | aa bb |\n| 3 | aa bb |\n";
        let theirs = "| 3 | aa Ybb |\n| 2 | aa bb |\n| 1 | aa bbY |\n";
        assert_eq!(merge_table(base, mine, theirs), None);
    }

    #[test]
    fn a_row_replaced_by_a_new_one_against_an_edit_of_it_is_a_conflict() {
        // Mine changed cell 2, theirs changed cells 1 and 2: cell 2 changed
        // on both sides.
        let base = "| 1 | aa |\n| 2 | aa |\n";
        let mine = "| 1 | Qaa |\n| 2 | aa |\n";
        let theirs = "| 8 | ab |\n| 2 | aa |\n";
        assert_eq!(merge_table(base, mine, theirs), None);
    }

    #[test]
    fn only_table_rows_take_the_cell_fallback() {
        // A prose line both sides changed stays a conflict, as in 0.30.0.
        assert!(matches!(
            three_way("one two three\n", "one X two three\n", "one two Y three\n"),
            MergeOutcome::Conflict
        ));
        // A conflicting run that holds a prose line is not taken either.
        assert!(matches!(
            three_way(
                "intro\n| a | b |\n| - | - |\n",
                "intro mine\n| aX | b |\n| - | - |\n",
                "intro\n| a | bY |\n| - | - |\n"
            ),
            MergeOutcome::Conflict
        ));
        // A changed delimiter row inside the run is not taken.
        assert!(matches!(
            three_way(
                "| a | b |\n| - | - |\n",
                "| aX | b |\n| :- | - |\n",
                "| a | bY |\n| - | - |\n"
            ),
            MergeOutcome::Conflict
        ));
        // A row in a fenced code block is not a table row.
        assert!(matches!(
            three_way(
                "```\n| a | b |\n| - | - |\n```\n",
                "```\n| aX | b |\n| - | - |\n```\n",
                "```\n| a | bY |\n| - | - |\n```\n"
            ),
            MergeOutcome::Conflict
        ));
        // A pipeless table, opened by its header and delimiter rows, is.
        let MergeOutcome::Clean(merged) = three_way(
            "a | b\n--|--\n1 | 2\n",
            "a | b\n--|--\n1x | 2\n",
            "a | b\n--|--\n1 | 2y\n",
        ) else {
            panic!("a pipeless table row merges per cell");
        };
        assert_eq!(merged, "a | b\n--|--\n1x | 2y\n");
    }

    #[test]
    fn an_escaped_pipe_stays_inside_its_cell() {
        let base = "| a \\| b | c |\n";
        let mine = "| a \\| bX | c |\n";
        let theirs = "| a \\| b | cY |\n";
        let merged = merge_table(base, mine, theirs);
        assert_eq!(
            merged.as_deref(),
            Some("| a \\| bX | cY |\n"),
            "an escaped pipe is not a cell boundary"
        );
        // Splitting a cell with a new pipe changes the cell count.
        assert_eq!(merge_table(base, "| a | b | c | d |\n", theirs), None);
    }

    #[test]
    fn an_edit_of_a_cell_a_replacing_row_kept_is_a_conflict() {
        // Theirs put a new row in the old one's place and kept the owner;
        // mine's edit of the owner was meant for the old row.
        let base = "| id | status | owner |\n| - | - | - |\n| 1 | open | Alice |\n";
        let mine = "| id | status | owner |\n| - | - | - |\n| 1 | open | Alice B. |\n";
        let theirs = "| id | status | owner |\n| - | - | - |\n| 3 | done | Alice |\n";
        assert!(matches!(
            three_way(base, mine, theirs),
            MergeOutcome::Conflict
        ));
        assert!(matches!(
            three_way(base, theirs, mine),
            MergeOutcome::Conflict
        ));
        // A kept empty cell is a cell like any other.
        assert_eq!(
            merge_table(
                "| 1 | Fix login |  |\n",
                "| 4 | Add search |  |\n",
                "| 1 | Fix login | blocked |\n"
            ),
            None
        );
        // Two cells changed on one side and one other cell on the other.
        assert_eq!(
            merge_table("| a | b | c |\n", "| aX | bX | c |\n", "| a | b | cY |\n"),
            None
        );
        // One cell each, in different cells, still merges.
        assert_eq!(
            merge_table("| a | b | c |\n", "| aX | b | c |\n", "| a | b | cY |\n").as_deref(),
            Some("| aX | b | cY |\n")
        );
    }

    #[test]
    fn a_line_outside_a_table_is_never_merged_per_cell() {
        // Prose that starts with a pipe.
        assert!(matches!(
            three_way(
                "|x| is the norm of x and |y| of y.\n",
                "|x| is the NORM of x and |y| of y.\n",
                "|x| is the norm of x and |y| of y, see below.\n"
            ),
            MergeOutcome::Conflict
        ));
        // Prose that ends with one.
        assert!(matches!(
            three_way(
                "choose a or b |\n",
                "choose A or b |\n",
                "choose a or b | c\n"
            ),
            MergeOutcome::Conflict
        ));
        // A list item shaped like a row, even under a delimiter row.
        assert!(matches!(
            three_way("- | a | b |\n", "- | aX | b |\n", "- | a | bY |\n"),
            MergeOutcome::Conflict
        ));
        assert!(matches!(
            three_way(
                "- | a | b |\n| - | - |\n",
                "- | aX | b |\n| - | - |\n",
                "- | a | bY |\n| - | - |\n"
            ),
            MergeOutcome::Conflict
        ));
        // A block quote shaped like a row.
        assert!(matches!(
            three_way("> | a | b |\n", "> | aX | b |\n", "> | a | bY |\n"),
            MergeOutcome::Conflict
        ));
        // A row with no header and delimiter row above it is no table.
        assert!(matches!(
            three_way("| a | b |\n", "| aX | b |\n", "| a | bY |\n"),
            MergeOutcome::Conflict
        ));
        // A delimiter row with another cell count opens no table.
        assert!(matches!(
            three_way(
                "| a | b |\n| - | - | - |\n",
                "| aX | b |\n| - | - | - |\n",
                "| a | bY |\n| - | - | - |\n"
            ),
            MergeOutcome::Conflict
        ));
    }

    #[test]
    fn an_indented_backtick_line_neither_opens_nor_closes_a_fence() {
        // The indented line is code, so the real fence below opens a code
        // block, and the table inside it is code too.
        let inside = |row: &str| format!("    ```\n\n```\n| h | i |\n|---|---|\n{row}\n```\n");
        let outcome = three_way(
            &inside("| a | b |"),
            &inside("| aX | b |"),
            &inside("| a | bY |"),
        );
        assert!(matches!(outcome, MergeOutcome::Conflict));
        // An indented line inside a fence does not close it either.
        let fenced = |row: &str| format!("```\n    ```\n| h | i |\n|---|---|\n{row}\n```\n");
        let outcome = three_way(
            &fenced("| a | b |"),
            &fenced("| aX | b |"),
            &fenced("| a | bY |"),
        );
        assert!(matches!(outcome, MergeOutcome::Conflict));
    }

    #[test]
    fn a_shorter_fence_inside_a_fence_does_not_close_it() {
        let inside = |fence: &str, inner: &str, row: &str| {
            format!("{fence}\n{inner}\n| a | b |\n| - | - |\n{row}\n{fence}\n")
        };
        for (fence, inner) in [
            ("````", "```"),
            ("~~~~", "~~~"),
            ("```", "~~~"),
            ("~~~", "```"),
            ("```", "```rust"),
        ] {
            let outcome = three_way(
                &inside(fence, inner, "| 1 | 2 |"),
                &inside(fence, inner, "| 1x | 2 |"),
                &inside(fence, inner, "| 1 | 2y |"),
            );
            assert!(
                matches!(outcome, MergeOutcome::Conflict),
                "{inner} inside {fence} closes nothing"
            );
        }
        // A fence closed by a run at least as long ends it: a table after
        // it merges.
        let after = |row: &str| format!("````\ncode\n`````\n| a | b |\n| - | - |\n{row}\n");
        let MergeOutcome::Clean(merged) = three_way(
            &after("| 1 | 2 |"),
            &after("| 1x | 2 |"),
            &after("| 1 | 2y |"),
        ) else {
            panic!("a table after a closed fence merges per cell");
        };
        assert_eq!(merged, after("| 1x | 2y |"));
        // A line of backticks with a backtick after them is a code span,
        // not a fence.
        let span = |row: &str| format!("```x```\n\n| a | b |\n| - | - |\n{row}\n");
        let MergeOutcome::Clean(merged) =
            three_way(&span("| 1 | 2 |"), &span("| 1x | 2 |"), &span("| 1 | 2y |"))
        else {
            panic!("a table after a code span merges per cell");
        };
        assert_eq!(merged, span("| 1x | 2y |"));
    }

    #[test]
    fn the_same_change_to_one_cell_from_both_sides_is_kept_once() {
        let merged = merge_table("| a | b |\n", "| aX | bZ |\n", "| aX | b |\n");
        assert_eq!(merged.as_deref(), Some("| aX | bZ |\n"));
    }

    #[test]
    fn two_rows_both_sides_changed_in_place_still_merge_per_cell() {
        let base = "| 1 | 2 |\n| 3 | 4 |\n";
        let mine = "| 1x | 2 |\n| 3x | 4 |\n";
        let theirs = "| 1 | 2y |\n| 3 | 4y |\n";
        let merged = merge_table(base, mine, theirs);
        assert_eq!(merged.as_deref(), Some("| 1x | 2y |\n| 3x | 4y |\n"));
    }

    #[test]
    fn a_date_changed_two_ways_in_the_frontmatter_is_a_conflict() {
        let base = "---\ntype: engram\ntitle: T\npermalink: t\nstatus: stable\ndate: 2026-11-02\n---\n\n# T\n\n| a | b |\n| - | - |\n| 1 | 2 |\n";
        let mine = base.replace("2026-11-02", "2026-11-05");
        let theirs = base.replace("2026-11-02", "2026-12-02");
        assert!(matches!(
            three_way(base, &mine, &theirs),
            MergeOutcome::Conflict
        ));
        // The body of the same page still merges per cell.
        let mine = base.replace("| 1 | 2 |", "| 1x | 2 |");
        let theirs = base.replace("| 1 | 2 |", "| 1 | 2y |");
        let MergeOutcome::Clean(merged) = three_way(base, &mine, &theirs) else {
            panic!("a body cell merge is clean");
        };
        assert_eq!(merged, base.replace("| 1 | 2 |", "| 1x | 2y |"));
    }

    #[test]
    fn a_lone_regional_indicator_beside_an_edit_survives_byte_identical() {
        let d = "\u{1F1E9}";
        let base = format!("| {d}a | b |\n");
        let mine = format!("| {d}ax | b |\n");
        let theirs = format!("| {d}a | by |\n");
        let merged = merge_table(&base, &mine, &theirs);
        assert_eq!(
            merged,
            Some(format!("| {d}ax | by |\n")),
            "a lone regional indicator beside the edits merges"
        );
    }

    /// One row of the oracle below: who it is (a base row's id, or the id a
    /// side gave a new row) and its three cells.
    type Tracked = (String, [String; 3]);

    /// One side of the oracle below: the rows it ends with and what it did.
    #[derive(Clone, Default)]
    struct Moved {
        rows: Vec<Tracked>,
        edited: Vec<(String, usize)>,
        structural: bool,
    }

    fn render_rows(rows: &[Tracked]) -> String {
        let body: String = rows
            .iter()
            .map(|(_, [id, x, y])| format!("| {id} | {x} | {y} |\n"))
            .collect();
        format!("| id | x | y |\n| - | - | - |\n{body}")
    }

    /// Every side the oracle tries against a base: nothing, one edit (a
    /// marker appended to cell 1 or 2 of a base row), a deleted row, an added
    /// row, a row replaced by a new one that keeps none, one or both of the
    /// replaced row's other cells, two rows swapped, and each of those moves
    /// with one edit on top.
    fn moves(base: &[Tracked], mark: char, new_id: &str) -> Vec<Moved> {
        let plain = Moved {
            rows: base.to_vec(),
            ..Moved::default()
        };
        let fresh = |old: &[String; 3], keep: [bool; 2]| -> Tracked {
            let cell = |c: usize, made: String| {
                if keep[c - 1] { old[c].clone() } else { made }
            };
            let cells = [
                new_id.to_string(),
                cell(1, format!("n{new_id}")),
                cell(2, format!("m{new_id}")),
            ];
            (new_id.to_string(), cells)
        };
        let mut moved = vec![plain.clone()];
        for (k, (_, old)) in base.iter().enumerate() {
            let mut d = plain.clone();
            d.rows.remove(k);
            d.structural = true;
            moved.push(d);
            for keep in [[false, false], [true, false], [false, true], [true, true]] {
                let mut r = plain.clone();
                r.rows[k] = fresh(old, keep);
                r.structural = true;
                moved.push(r);
            }
        }
        for q in 0..=base.len() {
            let mut a = plain.clone();
            a.rows
                .insert(q, fresh(&base[q.min(base.len() - 1)].1, [false, false]));
            a.structural = true;
            moved.push(a);
        }
        for a in 0..base.len() {
            for b in a + 1..base.len() {
                let mut w = plain.clone();
                w.rows.swap(a, b);
                w.structural = true;
                moved.push(w);
            }
        }
        let mut all = Vec::new();
        for side in moved {
            all.push(side.clone());
            for k in 0..side.rows.len() {
                if side.rows[k].0 == new_id {
                    continue;
                }
                for cell in [1, 2] {
                    let mut e = side.clone();
                    e.rows[k].1[cell].push(mark);
                    e.edited.push((e.rows[k].0.clone(), cell));
                    all.push(e);
                }
            }
        }
        all
    }

    /// What a side did to the base, by row identity: each base row's cells
    /// (None for a row it deleted) and the rows it added.
    type Reading = (Vec<Option<[String; 3]>>, Vec<[String; 3]>);

    /// The readings of one side: by identity, and, when it replaced a base
    /// row in place by a new row differing from it in exactly one cell, the
    /// accepted reading of that change as an edit of the row.
    fn readings(base: &[Tracked], side: &Moved) -> Vec<Reading> {
        let read = |rows: &[Tracked]| -> Reading {
            let kept = base
                .iter()
                .map(|(id, _)| rows.iter().find(|(r, _)| r == id).map(|(_, c)| c.clone()))
                .collect();
            let added = rows
                .iter()
                .filter(|(r, _)| !base.iter().any(|(id, _)| id == r))
                .map(|(_, c)| c.clone())
                .collect();
            (kept, added)
        };
        let mut all = vec![read(&side.rows)];
        if side.rows.len() == base.len() {
            let mut rows = side.rows.clone();
            let mut relabelled = false;
            for (row, (id, cells)) in rows.iter_mut().zip(base) {
                let differ = (0..3).filter(|&c| row.1[c] != cells[c]).count();
                if row.0 != *id && !base.iter().any(|(b, _)| *b == row.0) && differ == 1 {
                    row.0 = id.clone();
                    relabelled = true;
                }
            }
            if relabelled {
                all.push(read(&rows));
            }
        }
        all
    }

    /// The merge two readings call for, as sorted body lines, or None when
    /// only a conflict is right.
    fn expected(base: &[Tracked], m: &Reading, t: &Reading) -> Option<Vec<String>> {
        let line = |[id, x, y]: &[String; 3]| format!("| {id} | {x} | {y} |\n");
        let mut want = Vec::new();
        for (k, (_, b)) in base.iter().enumerate() {
            match (&m.0[k], &t.0[k]) {
                (None, None) => {}
                (None, Some(c)) | (Some(c), None) if c != b => return None,
                (None, Some(_)) | (Some(_), None) => {}
                (Some(mc), Some(tc)) => {
                    let mut row = b.clone();
                    for c in 0..3 {
                        row[c] = if mc[c] == tc[c] || tc[c] == b[c] {
                            mc[c].clone()
                        } else if mc[c] == b[c] {
                            tc[c].clone()
                        } else {
                            return None;
                        };
                    }
                    want.push(line(&row));
                }
            }
        }
        want.extend(m.1.iter().chain(&t.1).map(line));
        want.sort();
        Some(want)
    }

    #[test]
    fn the_cell_fallback_never_merges_a_move_wrong() {
        let mut cases = 0;
        let mut bases: Vec<Vec<Tracked>> = Vec::new();
        for n in [2usize, 3] {
            bases.push(
                (1..=n)
                    .map(|k| {
                        (
                            k.to_string(),
                            [k.to_string(), format!("x{k}"), format!("y{k}")],
                        )
                    })
                    .collect(),
            );
        }
        // Rows that share cells, empty ones among them, so a new row can
        // keep cells its neighbour holds too.
        let values = ["aa", ""];
        for x1 in values {
            for y1 in values {
                for x2 in values {
                    for y2 in values {
                        let row = |k: &str, x: &str, y: &str| {
                            (k.to_string(), [k.to_string(), x.to_string(), y.to_string()])
                        };
                        bases.push(vec![row("1", x1, y1), row("2", x2, y2)]);
                    }
                }
            }
        }
        for base in &bases {
            let text = render_rows(base);
            let mines = moves(base, 'Q', "7");
            let theirs = moves(base, 'Y', "8");
            for m in &mines {
                for t in &theirs {
                    let (ms, ts) = (render_rows(&m.rows), render_rows(&t.rows));
                    if line_merge(&text, &ms, &ts).is_some() {
                        continue;
                    }
                    cases += 1;
                    let context = format!("base {text:?}\nmine {ms:?}\ntheirs {ts:?}");
                    match three_way(&text, &ms, &ts) {
                        MergeOutcome::Clean(merged) => {
                            let body = merged
                                .strip_prefix("| id | x | y |\n| - | - | - |\n")
                                .expect("the header stays");
                            let mut got: Vec<String> =
                                body.split_inclusive('\n').map(str::to_string).collect();
                            got.sort();
                            let right = readings(base, m).iter().any(|rm| {
                                readings(base, t)
                                    .iter()
                                    .any(|rt| expected(base, rm, rt).as_ref() == Some(&got))
                            });
                            assert!(right, "a wrong merge {got:?}\n{context}");
                        }
                        MergeOutcome::Conflict => {
                            let simple = !m.structural && !t.structural;
                            let disjoint = !m.edited.iter().any(|e| t.edited.contains(e));
                            assert!(
                                !(simple && disjoint),
                                "edits of different cells merge\n{context}"
                            );
                        }
                    }
                }
            }
        }
        assert!(
            cases > 1000,
            "the oracle reached the fallback {cases} times"
        );
    }

    const HARBOR: &str = "---\ntype: manifest\ntitle: Harbor\npermalink: manifest\ntags:\n  - manifest\nstatus: stable\n---\n\n# Harbor\n\n## Scope\n\n- Engineering\n";

    fn with(text: &str, after: &str, line: &str) -> String {
        text.replacen(after, &format!("{after}{line}"), 1)
    }

    #[test]
    fn a_key_both_sides_added_with_one_value_lands_once() {
        let mine = with(HARBOR, "status: stable\n", "domain_name: harbor\n");
        let theirs = with(HARBOR, "title: Harbor\n", "domain_name: harbor\n");
        let MergeOutcome::Clean(merged) = three_way(HARBOR, &mine, &theirs) else {
            panic!("one value lands once");
        };
        assert_eq!(merged, theirs);
    }

    #[test]
    fn a_crlf_file_merges_by_key_in_session_space() {
        let base = HARBOR.replace('\n', "\r\n");
        let theirs = with(HARBOR, "title: Harbor\n", "domain_name: harbor\n").replace('\n', "\r\n");
        let mine = with(HARBOR, "status: stable\n", "domain_name: harbor\n");
        let MergeOutcome::Clean(merged) = three_way(&base, &mine, &theirs) else {
            panic!("every side is merged as LF");
        };
        assert_eq!(merged, to_lf(&theirs));
    }

    #[test]
    fn a_merge_that_would_break_the_frontmatter_is_a_conflict() {
        let base = "---\na: &x 1\nb: *x\n---\n\nBody.\n";
        let mine = "---\na: &x 1\nc: 1\nb: *x\n---\n\nBody.\n";
        let theirs = "---\na: &x 1\nb: *x\nc: 2\n---\n\nBody.\n";
        assert!(matches!(
            three_way(base, mine, theirs),
            MergeOutcome::Conflict
        ));
    }

    #[test]
    fn a_comment_typed_on_a_key_line_upstream_changed_is_a_conflict() {
        let mine = HARBOR.replace("status: stable", "status: stable # reviewed by kim");
        let theirs = HARBOR.replace("status: stable", "status: archived");
        assert!(matches!(
            three_way(HARBOR, &mine, &theirs),
            MergeOutcome::Conflict
        ));
    }

    #[test]
    fn a_comment_typed_under_a_key_upstream_changed_is_a_conflict() {
        let mine = with(HARBOR, "title: Harbor\n", "# the ship's engineer\n");
        let theirs = HARBOR.replace("title: Harbor", "title: Montgomery Scott");
        assert!(matches!(
            three_way(HARBOR, &mine, &theirs),
            MergeOutcome::Conflict
        ));
    }

    #[test]
    fn an_outside_comment_on_a_key_line_typed_over_is_a_conflict() {
        let mine = HARBOR.replace("status: stable", "status: archived");
        let theirs = HARBOR.replace("status: stable", "status: stable # reviewed by kim");
        assert!(matches!(
            three_way(HARBOR, &mine, &theirs),
            MergeOutcome::Conflict
        ));
    }

    #[test]
    fn an_outside_comment_under_a_key_typed_over_is_a_conflict() {
        let mine = HARBOR.replace("title: Harbor", "title: Montgomery Scott");
        let theirs = with(HARBOR, "title: Harbor\n", "# the ship's engineer\n");
        assert!(matches!(
            three_way(HARBOR, &mine, &theirs),
            MergeOutcome::Conflict
        ));
    }

    #[test]
    fn a_half_typed_frontmatter_keeps_todays_merge() {
        let mine = HARBOR.replace("title: Harbor", "title: \"Harbor");
        let theirs = with(HARBOR, "- Engineering\n", "- Warp\n");
        let MergeOutcome::Clean(merged) = three_way(HARBOR, &mine, &theirs) else {
            panic!("the person's unfinished quote is not the merge's business");
        };
        assert_eq!(merged, with(&mine, "- Engineering\n", "- Warp\n"));
    }
}
