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
///    of it is a table row on all three sides and no delimiter row in it
///    changed ([`table_rows`]). Any other conflicting line stays a conflict.
/// 3. Per cell: each row of such a run is merged cell by cell
///    ([`merge_row`]); a cell both sides changed differently is a conflict.
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

/// Classify each line of one text. A line is a table row when it starts or
/// ends with an unescaped pipe, or when it belongs to a table opened by a
/// header row and a delimiter row with as many cells (the pipeless GFM
/// form), up to the first line without an unescaped pipe. Lines inside a
/// fenced code block, and lines indented four spaces or more, are never
/// rows.
fn table_rows(lines: &[&str]) -> Vec<Row> {
    let mut kinds = vec![Row::Other; lines.len()];
    let mut fence: Option<char> = None;
    let mut in_table = false;
    for (k, line) in lines.iter().enumerate() {
        let text = line.trim_end_matches('\n');
        let trimmed = text.trim();
        if let Some(c) = fence {
            if trimmed.starts_with(&c.to_string().repeat(3)) {
                fence = None;
            }
            in_table = false;
            continue;
        }
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fence = trimmed.chars().next();
            in_table = false;
            continue;
        }
        if text.starts_with("    ") || text.starts_with('\t') || !has_unescaped_pipe(trimmed) {
            in_table = false;
            continue;
        }
        let opens = lines.get(k + 1).is_some_and(|next| {
            let next = next.trim_end_matches('\n').trim();
            is_delimiter(next) && cells(next).len() == cells(trimmed).len()
        });
        in_table = in_table || opens;
        let edged =
            trimmed.starts_with('|') || (trimmed.ends_with('|') && !trimmed.ends_with("\\|"));
        if in_table || edged {
            kinds[k] = if is_delimiter(trimmed) {
                Row::Delimiter
            } else {
                Row::Data
            };
        }
    }
    kinds
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
fn merge_row(base: &str, mine: &str, theirs: &str) -> Option<String> {
    let ending = |line: &str| line.ends_with('\n');
    if ending(base) != ending(mine) || ending(base) != ending(theirs) {
        return None;
    }
    let [b, m, t] = [base, mine, theirs].map(|line| split_cells(line.trim_end_matches('\n')));
    if b.len() != m.len() || b.len() != t.len() {
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

    #[test]
    fn two_cells_of_one_row_changed_on_each_side_merge_per_cell() {
        let MergeOutcome::Clean(merged) = three_way("| a | b |\n", "| aX | b |\n", "| a | bY |\n")
        else {
            panic!("two cells of one row merge");
        };
        assert_eq!(merged, "| aX | bY |\n");
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
        assert!(matches!(
            three_way("| a | b |\n", "| aX | b |\n", "| aY | b |\n"),
            MergeOutcome::Conflict
        ));
    }

    #[test]
    fn a_cell_merge_needs_every_conflicting_row_to_merge() {
        // The first row merges per character, the second does not: the hunk
        // is a conflict, never half merged.
        let base = "| a | b |\n| c | d |\n";
        let mine = "| aX | b |\n| cX | d |\n";
        let theirs = "| a | bY |\n| cY | d |\n";
        assert!(matches!(
            three_way(base, mine, theirs),
            MergeOutcome::Conflict
        ));
    }

    #[test]
    fn a_compound_emoji_beside_the_merge_survives_byte_identical() {
        let base = format!("| {FAMILY_GIRL} | {FLAG_DE} | c |\n");
        let mine = format!("| {FAMILY_GIRL} | {FLAG_DE} | cX |\n");
        let theirs = format!("| {FAMILY_BOY} | {FLAG_DK} | c |\n");
        let MergeOutcome::Clean(merged) = three_way(&base, &mine, &theirs) else {
            panic!("a change to the emoji cells and one to the third cell merge");
        };
        assert_eq!(merged, format!("| {FAMILY_BOY} | {FLAG_DK} | cX |\n"));
    }

    #[test]
    fn two_changes_inside_one_compound_emoji_are_a_conflict() {
        // Code point by code point these would merge into a family nobody
        // typed; a cluster is one token, so both sides replaced the same one.
        let base = format!("| {FAMILY_GIRL} | b |\n");
        let mine = format!("| {FAMILY_BOY} | b |\n");
        let theirs = format!("| {FAMILY_WOMAN_FIRST} | b |\n");
        assert!(matches!(
            three_way(&base, &mine, &theirs),
            MergeOutcome::Conflict
        ));
        // Two flags in one cell are one cell both sides changed.
        let base = format!("| {FLAG_DE} x {FLAG_DE} | b |\n");
        let mine = format!("| {FLAG_DK} x {FLAG_DE} | b |\n");
        let theirs = format!("| {FLAG_DE} x {FLAG_DK} | b |\n");
        assert!(matches!(
            three_way(&base, &mine, &theirs),
            MergeOutcome::Conflict
        ));
        // In two cells they merge, each taken byte for byte.
        let base = format!("| {FLAG_DE} | {FLAG_DE} |\n");
        let mine = format!("| {FLAG_DK} | {FLAG_DE} |\n");
        let theirs = format!("| {FLAG_DE} | {FLAG_DK} |\n");
        let MergeOutcome::Clean(merged) = three_way(&base, &mine, &theirs) else {
            panic!("flags in separate cells merge");
        };
        assert_eq!(merged, format!("| {FLAG_DK} | {FLAG_DK} |\n"));
    }

    #[test]
    fn a_row_without_a_final_line_ending_merges_per_cell() {
        let MergeOutcome::Clean(merged) = three_way("| a | b |", "| aX | b |", "| a | bY |") else {
            panic!("a last row without a line ending merges");
        };
        assert_eq!(merged, "| aX | bY |");
        // A line ending only one side has is a line change, decided by the
        // line merge alone.
        assert!(matches!(
            three_way("| a | b |", "| aX | b |\n", "| a | bY |"),
            MergeOutcome::Conflict
        ));
    }

    #[test]
    fn an_input_line_shaped_like_the_fallback_marker_keeps_the_conflict() {
        let marker = "<".repeat(FALLBACK_MARKER_LEN);
        let base = format!("{marker} ours\n| a | b |\n");
        let mine = format!("{marker} ours\n| aX | b |\n");
        let theirs = format!("{marker} ours\n| a | bY |\n");
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
        assert!(matches!(
            three_way(base, mine, theirs),
            MergeOutcome::Conflict
        ));
    }

    #[test]
    fn two_rows_one_side_swapped_are_a_conflict() {
        let base = "| 1 | same text |\n| 2 | same text |\n";
        let mine = "| 2 | same text. |\n| 1 | same text. |\n";
        let theirs = "| 1 | same text |\n| 2 | sameY text |\n";
        assert!(matches!(
            three_way(base, mine, theirs),
            MergeOutcome::Conflict
        ));
    }

    #[test]
    fn two_rows_one_side_dropped_at_the_top_and_added_at_the_bottom_are_a_conflict() {
        let base = "| 1 | aa | bb |\n| 4 | aa | bb |\n";
        let mine = "| 4 | aa | bbx |\n| 7 | aa | bb |\n";
        let theirs = "| 1 | aaY | bb |\n| 4 | aa | bb |\n";
        assert!(matches!(
            three_way(base, mine, theirs),
            MergeOutcome::Conflict
        ));
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
        assert!(matches!(
            three_way(base, mine, theirs),
            MergeOutcome::Conflict
        ));
    }

    #[test]
    fn a_row_replaced_by_a_new_one_against_an_edit_of_it_is_a_conflict() {
        // Mine changed cell 2, theirs changed cells 1 and 2: cell 2 changed
        // on both sides.
        let base = "| 1 | aa |\n| 2 | aa |\n";
        let mine = "| 1 | Qaa |\n| 2 | aa |\n";
        let theirs = "| 8 | ab |\n| 2 | aa |\n";
        assert!(matches!(
            three_way(base, mine, theirs),
            MergeOutcome::Conflict
        ));
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
                "intro\n| a | b |\n",
                "intro mine\n| aX | b |\n",
                "intro\n| a | bY |\n"
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
                "```\n| a | b |\n```\n",
                "```\n| aX | b |\n```\n",
                "```\n| a | bY |\n```\n"
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
        let MergeOutcome::Clean(merged) = three_way(base, mine, theirs) else {
            panic!("an escaped pipe is not a cell boundary");
        };
        assert_eq!(merged, "| a \\| bX | cY |\n");
        // Splitting a cell with a new pipe changes the cell count.
        assert!(matches!(
            three_way(base, "| a | b | c | d |\n", theirs),
            MergeOutcome::Conflict
        ));
    }

    #[test]
    fn the_same_change_to_one_cell_from_both_sides_is_kept_once() {
        let MergeOutcome::Clean(merged) = three_way("| a | b |\n", "| aX | bZ |\n", "| aX | b |\n")
        else {
            panic!("an identical cell change merges");
        };
        assert_eq!(merged, "| aX | bZ |\n");
    }

    #[test]
    fn two_rows_both_sides_changed_in_place_still_merge_per_cell() {
        let base = "| 1 | 2 |\n| 3 | 4 |\n";
        let mine = "| 1x | 2 |\n| 3x | 4 |\n";
        let theirs = "| 1 | 2y |\n| 3 | 4y |\n";
        let MergeOutcome::Clean(merged) = three_way(base, mine, theirs) else {
            panic!("rows changed in place pair line for line");
        };
        assert_eq!(merged, "| 1x | 2y |\n| 3x | 4y |\n");
    }

    #[test]
    fn a_date_changed_two_ways_in_the_frontmatter_is_a_conflict() {
        let base = "---\ntype: engram\ntitle: T\npermalink: t\nstatus: stable\ndate: 2026-11-02\n---\n\n# T\n\n| 1 | 2 |\n";
        let mine = base.replace("2026-11-02", "2026-11-05");
        let theirs = base.replace("2026-11-02", "2026-12-02");
        assert!(matches!(
            three_way(base, &mine, &theirs),
            MergeOutcome::Conflict
        ));
        // The body of the same page still merges per character.
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
        let MergeOutcome::Clean(merged) = three_way(&base, &mine, &theirs) else {
            panic!("a lone regional indicator beside the edits merges");
        };
        assert_eq!(merged, format!("| {d}ax | by |\n"));
    }

    /// One side of the oracle below: the rows it ends with and what it did.
    #[derive(Clone, Default)]
    struct Moved {
        rows: Vec<[String; 3]>,
        edited: Vec<(String, usize)>,
        deleted: Vec<String>,
        added: Vec<[String; 3]>,
        structural: bool,
    }

    fn render_rows(rows: &[[String; 3]]) -> String {
        rows.iter()
            .map(|[id, x, y]| format!("| {id} | {x} | {y} |\n"))
            .collect()
    }

    /// Every side the oracle tries against a base: nothing, one edit (a
    /// marker appended to cell 1 or 2 of a row), a deleted row, an added row,
    /// a row replaced by a new one, two rows swapped, and each of those
    /// moves with one edit on top.
    fn moves(base: &[[String; 3]], mark: char, new_id: &str) -> Vec<Moved> {
        let plain = Moved {
            rows: base.to_vec(),
            ..Moved::default()
        };
        let new_row = || {
            [
                new_id.to_string(),
                format!("n{new_id}"),
                format!("m{new_id}"),
            ]
        };
        let mut moved = vec![plain.clone()];
        for k in 0..base.len() {
            let mut d = plain.clone();
            let gone = d.rows.remove(k);
            d.deleted.push(gone[0].clone());
            d.structural = true;
            let mut r = d.clone();
            r.rows.insert(k, new_row());
            r.added.push(new_row());
            moved.push(d);
            moved.push(r);
        }
        for q in 0..=base.len() {
            let mut a = plain.clone();
            a.rows.insert(q, new_row());
            a.added.push(new_row());
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
                if side.added.iter().any(|row| row[0] == side.rows[k][0]) {
                    continue;
                }
                for cell in [1, 2] {
                    let mut e = side.clone();
                    e.rows[k][cell].push(mark);
                    e.edited.push((e.rows[k][0].clone(), cell));
                    all.push(e);
                }
            }
        }
        all
    }

    /// Why a Clean result is wrong, if it is.
    fn wrong_clean(base: &[[String; 3]], m: &Moved, t: &Moved, merged: &str) -> Option<String> {
        for (a, b) in [(m, t), (t, m)] {
            if a.deleted
                .iter()
                .any(|id| b.edited.iter().any(|(e, _)| e == id))
            {
                return Some("an edit of a deleted row".into());
            }
        }
        if m.edited.iter().any(|e| t.edited.contains(e)) {
            return Some("one cell edited on both sides".into());
        }
        let mut want: Vec<String> = Vec::new();
        for row in base {
            if m.deleted.contains(&row[0]) || t.deleted.contains(&row[0]) {
                continue;
            }
            let mut row = row.clone();
            for (side, mark) in [(m, 'Q'), (t, 'Y')] {
                for (id, cell) in &side.edited {
                    if *id == row[0] {
                        row[*cell].push(mark);
                    }
                }
            }
            want.push(render_rows(&[row]));
        }
        for row in m.added.iter().chain(&t.added) {
            want.push(render_rows(std::slice::from_ref(row)));
        }
        let mut got: Vec<String> = merged.split_inclusive('\n').map(str::to_string).collect();
        want.sort();
        got.sort();
        (want != got).then(|| format!("rows {got:?}, wanted {want:?}"))
    }

    #[test]
    fn the_cell_fallback_never_merges_a_move_wrong() {
        let mut cases = 0;
        for n in [2usize, 3] {
            let base: Vec<[String; 3]> = (1..=n)
                .map(|k| [k.to_string(), format!("x{k}"), format!("y{k}")])
                .collect();
            let text = render_rows(&base);
            let mines = moves(&base, 'Q', "7");
            let theirs = moves(&base, 'Y', "8");
            for m in &mines {
                for t in &theirs {
                    let (ms, ts) = (render_rows(&m.rows), render_rows(&t.rows));
                    if line_merge(&text, &ms, &ts).is_some() {
                        continue;
                    }
                    cases += 1;
                    let outcome = three_way(&text, &ms, &ts);
                    let simple = !m.structural && !t.structural;
                    let disjoint = !m.edited.iter().any(|e| t.edited.contains(e));
                    match outcome {
                        MergeOutcome::Clean(merged) => {
                            if let Some(why) = wrong_clean(&base, m, t, &merged) {
                                panic!("{why}\nbase {text:?}\nmine {ms:?}\ntheirs {ts:?}");
                            }
                        }
                        MergeOutcome::Conflict => assert!(
                            !(simple && disjoint),
                            "edits of different cells merge\nbase {text:?}\nmine {ms:?}\ntheirs {ts:?}"
                        ),
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
