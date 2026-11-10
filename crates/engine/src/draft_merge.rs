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
//!   `plan.md`'s bytes. A key with no readable copy is [`TeamText::Unreadable`]
//!   with the key's stamp, never [`TeamText::Absent`], and so is a snapshot that
//!   holds two spellings of one path on a folding disk, where it cannot hold two
//!   copies.
//! - **`folds_case` is the domain folder's**, since the same keys mean her own
//!   landed case move on one disk and a colleague's coincidence on the other.
//! - **`start_sha256` and `shared` are the record's.** Keep mine over a team file
//!   with no text writes the digest, so it settles like any other.
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
use std::collections::BTreeMap;

use crystalline_core::frontmatter::same_but_generated;
use crystalline_core::relink::{rebase_relative_links, respell_path_wikilinks};
use crystalline_core::to_lf;
use crystalline_remote::merge::{ConflictKind, FileMerge};
use crystalline_remote::ops::merge_with_manifest_rule;
use crystalline_remote::state::{BaseStamp, OriginState, ProposalStatus, ProposedChange};
use serde::Serialize;

use crate::draft_start::DraftStart;

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
    /// a file there and we cannot say what it holds, which is not absent. The
    /// digest is the key's stamp in the snapshot.
    Unreadable { sha256: &'a str },
}

/// One row, its record and the team's text.
#[derive(Clone, Copy, Debug)]
pub struct DraftSides<'a> {
    /// The row's exact path.
    pub path: &'a str,
    /// The team path it started from. `None` is a new file.
    pub from: Option<&'a str>,
    /// The text at `from` when last in step. `None` is absent there, unless
    /// `start_sha256` says what stood there.
    pub start: Option<&'a str>,
    /// The record's digest of a start that has no text (Keep mine on bytes that
    /// are not UTF-8 or have no readable copy). `start` is then `None`.
    pub start_sha256: Option<&'a str>,
    /// The text of hers the record says she last shared (`shared.text`), or
    /// `None`: nothing shared, or a deletion.
    pub shared: Option<&'a str>,
    /// Whether the domain's folder folds case (`folder_may_fold_case`). Only there
    /// does a case move's share land at the spelling the team already has.
    pub folds_case: bool,
    /// The record says a merge was being written into this row and its start
    /// has not moved yet (`DraftStart::merging`). The row may then hold the
    /// team's text over the old start, which the case clause would read as her
    /// case move landed; it never does while this stands.
    pub merging: bool,
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
    /// The team holds it and has let go of where she started. End the move, both
    /// halves.
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
/// write a replacement character into a draft. A key with no readable copy stays
/// that too, with its stamp: there is no arm that turns it into absent, so no
/// caller in this module can read it as gone.
enum Team<'a> {
    Absent,
    Text { text: Cow<'a, str>, bytes: &'a [u8] },
    NotText(&'a [u8]),
    Unreadable(&'a str),
}

impl<'a> Team<'a> {
    fn read(team: TeamText<'a>) -> Self {
        match team {
            TeamText::Absent => Team::Absent,
            TeamText::Held(bytes) => match std::str::from_utf8(bytes) {
                Ok(text) => Team::Text {
                    text: to_lf(text),
                    bytes,
                },
                Err(_) => Team::NotText(bytes),
            },
            TeamText::Unreadable { sha256 } => Team::Unreadable(sha256),
        }
    }

    fn text(&self) -> Option<&str> {
        match self {
            Team::Text { text, .. } => Some(text),
            _ => None,
        }
    }

    /// The sha-256 of the team's bytes as they stand, which is what a start with
    /// no text was recorded as.
    fn sha256(&self) -> Option<Cow<'a, str>> {
        match self {
            Team::Absent => None,
            Team::Text { bytes, .. } | Team::NotText(bytes) => {
                Some(Cow::Owned(crate::engine::sha256_hex(bytes)))
            }
            Team::Unreadable(sha256) => Some(Cow::Borrowed(sha256)),
        }
    }

    /// Whether the team holds what she started from. A change of only
    /// `generated` is no change: it is no one's words.
    fn is_start(&self, sides: &DraftSides<'_>) -> bool {
        match (sides.start, sides.start_sha256) {
            (None, None) => matches!(self, Team::Absent),
            (Some(start), None) => self
                .text()
                .is_some_and(|text| same_but_generated(text, start)),
            (None, Some(sha256)) => self.sha256().as_deref() == Some(sha256),
            // `can_tell` refuses this shape before anything asks.
            (Some(_), Some(_)) => false,
        }
    }
}

/// Whether these sides can be decided at all. A shape no writer leaves cannot: it
/// is answered as "we cannot tell" rather than guessed at, since a guess here can
/// be an overwrite.
fn can_tell(sides: &DraftSides<'_>) -> bool {
    let new_file_with_a_start = sides.from.is_none()
        && (sides.start.is_some()
            || sides.start_sha256.is_some()
            || sides.team_at_from != TeamText::Absent);
    let one_path_two_texts =
        sides.from == Some(sides.path) && sides.team_at_from != sides.team_at_path;
    let two_starts = sides.start.is_some() && sides.start_sha256.is_some();
    !(new_file_with_a_start || one_path_two_texts || two_starts)
}

/// The sides as they stand after her own share. When the team holds at her path
/// what she last shared, and has let go of where she started, her share is the
/// team's text and her start: a draft she edited again since reads that edit
/// against what she shared. A pass that did not run after the share would
/// otherwise leave her in conflict with herself. Only an equal text counts: a team
/// that went on after her share, or a share that has not landed, says nothing,
/// and the start she had answers.
fn after_her_share<'a>(sides: &DraftSides<'a>) -> DraftSides<'a> {
    let (Some(_), Some(shared)) = (sides.local, sides.shared) else {
        return *sides;
    };
    let holds_her_share = Team::read(sides.team_at_path)
        .text()
        .is_some_and(|text| same_but_generated(text, shared));
    let let_go = sides.from == Some(sides.path) || sides.team_at_from == TeamText::Absent;
    if !(holds_her_share && let_go) {
        return *sides;
    }
    DraftSides {
        from: Some(sides.path),
        start: Some(shared),
        start_sha256: None,
        team_at_from: sides.team_at_path,
        ..*sides
    }
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

