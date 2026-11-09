//! The one decision for a draft on a stale base: given the draft, where it
//! started and the team's text, what should happen. Pure, with no I/O, so every
//! gate and the convergence pass read one answer.
//!
//! The merge is the non-review merge (`merge_with_manifest_rule`), run in the
//! draft's coordinates: a moved draft's links were rebased when it moved, so the
//! start and the team's text are rebased the same way first. A conflict is never
//! stored and nothing here writes, so a stale row stays refused until something
//! has actually merged it.
//!
//! What the caller owes, because this function cannot see it:
//!
//! - **`team_at_path` and `team_at_from` come from real keys of the base
//!   snapshot.** On a disk that folds case a read of `Plan.md` also returns
//!   `plan.md`'s bytes. A key with no readable copy is [`TeamText::Unreadable`],
//!   never [`TeamText::Absent`], and so is a snapshot that holds two spellings
//!   of one path on a folding disk, where it cannot hold two copies.
//! - **A broken record is an error for the caller to refuse on.** It never
//!   reaches this module as a record of `None`s.
//! - **A row with no record gets [`has_landed`] only.** [`next_step`] assumes a
//!   record: `from` and `start` both `None` means her own new file, and that is in
//!   step over nothing. An old draft is decided elsewhere.
//! - **A tombstone that is half of a move is not asked on its own.** The draft
//!   stands for both. Alone, the tombstone of her edited case move that landed at
//!   the team's spelling reads [`DraftConflictKind::DeleteEdit`], as it must when
//!   no draft stands beside it.
//! - **[`DraftConflictKind::AddressTaken`] is never answered here.** It takes the
//!   domain's address map, and the caller asks that first.

use std::borrow::Cow;

use crystalline_core::frontmatter::same_but_generated;
use crystalline_core::relink::{rebase_relative_links, respell_path_wikilinks};
use crystalline_core::to_lf;
use crystalline_remote::merge::{ConflictKind, FileMerge};
use crystalline_remote::ops::merge_with_manifest_rule;
use serde::Serialize;

/// Why a draft does not fit the team's text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum DraftConflictKind {
    /// Both changed the same lines.
    EditEdit,
    /// She holds a draft of a file the team deleted.
    EditDelete,
    /// She deleted a file the team changed.
    DeleteEdit,
    /// The team holds a file at a path she did not start from.
    AddAdd,
    /// We cannot tell what the team changed since.
    UnknownStart,
    /// Another team file holds the draft's address.
    AddressTaken,
}

/// What the team holds at one path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TeamText<'a> {
    /// The path is not a key of the base snapshot.
    Absent,
    /// The key's bytes, in the line endings the team has them.
    Held(&'a [u8]),
    /// The path is a key of the base snapshot with no readable copy. The team has
    /// a file there and we cannot say what it holds, which is not absent.
    Unreadable,
}

/// One row, its record and the team's text.
#[derive(Clone, Copy, Debug)]
pub struct DraftSides<'a> {
    /// The row's exact path.
    pub path: &'a str,
    /// The team path it started from. `None` is a new file.
    pub from: Option<&'a str>,
    /// The text at `from` when last in step. `None` is absent there.
    pub start: Option<&'a str>,
    /// The draft's text. `None` is a tombstone.
    pub local: Option<&'a str>,
    /// What the team holds at `path`.
    pub team_at_path: TeamText<'a>,
    /// What the team holds at `from`.
    pub team_at_from: TeamText<'a>,
}

/// What should happen to a row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DraftStep {
    /// The team holds it. End the move, both halves.
    Landed,
    /// Nothing to do.
    InStep,
    /// Write this, and only then move the start to the team's text. `edited` says
    /// a merge changed her words, which is what she is told about.
    Take { text: String, edited: bool },
    /// Leave the row. It stands as this conflict.
    Conflict(DraftConflictKind),
}

/// A team text, read. Bytes that are not UTF-8 stay that: a lossy decode would
/// write a replacement character into a draft, and absent would read as gone.
enum Team<'a> {
    Absent,
    Text(Cow<'a, str>),
    NotText,
}

impl<'a> Team<'a> {
    fn read(team: TeamText<'a>) -> Self {
        match team {
            TeamText::Held(bytes) => match std::str::from_utf8(bytes) {
                Ok(text) => Team::Text(to_lf(text)),
                Err(_) => Team::NotText,
            },
            TeamText::Absent | TeamText::Unreadable => Team::Absent,
        }
    }
}

/// Whether these sides can be decided at all. Unreadable text cannot, and neither
/// can a shape no writer leaves: it is answered as "we cannot tell" rather than
/// guessed at, since a guess here can be an overwrite.
fn can_tell(sides: &DraftSides<'_>) -> bool {
    let unreadable =
        sides.team_at_path == TeamText::Unreadable || sides.team_at_from == TeamText::Unreadable;
    let new_file_with_a_start =
        sides.from.is_none() && (sides.start.is_some() || sides.team_at_from != TeamText::Absent);
    let one_path_two_texts =
        sides.from == Some(sides.path) && sides.team_at_from != sides.team_at_path;
    !(unreadable || new_file_with_a_start || one_path_two_texts)
}

/// `text`, written at the team path `from`, as it reads at `path`: the two steps
/// `DomainView::move_within` runs on a draft, in its order. A case move is a move
/// here too, as it is there; only the very same path skips it.
fn in_draft_coordinates<'t>(text: &'t str, from: Option<&str>, path: &str) -> Cow<'t, str> {
    match from {
        Some(from) if from != path => {
            let rebased = rebase_relative_links(text, from, path).0;
            Cow::Owned(respell_path_wikilinks(&rebased, from, path).0)
        }
        _ => Cow::Borrowed(text),
    }
}

/// Whether the team holds what this row says, so it is finished. Asked by the
/// pass and by the conflict table alike, so that her own landed share never reads
/// as a conflict, whether or not a pass got to it.
///
/// - A tombstone has landed when the team has no file at its path.
/// - A draft has landed when the team's text at its path is the draft, but for
///   `generated`.
/// - A case move can also land at the team's spelling (see below).
pub fn has_landed(sides: &DraftSides<'_>) -> bool {
    if !can_tell(sides) {
        return false;
    }
    let at_path = Team::read(sides.team_at_path);
    let Some(local) = sides.local else {
        return matches!(at_path, Team::Absent);
    };
    match at_path {
        Team::Text(team) => same_but_generated(&team, local),
        Team::NotText => false,
        Team::Absent => case_move_landed_at_the_old_spelling(sides, local),
    }
}

