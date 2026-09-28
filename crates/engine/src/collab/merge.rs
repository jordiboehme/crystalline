//! The external-change merge: three-way in LF space (frontmatter key by key,
//! body line by line, see `crystalline_core::frontmatter`), applied into
//! the live document as a minimal edit script. Conflict markers are never
//! output - diffy's marked text is discarded and the room resolves instead.

use crystalline_core::frontmatter::{MergedText, merge_text};
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
/// merged in LF space (frontmatter key by key, body line by line, see
/// `crystalline_core::frontmatter`). diffy's Err carries conflict-marked
/// text - discarded.
pub fn three_way(base_file: &str, mine_session: &str, theirs_file: &str) -> MergeOutcome {
    if !collab_eligible(theirs_file) {
        // Merging would force a silent line-ending rewrite on save; the room
        // gets the conflict view instead.
        return MergeOutcome::Conflict;
    }
    let base = session_text(base_file);
    let theirs = session_text(theirs_file);
    // Frontmatter by key, body by line, and never a result that stops
    // parsing where both sides parsed. diffy's marked text is discarded
    // inside `line_merge`: conflict markers must never reach an engram file
    // or the live document.
    match merge_text(Some(&base), mine_session, &theirs, &line_merge) {
        MergedText::Clean(merged) => MergeOutcome::Clean(merged),
        MergedText::Conflict => MergeOutcome::Conflict,
    }
}

/// diffy's three-way line merge, its conflict-marked `Err` discarded.
fn line_merge(base: &str, mine: &str, theirs: &str) -> Option<String> {
    diffy::merge(base, mine, theirs).ok()
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

    const SCOTTY: &str = "---\ntype: manifest\ntitle: Scotty\npermalink: manifest\ntags:\n  - manifest\nstatus: stable\n---\n\n# Scotty\n\n## Scope\n\n- Engineering\n";

    fn with(text: &str, after: &str, line: &str) -> String {
        text.replacen(after, &format!("{after}{line}"), 1)
    }

    #[test]
    fn a_key_both_sides_added_with_one_value_lands_once() {
        let mine = with(SCOTTY, "status: stable\n", "domain_name: scotty\n");
        let theirs = with(SCOTTY, "title: Scotty\n", "domain_name: scotty\n");
        let MergeOutcome::Clean(merged) = three_way(SCOTTY, &mine, &theirs) else {
            panic!("one value lands once");
        };
        assert_eq!(merged, theirs);
    }

    #[test]
    fn a_crlf_file_merges_by_key_in_session_space() {
        let base = SCOTTY.replace('\n', "\r\n");
        let theirs = with(SCOTTY, "title: Scotty\n", "domain_name: scotty\n").replace('\n', "\r\n");
        let mine = with(SCOTTY, "status: stable\n", "domain_name: scotty\n");
        let MergeOutcome::Clean(merged) = three_way(&base, &mine, &theirs) else {
            panic!("session space is LF on every side");
        };
        assert_eq!(merged, session_text(&theirs));
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
    fn a_half_typed_frontmatter_keeps_todays_merge() {
        let mine = SCOTTY.replace("title: Scotty", "title: \"Scotty");
        let theirs = with(SCOTTY, "- Engineering\n", "- Warp\n");
        let MergeOutcome::Clean(merged) = three_way(SCOTTY, &mine, &theirs) else {
            panic!("the person's unfinished quote is not the merge's business");
        };
        assert_eq!(merged, with(&mine, "- Engineering\n", "- Warp\n"));
    }
}
