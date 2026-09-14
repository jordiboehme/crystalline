//! The overlay journal: the durable mirror of every actor's drafts.
//!
//! An overlay entry is one actor's private draft of a path in a shared domain,
//! and it lives as an `engram` row carrying that actor's key. That row is
//! primary data - nothing on disk says it exists - so it is the one class of
//! row a wipe-and-rebuild cannot restore from the files it walks. The journal
//! is the copy that makes the rebuild honest: every draft is mirrored under the
//! state directory as it is written, and a rebuilt index takes them back from
//! there.
//!
//! The layout is one file per draft:
//!
//! ```text
//! <state_dir>/overlays/<domain>/<actor>/<path>            a draft
//! <state_dir>/overlays/<domain>/<actor>/<path>.tombstone   that actor's deletion of the base row
//! ```
//!
//! `<path>` is the engram's own domain-relative path, so a draft of
//! `notes/plan.md` held by `alice` in domain `team` is
//! `<state_dir>/overlays/team/alice/notes/plan.md`. The tombstone is a sidecar
//! beside it rather than a file of its own name, and the two spellings can
//! never collide: every engram path ends in `.md` (the write verbs normalize
//! it, `Engine::normalize_md`), which is asserted here on the way in, so
//! `<path>.tombstone` is never itself a legal draft path.
//!
//! **A draft and a tombstone are mutually exclusive on disk.** An actor either
//! holds a replacement of a path or a deletion of it, never both, so
//! [`journal_write`] removes any tombstone beside what it writes and
//! [`journal_tombstone`] removes the draft. Neither pair of steps is atomic,
//! and the order of each is chosen so that a crash between them leaves the
//! state that reads as the older of the two intentions rather than an
//! ambiguous one: a write puts the draft down first, a tombstone takes the
//! draft away first. [`journal_entries`] resolves a pair it still finds in
//! favour of the draft and says so in the log.
//!
//! The journal is machine state, never a domain: it lives beside the origin
//! state directories under `<state_dir>`, it is never indexed, never synced and
//! never shared. It is also **per machine**: a draft mirrored here is
//! unreachable from another instance sharing the same database.
//!
//! Containment is asserted here and not delegated. `domain`, `actor` and
//! `path` all arrive from callers that have their own screens, and the actor
//! key in particular will later be an account name chosen by whoever holds the
//! account, so every one of the three is re-checked against
//! [`crate::engine::is_within_domain`] before a path is built - and `domain`
//! and `actor` must additionally be a single segment, since neither names a
//! tree.

use std::io;
use std::path::{Path, PathBuf};

use crystalline_core::{config, parse_engram};
use crystalline_index::{DomainId, EngramRecord, Store};

use crate::engine::{is_within_domain, virtual_stamp};

/// The folder under the state directory the journal lives in.
pub const JOURNAL_DIR: &str = "overlays";

/// The suffix marking a tombstone sidecar.
const TOMBSTONE_SUFFIX: &str = ".tombstone";

/// One mirrored overlay entry: whose it is, which path it stands at, and the
/// draft's markdown - or `None`, which is this actor's deletion of the base row
/// at that path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JournalEntry {
    /// The actor key whose draft this is. Never empty: the empty key is the
    /// base row's own, and the base is on disk.
    pub actor: String,
    /// The domain-relative path, forward-slashed, ending in `.md`.
    pub path: String,
    /// The draft's full markdown, or `None` for a tombstone.
    pub content: Option<String>,
}

/// The journal root for one domain, `<state_dir>/overlays/<domain>`.
fn domain_dir(state_dir: &Path, domain: &str) -> io::Result<PathBuf> {
    Ok(state_dir
        .join(JOURNAL_DIR)
        .join(one_segment(domain, "domain")?))
}

/// One actor's folder inside a domain's journal.
fn actor_dir(state_dir: &Path, domain: &str, actor: &str) -> io::Result<PathBuf> {
    Ok(domain_dir(state_dir, domain)?.join(one_segment(actor, "actor")?))
}

/// The file one actor's draft of `path` is mirrored in.
fn entry_path(state_dir: &Path, domain: &str, actor: &str, path: &str) -> io::Result<PathBuf> {
    if !is_within_domain(path) || !is_md(path) {
        return Err(refused("path", path));
    }
    let mut file = actor_dir(state_dir, domain, actor)?;
    for seg in path.split('/') {
        file.push(seg);
    }
    Ok(file)
}

