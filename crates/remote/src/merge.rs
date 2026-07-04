//! The pure per-file three-way merge decision at the heart of pulling
//! upstream team knowledge into a local domain.
//!
//! [`merge_file`] takes the base, local and upstream content of a single
//! path and decides what should happen to the local working tree: apply
//! upstream content, delete the local file, do nothing because the two
//! sides already agree, or leave the local file untouched and record a
//! conflict. It has no side effects: it does not read or write files, spawn
//! no async work and knows nothing about paths, domains or GitHub. A later
//! task calls it once per upstream-changed path and acts on the result.
//!
//! Merge is plain-text three-way in v1: engram files (including their YAML
//! frontmatter) are merged line by line with [`diffy::merge`]. A
//! frontmatter-aware merge that reconciles YAML keys structurally rather
//! than by line is future work.
//!
//! The one rule this module exists to uphold: conflict markers must never
//! be produced as output. When a text merge cannot be reconciled cleanly,
//! diffy's conflict-marked text is discarded and the local file is left
//! untouched; the caller is only told which [`ConflictKind`] occurred.

/// How a conflict came about, recorded with the conflict for status
/// displays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictKind {
    /// Both sides edited the file, differently, since the common base, and
    /// the text merge could not reconcile the two versions cleanly.
    EditEdit,
    /// The file was edited locally and deleted upstream.
    EditDelete,
    /// The file was deleted locally and edited upstream.
    DeleteEdit,
    /// Both sides added the file (there is no common base version) with
    /// different content, and the text merge could not reconcile the two
    /// versions cleanly.
    AddAdd,
}

/// The outcome of merging one file across base, local and upstream
/// versions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileMerge {
    /// Write this content to the working tree: either a clean merge result
    /// or upstream content taken as-is because local did not change.
    Apply(Vec<u8>),
    /// Remove the local file.
    Delete,
    /// Local and upstream already agree; nothing to do.
    Converged,
    /// Keep the local file untouched and record a conflict. No conflict
    /// markers are ever produced; the caller decides how to surface this
    /// and how the user or agent resolves it later.
    Conflict(ConflictKind),
}

/// Decides how to merge one file given its content at the common base, in
/// the local working tree and upstream, each `None` when the file is
/// absent on that side.
///
/// The decision, in order (first match wins), with equality throughout
/// meaning byte equality:
///
/// 1. `local == upstream` -> [`FileMerge::Converged`]: covers both sides
///    absent, both added identically, both edited identically and both
///    deleted.
/// 2. `upstream == base` -> [`FileMerge::Converged`]: upstream did not
///    change this path, so there is nothing to integrate.
/// 3. `local == base` (local did not change) -> upstream wins:
///    [`FileMerge::Apply`] when upstream has content,
///    [`FileMerge::Delete`] when upstream removed the file.
/// 4. Base present, local absent, upstream present -> locally deleted
///    while upstream edited: [`ConflictKind::DeleteEdit`].
/// 5. Base present, local present, upstream absent -> locally edited while
///    upstream deleted: [`ConflictKind::EditDelete`].
/// 6. Base absent, local present, upstream present, and they differ -> both
///    sides added the file: attempt a text merge against an empty
///    ancestor; a clean result is applied, otherwise
///    [`ConflictKind::AddAdd`].
/// 7. Base present, local present, upstream present, and all three differ ->
///    attempt a text merge; a clean result is applied, otherwise
///    [`ConflictKind::EditEdit`].
///
/// A text merge needs every participating side to be valid UTF-8. If any
/// side that must participate is not, the merge is not attempted and the
/// case is treated as a conflict of the same kind that a non-mergeable
/// text merge would have produced. Case 3 lets non-UTF-8 content through
/// untouched (upstream is simply taken as bytes), which is the path
/// binary files travel as long as local did not change them.
pub fn merge_file(base: Option<&[u8]>, local: Option<&[u8]>, upstream: Option<&[u8]>) -> FileMerge {
    if local == upstream {
        return FileMerge::Converged;
    }
    if upstream == base {
        return FileMerge::Converged;
    }
    if local == base {
        return match upstream {
            Some(bytes) => FileMerge::Apply(bytes.to_vec()),
            None => FileMerge::Delete,
        };
    }

    match (base, local, upstream) {
        // Reachable only when local == base (both None), which case 3
        // above already returns from; likewise (None, Some, None) is
        // always resolved by case 2 (upstream == base) and (Some, None,
        // None) by case 1 (local == upstream). None of these three shapes
        // can still be here.
        (None, None, _) | (None, Some(_), None) | (Some(_), None, None) => {
            unreachable!("cases 1-3 already resolve every shape without three distinct sides")
        }
        (Some(_), None, Some(_)) => FileMerge::Conflict(ConflictKind::DeleteEdit),
        (Some(_), Some(_), None) => FileMerge::Conflict(ConflictKind::EditDelete),
        (None, Some(local_bytes), Some(upstream_bytes)) => {
            attempt_text_merge(&[], local_bytes, upstream_bytes, ConflictKind::AddAdd)
        }
        (Some(base_bytes), Some(local_bytes), Some(upstream_bytes)) => attempt_text_merge(
            base_bytes,
            local_bytes,
            upstream_bytes,
            ConflictKind::EditEdit,
        ),
    }
}