/// On a disk that folds case, a share of `plan.md` moved to `Plan.md` lands at the
/// spelling the team already has: `Plan.md` is no key, and her text stands at
/// `plan.md`. An unedited case move looks the same until something tells them
/// apart, and it must not end there, on either disk: the team's text has to differ
/// from the start.
///
/// With only `generated` between the team's text and an unedited draft, the stamp
/// is what tells. Her own landing carries her stamp, so the two are equal to the
/// letter. Another stamp is a colleague's touch of a line that is no one's words,
/// which must not end her move.
fn case_move_landed_at_the_old_spelling(sides: &DraftSides<'_>, local: &str) -> bool {
    let Some(from) = sides.from else {
        return false;
    };
    if from.to_lowercase() != sides.path.to_lowercase() {
        return false;
    }
    // A row at its own path never gets here with a text: `can_tell` holds its two
    // team texts equal, and the one at the path is absent.
    let Team::Text(team) = Team::read(sides.team_at_from) else {
        return false;
    };
    let start = sides.start.map(to_lf);
    if start.as_deref() == Some(&*team) || !same_but_generated(&team, local) {
        return false;
    }
    let unedited = start.is_some_and(|start| {
        same_but_generated(local, &in_draft_coordinates(&start, Some(from), sides.path))
    });
    !unedited || *team == *to_lf(local)
}

/// What should happen to this row. First match wins:
///
/// 1. What cannot be told: [`DraftConflictKind::UnknownStart`].
/// 2. It has landed ([`has_landed`]): [`DraftStep::Landed`].
/// 3. The team holds the path and she did not start from it, compared to the
///    letter: `AddAdd`, or `DeleteEdit` for a tombstone. On a disk that keeps
///    `Note.md` and `note.md` apart those are two files, and in step at one says
///    nothing about the other. Sides read from real keys never show a case move's
///    own copy here, so folding the names would protect nothing and lose his file.
/// 4. The team's text at `from` equals the start, or both are absent:
///    [`DraftStep::InStep`].
/// 5. A tombstone: `DeleteEdit`.
/// 6. The team's text is gone: `EditDelete`, edit or not. A moved file going away
///    is told.
/// 7. The team's text is not text: a conflict, as `merge_file` would answer one.
/// 8. No edit of hers: the team's text, with no merge.
/// 9. The team did nothing in the draft's coordinates: her text, and the start
///    moves on. `merge_file` says `Converged` here too, and that one must not take
///    the team's text, which would drop her edit.
/// 10. The merge.
pub fn next_step(sides: &DraftSides<'_>) -> DraftStep {
    use DraftConflictKind::{AddAdd, DeleteEdit, EditDelete, EditEdit, UnknownStart};

    if !can_tell(sides) {
        return DraftStep::Conflict(UnknownStart);
    }
    if has_landed(sides) {
        return DraftStep::Landed;
    }
    if sides.team_at_path != TeamText::Absent && sides.from != Some(sides.path) {
        return DraftStep::Conflict(if sides.local.is_some() {
            AddAdd
        } else {
            DeleteEdit
        });
    }
    let start = sides.start.map(to_lf);
    let team = Team::read(sides.team_at_from);
    let in_step = match (&team, &start) {
        (Team::Absent, None) => true,
        (Team::Text(team), Some(start)) => team == start,
        _ => false,
    };
    if in_step {
        return DraftStep::InStep;
    }
    let Some(local) = sides.local.map(to_lf) else {
        return DraftStep::Conflict(DeleteEdit);
    };
    let team = match &team {
        Team::Absent => return DraftStep::Conflict(EditDelete),
        Team::NotText => {
            return DraftStep::Conflict(if start.is_some() { EditEdit } else { AddAdd });
        }
        Team::Text(team) => in_draft_coordinates(team, sides.from, sides.path),
    };
    let ancestor = start
        .as_deref()
        .map(|start| in_draft_coordinates(start, sides.from, sides.path));
    if ancestor
        .as_deref()
        .is_some_and(|ancestor| same_but_generated(&local, ancestor))
    {
        return DraftStep::Take {
            text: team.into_owned(),
            edited: false,
        };
    }
    if ancestor.as_deref() == Some(&*team) {
        return DraftStep::Take {
            text: local.into_owned(),
            edited: false,
        };
    }
    let merged = merge_with_manifest_rule(
        sides.path,
        ancestor.as_deref().map(str::as_bytes),
        Some(local.as_bytes()),
        Some(team.as_bytes()),
    );
    match merged {
        FileMerge::Apply(bytes) => match String::from_utf8(bytes) {
            Ok(text) => DraftStep::Take { text, edited: true },
            Err(_) => DraftStep::Conflict(EditEdit),
        },
        FileMerge::Converged => DraftStep::Take {
            text: team.into_owned(),
            edited: true,
        },
        // Her draft and the team's text are both here, so nothing says delete.
        FileMerge::Delete => DraftStep::Conflict(EditDelete),
        FileMerge::Conflict(kind) => DraftStep::Conflict(kind.into()),
    }
}