/// A name that must be one contained segment: a domain and an actor each name
/// one folder, so a separator in either is a traversal whatever the segments
/// around it say.
fn one_segment<'a>(name: &'a str, what: &str) -> io::Result<&'a str> {
    if !is_within_domain(name) || name.contains('/') || name.contains('\\') {
        return Err(refused(what, name));
    }
    Ok(name)
}

/// Whether a path names a markdown file, the suffix every engram path carries.
fn is_md(path: &str) -> bool {
    path.to_lowercase().ends_with(".md")
}

fn refused(what: &str, value: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!(
            "the overlay journal refuses the {what} '{}': it must stay inside the journal folder",
            value.escape_debug()
        ),
    )
}

/// The tombstone sidecar beside a draft file.
fn tombstone_of(file: &Path) -> PathBuf {
    let mut name = file
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(TOMBSTONE_SUFFIX);
    file.with_file_name(name)
}

/// Write atomically through [`config::save_bytes`], so a reader never sees a
/// half-written draft and a crash never truncates one.
fn save(file: &Path, bytes: &[u8]) -> io::Result<()> {
    config::save_bytes(file, bytes).map_err(io::Error::other)
}

/// Remove a file that may not be there.
fn remove_if_present(file: &Path) -> io::Result<()> {
    match std::fs::remove_file(file) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Mirror one actor's draft of `path`.
///
/// The draft is written first and the tombstone beside it removed second, so a
/// crash in between leaves a draft and a tombstone - which
/// [`journal_entries`] resolves as the draft, the intention this call was
/// carrying out.
pub fn journal_write(
    state_dir: &Path,
    domain: &str,
    actor: &str,
    path: &str,
    content: &str,
) -> io::Result<()> {
    let file = entry_path(state_dir, domain, actor, path)?;
    save(&file, content.as_bytes())?;
    remove_if_present(&tombstone_of(&file))
}

/// Mirror one actor's deletion of the base row at `path`.
///
/// The draft is removed first and the sidecar written second: a crash in
/// between leaves neither, which reads as this actor holding nothing at the
/// path - behind the index by one write, rather than ambiguous or stale.
pub fn journal_tombstone(
    state_dir: &Path,
    domain: &str,
    actor: &str,
    path: &str,
) -> io::Result<()> {
    let file = entry_path(state_dir, domain, actor, path)?;
    remove_if_present(&file)?;
    save(&tombstone_of(&file), b"")
}

/// Drop whatever one actor holds at `path`, draft or tombstone.
///
/// The folders the entry stood in are pruned while they are empty, up to and
/// including the actor's own, so an actor who cleared their last draft leaves
/// no folder behind. The domain's own folder stays: it is the thing
/// [`journal_remove_domain`] answers about.
pub fn journal_clear(state_dir: &Path, domain: &str, actor: &str, path: &str) -> io::Result<()> {
    let file = entry_path(state_dir, domain, actor, path)?;
    remove_if_present(&tombstone_of(&file))?;
    remove_if_present(&file)?;
    let stop = actor_dir(state_dir, domain, actor)?;
    prune_empty(file.parent(), &stop);
    Ok(())
}

/// Remove every folder from `from` up to and including `stop` that is empty,
/// stopping at the first one that is not. Best effort: a folder that could not
/// be read or removed is left where it is.
fn prune_empty(from: Option<&Path>, stop: &Path) {
    let mut cur = from.map(Path::to_path_buf);
    while let Some(dir) = cur {
        if !dir.starts_with(stop) || std::fs::remove_dir(&dir).is_err() {
            return;
        }
        if dir == stop {
            return;
        }
        cur = dir.parent().map(Path::to_path_buf);
    }
}

/// Every draft mirrored for one domain, ordered by actor and then by path.
///
/// Answers with what it could read and never with an error: a journal that
/// cannot be read is a mirror that has nothing to say, and every caller here
/// either restores what it finds or counts it. Anything that is not a mirrored
/// entry is skipped - a file that does not end in `.md` or `.md.tombstone`
/// (the atomic write's temporary sibling among them) and a draft whose bytes
/// are not UTF-8.
pub fn journal_entries(state_dir: &Path, domain: &str) -> Vec<JournalEntry> {
    let Ok(dir) = domain_dir(state_dir, domain) else {
        return Vec::new();
    };
    let Ok(actors) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out: Vec<JournalEntry> = Vec::new();
    for actor in actors.flatten() {
        if !actor.path().is_dir() {
            continue;
        }
        let Some(name) = actor.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if one_segment(&name, "actor").is_err() {
            continue;
        }
        let mut found: Vec<JournalEntry> = Vec::new();
        collect(&actor.path(), "", &name, &mut found);
        out.extend(found);
    }
    // `read_dir` order is unspecified, and a restore that ran in a different
    // order per platform would be a different restore.
    out.sort_by(|a, b| (&a.actor, &a.path).cmp(&(&b.actor, &b.path)));
    // A draft and a tombstone at one path is the crash window the two writers
    // above document; the draft is the newer intention either way it happened.
    out.dedup_by(|b, a| {
        if a.actor == b.actor && a.path == b.path {
            tracing::debug!(
                actor = a.actor.as_str(),
                path = a.path.as_str(),
                "the overlay journal holds both a draft and a tombstone here; the draft is taken"
            );
            if a.content.is_none() {
                a.content = b.content.take();
            }
            true
        } else {
            false
        }
    });
    out
}

/// Walk one actor's folder, appending every mirrored entry under it.
fn collect(dir: &Path, prefix: &str, actor: &str, out: &mut Vec<JournalEntry>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let rel = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        let path = entry.path();
        if path.is_dir() {
            collect(&path, &rel, actor, out);
            continue;
        }
        let (rel, content) = match rel.strip_suffix(TOMBSTONE_SUFFIX) {
            Some(base) => (base.to_string(), None),
            None => match std::fs::read_to_string(&path) {
                Ok(text) => (rel, Some(text)),
                Err(_) => continue,
            },
        };
        if !is_within_domain(&rel) || !is_md(&rel) {
            continue;
        }
        out.push(JournalEntry {
            actor: actor.to_string(),
            path: rel,
            content,
        });
    }
}