/// Whether the team holds what this row says, so it is finished, both halves of a
/// move with it. Asked by the pass and by the conflict table alike, so that her own
/// landed share never reads as a conflict, whether or not a pass got to it.
///
/// - Nothing has landed while either path is a key with no readable copy.
/// - A tombstone has landed when the team has no file at its path.
/// - A draft has landed when the team's text at its path is the draft, but for
///   `generated`, **and** the team has let go of where she started: no `from`,
///   `from` is the path, or the team no longer holds `from`. A colleague who wrote
///   her text at her new path has not deleted the old one for her, and ending the
///   move then would drop her deletion with no word.
/// - A case move can also land at the team's spelling, on a disk that folds case
///   (see below).
pub fn has_landed(sides: &DraftSides<'_>) -> bool {
    // A key with no readable copy is never read as absent: with one on either
    // side, nothing has landed.
    let unreadable = matches!(sides.team_at_path, TeamText::Unreadable { .. })
        || matches!(sides.team_at_from, TeamText::Unreadable { .. });
    if unreadable || !can_tell(sides) {
        return false;
    }
    let sides = after_her_share(sides);
    let Some(local) = sides.local else {
        return matches!(Team::read(sides.team_at_path), Team::Absent);
    };
    match Team::read(sides.team_at_path) {
        Team::Text { text, .. } => {
            let let_go = sides.from.is_none_or(|from| from == sides.path)
                || sides.team_at_from == TeamText::Absent;
            same_but_generated(&text, local) && let_go
        }
        Team::Absent => case_move_landed_at_the_old_spelling(&sides, local),
        Team::NotText(_) | Team::Unreadable(_) => false,
    }
}

/// On a disk that folds case, a share of `plan.md` moved to `Plan.md` lands at the
/// spelling the team already has: `Plan.md` is no key, and her text stands at
/// `plan.md`. On a disk that keeps the two apart the same sides are a colleague who
/// wrote her text at the old spelling, and her move is not done, so this asks the
/// disk.
///
/// The team's text has to be a change from her start, beyond `generated`: an
/// unedited case move holds the start there until it lands and after, and it stays
/// in step, on either disk, rather than end on a stamp.
fn case_move_landed_at_the_old_spelling(sides: &DraftSides<'_>, local: &str) -> bool {
    let Some(from) = sides.from else {
        return false;
    };
    if sides.merging || !sides.folds_case || from.to_lowercase() != sides.path.to_lowercase() {
        return false;
    }
    // A row at its own path never gets here with a text: `can_tell` holds its two
    // team texts equal, and the one at the path is absent.
    let team = Team::read(sides.team_at_from);
    let Some(text) = team.text() else {
        return false;
    };
    !team.is_start(sides) && same_but_generated(text, local)
}

/// Rule 3's question: the team holds a file at her path that she did not start
/// from, and its text is not hers. Asked of sides already read after her share.
fn holds_a_file_of_theirs_at_path(sides: &DraftSides<'_>) -> bool {
    if sides.team_at_path == TeamText::Absent || sides.from == Some(sides.path) {
        return false;
    }
    let at_path = Team::read(sides.team_at_path);
    let hers = sides
        .local
        .zip(at_path.text())
        .is_some_and(|(local, team)| same_but_generated(team, local));
    !hers
}

/// Whether [`next_step`] answers from the team's text at the row's own path
/// rather than at `from`: when rule 3 meets a file of the team's there, when
/// she started there (or from nothing), and when her own share at the path is
/// her start. A caller that shows the team's side of a conflict reads it where
/// this says, so the text it shows is the one the answer came from.
pub fn answers_at_path(sides: &DraftSides<'_>) -> bool {
    let sides = after_her_share(sides);
    sides.from.is_none_or(|from| from == sides.path) || holds_a_file_of_theirs_at_path(&sides)
}

/// What should happen to this row. First match wins:
///
/// 1. A shape no writer leaves: [`DraftConflictKind::UnknownStart`].
///
///    Before the rest, a draft whose own share the team holds at its path reads
///    from that share (`after_her_share`).
/// 2. It has landed ([`has_landed`]): [`DraftStep::Landed`].
/// 3. The team holds the path and she did not start from it, compared to the
///    letter: `AddAdd`, or `DeleteEdit` for a tombstone; `UnknownStart` when
///    either path is a key with no readable copy. On a disk that keeps `Note.md` and `note.md` apart
///    those are two files, and in step at one says nothing about the other. Sides
///    read from real keys never show a case move's own copy here, so folding the
///    names would protect nothing and lose his file. A team text equal to her draft
///    is no file of his to lose, so it goes on to the rules below.
/// 4. The team's text at `from` is her start, but for `generated`, or both are
///    absent: [`DraftStep::InStep`]. A start with no text compares by digest.
/// 5. No readable copy at `from`: `UnknownStart`.
/// 6. A tombstone: `DeleteEdit`.
/// 7. The team's text is gone: `EditDelete`, edit or not. A moved file going away
///    is told.
/// 8. The team's text is not text, or her start is a digest: a conflict, as
///    `merge_file` would answer one. There is nothing to merge against.
/// 9. No edit of hers: the team's text, with no merge.
/// 10. The team did nothing in the draft's coordinates: her text, and the start
///     moves on. `merge_file` says `Converged` here too, and that one must not take
///     the team's text, which would drop her edit.
/// 11. The merge.
///
/// A `Take` is `edited` exactly when a merge changed her words, beyond
/// `generated`: that is what she is told about.
pub fn next_step(sides: &DraftSides<'_>) -> DraftStep {
    use DraftConflictKind::{AddAdd, DeleteEdit, EditDelete, EditEdit, UnknownStart};

    if !can_tell(sides) {
        return DraftStep::Conflict(UnknownStart);
    }
    let sides = &after_her_share(sides);
    if has_landed(sides) {
        return DraftStep::Landed;
    }
    if holds_a_file_of_theirs_at_path(sides) {
        let unreadable = matches!(sides.team_at_path, TeamText::Unreadable { .. })
            || matches!(sides.team_at_from, TeamText::Unreadable { .. });
        return DraftStep::Conflict(match sides.local {
            _ if unreadable => UnknownStart,
            Some(_) => AddAdd,
            None => DeleteEdit,
        });
    }
    let team = Team::read(sides.team_at_from);
    if team.is_start(sides) {
        return DraftStep::InStep;
    }
    if let Team::Unreadable(_) = team {
        return DraftStep::Conflict(UnknownStart);
    }
    let Some(local) = sides.local.map(to_lf) else {
        return DraftStep::Conflict(DeleteEdit);
    };
    let (team, start) = match (&team, sides.start) {
        (Team::Absent, _) => return DraftStep::Conflict(EditDelete),
        (Team::Text { text, .. }, Some(start)) => (text, Some(to_lf(start))),
        (Team::Text { text, .. }, None) if sides.start_sha256.is_none() => (text, None),
        _ if sides.start.is_some() || sides.start_sha256.is_some() => {
            return DraftStep::Conflict(EditEdit);
        }
        _ => return DraftStep::Conflict(AddAdd),
    };
    let team = in_draft_coordinates(team, sides.from, sides.path);
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
    if ancestor
        .as_deref()
        .is_some_and(|ancestor| same_but_generated(ancestor, &team))
    {
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
    let take = |text: String| DraftStep::Take {
        edited: !same_but_generated(&text, &local),
        text,
    };
    match merged {
        FileMerge::Apply(bytes) => match String::from_utf8(bytes) {
            Ok(text) => take(text),
            Err(_) => DraftStep::Conflict(EditEdit),
        },
        FileMerge::Converged => take(team.into_owned()),
        // Her draft and the team's text are both here, so nothing says delete.
        FileMerge::Delete => DraftStep::Conflict(EditDelete),
        FileMerge::Conflict(kind) => DraftStep::Conflict(kind.into()),
    }
}

/// What became of the proposal a record says holds this row
/// (`DraftStart::shared`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SharedFate {
    /// No proposal named, or one the chain still holds, whatever its status.
    Open,
    /// The team merged it: the row's start is now what the proposal held.
    Merged,
    /// Withdrawn, declined or no longer known: the proposal holds nothing of
    /// hers any more, and the row keeps the start it had.
    Ended,
}