impl From<ConflictKind> for DraftConflictKind {
    fn from(kind: ConflictKind) -> Self {
        match kind {
            ConflictKind::EditEdit => Self::EditEdit,
            ConflictKind::EditDelete => Self::EditDelete,
            ConflictKind::DeleteEdit => Self::DeleteEdit,
            ConflictKind::AddAdd => Self::AddAdd,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::DraftConflictKind::{AddAdd, DeleteEdit, EditDelete, EditEdit, UnknownStart};
    use super::TeamText::{Absent, Held, Unreadable};
    use super::*;

    // One text at every stage, in LF. Her line is at the top and his at the
    // bottom, so the two merge. SAME_LINE is his, on her line.
    const START: &str =
        "---\ntitle: Plan\nstatus: draft\n---\nalpha\nbeta\ngamma\ndelta\nepsilon\nzeta\neta\n";
    const HERS: &str = "---\ntitle: Plan\nstatus: draft\n---\nalpha hers\nbeta\ngamma\ndelta\nepsilon\nzeta\neta\n";
    const HIS: &str =
        "---\ntitle: Plan\nstatus: draft\n---\nalpha\nbeta\ngamma\ndelta\nepsilon\nzeta\neta his\n";
    const BOTH: &str = "---\ntitle: Plan\nstatus: draft\n---\nalpha hers\nbeta\ngamma\ndelta\nepsilon\nzeta\neta his\n";
    const SAME_LINE: &str =
        "---\ntitle: Plan\nstatus: draft\n---\nalpha his\nbeta\ngamma\ndelta\nepsilon\nzeta\neta\n";
    const NEW_FILE: &str = "---\ntitle: Other\n---\nhis own file, nothing like hers\n";
    const NOT_TEXT: &[u8] = &[0xff, 0xfe, 0x00, 0x41];

    fn held(text: &'static str) -> TeamText<'static> {
        Held(text.as_bytes())
    }

    fn crlf(text: &str) -> Vec<u8> {
        text.replace('\n', "\r\n").into_bytes()
    }

    fn stamped(text: &str, at: &str) -> String {
        text.replacen(
            "status: draft\n",
            &format!("status: draft\ngenerated:\n  at: {at}\n  by: human:someone\n"),
            1,
        )
    }

    fn take(text: &str, edited: bool) -> DraftStep {
        DraftStep::Take {
            text: text.to_string(),
            edited,
        }
    }

    fn conflict(kind: DraftConflictKind) -> DraftStep {
        DraftStep::Conflict(kind)
    }

    /// What `move_within` does to a text, so a moved draft in a test holds what
    /// a real one holds. The expected texts are written out by hand.
    fn as_moved(text: &str, from: &str, to: &str) -> String {
        let rebased = crystalline_core::relink::rebase_relative_links(text, from, to).0;
        crystalline_core::relink::respell_path_wikilinks(&rebased, from, to).0
    }

    struct Row {
        name: &'static str,
        path: &'static str,
        from: Option<&'static str>,
        start: Option<&'static str>,
        local: Option<&'static str>,
        at_path: TeamText<'static>,
        at_from: TeamText<'static>,
        step: DraftStep,
    }

    impl Row {
        fn sides(&self) -> DraftSides<'_> {
            DraftSides {
                path: self.path,
                from: self.from,
                start: self.start,
                local: self.local,
                team_at_path: self.at_path,
                team_at_from: self.at_from,
            }
        }
    }

    fn check(rows: Vec<Row>) {
        let mut wrong = Vec::new();
        for row in &rows {
            let step = next_step(&row.sides());
            let landed = has_landed(&row.sides());
            if step != row.step || landed != (row.step == DraftStep::Landed) {
                wrong.push(format!(
                    "{}: wanted {:?}, got {:?} (has_landed {landed})",
                    row.name, row.step, step
                ));
            }
        }
        assert!(wrong.is_empty(), "\n{}", wrong.join("\n"));
    }

    fn sides<'a>(
        path: &'a str,
        from: Option<&'a str>,
        start: Option<&'a str>,
        local: Option<&'a str>,
        team_at_path: TeamText<'a>,
        team_at_from: TeamText<'a>,
    ) -> DraftSides<'a> {
        DraftSides {
            path,
            from,
            start,
            local,
            team_at_path,
            team_at_from,
        }
    }

    /// A row at its own path: `from` is the path and one team text answers both.
    fn own(
        name: &'static str,
        start: Option<&'static str>,
        local: Option<&'static str>,
        team: TeamText<'static>,
        step: DraftStep,
    ) -> Row {
        Row {
            name,
            path: "plan.md",
            from: Some("plan.md"),
            start,
            local,
            at_path: team,
            at_from: team,
            step,
        }
    }

    /// A draft moved from `plan.md` to `renamed.md`.
    fn renamed(
        name: &'static str,
        local: &'static str,
        at_path: TeamText<'static>,
        at_from: TeamText<'static>,
        step: DraftStep,
    ) -> Row {
        Row {
            name,
            path: "renamed.md",
            from: Some("plan.md"),
            start: Some(START),
            local: Some(local),
            at_path,
            at_from,
            step,
        }
    }

    /// A draft case-moved from `plan.md` to `Plan.md`. The two disk kinds differ
    /// only in which keys the snapshot holds after a share, so a row is labelled
    /// with the disk whose keys it shows.
    fn case_moved(
        name: &'static str,
        local: &'static str,
        at_path: TeamText<'static>,
        at_from: TeamText<'static>,
        step: DraftStep,
    ) -> Row {
        Row {
            name,
            path: "Plan.md",
            from: Some("plan.md"),
            start: Some(START),
            local: Some(local),
            at_path,
            at_from,
            step,
        }
    }

    /// Her own new file at `plan.md`.
    fn new_file(
        name: &'static str,
        local: &'static str,
        at_path: TeamText<'static>,
        step: DraftStep,
    ) -> Row {
        Row {
            name,
            path: "plan.md",
            from: None,
            start: None,
            local: Some(local),
            at_path,
            at_from: Absent,
            step,
        }
    }

    // ---- The table, by the rule that answers. ----

    #[test]
    fn rule_1_what_the_team_holds_has_landed() {
        check(vec![
            own(
                "tombstone, file gone",
                Some(START),
                None,
                Absent,
                DraftStep::Landed,
            ),
            own(
                "tombstone, she never saw it, gone again",
                None,
                None,
                Absent,
                DraftStep::Landed,
            ),
            own(
                "draft equal at its own path",
                Some(START),
                Some(HERS),
                held(HERS),
                DraftStep::Landed,
            ),
            own(
                "draft equal to the start and the team",
                Some(START),
                Some(START),
                held(START),
                DraftStep::Landed,
            ),
            // Shadows rule 2: a file at her new path that is hers is no AddAdd.
            renamed(
                "edited rename landed, pass not run",
                HERS,
                held(HERS),
                Absent,
                DraftStep::Landed,
            ),
            renamed(
                "unedited rename landed",
                START,
                held(START),
                Absent,
                DraftStep::Landed,
            ),
            renamed(
                "a peer put the same text at her path, from in step",
                HERS,
                held(HERS),
                held(START),
                DraftStep::Landed,
            ),
            new_file("her new file landed", HERS, held(HERS), DraftStep::Landed),
            // Case move, edited, shared.
            case_moved(
                "[case-sensitive] edited case move landed at her spelling",
                HERS,
                held(HERS),
                Absent,
                DraftStep::Landed,
            ),
            case_moved(
                "[folding] edited case move landed at the team's spelling",
                HERS,
                Absent,
                held(HERS),
                DraftStep::Landed,
            ),
        ]);
    }

    #[test]
    fn rule_2_a_team_file_at_a_path_she_did_not_start_from_is_add_add() {
        check(vec![
            new_file(
                "both added, different text",
                HERS,
                held(NEW_FILE),
                conflict(AddAdd),
            ),
            new_file(
                "both added, team's not text",
                HERS,
                Held(NOT_TEXT),
                conflict(AddAdd),
            ),
            // Shadows rule 3: from is in step, and that says nothing about the path.
            renamed(
                "rename, from in step",
                HERS,
                held(NEW_FILE),
                held(START),
                conflict(AddAdd),
            ),
            renamed(
                "unedited rename, from in step",
                START,
                held(NEW_FILE),
                held(START),
                conflict(AddAdd),
            ),
            renamed(
                "rename, from moved on",
                HERS,
                held(NEW_FILE),
                held(HIS),
                conflict(AddAdd),
            ),
            renamed(
                "rename, from gone",
                HERS,
                held(NEW_FILE),
                Absent,
                conflict(AddAdd),
            ),
            renamed(
                "rename, team's not text",
                HERS,
                Held(NOT_TEXT),
                held(START),
                conflict(AddAdd),
            ),
            // Two files on a disk that keeps them apart. A compare of folded names
            // reads this as in step, and her share writes over his file.
            case_moved(
                "[case-sensitive] bob added his own Plan.md, from in step",
                START,
                held(NEW_FILE),
                held(START),
                conflict(AddAdd),
            ),
            case_moved(
                "[case-sensitive] same, her draft edited",
                HERS,
                held(NEW_FILE),
                held(START),
                conflict(AddAdd),
            ),
            // The team made the same case move and went on.
            case_moved(
                "[either disk] the team moved it to her spelling and edited it",
                START,
                held(HIS),
                Absent,
                conflict(AddAdd),
            ),
            case_moved(
                "[either disk] her landed case move, then a peer's edit, pass not run",
                HERS,
                held(BOTH),
                Absent,
                conflict(AddAdd),
            ),
        ]);
    }

