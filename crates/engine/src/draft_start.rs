//! Where each draft started: one small record per overlay row.
//!
//! A draft on a stale base must not overwrite a colleague's work, and telling
//! whether a base is stale takes knowing what the draft started from. A row
//! cannot say: a rebuild restores rows from the journal, and a tombstone's
//! mirror holds no text at all. So each row gets a record of its own beside the
//! journal, under the state directory, which also keeps every store backend the
//! same:
//!
//! ```text
//! <state_dir>/overlays/<domain>/<actor>/.draft-starts/<sha256 of path>.json
//! ```
//!
//! **The name is a hash of the exact path.** `note.md` and `Note.md` are two
//! rows and two records on every disk, where a sidecar beside the mirror would
//! be one file on a disk that folds case. The record's `path` field guards the
//! hash: a record that does not name the asked path is a broken record.
//!
//! **Missing and broken are two answers.** A missing file is `Ok(None)`, which
//! is an old draft or a draft from another machine, and a caller reads that as
//! "we cannot tell". A file that is not a record, an empty one after a power loss
//! among them, or a record of another path is an error of kind `InvalidData`. A
//! gate refuses on it, so a corrupt record can never read as a clean new file.
//!
//! **The journal walk never reads one as a draft.** It keeps only `.md` and
//! `.md.tombstone`, and a record ends in `.json`. Removing an actor or a domain
//! takes the records with the folder, and a domain rename moves them with it.
//!
//! Every name is screened before a path is built: `domain` and `actor` by
//! `overlay_journal::actor_dir`, which holds each to one contained segment, and
//! the row's path by `overlay_journal::entry_path`. A hash could hold any path
//! safely, but a row the journal would refuse has no business having a record.

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::engine::sha256_hex;
use crate::overlay_journal::{actor_dir, entry_path, prune_empty, remove_if_present};

/// The folder inside an actor's journal folder the records stand in. A leading
/// dot only by convention: what keeps a record from being read as a draft is its
/// `.json` ending.
const STARTS_DIR: &str = ".draft-starts";

/// The ending of a record's file name.
const RECORD_SUFFIX: &str = ".json";

/// How long [`save`] waits before each extra try. Short, since it can run on an
/// async worker, but long enough for a scanner to let go of a file on Windows.
/// The wait is outside the journal's lock, which each try takes for itself.
const RETRY_WAITS: [Duration; 2] = [Duration::from_millis(10), Duration::from_millis(50)];

/// Where one overlay row started.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DraftStart {
    /// The row's exact path. Guards the hash in the file name.
    pub path: String,
    /// The team path the row started from. `None` is a new file.
    pub from: Option<String>,
    /// The LF text at `from` when last in step. `None` is absent there, unless
    /// `start_sha256` says what stood there.
    pub start: Option<String>,
    /// The sha-256 of the team's bytes at `from` when last in step, for a start
    /// that has no text: bytes that are not UTF-8, or a key of the snapshot with no
    /// readable copy, which Keep mine has to be able to settle. `start` is then
    /// `None`. Absent from every record written before it, which serde reads as
    /// `None`, as it does any missing `Option`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_sha256: Option<String>,
    /// What the open proposal holds.
    pub shared: Option<SharedText>,
    /// Set when a merge changed the row. Drives the notice.
    pub merged_at: Option<DateTime<Utc>>,
}

/// What an open proposal holds of one row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedText {
    /// The proposal's number.
    pub proposal: u64,
    /// Its text. `None` is the proposal deleting the file.
    pub text: Option<String>,
}

/// The folder one actor's records stand in.
fn starts_dir(state_dir: &Path, domain: &str, actor: &str) -> io::Result<PathBuf> {
    Ok(actor_dir(state_dir, domain, actor)?.join(STARTS_DIR))
}

/// The file the record of `path` stands in, the bytes of the path hashed as they
/// are: no case folding and no Unicode normalizing, since each spelling is a row
/// of its own. Public for a test that has to stand something in the record's
/// way; nothing else outside this module should know where a record lives.
#[doc(hidden)]
pub fn record_file(state_dir: &Path, domain: &str, actor: &str, path: &str) -> io::Result<PathBuf> {
    entry_path(state_dir, domain, actor, path)?;
    let name = format!("{}{RECORD_SUFFIX}", sha256_hex(path.as_bytes()));
    Ok(starts_dir(state_dir, domain, actor)?.join(name))
}

