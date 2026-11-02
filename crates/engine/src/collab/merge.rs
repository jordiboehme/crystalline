//! The external-change merge: three-way over LF text (frontmatter key by key,
//! body line by line, see `crystalline_core::frontmatter`, and a line both
//! sides changed once more character by character), applied into the live
//! document as a minimal edit script. Conflict markers are never output -
//! diffy's marked text is discarded and the room resolves instead.
//!
//! "Character" means an extended grapheme cluster throughout: a merge or an
//! edit script never splits a surrogate pair, a flag or a compound emoji.

use std::collections::HashMap;

use crystalline_core::frontmatter::{MergedText, merge_text};
use diffy::{ConflictStyle, MergeOptions};
use similar::{Algorithm, DiffOp, capture_diff_slices};
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
/// A line both sides changed (a person typing in one cell of a table row, an
/// agent changing another cell of the same row) is merged once more per
/// character (see `line_merge`); the merge is clean only when that is.
pub fn three_way(base_file: &str, mine_session: &str, theirs_file: &str) -> MergeOutcome {
    let base = to_lf(base_file);
    let theirs = to_lf(theirs_file);
    let mine_session = &*to_lf(mine_session);
    // Frontmatter by key, body by line, and never a result that stops
    // parsing where both sides parsed. diffy's marked text is discarded
    // inside `line_merge`: conflict markers must never reach an engram file
    // or the live document.
    match merge_text(Some(&base), mine_session, &theirs, &line_merge) {
        MergedText::Clean(merged) => MergeOutcome::Clean(merged),
        MergedText::Conflict => MergeOutcome::Conflict,
    }
}

/// The length of the markers the character fallback asks diffy for. Long
/// enough that no engram line is one by accident, and checked anyway: an
/// input with a line that starts like a marker keeps today's conflict.
const FALLBACK_MARKER_LEN: usize = 64;

/// diffy's three-way line merge, its conflict-marked `Err` discarded.
///
/// When the line merge conflicts, each conflicting hunk whose three sides
/// have the same number of lines is merged again line against line, per
/// grapheme cluster ([`grapheme_merge`]). The result is clean only when every
/// such line merges clean; any other hunk shape (lines added or removed on
/// one side, or a cluster both sides changed) stays a conflict, as before.
fn line_merge(base: &str, mine: &str, theirs: &str) -> Option<String> {
    match diffy::merge(base, mine, theirs) {
        Ok(merged) => Some(merged),
        Err(_) => hunk_merge(base, mine, theirs),
    }
}

/// The character fallback of [`line_merge`]: diffy's conflicting hunks, read
/// from its diff3-style output with long markers, each merged per character.
fn hunk_merge(base: &str, mine: &str, theirs: &str) -> Option<String> {
    let texts = [base, mine, theirs];
    // diffy closes an incomplete last line with its marker unless asked for
    // git's style, which adds a line ending the parse cannot tell from the
    // text's own. So the sides all end in a line ending, or all get one that
    // the result gives back; a line ending only some sides have is a line
    // change, and the line merge's verdict on it stands.
    let complete = |t: &str| t.is_empty() || t.ends_with('\n');
    let added = if texts.iter().all(|t| complete(t)) {
        false
    } else if texts.iter().all(|t| !complete(t)) {
        true
    } else {
        return None;
    };
    let [base, mine, theirs] = texts.map(|t| {
        if added {
            format!("{t}\n")
        } else {
            t.to_string()
        }
    });
    let open = "<".repeat(FALLBACK_MARKER_LEN);
    let original = "|".repeat(FALLBACK_MARKER_LEN);
    let middle = "=".repeat(FALLBACK_MARKER_LEN);
    let close = ">".repeat(FALLBACK_MARKER_LEN);
    let markers = [&open, &original, &middle, &close];
    if [&base, &mine, &theirs]
        .iter()
        .flat_map(|t| t.lines())
        .any(|line| markers.iter().any(|m| line.starts_with(m.as_str())))
    {
        return None;
    }
    let marked = MergeOptions::new()
        .set_conflict_style(ConflictStyle::Diff3)
        .set_conflict_marker_length(FALLBACK_MARKER_LEN)
        .merge(&base, &mine, &theirs)
        .err()?;

    let (open, original, middle, close) = (
        format!("{open} ours\n"),
        format!("{original} original\n"),
        format!("{middle}\n"),
        format!("{close} theirs\n"),
    );
    enum Part {
        Outside,
        Mine,
        Base,
        Theirs,
    }
    let mut part = Part::Outside;
    let mut hunk: [Vec<&str>; 3] = Default::default();
    let mut merged = String::with_capacity(marked.len());
    for line in marked.split_inclusive('\n') {
        match part {
            Part::Outside if line == open => part = Part::Mine,
            Part::Outside => merged.push_str(line),
            Part::Mine if line == original => part = Part::Base,
            Part::Mine => hunk[1].push(line),
            Part::Base if line == middle => part = Part::Theirs,
            Part::Base => hunk[0].push(line),
            Part::Theirs if line == close => {
                let [b, m, t] = std::mem::take(&mut hunk);
                if b.len() != m.len() || b.len() != t.len() {
                    return None;
                }
                for ((b, m), t) in b.iter().zip(&m).zip(&t) {
                    merged.push_str(&grapheme_merge(b, m, t)?);
                }
                part = Part::Outside;
            }
            Part::Theirs => hunk[2].push(line),
        }
    }
    if !matches!(part, Part::Outside) {
        return None;
    }
    if added {
        merged.pop();
    }
    Some(merged)
}

/// A three-way merge of one line per extended grapheme cluster: each cluster
/// is interned to an id, the ids are merged by diffy one per line, and a
/// clean result is read back. Clusters are whole tokens, so a merged line is
/// always made of clusters one of the sides holds.
fn grapheme_merge(base: &str, mine: &str, theirs: &str) -> Option<String> {
    let mut clusters: Vec<&str> = Vec::new();
    let mut ids: HashMap<&str, usize> = HashMap::new();
    let [base, mine, theirs] = [base, mine, theirs].map(|text| {
        let mut out = String::new();
        for cluster in text.graphemes(true) {
            let id = *ids.entry(cluster).or_insert_with(|| {
                clusters.push(cluster);
                clusters.len() - 1
            });
            out.push_str(&id.to_string());
            out.push('\n');
        }
        out
    });
    let merged = diffy::merge(&base, &mine, &theirs).ok()?;
    merged
        .lines()
        .map(|id| {
            id.parse::<usize>()
                .ok()
                .and_then(|i| clusters.get(i).copied())
        })
        .collect()
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
    fn two_cells_of_one_row_changed_on_each_side_merge_per_character() {
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
    fn a_character_merge_needs_every_conflicting_line_to_merge() {
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
        // Two flags side by side are two clusters, each its own token.
        let base = format!("| {FLAG_DE} x {FLAG_DE} |\n");
        let mine = format!("| {FLAG_DK} x {FLAG_DE} |\n");
        let theirs = format!("| {FLAG_DE} x {FLAG_DK} |\n");
        let MergeOutcome::Clean(merged) = three_way(&base, &mine, &theirs) else {
            panic!("separate flags merge");
        };
        assert_eq!(merged, format!("| {FLAG_DK} x {FLAG_DK} |\n"));
    }

    #[test]
    fn a_row_without_a_final_line_ending_merges_per_character() {
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