    #[test]
    fn rule_3_a_team_text_equal_to_the_start_is_in_step() {
        check(vec![
            own(
                "edit at its own path",
                Some(START),
                Some(HERS),
                held(START),
                DraftStep::InStep,
            ),
            own(
                "tombstone",
                Some(START),
                None,
                held(START),
                DraftStep::InStep,
            ),
            new_file(
                "new file, team has nothing",
                HERS,
                Absent,
                DraftStep::InStep,
            ),
            renamed(
                "unedited rename",
                START,
                Absent,
                held(START),
                DraftStep::InStep,
            ),
            renamed(
                "edited rename",
                HERS,
                Absent,
                held(START),
                DraftStep::InStep,
            ),
            case_moved(
                "[either disk] unedited case move",
                START,
                Absent,
                held(START),
                DraftStep::InStep,
            ),
            case_moved(
                "[either disk] edited case move",
                HERS,
                Absent,
                held(START),
                DraftStep::InStep,
            ),
        ]);
    }

    #[test]
    fn rule_5_a_draft_with_no_edit_takes_the_teams_text() {
        check(vec![
            own(
                "at its own path",
                Some(START),
                Some(START),
                held(HIS),
                take(HIS, false),
            ),
            renamed(
                "unedited rename",
                START,
                Absent,
                held(HIS),
                take(HIS, false),
            ),
            case_moved(
                "[either disk] unedited case move",
                START,
                Absent,
                held(HIS),
                take(HIS, false),
            ),
        ]);
    }

    #[test]
    fn rule_6_a_draft_whose_team_file_went_away_is_edit_delete() {
        check(vec![
            own(
                "edited",
                Some(START),
                Some(HERS),
                Absent,
                conflict(EditDelete),
            ),
            own(
                "not edited",
                Some(START),
                Some(START),
                Absent,
                conflict(EditDelete),
            ),
            renamed("edited rename", HERS, Absent, Absent, conflict(EditDelete)),
            renamed(
                "unedited rename",
                START,
                Absent,
                Absent,
                conflict(EditDelete),
            ),
            case_moved(
                "[either disk] unedited case move",
                START,
                Absent,
                Absent,
                conflict(EditDelete),
            ),
            case_moved(
                "[either disk] edited case move",
                HERS,
                Absent,
                Absent,
                conflict(EditDelete),
            ),
        ]);
    }

    #[test]
    fn rule_7_a_tombstone_over_a_changed_team_file_is_delete_edit() {
        check(vec![
            own(
                "the team edited it",
                Some(START),
                None,
                held(HIS),
                conflict(DeleteEdit),
            ),
            own(
                "the team added it after the draft came here",
                None,
                None,
                held(HIS),
                conflict(DeleteEdit),
            ),
            own(
                "the team's is not text",
                Some(START),
                None,
                Held(NOT_TEXT),
                conflict(DeleteEdit),
            ),
            // The hazard Tasks 5 and 6 must carry: asked alone, the tombstone half
            // of her own edited case move, landed at the team's spelling, reads as
            // this. A tombstone that is half of a move is not asked on its own.
            own(
                "[folding] the tombstone half of her landed case move, asked alone",
                Some(START),
                None,
                held(HERS),
                conflict(DeleteEdit),
            ),
        ]);
    }

    #[test]
    fn rule_8_an_edit_against_a_change_merges() {
        check(vec![
            own(
                "clean merge",
                Some(START),
                Some(HERS),
                held(HIS),
                take(BOTH, true),
            ),
            own(
                "same line",
                Some(START),
                Some(HERS),
                held(SAME_LINE),
                conflict(EditEdit),
            ),
            renamed(
                "edited rename, clean",
                HERS,
                Absent,
                held(HIS),
                take(BOTH, true),
            ),
            renamed(
                "edited rename, same line",
                HERS,
                Absent,
                held(SAME_LINE),
                conflict(EditEdit),
            ),
            case_moved(
                "[either disk] edited case move, clean",
                HERS,
                Absent,
                held(HIS),
                take(BOTH, true),
            ),
            // Both made one edit, seen from a moved draft: the team's text, and the
            // start moves on.
            renamed(
                "both made the same edit, moved",
                HERS,
                Absent,
                held(HERS),
                take(HERS, true),
            ),
            // Nothing to work from: no start, so the two texts are a both-added.
            own(
                "no start, her text and the team's",
                None,
                Some(HERS),
                held(NEW_FILE),
                conflict(AddAdd),
            ),
        ]);
    }

    /// C5, and what the struct allows and the record rules forbid: we cannot
    /// tell, and it reads so, ahead of every rule.
    #[test]
    fn what_cannot_be_told_is_unknown_start() {
        let unknown = || conflict(UnknownStart);
        check(vec![
            // A key with no readable copy is never "absent".
            own(
                "tombstone over a key with no copy",
                Some(START),
                None,
                Unreadable,
                unknown(),
            ),
            own(
                "draft at a key with no copy",
                Some(START),
                Some(HERS),
                Unreadable,
                unknown(),
            ),
            own(
                "draft equal to the start at a key with no copy",
                Some(START),
                Some(START),
                Unreadable,
                unknown(),
            ),
            new_file(
                "new file over a key with no copy",
                HERS,
                Unreadable,
                unknown(),
            ),
            renamed(
                "no copy at the path, from in step",
                HERS,
                Unreadable,
                held(START),
                unknown(),
            ),
            renamed(
                "no copy at from, from would be gone",
                HERS,
                Absent,
                Unreadable,
                unknown(),
            ),
            renamed(
                "no copy at from, her text at the path",
                HERS,
                held(HERS),
                Unreadable,
                unknown(),
            ),
            case_moved(
                "[folding] the snapshot holds both spellings, with one copy",
                HERS,
                Unreadable,
                Unreadable,
                unknown(),
            ),
            // Shapes no writer leaves.
            Row {
                name: "new file with a start",
                path: "plan.md",
                from: None,
                start: Some(START),
                local: Some(HERS),
                at_path: Absent,
                at_from: Absent,
                step: unknown(),
            },
            Row {
                name: "new file with a team text at a from it does not have",
                path: "plan.md",
                from: None,
                start: None,
                local: Some(HERS),
                at_path: Absent,
                at_from: held(START),
                step: unknown(),
            },
            Row {
                name: "one path, two team texts",
                path: "plan.md",
                from: Some("plan.md"),
                start: Some(START),
                local: Some(HERS),
                at_path: held(HERS),
                at_from: held(START),
                step: unknown(),
            },
        ]);
    }