/// The error of a file that does not hold the record it should.
fn broken(file: &Path, why: impl std::fmt::Display) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("the draft start record {} is broken: {why}", file.display()),
    )
}

/// Whether a failed call says the file is not there: nothing at the name, or a
/// file standing where one of its folders would be, which holds no record either.
fn no_such_file(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
    )
}

/// Read one record file. Only a file that is not there answers `None`; a file
/// that is not a record is [`io::ErrorKind::InvalidData`].
fn read_record(file: &Path) -> io::Result<Option<DraftStart>> {
    let bytes = match std::fs::read(file) {
        Ok(bytes) => bytes,
        Err(e) if no_such_file(&e) => return Ok(None),
        Err(e) => return Err(e),
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|e| broken(file, e))
}

/// Refuse a file standing at the actor's folder or at the records' own, which
/// reads as "no record" to a plain open on every platform but has lost whatever
/// records stood there, so it is a broken record and not a missing one. A file
/// higher up is a broken journal, which holds nothing of this actor's at all and
/// which a write only warns about.
fn no_file_in_a_folders_place(state_dir: &Path, domain: &str, actor: &str) -> io::Result<()> {
    for dir in [
        actor_dir(state_dir, domain, actor)?,
        starts_dir(state_dir, domain, actor)?,
    ] {
        if std::fs::metadata(&dir).is_ok_and(|found| !found.is_dir()) {
            return Err(broken(&dir, "a file stands where the folder belongs"));
        }
    }
    Ok(())
}

/// The record of one row. `Ok(None)` means no file, and only that: broken JSON,
/// an empty file, a record that names another path or a file where the records'
/// folder belongs is an error of kind [`io::ErrorKind::InvalidData`], and any other
/// failed read is an error too.
pub fn load(
    state_dir: &Path,
    domain: &str,
    actor: &str,
    path: &str,
) -> io::Result<Option<DraftStart>> {
    let file = record_file(state_dir, domain, actor, path)?;
    match read_record(&file)? {
        Some(record) if record.path != path => Err(broken(&file, "it names another path")),
        Some(record) => Ok(Some(record)),
        None => no_file_in_a_folders_place(state_dir, domain, actor).map(|()| None),
    }
}

/// Run `attempt`, and on any error run it up to two more times, answering the
/// last error.
///
/// Any error, because the kind says nothing: `overlay_journal::save` wraps the
/// error so its kind is lost.
fn with_retries(mut attempt: impl FnMut() -> io::Result<()>) -> io::Result<()> {
    for wait in RETRY_WAITS {
        match attempt() {
            Ok(()) => return Ok(()),
            Err(e) => tracing::debug!("a draft start record is tried again: {e}"),
        }
        std::thread::sleep(wait);
    }
    attempt()
}

/// Write one row's record, atomically through `overlay_journal::save`.
///
/// A prune in this process cannot take the folder from under it: the journal's
/// lock holds them apart. It still tries up to three times, for the two things
/// that lock cannot reach, because a failure here refuses a person's edit: a file
/// a scanner holds open on Windows, where the rename answers `PermissionDenied`
/// for a moment, and a second process pruning this actor's folder, which a
/// one-shot command with its own `--db` or `--config` beside a daemon can be.
pub fn save(state_dir: &Path, domain: &str, actor: &str, start: &DraftStart) -> io::Result<()> {
    let file = record_file(state_dir, domain, actor, &start.path)?;
    let bytes = serde_json::to_vec_pretty(start).map_err(io::Error::other)?;
    with_retries(|| crate::overlay_journal::save(&file, &bytes))
}

