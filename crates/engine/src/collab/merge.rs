//! The external-change merge: line-based three-way in LF space, applied into
//! the live document as a minimal edit script. Conflict markers are never
//! output - diffy's marked text is discarded and the room resolves instead.

use similar::{ChangeTag, TextDiff};
use yrs::{Text, TextRef, TransactionMut};

use super::text::{collab_eligible, session_text};

/// What a three-way merge of an external change against the live session says.
pub enum MergeOutcome {
    /// The merged text, in SESSION (LF) space, ready to apply into the doc.
    Clean(String),
    /// Concurrent edits collide (or theirs is mixed-endings): the room decides.
    Conflict,
}

/// base and theirs arrive in FILE space, mine in session space; everything is
/// merged in LF space. diffy's Err carries conflict-marked text - discarded.
pub fn three_way(base_file: &str, mine_session: &str, theirs_file: &str) -> MergeOutcome {
    if !collab_eligible(theirs_file) {
        // Merging would force a silent line-ending rewrite on save; the room
        // gets the conflict view instead.
        return MergeOutcome::Conflict;
    }
    let base = session_text(base_file);
    let theirs = session_text(theirs_file);
    match diffy::merge(&base, mine_session, &theirs) {
        Ok(merged) => MergeOutcome::Clean(merged),
        // The marked text is discarded, never surfaced: conflict markers must
        // never reach an engram file or the live document.
        Err(_marked) => MergeOutcome::Conflict,
    }
}

/// Morph the live Y.Text into `target` with a minimal line-based edit script,
/// positions in UTF-16 code units (the doc is `OffsetKind::Utf16`).
///
/// Line granularity is deliberate: whole-line removals keep the edits clear of
/// the compound-emoji deletion shapes yrs can panic on (y-crdt/y-crdt#386).
pub fn apply_target(text: &TextRef, txn: &mut TransactionMut, current: &str, target: &str) {
    let diff = TextDiff::from_lines(current, target);
    let mut pos: u32 = 0;
    for change in diff.iter_all_changes() {
        // UTF-16 units, never bytes: the doc is OffsetKind::Utf16 so its
        // indexes are the same ones a JS client counts.
        let units = change.value().encode_utf16().count() as u32;
        match change.tag() {
            ChangeTag::Equal => pos += units,
            ChangeTag::Delete => text.remove_range(txn, pos, units),
            ChangeTag::Insert => {
                text.insert(txn, pos, change.value());
                pos += units;
            }
        }
    }
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
        let mine = "# T\n\nSection A: mine\n\nSection B: original\n"; // session space
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
    fn a_mixed_endings_theirs_is_a_conflict_not_a_rewrite() {
        let outcome = three_way("a\r\n", "a\n", "a\r\nb\nmixed\r\n");
        assert!(matches!(outcome, MergeOutcome::Conflict));
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
}