    /// C5's check: a tombstone answers for the path it deletes, whatever a
    /// record says it started from.
    #[test]
    fn a_tombstone_that_names_another_start_still_answers_for_its_own_path() {
        check(vec![
            Row {
                name: "from another path, in step there, the team has this path",
                path: "renamed.md",
                from: Some("plan.md"),
                start: Some(START),
                local: None,
                at_path: held(NEW_FILE),
                at_from: held(START),
                step: conflict(DeleteEdit),
            },
            Row {
                name: "no from, the team has this path",
                path: "plan.md",
                from: None,
                start: None,
                local: None,
                at_path: held(NEW_FILE),
                at_from: Absent,
                step: conflict(DeleteEdit),
            },
            Row {
                name: "from another path, the team does not have this path",
                path: "renamed.md",
                from: Some("plan.md"),
                start: Some(START),
                local: None,
                at_path: Absent,
                at_from: held(HIS),
                step: DraftStep::Landed,
            },
        ]);
    }

    // ---- The rows that need a text of their own. ----

    #[test]
    fn a_crlf_team_text_equal_to_the_draft_as_lf_has_landed() {
        let team = crlf(HERS);
        let row = sides(
            "plan.md",
            Some("plan.md"),
            Some(START),
            Some(HERS),
            Held(&team),
            Held(&team),
        );
        assert!(has_landed(&row));
        assert_eq!(next_step(&row), DraftStep::Landed);
    }

    #[test]
    fn a_crlf_team_text_equal_to_the_start_as_lf_is_in_step() {
        let team = crlf(START);
        let row = sides(
            "plan.md",
            Some("plan.md"),
            Some(START),
            Some(HERS),
            Held(&team),
            Held(&team),
        );
        assert_eq!(next_step(&row), DraftStep::InStep);
        // Both kinds of row, and a tombstone over it.
        let row = sides(
            "plan.md",
            Some("plan.md"),
            Some(START),
            None,
            Held(&team),
            Held(&team),
        );
        assert_eq!(next_step(&row), DraftStep::InStep);
        let row = sides(
            "renamed.md",
            Some("plan.md"),
            Some(START),
            Some(HERS),
            Absent,
            Held(&team),
        );
        assert_eq!(next_step(&row), DraftStep::InStep);
    }

    #[test]
    fn a_crlf_team_text_is_merged_and_taken_as_lf() {
        let team = crlf(HIS);
        let row = sides(
            "plan.md",
            Some("plan.md"),
            Some(START),
            Some(HERS),
            Held(&team),
            Held(&team),
        );
        assert_eq!(next_step(&row), take(BOTH, true));
        let row = sides(
            "plan.md",
            Some("plan.md"),
            Some(START),
            Some(START),
            Held(&team),
            Held(&team),
        );
        assert_eq!(next_step(&row), take(HIS, false));
    }

    #[test]
    fn a_crlf_landed_case_move_at_the_old_spelling_has_landed() {
        let team = crlf(HERS);
        let row = sides(
            "Plan.md",
            Some("plan.md"),
            Some(START),
            Some(HERS),
            Absent,
            Held(&team),
        );
        assert_eq!(next_step(&row), DraftStep::Landed);
    }

    #[test]
    fn a_draft_that_differs_only_in_generated_has_landed() {
        let team = stamped(HERS, "2026-11-06T10:00:00Z");
        let mine = stamped(HERS, "2026-11-06T11:00:00Z");
        let row = sides(
            "plan.md",
            Some("plan.md"),
            Some(START),
            Some(&mine),
            Held(team.as_bytes()),
            Held(team.as_bytes()),
        );
        assert!(has_landed(&row));
        assert_eq!(next_step(&row), DraftStep::Landed);
        // And one word more is not.
        let other = mine.replace("alpha hers", "alpha hers!");
        let row = sides(
            "plan.md",
            Some("plan.md"),
            Some(START),
            Some(&other),
            Held(team.as_bytes()),
            Held(team.as_bytes()),
        );
        assert!(!has_landed(&row));
    }

    #[test]
    fn an_unedited_case_move_has_not_landed() {
        // Team at from equals the start, on either disk, with no line touched.
        let row = sides(
            "Plan.md",
            Some("plan.md"),
            Some(START),
            Some(START),
            Absent,
            held(START),
        );
        assert!(!has_landed(&row));
        assert_eq!(next_step(&row), DraftStep::InStep);
        // A CRLF team file is no change either.
        let team = crlf(START);
        let row = sides(
            "Plan.md",
            Some("plan.md"),
            Some(START),
            Some(START),
            Absent,
            Held(&team),
        );
        assert!(!has_landed(&row));
        assert_eq!(next_step(&row), DraftStep::InStep);
        // The move touched her `generated` line; nothing else changed.
        let mine = stamped(START, "2026-11-06T11:00:00Z");
        let row = sides(
            "Plan.md",
            Some("plan.md"),
            Some(START),
            Some(&mine),
            Absent,
            held(START),
        );
        assert!(!has_landed(&row));
        assert_eq!(next_step(&row), DraftStep::InStep);
    }

    /// F5: a colleague's touch of the line that is no one's words must not end
    /// her case move. The clause in the brief reads this as landed.
    #[test]
    fn a_peers_touch_of_generated_does_not_end_an_unedited_case_move() {
        let team = stamped(START, "2026-11-06T12:00:00Z");
        let row = sides(
            "Plan.md",
            Some("plan.md"),
            Some(START),
            Some(START),
            Absent,
            Held(team.as_bytes()),
        );
        assert!(!has_landed(&row));
        assert_eq!(next_step(&row), take(&team, false));
        // Her move touched the line as well, with another stamp.
        let mine = stamped(START, "2026-11-06T11:00:00Z");
        let row = sides(
            "Plan.md",
            Some("plan.md"),
            Some(START),
            Some(&mine),
            Absent,
            Held(team.as_bytes()),
        );
        assert!(!has_landed(&row));
        assert_eq!(next_step(&row), take(&team, false));
    }

    #[test]
    fn a_case_move_whose_only_change_is_generated_ends_when_it_lands() {
        let mine = stamped(START, "2026-11-06T11:00:00Z");
        let row = sides(
            "Plan.md",
            Some("plan.md"),
            Some(START),
            Some(&mine),
            Absent,
            Held(mine.as_bytes()),
        );
        assert!(has_landed(&row));
        assert_eq!(next_step(&row), DraftStep::Landed);
    }