/// What became of the proposal `record` says holds its row, read from the
/// origin state. `tombstone` says the row is a deletion.
///
/// Only history decides a merge. A status probe marks a record `Merged` in the
/// chain before a pull has brought its text into the base, and moving the start
/// to the shared text then would read the team's old text as a change of
/// theirs: her own draft would take it. So a proposal still in the chain, in
/// any status, is left as it is. One found in neither list is ended: history is
/// capped, and a start that stays where it was can only refuse, never lose a
/// word.
///
/// **A merge counts only where the merged proposal still held her row.** Its
/// history record lists the files of the last share that wrote its branch, and
/// that may not be hers: a colleague's share can update the same proposal in
/// place with only his drafts, and an amend carries only what it names. So the
/// record must list her row's key (her path, or the path her case move started
/// from, which is where a share carries it) with the digest of the shared text
/// in either line ending, or a deletion for a tombstone. Anything else is
/// ended: the merged branch held the team's text there, and her row keeps the
/// start it had.
pub fn settle_shared(record: &DraftStart, tombstone: bool, state: &OriginState) -> SharedFate {
    let Some(shared) = record.shared.as_ref() else {
        return SharedFate::Open;
    };
    if state.proposals.iter().any(|p| p.number == shared.proposal) {
        return SharedFate::Open;
    }
    let Some(merged) = state
        .history
        .iter()
        .find(|p| p.number == shared.proposal && p.status == ProposalStatus::Merged)
    else {
        return SharedFate::Ended;
    };
    let folded = crystalline_core::fold_path_case(&record.path);
    let case_move_from = record
        .from
        .as_deref()
        .filter(|from| crystalline_core::fold_path_case(from) == folded);
    let held_hers = merged.files.iter().any(|file| {
        let at_her_key = file.path == record.path || Some(file.path.as_str()) == case_move_from;
        let same_text = match (file.change, file.sha256.as_deref(), shared.text.as_deref()) {
            (ProposedChange::Deleted, _, None) => tombstone,
            (ProposedChange::Deleted, _, Some(_)) => false,
            (_, Some(digest), Some(text)) => !tombstone && is_text_digest(text, digest),
            _ => false,
        };
        at_her_key && same_text
    });
    if held_hers {
        SharedFate::Merged
    } else {
        SharedFate::Ended
    }
}

/// Whether `digest` names `text` as stored (LF) or as a share stages it for a
/// wholly CRLF team file.
fn is_text_digest(text: &str, digest: &str) -> bool {
    crate::engine::sha256_hex(text.as_bytes()) == digest
        || crate::engine::sha256_hex(
            crystalline_core::with_line_endings_of(text, b"\r\n").as_bytes(),
        ) == digest
}