/// Drop a whole domain's journal, answering with how many entries it held.
///
/// Called from both removal paths. A domain nobody registers must keep no
/// mirror: the rows and the copy that would bring them back go together, or
/// the next sync resurrects drafts for a domain that no longer exists.
pub fn journal_remove_domain(state_dir: &Path, domain: &str) -> io::Result<u64> {
    let dir = domain_dir(state_dir, domain)?;
    let held = journal_entries(state_dir, domain).len() as u64;
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => Ok(held),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(0),
        Err(e) => Err(e),
    }
}

/// Put every mirrored draft this domain holds back into the index, answering
/// with how many rows were written.
///
/// **Store rows win.** An actor already holding a row at a path keeps it: the
/// index is the live truth and the journal is the mirror, so a restore is only
/// ever allowed to fill a gap. That makes it safe to run on every sync, where
/// almost always it writes nothing at all.
///
/// A tombstone carries no content of its own, so its row is rebuilt from the
/// base row it deletes. A tombstone whose base is not there is skipped and its
/// entry left alone: there is nothing to shadow, and clearing it would be
/// convergence rather than restoration.
pub async fn restore_into(
    store: &dyn Store,
    state_dir: &Path,
    domain_name: &str,
    domain: DomainId,
) -> crystalline_index::Result<u64> {
    let mut restored = 0u64;
    for entry in journal_entries(state_dir, domain_name) {
        if store
            .overlay_entry(domain, &entry.actor, &entry.path)
            .await?
            .is_some()
        {
            continue;
        }
        let tombstone = entry.content.is_none();
        let text = match entry.content {
            Some(text) => text,
            None => match store.engram_content(domain, &entry.path).await? {
                Some(base) => base,
                None => continue,
            },
        };
        let engram = match parse_engram(&text) {
            Ok(engram) => engram,
            Err(e) => {
                tracing::warn!(
                    domain = domain_name,
                    actor = entry.actor.as_str(),
                    path = entry.path.as_str(),
                    "the mirrored draft could not be parsed and was left in the journal: {e}"
                );
                continue;
            }
        };
        let mut record = EngramRecord::from_engram(&engram, &entry.path, virtual_stamp(&text));
        record.tombstone = tombstone;
        store.upsert_overlay(domain, &entry.actor, &record).await?;
        restored += 1;
    }
    Ok(restored)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    const DRAFT: &str =
        "---\ntype: engram\ntitle: Plan\npermalink: plan\nstatus: draft\n---\n\n# Plan\n";

    /// The three writers and the reader, over one actor's two paths and a
    /// second actor's one, which is the shape every caller in this task uses:
    /// a draft is content, a tombstone is `None`, a clear leaves nothing, and
    /// the two forms never coexist at one path.
    #[test]
    fn journal_round_trips_write_tombstone_and_clear() {
        let tmp = dir();
        let state = tmp.path();

        journal_write(state, "team", "alice", "notes/plan.md", DRAFT).unwrap();
        journal_write(state, "team", "alice", "top.md", DRAFT).unwrap();
        journal_tombstone(state, "team", "bob", "notes/plan.md").unwrap();

        // The layout is the documented one, on disk.
        assert!(
            state.join("overlays/team/alice/notes/plan.md").is_file(),
            "a draft is mirrored at <state_dir>/overlays/<domain>/<actor>/<path>"
        );
        assert!(
            state
                .join("overlays/team/bob/notes/plan.md.tombstone")
                .is_file(),
            "a tombstone is a sidecar beside the path it deletes"
        );

        let entries = journal_entries(state, "team");
        assert_eq!(
            entries,
            vec![
                JournalEntry {
                    actor: "alice".to_string(),
                    path: "notes/plan.md".to_string(),
                    content: Some(DRAFT.to_string()),
                },
                JournalEntry {
                    actor: "alice".to_string(),
                    path: "top.md".to_string(),
                    content: Some(DRAFT.to_string()),
                },
                JournalEntry {
                    actor: "bob".to_string(),
                    path: "notes/plan.md".to_string(),
                    content: None,
                },
            ],
            "ordered by actor then path, a tombstone carrying no content"
        );

        // A tombstone over a draft replaces it rather than joining it.
        journal_tombstone(state, "team", "alice", "top.md").unwrap();
        assert!(!state.join("overlays/team/alice/top.md").exists());
        let entries = journal_entries(state, "team");
        assert_eq!(entries.len(), 3, "still one entry per (actor, path)");
        assert_eq!(entries[1].content, None, "alice's draft became a deletion");

        // And a draft over a tombstone replaces it the other way round.
        journal_write(state, "team", "alice", "top.md", DRAFT).unwrap();
        assert!(!state.join("overlays/team/alice/top.md.tombstone").exists());
        assert_eq!(
            journal_entries(state, "team")[1].content.as_deref(),
            Some(DRAFT)
        );

        // Clearing takes both forms and prunes the folders that held them.
        journal_clear(state, "team", "alice", "notes/plan.md").unwrap();
        journal_clear(state, "team", "bob", "notes/plan.md").unwrap();
        assert!(
            !state.join("overlays/team/bob").exists(),
            "an actor holding nothing leaves no folder behind"
        );
        assert_eq!(
            journal_entries(state, "team"),
            vec![JournalEntry {
                actor: "alice".to_string(),
                path: "top.md".to_string(),
                content: Some(DRAFT.to_string()),
            }],
            "only what was not cleared is left"
        );

        // The whole domain goes, count and all.
        assert_eq!(journal_remove_domain(state, "team").unwrap(), 1);
        assert!(journal_entries(state, "team").is_empty());
        assert_eq!(
            journal_remove_domain(state, "team").unwrap(),
            0,
            "a domain with no journal sweeps to nothing"
        );
    }

    /// Every one of the three names is screened, because every one of them
    /// becomes a path segment - and the actor key is the one a later wave
    /// feeds from an account name somebody else chose.
    #[test]
    fn journal_refuses_traversal() {
        let tmp = dir();
        let state = tmp.path();

        for (domain, actor, path) in [
            ("../escape", "alice", "plan.md"),
            ("team/nested", "alice", "plan.md"),
            ("team", "..", "plan.md"),
            ("team", "a/b", "plan.md"),
            ("team", "alice", "../escape.md"),
            ("team", "alice", "notes/../../escape.md"),
            ("team", "alice", "/absolute.md"),
            ("team", "alice", ""),
            // Not a traversal, but the invariant the sidecar rests on: an
            // engram path ends in `.md`, so `<path>.tombstone` is never a
            // legal draft path and the two spellings cannot collide.
            ("team", "alice", "plan.md.tombstone"),
        ] {
            let write = journal_write(state, domain, actor, path, DRAFT);
            assert!(
                write.is_err(),
                "journal_write took ({domain}, {actor}, {path})"
            );
            assert!(journal_tombstone(state, domain, actor, path).is_err());
            assert!(journal_clear(state, domain, actor, path).is_err());
        }
        assert!(
            !tmp.path().join("escape.md").exists()
                && !tmp.path().parent().unwrap().join("escape.md").exists(),
            "nothing was written outside the journal"
        );
        assert!(journal_remove_domain(state, "../escape").is_err());
        assert!(
            journal_entries(state, "../escape").is_empty(),
            "a refused domain reads as an empty journal rather than a folder above the root"
        );
    }
}