    #[test]
    fn an_edited_case_move_whose_text_stands_at_the_old_spelling_has_landed() {
        // [folding]: the share kept the team's spelling.
        let row = sides(
            "Plan.md",
            Some("plan.md"),
            Some(START),
            Some(HERS),
            Absent,
            held(HERS),
        );
        assert!(has_landed(&row));
        assert_eq!(next_step(&row), DraftStep::Landed);
        // A touched stamp since then changes nothing.
        let team = stamped(HERS, "2026-11-06T12:00:00Z");
        let row = sides(
            "Plan.md",
            Some("plan.md"),
            Some(START),
            Some(HERS),
            Absent,
            Held(team.as_bytes()),
        );
        assert!(has_landed(&row));
        // A spelling of another name is not a case move: this is a rename, and a
        // file holding her text at its old path has not landed at the new one.
        let row = sides(
            "renamed.md",
            Some("plan.md"),
            Some(START),
            Some(HERS),
            Absent,
            held(HERS),
        );
        assert!(!has_landed(&row));
        assert_eq!(next_step(&row), take(HERS, true));
        // [case-sensitive] two files: his at her spelling, her text at the old one.
        // Her text did not land where she moved it.
        let row = sides(
            "Plan.md",
            Some("plan.md"),
            Some(START),
            Some(HERS),
            held(NEW_FILE),
            held(HERS),
        );
        assert!(!has_landed(&row));
        assert_eq!(next_step(&row), conflict(AddAdd));
    }

    #[test]
    fn a_case_move_of_a_file_with_a_link_to_itself_stays_unedited() {
        let start = "---\ntitle: Plan\n---\nself [me](plan.md)\n";
        let mine = "---\ntitle: Plan\n---\nself [me](Plan.md)\n";
        assert_eq!(as_moved(start, "plan.md", "Plan.md"), mine);
        // Not landed while the team holds the start.
        let row = sides(
            "Plan.md",
            Some("plan.md"),
            Some(start),
            Some(mine),
            Absent,
            Held(start.as_bytes()),
        );
        assert!(!has_landed(&row));
        assert_eq!(next_step(&row), DraftStep::InStep);
        // A peer's edit is taken with the link rebased: no edit of hers.
        let his = "---\ntitle: Plan\n---\nself [me](plan.md)\nhis\n";
        let row = sides(
            "Plan.md",
            Some("plan.md"),
            Some(start),
            Some(mine),
            Absent,
            Held(his.as_bytes()),
        );
        assert_eq!(
            next_step(&row),
            take("---\ntitle: Plan\n---\nself [me](Plan.md)\nhis\n", false)
        );
    }

    // Links: moved from a/plan.md to b/c/plan.md.
    const LINKED: &str = "---\ntitle: Plan\nstatus: draft\n---\nsee [o](other.md)\nme [[a/plan.md]]\nbeta\ngamma\ndelta\nepsilon\nzeta\neta\n";
    const LINKED_MOVED: &str = "---\ntitle: Plan\nstatus: draft\n---\nsee [o](../../a/other.md)\nme [[b/c/plan.md]]\nbeta\ngamma\ndelta\nepsilon\nzeta\neta\n";

    #[test]
    fn an_unedited_move_takes_the_teams_text_with_its_links_rebased() {
        assert_eq!(as_moved(LINKED, "a/plan.md", "b/c/plan.md"), LINKED_MOVED);
        let his = LINKED.replace("\neta\n", "\neta [p](peer.md)\n");
        let row = sides(
            "b/c/plan.md",
            Some("a/plan.md"),
            Some(LINKED),
            Some(LINKED_MOVED),
            Absent,
            Held(his.as_bytes()),
        );
        assert_eq!(
            next_step(&row),
            take(
                "---\ntitle: Plan\nstatus: draft\n---\nsee [o](../../a/other.md)\nme [[b/c/plan.md]]\nbeta\ngamma\ndelta\nepsilon\nzeta\neta [p](../../a/peer.md)\n",
                false
            )
        );
    }

    #[test]
    fn an_unedited_move_with_only_a_touched_generated_line_is_unedited() {
        let start = stamped(LINKED, "2026-11-06T09:00:00Z");
        // What `move_within` writes when it rebased a link.
        let mine = stamped(LINKED_MOVED, "2026-11-06T11:00:00Z");
        let his = stamped(
            &LINKED.replace("\neta\n", "\neta his\n"),
            "2026-11-06T10:00:00Z",
        );
        let row = sides(
            "b/c/plan.md",
            Some("a/plan.md"),
            Some(&start),
            Some(&mine),
            Absent,
            Held(his.as_bytes()),
        );
        let wanted = stamped(
            &LINKED_MOVED.replace("\neta\n", "\neta his\n"),
            "2026-11-06T10:00:00Z",
        );
        assert_eq!(next_step(&row), take(&wanted, false));
    }

    #[test]
    fn an_edited_move_merges_in_the_new_folder() {
        // Her link is in the draft's folder, his in the team's. Each is in the
        // result once and right for the new folder, and hers is not rebased again.
        let mine = LINKED_MOVED.replace("gamma\n", "gamma [h](../../a/hers.md)\n");
        let his = LINKED.replace("\neta\n", "\neta [p](peer.md)\n");
        let row = sides(
            "b/c/plan.md",
            Some("a/plan.md"),
            Some(LINKED),
            Some(&mine),
            Absent,
            Held(his.as_bytes()),
        );
        assert_eq!(
            next_step(&row),
            take(
                "---\ntitle: Plan\nstatus: draft\n---\nsee [o](../../a/other.md)\nme [[b/c/plan.md]]\nbeta\ngamma [h](../../a/hers.md)\ndelta\nepsilon\nzeta\neta [p](../../a/peer.md)\n",
                true
            )
        );
    }

    #[test]
    fn a_draft_at_its_own_path_is_not_rebased() {
        // A rebase from a path to itself would still tidy `sub/../x.md`, and
        // that would read as an edit of hers.
        let start = "---\ntitle: Plan\nstatus: draft\n---\nsee [o](sub/../x.md)\nbeta\ngamma\ndelta\nepsilon\nzeta\neta\n";
        let his = start.replace("\neta\n", "\neta his\n");
        let row = sides(
            "a/plan.md",
            Some("a/plan.md"),
            Some(start),
            Some(start),
            Held(his.as_bytes()),
            Held(his.as_bytes()),
        );
        assert_eq!(next_step(&row), take(&his, false));
    }