/// The key of the base snapshot that holds the team's text for her row at
/// `path` once a proposal of hers merged, which is where her start now is.
///
/// Her own path when the snapshot holds it. A share does not carry a case-only
/// change of spelling, so her case move from `plan.md` to `Plan.md` lands at
/// `plan.md`, and `Plan.md` is no key on either disk: then the key is where she
/// started, when it is that spelling of her path. On a disk that folds case the
/// folder holds one file for the two spellings, so the one key spelled so
/// answers. Anywhere else her path, which the snapshot does not hold and which
/// reads as the team having deleted it: a refusal, never a guess.
pub fn merged_key<'a>(
    path: &'a str,
    from: Option<&'a str>,
    files: &'a BTreeMap<String, BaseStamp>,
    folds_case: bool,
) -> &'a str {
    if files.contains_key(path) {
        return path;
    }
    let folded = crystalline_core::fold_path_case(path);
    let spelled_so = |key: &str| crystalline_core::fold_path_case(key) == folded;
    if let Some(from) = from.filter(|from| spelled_so(from) && files.contains_key(*from)) {
        return from;
    }
    if folds_case {
        let mut keys = files.keys().filter(|key| spelled_so(key));
        if let (Some(key), None) = (keys.next(), keys.next()) {
            return key;
        }
    }
    path
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
    use super::TeamText::{Absent, Held};

    const NO_COPY: TeamText<'static> = TeamText::Unreadable { sha256: "0000" };
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
        disk: Disk,
    }

    /// Which disk kinds a row stands for. The function is pure, so a disk shows
    /// only in which keys the snapshot holds and in `folds_case`; a row the two
    /// answer alike runs on both.
    #[derive(Clone, Copy)]
    enum Disk {
        Either,
        Folding,
        CaseSensitive,
    }

    impl Disk {
        fn folds(self) -> &'static [bool] {
            match self {
                Disk::Either => &[true, false],
                Disk::Folding => &[true],
                Disk::CaseSensitive => &[false],
            }
        }
    }

    impl Row {
        fn on(mut self, disk: Disk) -> Self {
            self.disk = disk;
            self
        }

        fn sides(&self, folds_case: bool) -> DraftSides<'_> {
            DraftSides {
                path: self.path,
                from: self.from,
                start: self.start,
                start_sha256: None,
                shared: None,
                local: self.local,
                team_at_path: self.at_path,
                team_at_from: self.at_from,
                folds_case,
                merging: false,
            }
        }
    }

    /// The path a conflict's team side is read at follows the decision: rule 3's
    /// file of theirs, a start at the path itself or at nothing, and her own share
    /// at the path read as her start.
    #[test]
    fn answers_at_path_reads_the_sides_as_next_step_does() {
        let side = |from, local, shared, at_path, at_from| DraftSides {
            path: "renamed.md",
            from,
            start: Some("start\n"),
            start_sha256: None,
            shared,
            folds_case: false,
            merging: false,
            local,
            team_at_path: at_path,
            team_at_from: at_from,
        };
        let held = |text: &'static str| TeamText::Held(text.as_bytes());
        let mine = Some("mine\n");
        let cases = [
            // A file of the team's at her path: there.
            (
                side(Some("plan.md"), mine, None, held("bob\n"), held("start\n")),
                true,
            ),
            // Her own text at her path, the start elsewhere: where she started.
            (
                side(Some("plan.md"), mine, None, held("mine\n"), held("bob\n")),
                false,
            ),
            (
                side(Some("plan.md"), mine, None, TeamText::Absent, held("bob\n")),
                false,
            ),
            // A new file, and a start at the path itself: there.
            (
                side(None, mine, None, TeamText::Absent, TeamText::Absent),
                true,
            ),
            (
                side(Some("renamed.md"), mine, None, held("bob\n"), held("bob\n")),
                true,
            ),
            // Her share stands at her path and the team let go of where she
            // started: her share is her start, at the path.
            (
                side(
                    Some("plan.md"),
                    mine,
                    mine,
                    held("mine\n"),
                    TeamText::Absent,
                ),
                true,
            ),
        ];
        for (i, (sides, want)) in cases.iter().enumerate() {
            assert_eq!(answers_at_path(sides), *want, "case {i}");
        }
    }

    fn check(rows: Vec<Row>) {
        let mut wrong = Vec::new();
        for row in &rows {
            for &folds in row.disk.folds() {
                let step = next_step(&row.sides(folds));
                let landed = has_landed(&row.sides(folds));
                if step != row.step || landed != (row.step == DraftStep::Landed) {
                    wrong.push(format!(
                        "{} (folds case: {folds}): wanted {:?}, got {:?} (has_landed {landed})",
                        row.name, row.step, step
                    ));
                }
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
            start_sha256: None,
            shared: None,
            local,
            team_at_path,
            team_at_from,
            folds_case: false,
            merging: false,
        }
    }

    /// The same sides on a disk that folds case.
    fn folding(sides: DraftSides<'_>) -> DraftSides<'_> {
        DraftSides {
            folds_case: true,
            ..sides
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
            disk: Disk::Either,
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
            disk: Disk::Either,
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
            disk: Disk::Either,
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
            disk: Disk::Either,
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
            new_file("her new file landed", HERS, held(HERS), DraftStep::Landed),
            // Case move, edited, shared.
            case_moved(
                "[case-sensitive] edited case move landed at her spelling",
                HERS,
                held(HERS),
                Absent,
                DraftStep::Landed,
            )
            .on(Disk::CaseSensitive),
            case_moved(
                "[folding] edited case move landed at the team's spelling",
                HERS,
                Absent,
                held(HERS),
                DraftStep::Landed,
            )
            .on(Disk::Folding),
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
            )
            .on(Disk::CaseSensitive),
            case_moved(
                "[case-sensitive] same, her draft edited",
                HERS,
                held(NEW_FILE),
                held(START),
                conflict(AddAdd),
            )
            .on(Disk::CaseSensitive),
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
            )
            .on(Disk::Folding),
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
                take(HERS, false),
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

    /// A key with no readable copy, and what the struct allows and the record
    /// rules forbid: we cannot tell, and it reads so, ahead of every rule.
    #[test]
    fn what_cannot_be_told_is_unknown_start() {
        let unknown = || conflict(UnknownStart);
        check(vec![
            // A key with no readable copy is never "absent".
            own(
                "tombstone over a key with no copy",
                Some(START),
                None,
                NO_COPY,
                unknown(),
            ),
            own(
                "draft at a key with no copy",
                Some(START),
                Some(HERS),
                NO_COPY,
                unknown(),
            ),
            own(
                "draft equal to the start at a key with no copy",
                Some(START),
                Some(START),
                NO_COPY,
                unknown(),
            ),
            new_file("new file over a key with no copy", HERS, NO_COPY, unknown()),
            renamed(
                "no copy at the path, from in step",
                HERS,
                NO_COPY,
                held(START),
                unknown(),
            ),
            renamed(
                "no copy at from, from would be gone",
                HERS,
                Absent,
                NO_COPY,
                unknown(),
            ),
            renamed(
                "no copy at from, her text at the path",
                HERS,
                held(HERS),
                NO_COPY,
                unknown(),
            ),
            case_moved(
                "[folding] the snapshot holds both spellings, with one copy",
                HERS,
                NO_COPY,
                NO_COPY,
                unknown(),
            )
            .on(Disk::Folding),
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
                disk: Disk::Either,
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
                disk: Disk::Either,
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
                disk: Disk::Either,
            },
        ]);
    }

    /// A tombstone answers for the path it deletes, whatever a record says it
    /// started from.
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
                disk: Disk::Either,
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
                disk: Disk::Either,
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
                disk: Disk::Either,
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
        let row = folding(sides(
            "Plan.md",
            Some("plan.md"),
            Some(START),
            Some(HERS),
            Absent,
            Held(&team),
        ));
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
        let row = folding(sides(
            "Plan.md",
            Some("plan.md"),
            Some(START),
            Some(START),
            Absent,
            held(START),
        ));
        assert!(!has_landed(&row));
        assert_eq!(next_step(&row), DraftStep::InStep);
        // A CRLF team file is no change either.
        let team = crlf(START);
        let row = folding(sides(
            "Plan.md",
            Some("plan.md"),
            Some(START),
            Some(START),
            Absent,
            Held(&team),
        ));
        assert!(!has_landed(&row));
        assert_eq!(next_step(&row), DraftStep::InStep);
        // The move touched her `generated` line; nothing else changed.
        let mine = stamped(START, "2026-11-06T11:00:00Z");
        let row = folding(sides(
            "Plan.md",
            Some("plan.md"),
            Some(START),
            Some(&mine),
            Absent,
            held(START),
        ));
        assert!(!has_landed(&row));
        assert_eq!(next_step(&row), DraftStep::InStep);
    }

    /// A colleague's touch of the `generated` line, which is no one's words, is
    /// no change. It neither ends her case move nor moves her start.
    #[test]
    fn a_peers_touch_of_generated_does_not_end_an_unedited_case_move() {
        let team = stamped(START, "2026-11-06T12:00:00Z");
        for mine in [START.to_string(), stamped(START, "2026-11-06T11:00:00Z")] {
            for folds_case in [true, false] {
                let row = DraftSides {
                    folds_case,
                    ..sides(
                        "Plan.md",
                        Some("plan.md"),
                        Some(START),
                        Some(&mine),
                        Absent,
                        Held(team.as_bytes()),
                    )
                };
                assert!(!has_landed(&row));
                assert_eq!(next_step(&row), DraftStep::InStep);
            }
        }
    }

    /// An unedited case move whose share changed only her stamp is no change
    /// either, so it stays in step, as an unedited case move does, on both disks.
    #[test]
    fn a_case_move_whose_only_change_is_generated_stays_in_step_when_it_lands() {
        let mine = stamped(START, "2026-11-06T11:00:00Z");
        for folds_case in [true, false] {
            let row = DraftSides {
                folds_case,
                ..sides(
                    "Plan.md",
                    Some("plan.md"),
                    Some(START),
                    Some(&mine),
                    Absent,
                    Held(mine.as_bytes()),
                )
            };
            assert!(!has_landed(&row));
            assert_eq!(next_step(&row), DraftStep::InStep);
        }
    }

    #[test]
    fn an_edited_case_move_whose_text_stands_at_the_old_spelling_has_landed() {
        // [folding]: the share kept the team's spelling.
        let row = folding(sides(
            "Plan.md",
            Some("plan.md"),
            Some(START),
            Some(HERS),
            Absent,
            held(HERS),
        ));
        assert!(has_landed(&row));
        assert_eq!(next_step(&row), DraftStep::Landed);
        // A touched stamp since then changes nothing.
        let team = stamped(HERS, "2026-11-06T12:00:00Z");
        let row = folding(sides(
            "Plan.md",
            Some("plan.md"),
            Some(START),
            Some(HERS),
            Absent,
            Held(team.as_bytes()),
        ));
        assert!(has_landed(&row));
        // A spelling of another name is not a case move: this is a rename, and a
        // file holding her text at its old path has not landed at the new one.
        let row = folding(sides(
            "renamed.md",
            Some("plan.md"),
            Some(START),
            Some(HERS),
            Absent,
            held(HERS),
        ));
        assert!(!has_landed(&row));
        assert_eq!(next_step(&row), take(HERS, false));
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
        // His also touched the stamp: still nothing of his in the new folder.
        let his_stamped =
            "---\ntitle: Plan\ngenerated: x\n---\nsee [o](./other.md)\nbeta\ngamma\ndelta\n";
        let row = sides(
            "b/plan.md",
            Some("a/plan.md"),
            Some(start),
            Some(mine),
            Absent,
            Held(his_stamped.as_bytes()),
        );
        assert_eq!(next_step(&row), take(mine, false));
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

    /// A team change of only `generated` is no change. A
    /// tombstone against it is no conflict, an edit against it is in step and gets
    /// no notice, on both disks.
    #[test]
    fn a_team_change_of_only_generated_is_no_change() {
        let team = stamped(START, "2026-11-06T12:00:00Z");
        let team = Held(team.as_bytes());
        for folds_case in [true, false] {
            let at = |path, from, local| DraftSides {
                folds_case,
                ..sides(path, from, Some(START), local, team, team)
            };
            // Her deletion.
            assert_eq!(
                next_step(&at("plan.md", Some("plan.md"), None)),
                DraftStep::InStep
            );
            // Her edit at its own path, and moved.
            assert_eq!(
                next_step(&at("plan.md", Some("plan.md"), Some(HERS))),
                DraftStep::InStep
            );
            let moved = DraftSides {
                team_at_path: Absent,
                ..at("renamed.md", Some("plan.md"), Some(HERS))
            };
            assert_eq!(next_step(&moved), DraftStep::InStep);
        }
        // A real change with a stamp on it is still a change, and her edit merges.
        let his = stamped(HIS, "2026-11-06T12:00:00Z");
        let row = sides(
            "plan.md",
            Some("plan.md"),
            Some(START),
            Some(HERS),
            Held(his.as_bytes()),
            Held(his.as_bytes()),
        );
        assert_eq!(
            next_step(&row),
            take(&stamped(BOTH, "2026-11-06T12:00:00Z"), true)
        );
    }

    /// `edited` says whether her words changed, so a merge that
    /// leaves her text as it was sends no notice.
    #[test]
    fn a_merge_that_changes_nothing_of_hers_is_not_edited() {
        // The team made her edit too, with another stamp.
        let mine = stamped(HERS, "2026-11-06T11:00:00Z");
        let his = stamped(HERS, "2026-11-06T10:00:00Z");
        let row = sides(
            "renamed.md",
            Some("plan.md"),
            Some(START),
            Some(&mine),
            Absent,
            Held(his.as_bytes()),
        );
        assert_eq!(next_step(&row), take(&mine, false));
        // The other way round, the merge takes his newer stamp: her words are as
        // they were, so still no notice.
        let row = sides(
            "renamed.md",
            Some("plan.md"),
            Some(START),
            Some(&his),
            Absent,
            Held(mine.as_bytes()),
        );
        assert_eq!(next_step(&row), take(&mine, false));
    }

    /// Her text at her new path ends the move only when the team
    /// has let go of where she started, or, on a folding disk, keeps her case
    /// move's text at the old spelling. Otherwise her deletion of `from` is still
    /// to come and must not vanish with the draft.
    #[test]
    fn a_landed_text_ends_the_move_only_when_the_team_let_go_of_from() {
        let stamped_hers: &'static str =
            Box::leak(stamped(HERS, "2026-11-06T12:00:00Z").into_boxed_str());
        check(vec![
            renamed(
                "edited, a peer wrote her text at her path, from held at the start",
                HERS,
                held(HERS),
                held(START),
                DraftStep::InStep,
            ),
            renamed(
                "edited, his copy of her text with another stamp",
                HERS,
                held(stamped_hers),
                held(START),
                DraftStep::InStep,
            ),
            renamed(
                "unedited, the team holds her text at both paths",
                START,
                held(START),
                held(START),
                DraftStep::InStep,
            ),
            renamed(
                "edited, the team also changed from: merged in her coordinates",
                HERS,
                held(HERS),
                held(HIS),
                take(BOTH, true),
            ),
            renamed(
                "from gone: landed",
                HERS,
                held(HERS),
                Absent,
                DraftStep::Landed,
            ),
            case_moved(
                "[folding] a peer's copy at the old spelling is her own landing",
                HERS,
                Absent,
                held(HERS),
                DraftStep::Landed,
            )
            .on(Disk::Folding),
            case_moved(
                "[case-sensitive] a peer wrote her edit at plan.md, Plan.md absent",
                HERS,
                Absent,
                held(HERS),
                take(HERS, false),
            )
            .on(Disk::CaseSensitive),
            case_moved(
                "[case-sensitive] her landing there: her spelling held, the old one gone",
                HERS,
                held(HERS),
                Absent,
                DraftStep::Landed,
            )
            .on(Disk::CaseSensitive),
        ]);
    }

    const AGAIN: &str = "---\ntitle: Plan\nstatus: draft\n---\nalpha hers\nbeta\ngamma again\ndelta\nepsilon\nzeta\neta\n";

    /// Her own share, edited again before a pass got to it, reads
    /// her newer edit against what she shared, not as a conflict with herself.
    #[test]
    fn her_own_share_edited_again_after_a_failed_pass_is_in_step() {
        let shared = |row: DraftSides<'static>| DraftSides {
            shared: Some(HERS),
            ..row
        };
        for folds_case in [true, false] {
            let on = |row: DraftSides<'static>| DraftSides { folds_case, ..row };
            // A move shared directly, then edited again.
            let moved = shared(on(sides(
                "renamed.md",
                Some("plan.md"),
                Some(START),
                Some(AGAIN),
                held(HERS),
                Absent,
            )));
            assert!(!has_landed(&moved));
            assert_eq!(next_step(&moved), DraftStep::InStep);
            // The same at its own path.
            let in_place = shared(on(sides(
                "plan.md",
                Some("plan.md"),
                Some(START),
                Some(AGAIN),
                held(HERS),
                held(HERS),
            )));
            assert_eq!(next_step(&in_place), DraftStep::InStep);
            // The team's copy of her share carries another stamp: still hers.
            let stamped_share: &'static str =
                Box::leak(stamped(HERS, "2026-11-06T12:00:00Z").into_boxed_str());
            let in_place = DraftSides {
                team_at_path: held(stamped_share),
                team_at_from: held(stamped_share),
                ..in_place
            };
            assert_eq!(next_step(&in_place), DraftStep::InStep);
            // Unedited since the share: landed, as without the record of it.
            let landed = shared(on(sides(
                "plan.md",
                Some("plan.md"),
                Some(START),
                Some(HERS),
                held(HERS),
                held(HERS),
            )));
            assert_eq!(next_step(&landed), DraftStep::Landed);
        }
        // Without the record of her share these are the false conflicts.
        let moved = sides(
            "renamed.md",
            Some("plan.md"),
            Some(START),
            Some(AGAIN),
            held(HERS),
            Absent,
        );
        assert_eq!(next_step(&moved), conflict(AddAdd));
        // The team went on after her share: nothing says it holds her share, so
        // the start she had answers, and his line and her newer one both stay.
        let on_top = "---\ntitle: Plan\nstatus: draft\n---\nalpha hers\nbeta\ngamma\ndelta\nepsilon\nzeta\neta his\n";
        let row = shared(sides(
            "plan.md",
            Some("plan.md"),
            Some(START),
            Some(AGAIN),
            held(on_top),
            held(on_top),
        ));
        assert_eq!(
            next_step(&row),
            take(
                "---\ntitle: Plan\nstatus: draft\n---\nalpha hers\nbeta\ngamma again\ndelta\nepsilon\nzeta\neta his\n",
                true
            )
        );
        // A share of a move whose start the team still holds is not taken as
        // landed: her deletion of plan.md has not happened.
        let row = shared(sides(
            "renamed.md",
            Some("plan.md"),
            Some(START),
            Some(AGAIN),
            held(HERS),
            held(START),
        ));
        assert_ne!(next_step(&row), DraftStep::Landed);
        assert_ne!(next_step(&row), DraftStep::InStep);
    }

    /// Keep mine on a team file with no text writes the
    /// file's digest as the start, and that settles it: in step until the team's
    /// bytes change again.
    #[test]
    fn keep_mine_settles_a_team_file_with_no_text() {
        let not_text_sha = crate::engine::sha256_hex(NOT_TEXT);
        let his_sha = crate::engine::sha256_hex(HIS.as_bytes());
        let kept = |start_sha256, local, team| DraftSides {
            start_sha256: Some(start_sha256),
            ..sides("plan.md", Some("plan.md"), None, local, team, team)
        };
        for local in [Some(HERS), None] {
            // A key with no readable copy: the stamp's digest.
            assert_eq!(next_step(&kept("0000", local, NO_COPY)), DraftStep::InStep);
            let other = TeamText::Unreadable { sha256: "1111" };
            assert_eq!(
                next_step(&kept("0000", local, other)),
                conflict(UnknownStart)
            );
            // Bytes that are not UTF-8.
            assert_eq!(
                next_step(&kept(&not_text_sha, local, Held(NOT_TEXT))),
                DraftStep::InStep
            );
            // The copy came back, or the team wrote text: the digest still compares.
            assert_eq!(
                next_step(&kept(&his_sha, local, held(HIS))),
                DraftStep::InStep
            );
        }
        // Changed since she kept hers: the kinds as before.
        assert_eq!(
            next_step(&kept(&his_sha, Some(HERS), Held(NOT_TEXT))),
            conflict(EditEdit)
        );
        assert_eq!(
            next_step(&kept(&not_text_sha, None, Held(&[0xfe]))),
            conflict(DeleteEdit)
        );
        assert_eq!(
            next_step(&kept(&not_text_sha, Some(HERS), held(HIS))),
            conflict(EditEdit)
        );
        // Gone since: as for a text start.
        assert_eq!(
            next_step(&kept(&not_text_sha, Some(HERS), Absent)),
            conflict(EditDelete)
        );
        assert_eq!(
            next_step(&kept(&not_text_sha, None, Absent)),
            DraftStep::Landed
        );
        // Shapes no writer leaves: a text and a digest, or a digest for a new file.
        let both = DraftSides {
            start: Some(START),
            ..kept(&his_sha, Some(HERS), held(HIS))
        };
        assert_eq!(next_step(&both), conflict(UnknownStart));
        let new_file = DraftSides {
            from: None,
            team_at_from: Absent,
            team_at_path: Absent,
            ..kept(&his_sha, Some(HERS), Absent)
        };
        assert_eq!(next_step(&new_file), conflict(UnknownStart));
    }

    /// A key with no readable copy is never read as absent, also where
    /// she never saw the file.
    #[test]
    fn a_key_with_no_copy_is_never_absent() {
        let row = sides("plan.md", Some("plan.md"), None, None, NO_COPY, NO_COPY);
        assert!(!has_landed(&row));
        assert_eq!(next_step(&row), conflict(UnknownStart));
        let row = sides(
            "plan.md",
            Some("plan.md"),
            None,
            Some(HERS),
            NO_COPY,
            NO_COPY,
        );
        assert_eq!(next_step(&row), conflict(UnknownStart));
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
            NO_COPY,
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
        for (from, folds_case) in froms
            .into_iter()
            .flat_map(|from| [(from, true), (from, false)])
        {
            for start in [None, Some(START)] {
                for local in locals {
                    for &at_path in &teams {
                        for &at_from in &teams {
                            seen += 1;
                            let row = DraftSides {
                                folds_case,
                                ..sides("plan.md", from, start, local, at_path, at_from)
                            };
                            let context = format!("{row:?}");
                            let step = next_step(&row);
                            let landed = has_landed(&row);
                            assert_eq!(landed, step == DraftStep::Landed, "{context}");
                            let unreadable = matches!(at_path, TeamText::Unreadable { .. })
                                || matches!(at_from, TeamText::Unreadable { .. });
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
                                        at_path == Absent
                                            || from == Some("plan.md")
                                            || path_text.is_some() && path_text.as_deref() == local,
                                        "{context}"
                                    );
                                    assert_eq!(from_text.as_deref(), start, "{context}");
                                }
                                DraftStep::Landed => match local {
                                    None => assert_eq!(at_path, Absent, "{context}"),
                                    // Her text stands where she put it and the team
                                    // let go of where she started, or a folding
                                    // disk kept the old spelling for it.
                                    Some(local) => assert!(
                                        path_text.as_deref() == Some(local)
                                            && (at_from == Absent
                                                || from.is_none()
                                                || from == Some("plan.md"))
                                            || folds_case
                                                && at_path == Absent
                                                && from_text.as_deref() == Some(local),
                                        "{context}"
                                    ),
                                },
                                DraftStep::Take { text, edited } => {
                                    let local = local.expect("a tombstone never takes");
                                    let team =
                                        from_text.as_deref().expect("a take has a team text");
                                    assert!(!text.contains('\r'), "{context}");
                                    assert!(!text.contains('\u{fffd}'), "{context}");
                                    // No notice when her words stayed as they were.
                                    assert!(
                                        !*edited || !same_but_generated(text, local),
                                        "{context}"
                                    );
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
        assert_eq!(seen, 4 * 2 * 2 * 6 * 11 * 11);
    }

    fn proposal(number: u64, status: ProposalStatus) -> crystalline_remote::state::Proposal {
        crystalline_remote::state::Proposal {
            number,
            url: String::new(),
            branch: format!("share/{number}"),
            title: String::new(),
            created_at: chrono::Utc::now(),
            status,
            files: vec![crystalline_remote::state::ProposedFile {
                path: "plan.md".to_string(),
                change: ProposedChange::Modified,
                sha256: Some(crate::engine::sha256_hex(HERS.as_bytes())),
                blob_sha: None,
                size: None,
            }],
            head_commit: None,
            pending_head_commit: None,
            base_commit: None,
            review_state: None,
            feedback: Vec::new(),
            updated_at: None,
            author_login: None,
        }
    }

    fn sharing(proposal: Option<u64>) -> DraftStart {
        DraftStart {
            path: "plan.md".to_string(),
            from: Some("plan.md".to_string()),
            start: Some(START.to_string()),
            start_sha256: None,
            shared: proposal.map(|proposal| crate::draft_start::SharedText {
                proposal,
                text: Some(HERS.to_string()),
            }),
            merged_at: None,
            merging: None,
        }
    }

    /// Only history decides a merge; the chain, in any status, leaves the
    /// record be; anything else ends what the proposal held.
    #[test]
    fn settle_shared_reads_history_and_never_the_chain() {
        use ProposalStatus::{Declined, Merged, Open, Withdrawn};
        let state = |chain: Vec<(u64, ProposalStatus)>, history: Vec<(u64, ProposalStatus)>| {
            let mut state = OriginState::new("acme/team", "main");
            state.proposals = chain.into_iter().map(|(n, s)| proposal(n, s)).collect();
            state.history = history.into_iter().map(|(n, s)| proposal(n, s)).collect();
            state
        };
        let record = sharing(Some(7));
        assert_eq!(
            settle_shared(&sharing(None), false, &state(vec![], vec![(7, Merged)])),
            SharedFate::Open,
            "a record that names no proposal"
        );
        for status in [Open, Merged, Declined, Withdrawn] {
            assert_eq!(
                settle_shared(&record, false, &state(vec![(7, status)], vec![(7, Merged)])),
                SharedFate::Open,
                "{status:?} in the chain"
            );
        }
        assert_eq!(
            settle_shared(
                &record,
                false,
                &state(vec![(8, Open)], vec![(6, Withdrawn), (7, Merged)])
            ),
            SharedFate::Merged
        );
        for status in [Open, Declined, Withdrawn] {
            assert_eq!(
                settle_shared(&record, false, &state(vec![], vec![(7, status)])),
                SharedFate::Ended,
                "{status:?} in history"
            );
        }
        assert_eq!(
            settle_shared(&record, false, &state(vec![], vec![(8, Merged)])),
            SharedFate::Ended,
            "in neither list"
        );
    }

    /// A merge settles her row only where the merged proposal's own record still
    /// lists it with her text, or as a deletion for a tombstone.
    #[test]
    fn settle_shared_reads_a_merge_only_where_the_proposal_held_her_row() {
        let file =
            |path: &str, change, text: Option<&str>| crystalline_remote::state::ProposedFile {
                path: path.to_string(),
                change,
                sha256: text.map(|text| crate::engine::sha256_hex(text.as_bytes())),
                blob_sha: None,
                size: None,
            };
        let merged = |files| {
            let mut state = OriginState::new("acme/team", "main");
            let mut held = proposal(7, ProposalStatus::Merged);
            held.files = files;
            state.history = vec![held];
            state
        };
        let at = |path: &str, from: &str, text: Option<&str>| DraftStart {
            path: path.to_string(),
            from: Some(from.to_string()),
            shared: Some(crate::draft_start::SharedText {
                proposal: 7,
                text: text.map(str::to_string),
            }),
            ..sharing(None)
        };
        use ProposedChange::{Added, Deleted, Modified};
        let draft = at("plan.md", "plan.md", Some(HERS));
        let fate = |record: &DraftStart, tombstone, files| {
            settle_shared(record, tombstone, &merged(files))
        };
        assert_eq!(
            fate(&draft, false, vec![file("plan.md", Modified, Some(HERS))]),
            SharedFate::Merged
        );
        let crlf = String::from_utf8(crlf(HERS)).unwrap();
        assert_eq!(
            fate(&draft, false, vec![file("plan.md", Modified, Some(&crlf))]),
            SharedFate::Merged,
            "CRLF"
        );
        assert_eq!(
            fate(&draft, false, vec![]),
            SharedFate::Ended,
            "rewritten without her row"
        );
        assert_eq!(
            fate(&draft, false, vec![file("other.md", Added, Some(HERS))]),
            SharedFate::Ended
        );
        assert_eq!(
            fate(&draft, false, vec![file("plan.md", Modified, Some(HIS))]),
            SharedFate::Ended,
            "another text"
        );
        assert_eq!(
            fate(&draft, false, vec![file("plan.md", Deleted, None)]),
            SharedFate::Ended
        );
        let case_move = at("Plan.md", "plan.md", Some(HERS));
        assert_eq!(
            fate(
                &case_move,
                false,
                vec![file("plan.md", Modified, Some(HERS))]
            ),
            SharedFate::Merged
        );
        let moved = at("renamed.md", "plan.md", Some(HERS));
        assert_eq!(
            fate(&moved, false, vec![file("plan.md", Modified, Some(HERS))]),
            SharedFate::Ended,
            "a move is not her key"
        );
        let deletion = at("plan.md", "plan.md", None);
        assert_eq!(
            fate(&deletion, true, vec![file("plan.md", Deleted, None)]),
            SharedFate::Merged
        );
        assert_eq!(
            fate(&deletion, true, vec![file("plan.md", Modified, Some(HERS))]),
            SharedFate::Ended
        );
        assert_eq!(
            fate(&deletion, false, vec![file("plan.md", Deleted, None)]),
            SharedFate::Ended,
            "a draft is no deletion"
        );
    }

    /// Her path when held; else where her case move started, on either disk;
    /// else, only where the folder folds case, the one key spelled so; else her
    /// path.
    #[test]
    fn merged_key_is_never_a_spelling_the_snapshot_does_not_hold() {
        let files = |keys: &[&str]| -> BTreeMap<String, BaseStamp> {
            keys.iter()
                .map(|key| {
                    let stamp = BaseStamp {
                        sha256: String::new(),
                        size: 0,
                    };
                    (key.to_string(), stamp)
                })
                .collect()
        };
        for folds in [true, false] {
            let held = files(&["Plan.md", "plan.md"]);
            assert_eq!(
                merged_key("Plan.md", Some("plan.md"), &held, folds),
                "Plan.md"
            );
            let held = files(&["plan.md"]);
            assert_eq!(
                merged_key("Plan.md", Some("plan.md"), &held, folds),
                "plan.md"
            );
            let held = files(&["other.md"]);
            assert_eq!(
                merged_key("Plan.md", Some("plan.md"), &held, folds),
                "Plan.md"
            );
            // A move to another name is no spelling of where it started.
            let held = files(&["plan.md"]);
            assert_eq!(
                merged_key("renamed.md", Some("plan.md"), &held, folds),
                "renamed.md"
            );
        }
        let held = files(&["PLAN.md"]);
        assert_eq!(merged_key("Plan.md", None, &held, true), "PLAN.md");
        assert_eq!(merged_key("Plan.md", None, &held, false), "Plan.md");
        assert_eq!(
            merged_key("Plan.md", Some("plan.md"), &held, true),
            "PLAN.md"
        );
        let held = files(&["PLAN.md", "plan.md"]);
        assert_eq!(
            merged_key("Plan.md", None, &held, true),
            "Plan.md",
            "two spellings are no one file"
        );
    }
}