/// Attempts a text merge of `local` and `upstream` against `ancestor`,
/// falling back to `kind` as a conflict whenever any side is not valid
/// UTF-8 or `diffy::merge` cannot reconcile the two versions cleanly.
///
/// `diffy::merge` returns conflict-marked text in its `Err` case; that text
/// is deliberately discarded rather than surfaced, since conflict markers
/// must never reach an engram file.
fn attempt_text_merge(
    ancestor: &[u8],
    local: &[u8],
    upstream: &[u8],
    kind: ConflictKind,
) -> FileMerge {
    let ancestor = std::str::from_utf8(ancestor);
    let local = std::str::from_utf8(local);
    let upstream = std::str::from_utf8(upstream);
    match (ancestor, local, upstream) {
        (Ok(ancestor), Ok(local), Ok(upstream)) => match diffy::merge(ancestor, local, upstream) {
            Ok(merged) => FileMerge::Apply(merged.into_bytes()),
            Err(_conflict_marked_text) => FileMerge::Conflict(kind),
        },
        _ => FileMerge::Conflict(kind),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE_TWO_SECTIONS: &[u8] = b"# Title\n\nSection A: original\n\nSection B: original\n";
    const LOCAL_EDITS_A: &[u8] = b"# Title\n\nSection A: local edit\n\nSection B: original\n";
    const UPSTREAM_EDITS_B: &[u8] = b"# Title\n\nSection A: original\n\nSection B: upstream edit\n";
    const MERGED_BOTH_EDITS: &[u8] =
        b"# Title\n\nSection A: local edit\n\nSection B: upstream edit\n";

    #[test]
    fn clean_merge_applies_disjoint_edits_from_both_sides() {
        let result = merge_file(
            Some(BASE_TWO_SECTIONS),
            Some(LOCAL_EDITS_A),
            Some(UPSTREAM_EDITS_B),
        );
        assert_eq!(result, FileMerge::Apply(MERGED_BOTH_EDITS.to_vec()));
    }

    #[test]
    fn same_line_edited_differently_conflicts_edit_edit() {
        let base: &[u8] = b"# Title\n\nline one\n";
        let local: &[u8] = b"# Title\n\nline one LOCAL\n";
        let upstream: &[u8] = b"# Title\n\nline one UPSTREAM\n";
        let result = merge_file(Some(base), Some(local), Some(upstream));
        assert_eq!(result, FileMerge::Conflict(ConflictKind::EditEdit));
    }

    const FRONTMATTER_BASE: &[u8] =
        b"---\ntitle: Example\ntags: [a, b]\nstatus: active\n---\n\n# Body\n\nOriginal body line.\n";

    #[test]
    fn same_frontmatter_line_edited_differently_conflicts_edit_edit() {
        let local: &[u8] =
            b"---\ntitle: Example\ntags: [a, b, c]\nstatus: active\n---\n\n# Body\n\nOriginal body line.\n";
        let upstream: &[u8] =
            b"---\ntitle: Example\ntags: [a, b, d]\nstatus: active\n---\n\n# Body\n\nOriginal body line.\n";
        let result = merge_file(Some(FRONTMATTER_BASE), Some(local), Some(upstream));
        assert_eq!(result, FileMerge::Conflict(ConflictKind::EditEdit));
    }

    #[test]
    fn frontmatter_disjoint_from_body_edit_merges_cleanly() {
        let local_body_edit: &[u8] =
            b"---\ntitle: Example\ntags: [a, b]\nstatus: active\n---\n\n# Body\n\nOriginal body line edited locally.\n";
        let upstream_frontmatter_edit: &[u8] =
            b"---\ntitle: Example\ntags: [a, b]\nstatus: archived\n---\n\n# Body\n\nOriginal body line.\n";
        let expected: &[u8] =
            b"---\ntitle: Example\ntags: [a, b]\nstatus: archived\n---\n\n# Body\n\nOriginal body line edited locally.\n";
        let result = merge_file(
            Some(FRONTMATTER_BASE),
            Some(local_body_edit),
            Some(upstream_frontmatter_edit),
        );
        assert_eq!(result, FileMerge::Apply(expected.to_vec()));
    }

    #[test]
    fn add_add_identical_content_converges() {
        let content: &[u8] = b"same content on both sides\n";
        let result = merge_file(None, Some(content), Some(content));
        assert_eq!(result, FileMerge::Converged);
    }

    #[test]
    fn add_add_divergent_content_conflicts_add_add() {
        // diffy's three-way merge always conflicts when the ancestor is
        // empty and the two sides diverge at all, even for a strict
        // prefix extension of one side by the other (verified against
        // diffy 0.5.0 directly: prefix-extension, shared-prefix-diverge,
        // disjoint-single-line and prepend-vs-append all return `Err`).
        // There is no case in which an empty-ancestor divergence merges
        // cleanly, so add/add divergence always conflicts.
        let local: &[u8] = b"line1\n";
        let upstream: &[u8] = b"line1\nline2\n";
        let result = merge_file(None, Some(local), Some(upstream));
        assert_eq!(result, FileMerge::Conflict(ConflictKind::AddAdd));
    }

    #[test]
    fn add_add_disjoint_single_line_conflicts_add_add() {
        let local: &[u8] = b"aaa\n";
        let upstream: &[u8] = b"bbb\n";
        let result = merge_file(None, Some(local), Some(upstream));
        assert_eq!(result, FileMerge::Conflict(ConflictKind::AddAdd));
    }

    #[test]
    fn local_edit_upstream_delete_conflicts_edit_delete() {
        let base: &[u8] = b"original content\n";
        let local: &[u8] = b"locally edited content\n";
        let result = merge_file(Some(base), Some(local), None);
        assert_eq!(result, FileMerge::Conflict(ConflictKind::EditDelete));
    }

    #[test]
    fn local_delete_upstream_edit_conflicts_delete_edit() {
        let base: &[u8] = b"original content\n";
        let upstream: &[u8] = b"upstream edited content\n";
        let result = merge_file(Some(base), None, Some(upstream));
        assert_eq!(result, FileMerge::Conflict(ConflictKind::DeleteEdit));
    }

    #[test]
    fn delete_delete_converges() {
        let base: &[u8] = b"original content\n";
        let result = merge_file(Some(base), None, None);
        assert_eq!(result, FileMerge::Converged);
    }

    #[test]
    fn local_unchanged_upstream_edited_applies_upstream() {
        let base: &[u8] = b"original content\n";
        let upstream: &[u8] = b"upstream edited content\n";
        let result = merge_file(Some(base), Some(base), Some(upstream));
        assert_eq!(result, FileMerge::Apply(upstream.to_vec()));
    }

    #[test]
    fn local_unchanged_upstream_deleted_deletes() {
        let base: &[u8] = b"original content\n";
        let result = merge_file(Some(base), Some(base), None);
        assert_eq!(result, FileMerge::Delete);
    }

    #[test]
    fn local_edited_upstream_unchanged_converges() {
        let base: &[u8] = b"original content\n";
        let local: &[u8] = b"locally edited content\n";
        let result = merge_file(Some(base), Some(local), Some(base));
        assert_eq!(result, FileMerge::Converged);
    }

    #[test]
    fn non_utf8_local_unchanged_applies_upstream_binary_bytes() {
        let base: &[u8] = &[0xff, 0xfe, 0x00, 0x01];
        let upstream: &[u8] = &[0xff, 0xfe, 0x00, 0x02];
        let result = merge_file(Some(base), Some(base), Some(upstream));
        assert_eq!(result, FileMerge::Apply(upstream.to_vec()));
    }

    #[test]
    fn non_utf8_edited_on_both_sides_conflicts_edit_edit() {
        let base: &[u8] = &[0xff, 0xfe, 0x00, 0x01];
        let local: &[u8] = &[0xff, 0xfe, 0x00, 0x02];
        let upstream: &[u8] = &[0xff, 0xfe, 0x00, 0x03];
        let result = merge_file(Some(base), Some(local), Some(upstream));
        assert_eq!(result, FileMerge::Conflict(ConflictKind::EditEdit));
    }

    #[test]
    fn non_utf8_added_on_both_sides_conflicts_add_add() {
        let local: &[u8] = &[0xff, 0xfe, 0x00, 0x01];
        let upstream: &[u8] = &[0xff, 0xfe, 0x00, 0x02];
        let result = merge_file(None, Some(local), Some(upstream));
        assert_eq!(result, FileMerge::Conflict(ConflictKind::AddAdd));
    }

    #[test]
    fn empty_base_both_sides_add_different_content_conflicts() {
        let local: &[u8] = b"local content\n";
        let upstream: &[u8] = b"upstream content\n";
        let result = merge_file(None, Some(local), Some(upstream));
        assert_eq!(result, FileMerge::Conflict(ConflictKind::AddAdd));
    }

    #[test]
    fn existing_empty_file_deleted_locally_while_upstream_edits_conflicts_delete_edit() {
        // base is an existing empty file (Some(b"")), not an absent file
        // (None): deleting it locally while upstream edits it is a
        // delete/edit conflict just like a non-empty base would be.
        let base: &[u8] = b"";
        let upstream: &[u8] = b"upstream added content\n";
        let result = merge_file(Some(base), None, Some(upstream));
        assert_eq!(result, FileMerge::Conflict(ConflictKind::DeleteEdit));
    }

    #[test]
    fn existing_empty_file_unchanged_locally_while_upstream_deletes_it_deletes() {
        let base: &[u8] = b"";
        let result = merge_file(Some(base), Some(base), None);
        assert_eq!(result, FileMerge::Delete);
    }

    #[test]
    fn add_add_both_sides_create_the_same_empty_file_converges() {
        // Some(b"") on both sides, not None: both sides created the same
        // (empty) file, which converges just like identical non-empty
        // content would.
        let empty: &[u8] = b"";
        let result = merge_file(None, Some(empty), Some(empty));
        assert_eq!(result, FileMerge::Converged);
    }

    #[test]
    fn all_three_absent_converges() {
        let result = merge_file(None, None, None);
        assert_eq!(result, FileMerge::Converged);
    }

    #[test]
    fn upstream_only_addition_not_present_in_base_or_local_applies_upstream() {
        // base and local both absent, upstream added the file: local is
        // "unchanged" (still absent, same as base) so upstream wins.
        let upstream: &[u8] = b"new file from upstream\n";
        let result = merge_file(None, None, Some(upstream));
        assert_eq!(result, FileMerge::Apply(upstream.to_vec()));
    }

    #[test]
    fn local_only_addition_with_no_base_and_no_upstream_converges() {
        // base and upstream both absent: upstream did not touch this path
        // at all, so whatever local did (adding a brand-new file) stands
        // untouched.
        let local: &[u8] = b"new local-only file\n";
        let result = merge_file(None, Some(local), None);
        assert_eq!(result, FileMerge::Converged);
    }
}