    /// `merge_file` answers `Converged` also when the team did not change the
    /// file, and the rule "`Converged` gives the team's text" then drops her
    /// edit. Only a rebase can bring this about: two spellings of one link are
    /// two texts at `from` and one in the new folder.
    #[test]
    fn a_team_change_that_is_nothing_in_the_new_folder_keeps_her_edit() {
        let start = "---\ntitle: Plan\n---\nsee [o](other.md)\nbeta\ngamma\ndelta\n";
        let his = "---\ntitle: Plan\n---\nsee [o](./other.md)\nbeta\ngamma\ndelta\n";
        let mine = "---\ntitle: Plan\n---\nsee [o](../a/other.md)\nbeta\ngamma\ndelta hers\n";
        let row = sides(
            "b/plan.md",
            Some("a/plan.md"),
            Some(start),
            Some(mine),
            Absent,
            Held(his.as_bytes()),
        );
        assert_eq!(next_step(&row), take(mine, false));
        // What it takes is LF, whatever the row held.
        let mine_crlf = String::from_utf8(crlf(mine)).unwrap();
        let row = sides(
            "b/plan.md",
            Some("a/plan.md"),
            Some(start),
            Some(&mine_crlf),
            Absent,
            Held(his.as_bytes()),
        );
        assert_eq!(next_step(&row), take(mine, false));
    }

    #[test]
    fn a_same_line_edit_is_edit_edit() {
        let row = sides(
            "plan.md",
            Some("plan.md"),
            Some(START),
            Some(HERS),
            held(SAME_LINE),
            held(SAME_LINE),
        );
        assert_eq!(next_step(&row), conflict(EditEdit));
    }

    #[test]
    fn frontmatter_merges_by_key() {
        let start = "---\ntitle: Plan\nstatus: draft\n---\nbody\n";
        let mine = "---\ntitle: Plan\nstatus: active\n---\nbody\n";
        let his = "---\ntitle: The Plan\nstatus: draft\n---\nbody\n";
        let row = sides(
            "plan.md",
            Some("plan.md"),
            Some(start),
            Some(mine),
            Held(his.as_bytes()),
            Held(his.as_bytes()),
        );
        assert_eq!(
            next_step(&row),
            take("---\ntitle: The Plan\nstatus: active\n---\nbody\n", true)
        );
    }

    #[test]
    fn two_touched_generated_lines_merge_to_the_newer() {
        let start = stamped(START, "2026-11-06T09:00:00Z");
        let mine = stamped(HERS, "2026-11-06T11:00:00Z");
        let his = stamped(HIS, "2026-11-06T10:00:00Z");
        let row = sides(
            "plan.md",
            Some("plan.md"),
            Some(&start),
            Some(&mine),
            Held(his.as_bytes()),
            Held(his.as_bytes()),
        );
        assert_eq!(
            next_step(&row),
            take(&stamped(BOTH, "2026-11-06T11:00:00Z"), true)
        );
    }

    #[test]
    fn a_named_new_permalink_survives_the_merge_as_her_change() {
        let start = "---\ntitle: Plan\npermalink: plan\n---\nalpha\nbeta\ngamma\ndelta\nepsilon\n";
        // The move named a new address; no other word changed. That is an edit.
        let mine =
            "---\ntitle: Plan\npermalink: renamed\n---\nalpha\nbeta\ngamma\ndelta\nepsilon\n";
        let his =
            "---\ntitle: Plan\npermalink: plan\n---\nalpha\nbeta\ngamma\ndelta\nepsilon his\n";
        let row = sides(
            "renamed.md",
            Some("plan.md"),
            Some(start),
            Some(mine),
            Absent,
            Held(his.as_bytes()),
        );
        assert_eq!(
            next_step(&row),
            take(
                "---\ntitle: Plan\npermalink: renamed\n---\nalpha\nbeta\ngamma\ndelta\nepsilon his\n",
                true
            )
        );
    }

    #[test]
    fn a_draft_with_no_change_against_a_delete_is_a_conflict_when_moved() {
        let row = sides(
            "renamed.md",
            Some("plan.md"),
            Some(START),
            Some(START),
            Absent,
            Absent,
        );
        assert_eq!(next_step(&row), conflict(EditDelete));
    }

    #[test]
    fn a_delete_against_an_edit_is_delete_edit() {
        let row = sides(
            "plan.md",
            Some("plan.md"),
            Some(START),
            None,
            held(HIS),
            held(HIS),
        );
        assert_eq!(next_step(&row), conflict(DeleteEdit));
    }

    #[test]
    fn a_delete_against_a_delete_has_landed() {
        let row = sides(
            "plan.md",
            Some("plan.md"),
            Some(START),
            None,
            Absent,
            Absent,
        );
        assert!(has_landed(&row));
        assert_eq!(next_step(&row), DraftStep::Landed);
    }

    #[test]
    fn both_added_with_one_text_has_landed() {
        let row = sides("plan.md", None, None, Some(HERS), held(HERS), Absent);
        assert_eq!(next_step(&row), DraftStep::Landed);
        let row = sides("plan.md", None, None, Some(HERS), held(HIS), Absent);
        assert_eq!(next_step(&row), conflict(AddAdd));
    }

    #[test]
    fn a_team_file_at_the_path_she_moved_to_is_add_add_though_from_is_in_step() {
        let row = sides(
            "renamed.md",
            Some("plan.md"),
            Some(START),
            Some(START),
            held(NEW_FILE),
            held(START),
        );
        assert!(!has_landed(&row));
        assert_eq!(next_step(&row), conflict(AddAdd));
    }

    #[test]
    fn a_team_text_that_is_not_utf8_is_a_conflict() {
        check(vec![
            own(
                "edited",
                Some(START),
                Some(HERS),
                Held(NOT_TEXT),
                conflict(EditEdit),
            ),
            // Rule 5 has no text to take, and a lossy decode would write one.
            own(
                "not edited",
                Some(START),
                Some(START),
                Held(NOT_TEXT),
                conflict(EditEdit),
            ),
            own(
                "no start",
                None,
                Some(HERS),
                Held(NOT_TEXT),
                conflict(AddAdd),
            ),
            renamed(
                "edited rename",
                HERS,
                Absent,
                Held(NOT_TEXT),
                conflict(EditEdit),
            ),
            renamed(
                "unedited rename",
                START,
                Absent,
                Held(NOT_TEXT),
                conflict(EditEdit),
            ),
            case_moved(
                "[either disk] case move",
                HERS,
                Absent,
                Held(NOT_TEXT),
                conflict(EditEdit),
            ),
            own(
                "tombstone",
                Some(START),
                None,
                Held(NOT_TEXT),
                conflict(DeleteEdit),
            ),
        ]);
    }

    #[test]
    fn a_text_that_is_nearly_utf8_is_never_decoded_with_a_loss() {
        // Valid but for one byte: a lossy decode makes it a text, and one equal
        // to nothing here, so only the answer can show it.
        let mut team = HERS.as_bytes().to_vec();
        team.push(0xff);
        let row = sides(
            "plan.md",
            Some("plan.md"),
            Some(START),
            Some(HERS),
            Held(&team),
            Held(&team),
        );
        assert!(!has_landed(&row));
        assert_eq!(next_step(&row), conflict(EditEdit));
    }

