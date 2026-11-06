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
//! hash: a record that does not name the asked path reads as no record.
//!
//! **No record means unknown, the safe side.** A missing file, a file that is
//! not a record and a record of another path all answer the same, and a caller
//! reads that as "we cannot tell", never as "nothing changed".
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

/// Where one overlay row started.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DraftStart {
    /// The row's exact path. Guards the hash in the file name.
    pub path: String,
    /// The team path the row started from. `None` is a new file.
    pub from: Option<String>,
    /// The LF text at `from` when last in step. `None` is absent there.
    pub start: Option<String>,
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
/// of its own.
fn record_file(state_dir: &Path, domain: &str, actor: &str, path: &str) -> io::Result<PathBuf> {
    entry_path(state_dir, domain, actor, path)?;
    let name = format!("{}{RECORD_SUFFIX}", sha256_hex(path.as_bytes()));
    Ok(starts_dir(state_dir, domain, actor)?.join(name))
}

/// Read one record file. A file that is not there, or is not a record, answers
/// `None`; only a read that failed for another reason is an error.
fn read_record(file: &Path, domain: &str, actor: &str) -> io::Result<Option<DraftStart>> {
    let bytes = match std::fs::read(file) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    match serde_json::from_slice(&bytes) {
        Ok(record) => Ok(Some(record)),
        Err(e) => {
            tracing::warn!(
                domain = domain,
                actor = actor,
                file = %file.display(),
                "a draft start record could not be read and counts as unknown: {e}"
            );
            Ok(None)
        }
    }
}

/// The record of one row, or `None` when there is none to go by: no file,
/// unreadable JSON (logged), or a record that names another path. All of them
/// read as "unknown".
pub fn load(
    state_dir: &Path,
    domain: &str,
    actor: &str,
    path: &str,
) -> io::Result<Option<DraftStart>> {
    let file = record_file(state_dir, domain, actor, path)?;
    let record = read_record(&file, domain, actor)?;
    Ok(record.filter(|record| {
        let same = record.path == path;
        if !same {
            tracing::warn!(
                domain = domain,
                actor = actor,
                file = %file.display(),
                "a draft start record names another path and counts as unknown"
            );
        }
        same
    }))
}

/// Write one row's record, atomically through `overlay_journal::save`.
pub fn save(state_dir: &Path, domain: &str, actor: &str, start: &DraftStart) -> io::Result<()> {
    let file = record_file(state_dir, domain, actor, &start.path)?;
    let bytes = serde_json::to_vec_pretty(start).map_err(io::Error::other)?;
    crate::overlay_journal::save(&file, &bytes)
}

/// Drop one row's record, and any folder that leaves empty, up to and including
/// the actor's own, by the rule `overlay_journal::journal_clear` follows. A
/// record that is not there is no error.
///
/// The plain removal is right here where the journal needs a case-aware one:
/// the name is a lowercase hash, so no other spelling of it is another record.
pub fn clear(state_dir: &Path, domain: &str, actor: &str, path: &str) -> io::Result<()> {
    let file = record_file(state_dir, domain, actor, path)?;
    remove_if_present(&file)?;
    let stop = actor_dir(state_dir, domain, actor)?;
    prune_empty(file.parent(), &stop);
    Ok(())
}

/// Every record one actor holds in a domain, ordered by path.
///
/// This folder only, not `files/` below it, and only what [`load`] would answer:
/// a broken record, or one whose path does not hash to its file name, is left
/// out. A file that is not named like a record is passed over in silence, the
/// atomic write's temporary sibling and a draft in a folder of this name among
/// them.
pub fn all(state_dir: &Path, domain: &str, actor: &str) -> io::Result<Vec<DraftStart>> {
    let dir = starts_dir(state_dir, domain, actor)?;
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut records = Vec::new();
    for entry in entries {
        let entry = entry?;
        let file = entry.path();
        if !entry.file_name().to_string_lossy().ends_with(RECORD_SUFFIX) || !file.is_file() {
            continue;
        }
        let Some(record) = read_record(&file, domain, actor)? else {
            continue;
        };
        match record_file(state_dir, domain, actor, &record.path) {
            Ok(expected) if expected == file => records.push(record),
            _ => tracing::warn!(
                domain = domain,
                actor = actor,
                file = %file.display(),
                "a draft start record names another path and counts as unknown"
            ),
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
        JournalEntry, journal_actor_count, journal_counts, journal_entries, journal_remove_actor,
        journal_write,
    };

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

    /// Unknown is the safe side, for every way a file can fail to be a record.
    #[test]
    fn broken_json_reads_as_no_record() {
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

            assert_eq!(load(state, "team", "alice", "note.md").unwrap(), None);
            assert_eq!(
                all(state, "team", "alice").unwrap(),
                vec![good],
                "a broken record does not hide a good one"
            );

            // And it does not stand in the way of a new one.
            let fresh = record("note.md", "fresh\n");
            save(state, "team", "alice", &fresh).unwrap();
            assert_eq!(
                load(state, "team", "alice", "note.md").unwrap(),
                Some(fresh)
            );
        }
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
    fn a_record_of_another_path_reads_as_no_record() {
        let tmp = dir();
        let state = tmp.path();
        plant(
            state,
            "b.md",
            &serde_json::to_vec(&record("a.md", "a\n")).unwrap(),
        );

        assert_eq!(load(state, "team", "alice", "b.md").unwrap(), None);
        assert_eq!(
            all(state, "team", "alice").unwrap(),
            Vec::new(),
            "all answers only what load would answer"
        );
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
                shared: None,
                merged_at: None,
            })
        );
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