/// Drop one row's record, and any folder that leaves empty, up to and including
/// the actor's own, by the rule `overlay_journal::journal_clear` follows. A
/// record that is not there is no error.
///
/// The plain removal is right here where the journal needs a case-aware one:
/// the name is a lowercase hash, so no other spelling of it is another record.
pub fn clear(state_dir: &Path, domain: &str, actor: &str, path: &str) -> io::Result<()> {
    let file = record_file(state_dir, domain, actor, path)?;
    match remove_if_present(&file) {
        Err(e) if !no_such_file(&e) => return Err(e),
        _ => {}
    }
    let stop = actor_dir(state_dir, domain, actor)?;
    prune_empty(file.parent(), &stop);
    Ok(())
}

/// Every record one actor holds in a domain, ordered by path.
///
/// This folder only, not `files/` below it. A broken record, or one whose path
/// does not hash to its file name, is an error of kind
/// [`io::ErrorKind::InvalidData`] and not a record left out, as with [`load`]. A
/// file not named like a record is passed over in silence, the atomic write's
/// temporary sibling and a draft in a folder of this name among them.
pub fn all(state_dir: &Path, domain: &str, actor: &str) -> io::Result<Vec<DraftStart>> {
    let dir = starts_dir(state_dir, domain, actor)?;
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) => {
            no_file_in_a_folders_place(state_dir, domain, actor)?;
            return if no_such_file(&e) {
                Ok(Vec::new())
            } else {
                Err(e)
            };
        }
    };
    let mut records = Vec::new();
    for entry in entries {
        let entry = entry?;
        let file = entry.path();
        if !entry.file_name().to_string_lossy().ends_with(RECORD_SUFFIX) || !file.is_file() {
            continue;
        }
        // A record cleared since the folder was listed is simply gone.
        let Some(record) = read_record(&file)? else {
            continue;
        };
        match record_file(state_dir, domain, actor, &record.path) {
            Ok(expected) if expected == file => records.push(record),
            _ => return Err(broken(&file, "its name is not the hash of its path")),
        }
    }
    // `read_dir` order is unspecified, and callers act on the order.
    records.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overlay_journal::{
        JournalEntry, journal_actor_count, journal_clear, journal_counts, journal_entries,
        journal_remove_actor, journal_write,
    };

    #[cfg(unix)]
    const RACE_ROUNDS: usize = 3000;

    const DRAFT: &str =
        "---\ntype: engram\ntitle: Plan\npermalink: plan\nstatus: draft\n---\n\n# Plan\n";

    fn dir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn record(path: &str, start: &str) -> DraftStart {
        DraftStart {
            path: path.to_string(),
            from: Some(path.to_string()),
            start: Some(start.to_string()),
            start_sha256: None,
            shared: None,
            merged_at: None,
        }
    }

    /// Put bytes where the record of `path` is read from, as a crash, an older
    /// build or a hand edit would have left them.
    fn plant(state: &Path, path: &str, bytes: &[u8]) {
        let file = record_file(state, "team", "alice", path).unwrap();
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, bytes).unwrap();
    }

    /// The hash holds on either disk kind: the two spellings have two file
    /// names, so the answer does not depend on what the temp folder folds.
    #[test]
    fn a_record_round_trips_and_two_spellings_do_not_share_one() {
        let tmp = dir();
        let state = tmp.path();
        let lower = DraftStart {
            path: "note.md".to_string(),
            from: Some("old/note.md".to_string()),
            start: Some("the team's text\n".to_string()),
            start_sha256: None,
            shared: Some(SharedText {
                proposal: 42,
                text: Some("what the proposal holds\n".to_string()),
            }),
            merged_at: Some("2026-11-06T10:00:00Z".parse().unwrap()),
        };
        let upper = DraftStart {
            path: "Note.md".to_string(),
            from: None,
            start: None,
            start_sha256: None,
            shared: Some(SharedText {
                proposal: 43,
                text: None,
            }),
            merged_at: None,
        };

        save(state, "team", "alice", &lower).unwrap();
        save(state, "team", "alice", &upper).unwrap();

        assert_eq!(
            load(state, "team", "alice", "note.md").unwrap().as_ref(),
            Some(&lower)
        );
        assert_eq!(
            load(state, "team", "alice", "Note.md").unwrap().as_ref(),
            Some(&upper)
        );
        assert_eq!(
            all(state, "team", "alice").unwrap(),
            vec![upper.clone(), lower.clone()],
            "two records, ordered by path whatever the folder's order is"
        );

        // A second save replaces the record and leaves no temporary file.
        let replaced = record("note.md", "newer\n");
        save(state, "team", "alice", &replaced).unwrap();
        assert_eq!(
            load(state, "team", "alice", "note.md").unwrap().as_ref(),
            Some(&replaced)
        );
        let held = std::fs::read_dir(
            record_file(state, "team", "alice", "note.md")
                .unwrap()
                .parent()
                .unwrap(),
        )
        .unwrap()
        .count();
        assert_eq!(held, 2, "one file per record");

        // Clearing one spelling leaves the other.
        clear(state, "team", "alice", "note.md").unwrap();
        assert_eq!(load(state, "team", "alice", "note.md").unwrap(), None);
        assert_eq!(
            load(state, "team", "alice", "Note.md").unwrap().as_ref(),
            Some(&upper)
        );
    }

    /// A record is nothing the journal walk knows about, so no count and no
    /// restore grows by one, and an actor holding only a record holds no draft.
    #[test]
    fn a_record_is_not_a_draft_to_the_journal_walk() {
        let tmp = dir();
        let state = tmp.path();
        journal_write(state, "team", "alice", "notes/plan.md", DRAFT).unwrap();
        save(state, "team", "alice", &record("notes/plan.md", DRAFT)).unwrap();
        save(state, "team", "bob", &record("notes/plan.md", DRAFT)).unwrap();

        let read = journal_entries(state, "team");
        assert_eq!(
            read.entries,
            vec![JournalEntry {
                actor: "alice".to_string(),
                path: "notes/plan.md".to_string(),
                content: Some(DRAFT.to_string()),
            }]
        );
        assert!(!read.unreadable);

        let counts = journal_counts(state, "team");
        assert_eq!(counts.total, 1);
        assert_eq!(counts.per_actor.len(), 1, "{:?}", counts.per_actor);
        assert_eq!(counts.per_actor.get("alice"), Some(&1));
        assert!(!counts.unreadable);

        assert_eq!(journal_actor_count(state, "team", "alice"), (1, false));
        assert_eq!(journal_actor_count(state, "team", "bob"), (0, false));
    }

    #[test]
    fn clearing_the_last_record_prunes_the_folder_and_keeps_the_actor_folder_rule() {
        let tmp = dir();
        let state = tmp.path();
        let actor = actor_dir(state, "team", "alice").unwrap();
        let starts = actor.join(STARTS_DIR);

        // Nothing else held: the record's folder and the actor's folder go, the
        // domain's stays, as when a last draft is cleared.
        save(state, "team", "alice", &record("a.md", "a\n")).unwrap();
        save(state, "team", "alice", &record("b.md", "b\n")).unwrap();
        clear(state, "team", "alice", "a.md").unwrap();
        assert!(starts.is_dir(), "another record still stands");
        clear(state, "team", "alice", "b.md").unwrap();
        assert!(!actor.exists(), "an actor holding nothing leaves no folder");
        assert!(actor.parent().unwrap().is_dir(), "the domain folder stays");

        // A draft beside the record: its folder goes, the actor's stays.
        journal_write(state, "team", "alice", "plan.md", DRAFT).unwrap();
        save(state, "team", "alice", &record("plan.md", DRAFT)).unwrap();
        assert!(starts.is_dir());
        clear(state, "team", "alice", "plan.md").unwrap();
        assert!(!starts.exists(), "the empty record folder is pruned");
        assert!(actor.is_dir(), "the actor still holds a draft");
        assert_eq!(journal_actor_count(state, "team", "alice"), (1, false));

        // Clearing a record that is not there is no error.
        clear(state, "team", "alice", "plan.md").unwrap();
        clear(state, "team", "nobody", "plan.md").unwrap();
    }

    #[test]
    fn removing_an_actor_takes_the_records() {
        let tmp = dir();
        let state = tmp.path();
        journal_write(state, "team", "alice", "plan.md", DRAFT).unwrap();
        save(state, "team", "alice", &record("plan.md", DRAFT)).unwrap();
        save(state, "team", "alice", &record("gone.md", "gone\n")).unwrap();
        let bob = record("plan.md", DRAFT);
        save(state, "team", "bob", &bob).unwrap();

        assert_eq!(
            journal_remove_actor(state, "team", "alice").unwrap(),
            1,
            "only the draft is counted as a draft"
        );

        assert_eq!(all(state, "team", "alice").unwrap(), Vec::new());
        assert_eq!(load(state, "team", "alice", "plan.md").unwrap(), None);
        assert_eq!(all(state, "team", "bob").unwrap(), vec![bob]);
    }

    /// A later gate refuses on any error, so a corrupt record can never read as
    /// a clean new file. A power loss can leave an empty one.
    #[test]
    fn broken_json_is_an_error_and_never_a_missing_record() {
        let broken: [&[u8]; 5] = [
            b"",
            b"{ not json",
            b"[1, 2]",
            b"{\"from\": \"note.md\"}",
            b"\xff\xfe not text",
        ];
        for bytes in broken {
            let tmp = dir();
            let state = tmp.path();
            plant(state, "note.md", bytes);
            let good = record("good.md", "good\n");
            save(state, "team", "alice", &good).unwrap();

            assert_eq!(
                load(state, "team", "alice", "note.md").unwrap_err().kind(),
                io::ErrorKind::InvalidData
            );
            assert_eq!(
                all(state, "team", "alice").unwrap_err().kind(),
                io::ErrorKind::InvalidData,
                "a broken record is not skipped"
            );

            // A new record replaces it, and then all is well again.
            let fresh = record("note.md", "fresh\n");
            save(state, "team", "alice", &fresh).unwrap();
            assert_eq!(
                load(state, "team", "alice", "note.md").unwrap().as_ref(),
                Some(&fresh)
            );
            assert_eq!(all(state, "team", "alice").unwrap(), vec![good, fresh]);
        }
    }

    /// Only a file that is not there is an answer of "no record".
    #[test]
    fn a_missing_record_is_none() {
        let tmp = dir();
        let state = tmp.path();
        assert_eq!(load(state, "team", "alice", "note.md").unwrap(), None);
        assert_eq!(all(state, "team", "alice").unwrap(), Vec::new());

        save(state, "team", "alice", &record("other.md", "text\n")).unwrap();
        assert_eq!(load(state, "team", "alice", "note.md").unwrap(), None);
    }

    /// A journal folder broken by a file in its place holds no record, so an
    /// ordinary write with no record to keep is not refused for it. A save has
    /// something to lose, and says so.
    #[test]
    fn a_file_where_a_folder_belongs_holds_no_record() {
        let tmp = dir();
        let state = tmp.path();
        std::fs::create_dir_all(state.join("overlays")).unwrap();
        std::fs::write(state.join("overlays").join("team"), b"not a folder").unwrap();

        assert_eq!(load(state, "team", "alice", "note.md").unwrap(), None);
        clear(state, "team", "alice", "note.md").unwrap();
        save(state, "team", "alice", &record("note.md", "text\n")).unwrap_err();
    }

    /// A file at the records' own folder or at the actor's is a lost store of
    /// records, not a missing record, and a caller refuses on it. Above the actor's
    /// folder it is a broken journal, which holds nothing of this actor's.
    #[test]
    fn a_file_in_the_records_folders_place_is_an_error_to_load_and_all() {
        let broken = |state: &Path| {
            assert_eq!(
                load(state, "team", "alice", "note.md").unwrap_err().kind(),
                io::ErrorKind::InvalidData
            );
            assert_eq!(
                all(state, "team", "alice").unwrap_err().kind(),
                io::ErrorKind::InvalidData
            );
        };

        let tmp = dir();
        let starts = starts_dir(tmp.path(), "team", "alice").unwrap();
        std::fs::create_dir_all(starts.parent().unwrap()).unwrap();
        std::fs::write(&starts, b"not a folder").unwrap();
        broken(tmp.path());

        let tmp = dir();
        let actor = actor_dir(tmp.path(), "team", "alice").unwrap();
        std::fs::create_dir_all(actor.parent().unwrap()).unwrap();
        std::fs::write(&actor, b"not a folder").unwrap();
        broken(tmp.path());

        let tmp = dir();
        let domain = actor_dir(tmp.path(), "team", "alice")
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        std::fs::create_dir_all(domain.parent().unwrap()).unwrap();
        std::fs::write(&domain, b"not a folder").unwrap();
        assert_eq!(load(tmp.path(), "team", "alice", "note.md").unwrap(), None);
        assert_eq!(all(tmp.path(), "team", "alice").unwrap(), Vec::new());
    }

    #[test]
    fn a_record_refuses_an_actor_that_is_a_traversal() {
        let tmp = dir();
        let state = tmp.path();
        let start = record("note.md", "text\n");
        let refused = |got: io::Result<()>, who: &str| {
            let error = got.expect_err(who);
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput, "{who}: {error}");
        };

        // `C:` is refused by the colon rule, so this runs the same on any
        // platform; on Windows it would otherwise replace the path built so far.
        for actor in ["..", ".", "C:", "a/b", "a\\b", ""] {
            refused(save(state, "team", actor, &start), actor);
            refused(clear(state, "team", actor, "note.md"), actor);
            refused(load(state, "team", actor, "note.md").map(drop), actor);
            refused(all(state, "team", actor).map(drop), actor);
        }
        for domain in ["..", "C:"] {
            refused(save(state, domain, "alice", &start), domain);
            refused(clear(state, domain, "alice", "note.md"), domain);
            refused(load(state, domain, "alice", "note.md").map(drop), domain);
            refused(all(state, domain, "alice").map(drop), domain);
        }

        let touched = std::fs::read_dir(state).unwrap().count();
        assert_eq!(touched, 0, "a refused name creates nothing");
    }

    /// The name is a hash, so nothing but this screen would notice: a path the
    /// journal refuses gets no record either.
    #[test]
    fn a_record_refuses_a_path_the_journal_refuses() {
        let tmp = dir();
        let state = tmp.path();
        for path in ["../up.md", "a/../../up.md", "/abs.md", "", "note.txt"] {
            let start = record(path, "text\n");
            assert_eq!(
                save(state, "team", "alice", &start).unwrap_err().kind(),
                io::ErrorKind::InvalidInput,
                "{path}"
            );
            assert_eq!(
                load(state, "team", "alice", path).unwrap_err().kind(),
                io::ErrorKind::InvalidInput,
                "{path}"
            );
            assert_eq!(
                clear(state, "team", "alice", path).unwrap_err().kind(),
                io::ErrorKind::InvalidInput,
                "{path}"
            );
        }
        assert_eq!(std::fs::read_dir(state).unwrap().count(), 0);
    }

    #[test]
    fn a_record_of_another_path_is_an_error() {
        let tmp = dir();
        let state = tmp.path();
        plant(
            state,
            "b.md",
            &serde_json::to_vec(&record("a.md", "a\n")).unwrap(),
        );

        assert_eq!(
            load(state, "team", "alice", "b.md").unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(
            all(state, "team", "alice").unwrap_err().kind(),
            io::ErrorKind::InvalidData,
            "the name does not hash from the path it holds"
        );
    }

    /// A record whose name is the hash of its own path is still broken if no row
    /// could stand at that path.
    #[test]
    fn a_record_of_a_path_the_journal_refuses_is_an_error_to_all() {
        let tmp = dir();
        let state = tmp.path();
        let path = "../up.md";
        let starts = starts_dir(state, "team", "alice").unwrap();
        std::fs::create_dir_all(&starts).unwrap();
        std::fs::write(
            starts.join(format!("{}{RECORD_SUFFIX}", sha256_hex(path.as_bytes()))),
            serde_json::to_vec(&record(path, "text\n")).unwrap(),
        )
        .unwrap();

        assert_eq!(
            all(state, "team", "alice").unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn a_failed_try_is_tried_again() {
        let mut tries = 0;
        let got = with_retries(|| {
            tries += 1;
            if tries < 3 {
                Err(io::Error::other("the folder went away"))
            } else {
                Ok(())
            }
        });
        got.unwrap();
        assert_eq!(tries, 3, "two extra tries");
    }

    #[test]
    fn a_good_try_is_not_repeated() {
        let mut tries = 0;
        with_retries(|| {
            tries += 1;
            Ok(())
        })
        .unwrap();
        assert_eq!(tries, 1);
    }

    #[test]
    fn three_failed_tries_answer_the_last_error() {
        let mut tries = 0;
        let error = with_retries(|| {
            tries += 1;
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("try {tries}"),
            ))
        })
        .unwrap_err();
        assert_eq!(tries, 3, "the retry is bounded");
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(error.to_string(), "try 3");
    }

    /// A save that can never land still says so: a folder stands where the file
    /// has to go.
    #[test]
    fn a_save_that_cannot_land_is_an_error() {
        let tmp = dir();
        let state = tmp.path();
        let start = record("note.md", "text\n");
        let file = record_file(state, "team", "alice", "note.md").unwrap();
        std::fs::create_dir_all(&file).unwrap();

        save(state, "team", "alice", &start).unwrap_err();
    }

    /// Run `other` in a hot loop on a second thread while `round` runs
    /// [`RACE_ROUNDS`] times on this one, and answer the rounds that failed.
    /// Unix only: on Windows a hot loop of deletes meets its own pending deletes,
    /// which says nothing about these races.
    #[cfg(unix)]
    fn failures_beside(
        state: &Path,
        other: impl Fn(&Path) + Send + 'static,
        mut round: impl FnMut() -> io::Result<()>,
    ) -> Vec<io::Error> {
        use std::sync::atomic::{AtomicBool, Ordering};
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let thread = {
            let (state, stop) = (state.to_path_buf(), stop.clone());
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    other(&state);
                }
            })
        };
        let failed = (0..RACE_ROUNDS).filter_map(|_| round().err()).collect();
        stop.store(true, Ordering::Relaxed);
        thread.join().unwrap();
        failed
    }

    /// One try of a save, with the retry in [`save`] out of the way: it must not
    /// be what makes these tests green.
    #[cfg(unix)]
    fn save_one_try(state: &Path, start: &DraftStart) -> io::Result<()> {
        let file = record_file(state, "team", "alice", &start.path).unwrap();
        let bytes = serde_json::to_vec_pretty(start).unwrap();
        crate::overlay_journal::save(&file, &bytes)
    }

    /// Clearing the actor's last record prunes the folders while a save of
    /// another path is between making them and writing into them.
    #[cfg(unix)]
    #[test]
    fn a_save_is_never_hit_by_a_record_clear_pruning_the_folder() {
        let tmp = dir();
        let state = tmp.path();
        let start = record("note.md", "text\n");
        let failed = failures_beside(
            state,
            |state| {
                let _ = save_one_try(state, &record("sibling.md", "text\n"));
                let _ = clear(state, "team", "alice", "sibling.md");
            },
            || {
                let saved = save_one_try(state, &start);
                clear(state, "team", "alice", "note.md").unwrap();
                saved
            },
        );
        assert_eq!(failed.len(), 0, "{:?}", failed.first());
    }

    /// The journal prunes the actor's folder too, which `save` is making on its
    /// way to the record's folder.
    #[cfg(unix)]
    #[test]
    fn a_save_is_never_hit_by_the_journal_pruning_the_actor_folder() {
        let tmp = dir();
        let state = tmp.path();
        let start = record("note.md", "text\n");
        let failed = failures_beside(
            state,
            |state| {
                let _ = journal_write(state, "team", "alice", "draft.md", DRAFT);
                let _ = journal_clear(state, "team", "alice", "draft.md");
            },
            || {
                let saved = save_one_try(state, &start);
                clear(state, "team", "alice", "note.md").unwrap();
                saved
            },
        );
        assert_eq!(failed.len(), 0, "{:?}", failed.first());
    }

    /// The other way round: a record's clear must not refuse a person's draft by
    /// pruning the actor's folder under the journal's write.
    #[cfg(unix)]
    #[test]
    fn a_journal_write_is_never_hit_by_a_record_clear_pruning_the_actor_folder() {
        let tmp = dir();
        let state = tmp.path();
        let failed = failures_beside(
            state,
            |state| {
                let _ = save_one_try(state, &record("sibling.md", "text\n"));
                let _ = clear(state, "team", "alice", "sibling.md");
            },
            || {
                let written = journal_write(state, "team", "alice", "draft.md", DRAFT);
                journal_clear(state, "team", "alice", "draft.md").unwrap();
                written
            },
        );
        assert_eq!(failed.len(), 0, "{:?}", failed.first());
    }

    #[test]
    fn a_field_this_build_does_not_know_is_ignored_and_a_missing_one_is_none() {
        let tmp = dir();
        let state = tmp.path();
        plant(
            state,
            "note.md",
            br#"{"path": "note.md", "start": "text\n", "later": {"a": 1}}"#,
        );

        assert_eq!(
            load(state, "team", "alice", "note.md").unwrap(),
            Some(DraftStart {
                path: "note.md".to_string(),
                from: None,
                start: Some("text\n".to_string()),
                start_sha256: None,
                shared: None,
                merged_at: None,
            })
        );
    }

    /// A start with no text (Keep mine over bytes that are not UTF-8, or a key
    /// with no readable copy) keeps its digest. A record without one is written
    /// as before, so a record of this build and of the build before read alike.
    #[test]
    fn a_start_with_no_text_keeps_its_digest_and_a_text_start_is_written_as_before() {
        let tmp = dir();
        let state = tmp.path();
        let digest = DraftStart {
            start: None,
            start_sha256: Some("ab12".to_string()),
            ..record("bytes.md", "unused")
        };
        save(state, "team", "alice", &digest).unwrap();
        assert_eq!(
            load(state, "team", "alice", "bytes.md").unwrap(),
            Some(digest)
        );

        let text = record("text.md", "text\n");
        save(state, "team", "alice", &text).unwrap();
        let written =
            std::fs::read_to_string(record_file(state, "team", "alice", "text.md").unwrap())
                .unwrap();
        assert!(!written.contains("start_sha256"), "{written}");
        assert_eq!(load(state, "team", "alice", "text.md").unwrap(), Some(text));
    }

    /// The folder's order is the disk's, so enough names to make a lucky order
    /// out of the question.
    #[test]
    fn all_is_ordered_by_path() {
        let tmp = dir();
        let state = tmp.path();
        let paths: Vec<String> = (0..24).map(|n| format!("n{n:02}.md")).collect();
        for path in paths.iter().rev() {
            save(state, "team", "alice", &record(path, "text\n")).unwrap();
        }

        let held: Vec<String> = all(state, "team", "alice")
            .unwrap()
            .into_iter()
            .map(|record| record.path)
            .collect();
        assert_eq!(held, paths);
    }

    /// The records of attached files stand in `files/`, and one must never be
    /// read as a blank record here. A draft can stand in this folder too, since
    /// a person may name a folder anything, and a temporary sibling can be left
    /// by a crash.
    #[test]
    fn all_reads_this_folder_only() {
        let tmp = dir();
        let state = tmp.path();
        assert_eq!(all(state, "team", "alice").unwrap(), Vec::new());

        let good = record("note.md", "text\n");
        save(state, "team", "alice", &good).unwrap();
        let bytes = serde_json::to_vec(&record("nested.md", "text\n")).unwrap();

        let file = record_file(state, "team", "alice", "note.md").unwrap();
        let starts = file.parent().unwrap().to_path_buf();
        let nested = record_file(state, "team", "alice", "nested.md").unwrap();
        let name = nested.file_name().unwrap();
        std::fs::create_dir_all(starts.join("files")).unwrap();
        std::fs::write(starts.join("files").join(name), &bytes).unwrap();
        // A crash's temporary sibling of a record that was never renamed in.
        let mut tmp_name = name.to_os_string();
        tmp_name.push(".tmp.123");
        std::fs::write(starts.join(tmp_name), &bytes).unwrap();
        journal_write(state, "team", "alice", ".draft-starts/loose.md", DRAFT).unwrap();

        assert_eq!(all(state, "team", "alice").unwrap(), vec![good]);

        // A folder that still holds `files/` is not pruned from under it.
        clear(state, "team", "alice", "note.md").unwrap();
        assert!(starts.join("files").join(name).is_file());
    }
}