    #[test]
    fn the_evolve_home_rule_applies_to_the_manifest() {
        let base = "---\ntitle: Team\ntype: manifest\nstatus: active\n---\nAbout the team.\n";
        let hers = "---\ntitle: Team\ntype: manifest\nstatus: active\nevolve_home: https://hers.example\n---\nAbout the team.\n";
        let team = "---\ntitle: Team\ntype: manifest\nstatus: active\nevolve_home: https://team.example\n---\nAbout the team.\n";
        let row = sides(
            "MANIFEST.md",
            Some("MANIFEST.md"),
            Some(base),
            Some(hers),
            Held(team.as_bytes()),
            Held(team.as_bytes()),
        );
        assert_eq!(next_step(&row), take(team, true));
        // Anywhere else it is two lines of hers and his.
        let row = sides(
            "notes/MANIFEST.md",
            Some("notes/MANIFEST.md"),
            Some(base),
            Some(hers),
            Held(team.as_bytes()),
            Held(team.as_bytes()),
        );
        assert_eq!(next_step(&row), conflict(EditEdit));
    }

    #[test]
    fn a_start_or_a_draft_in_crlf_reads_as_lf() {
        let start = String::from_utf8(crlf(START)).unwrap();
        let row = sides(
            "plan.md",
            Some("plan.md"),
            Some(&start),
            Some(HERS),
            held(START),
            held(START),
        );
        assert_eq!(next_step(&row), DraftStep::InStep);
        let mine = String::from_utf8(crlf(HERS)).unwrap();
        let row = sides(
            "plan.md",
            Some("plan.md"),
            Some(START),
            Some(&mine),
            held(HIS),
            held(HIS),
        );
        assert_eq!(next_step(&row), take(BOTH, true));
        let row = sides(
            "plan.md",
            Some("plan.md"),
            Some(START),
            Some(&mine),
            held(HERS),
            held(HERS),
        );
        assert_eq!(next_step(&row), DraftStep::Landed);
    }

    #[test]
    fn a_peers_touch_of_generated_does_not_end_a_case_move_with_a_link_to_itself() {
        // Her draft is the start with the link rebased, so only a start in the
        // draft's coordinates reads it as unedited.
        let start = "---\ntitle: Plan\nstatus: draft\n---\nself [me](plan.md)\n";
        let mine = "---\ntitle: Plan\nstatus: draft\n---\nself [me](Plan.md)\n";
        // A team text in her coordinates, one stamp apart: a peer who made this
        // same link change has not landed her unedited move.
        let team = stamped(mine, "2026-11-06T12:00:00Z");
        let row = sides(
            "Plan.md",
            Some("plan.md"),
            Some(start),
            Some(mine),
            Absent,
            Held(team.as_bytes()),
        );
        assert!(!has_landed(&row));
    }

    #[test]
    fn a_merge_conflict_keeps_its_kind() {
        for (kind, wanted) in [
            (ConflictKind::EditEdit, EditEdit),
            (ConflictKind::EditDelete, EditDelete),
            (ConflictKind::DeleteEdit, DeleteEdit),
            (ConflictKind::AddAdd, AddAdd),
        ] {
            assert_eq!(DraftConflictKind::from(kind), wanted);
        }
    }

    #[test]
    fn a_conflict_kind_serializes_by_its_name() {
        for (kind, name) in [
            (EditEdit, "EditEdit"),
            (EditDelete, "EditDelete"),
            (DeleteEdit, "DeleteEdit"),
            (AddAdd, "AddAdd"),
            (UnknownStart, "UnknownStart"),
            (DraftConflictKind::AddressTaken, "AddressTaken"),
        ] {
            assert_eq!(serde_json::to_value(kind).unwrap(), name);
        }
    }

    /// Every combination the sides can take, against what must hold whichever
    /// rule answers. Nothing here repeats a rule: each line is a way of losing
    /// a colleague's work or hers.
    #[test]
    fn no_combination_loses_a_side_or_panics() {
        let crlf_start = crlf(START);
        let crlf_his = crlf(HIS);
        let teams: Vec<TeamText<'_>> = vec![
            Absent,
            Unreadable,
            Held(NOT_TEXT),
            held(START),
            Held(&crlf_start),
            held(HERS),
            held(HIS),
            Held(&crlf_his),
            held(BOTH),
            held(SAME_LINE),
            held(NEW_FILE),
        ];
        let locals = [
            None,
            Some(START),
            Some(HERS),
            Some(HIS),
            Some(BOTH),
            Some(SAME_LINE),
        ];
        let froms = [None, Some("plan.md"), Some("Plan.md"), Some("old.md")];
        let mut seen = 0usize;
        for from in froms {
            for start in [None, Some(START)] {
                for local in locals {
                    for &at_path in &teams {
                        for &at_from in &teams {
                            seen += 1;
                            let row = sides("plan.md", from, start, local, at_path, at_from);
                            let context = format!("{row:?}");
                            let step = next_step(&row);
                            let landed = has_landed(&row);
                            assert_eq!(landed, step == DraftStep::Landed, "{context}");
                            let unreadable = at_path == Unreadable || at_from == Unreadable;
                            if unreadable {
                                assert_eq!(step, conflict(UnknownStart), "{context}");
                            }
                            let is_text = |team: TeamText<'_>| match team {
                                Held(bytes) => std::str::from_utf8(bytes)
                                    .ok()
                                    .map(|text| text.replace("\r\n", "\n")),
                                _ => None,
                            };
                            let path_text = is_text(at_path);
                            let from_text = is_text(at_from);
                            match &step {
                                // The team's file at a path she did not start
                                // from is never written over with no word.
                                DraftStep::InStep => {
                                    assert!(
                                        at_path == Absent || from == Some("plan.md"),
                                        "{context}"
                                    );
                                    assert_eq!(from_text.as_deref(), start, "{context}");
                                }
                                DraftStep::Landed => match local {
                                    None => assert_eq!(at_path, Absent, "{context}"),
                                    Some(local) => assert!(
                                        path_text.as_deref() == Some(local)
                                            || from_text.as_deref() == Some(local),
                                        "{context}"
                                    ),
                                },
                                DraftStep::Take { text, .. } => {
                                    let local = local.expect("a tombstone never takes");
                                    let team =
                                        from_text.as_deref().expect("a take has a team text");
                                    assert!(!text.contains('\r'), "{context}");
                                    assert!(!text.contains('\u{fffd}'), "{context}");
                                    // His line stays, and hers stays when it is hers.
                                    if team.contains("eta his") {
                                        assert!(text.contains("eta his"), "{context}");
                                    }
                                    if local.contains("alpha hers") {
                                        assert!(text.contains("alpha hers"), "{context}");
                                    }
                                }
                                DraftStep::Conflict(_) => {}
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(seen, 4 * 2 * 6 * 11 * 11);
    }
}
