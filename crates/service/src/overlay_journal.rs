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

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};

use crystalline_core::{config, parse_engram};
use crystalline_index::{ChunkParams, DomainId, EngramRecord, Store, chunk_engram};
use serde::{Deserialize, Serialize};

use crate::engine::{is_contained_rel, is_within_domain, virtual_stamp};

/// The folder under the state directory the journal lives in.
pub const JOURNAL_DIR: &str = "overlays";

/// The suffix marking a tombstone sidecar.
///
/// Shared with [`crate::overlay_files`], which marks a deletion in the files
/// overlay the same way: one spelling, so the two substrates can never disagree
/// about what a deletion looks like on disk.
pub(crate) const TOMBSTONE_SUFFIX: &str = ".tombstone";

/// The file one domain's convergence record lives in, inside that domain's
/// journal folder.
///
/// A leading dot so it can never be mistaken for an actor: the walk both
/// readers share takes only directories as actor folders, so a file here is
/// skipped whatever it is called, and the dot makes the name unreachable for a
/// sanitized login besides. Inside the domain folder rather than beside it, so
/// [`journal_remove_domain`]'s one `remove_dir_all` sweeps the record with the
/// drafts it describes - a record that outlived them would name conflicts in
/// drafts nobody holds any more.
const RECORD_FILE: &str = ".convergence.json";

/// What one domain's pulls have left unsettled, and whose open proposals are
/// whose.
///
/// Kept beside the drafts it is about and for the same reason they are: it is
/// primary data that nothing on disk otherwise says. A conflict is a fact about
/// one actor's draft standing against a base that moved under it, not about the
/// pull that happened to notice it, so it survives every later pull that is
/// about something else - and, because it is here rather than in memory, it
/// survives a restart too.
///
/// Per machine, exactly like the drafts: a conflict recorded here is
/// unreachable from another instance sharing the same database, which is the
/// same thing the journal's own module doc says about a mirrored draft.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConvergenceRecord {
    /// Every path still standing as a conflict, per actor, each list ordered
    /// and free of duplicates.
    #[serde(default)]
    pub conflicts: BTreeMap<String, Vec<String>>,
    /// How many entries the last convergence pass took out of the overlay.
    /// A fact about that pass alone, which is why it is a plain number beside
    /// the merged map rather than part of it.
    #[serde(default)]
    pub cleared: u64,
    /// Which overlay actor opened each open proposal, keyed by the proposal
    /// number as text.
    ///
    /// The forge record carries `author_login`, which is the GitHub account a
    /// share's credential was connected as and not the actor whose drafts it
    /// carried - in the default instance identity mode every actor's share goes
    /// out on one login. So whose a proposal is, in the sense review mode means
    /// it, is recorded here. A number this map does not name is somebody else's
    /// as far as anything can tell, which is the safe way round: it never tells
    /// one actor to withdraw a proposal that is not theirs.
    #[serde(default)]
    pub proposals: BTreeMap<String, String>,
}

impl ConvergenceRecord {
    /// Record that `actor` has a conflict at `path`, once.
    pub fn diverge(&mut self, actor: &str, path: &str) {
        let paths = self.conflicts.entry(actor.to_string()).or_default();
        if let Err(at) = paths.binary_search(&path.to_string()) {
            paths.insert(at, path.to_string());
        }
    }

    /// Take `path` out of `actor`'s conflicts, answering how many they have
    /// left. An actor with none left leaves no entry behind.
    pub fn settle(&mut self, actor: &str, path: &str) -> u64 {
        let Some(paths) = self.conflicts.get_mut(actor) else {
            return 0;
        };
        paths.retain(|held| held != path);
        let left = paths.len() as u64;
        if paths.is_empty() {
            self.conflicts.remove(actor);
        }
        left
    }

    /// Every conflict standing, across every actor.
    pub fn diverged(&self) -> u64 {
        self.conflicts
            .values()
            .map(|paths| paths.len() as u64)
            .sum()
    }
}

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
///
/// Shared with [`crate::overlay_files`], whose per-actor trees stand inside the
/// actor folders under it.
pub(crate) fn domain_dir(state_dir: &Path, domain: &str) -> io::Result<PathBuf> {
    Ok(state_dir
        .join(JOURNAL_DIR)
        .join(one_segment(domain, "domain")?))
}

/// One actor's folder inside a domain's journal.
///
/// The files overlay lives INSIDE this folder, under
/// [`crate::overlay_files::FILES_DIR`], which is why it is shared rather than
/// private: an actor's drafts and the files they wrote in review mode are one
/// actor's private work, so [`journal_remove_domain`]'s single
/// `remove_dir_all` takes both by construction rather than by a second sweep
/// somebody has to remember to call.
///
/// The two trees coexist because neither can hold the other's paths. The walk
/// below keeps only `.md` and `.md.tombstone`, and an attachment path can never
/// end in `.md` (`.md` is not on the attachment extension allowlist), so
/// nothing in `files/` is ever read back as a draft and nothing the journal
/// writes is ever served as an attachment.
pub(crate) fn actor_dir(state_dir: &Path, domain: &str, actor: &str) -> io::Result<PathBuf> {
    Ok(domain_dir(state_dir, domain)?.join(one_segment(actor, "actor")?))
}

/// The file one actor's draft of `path` is mirrored in.
///
/// Shared with [`crate::engine::Engine::draft_lock`], which keys a draft's
/// write lock on this path: the lock has to name the file the write produces,
/// and a second hand-built spelling of it would let two writers of one draft
/// serialize on two different keys.
pub(crate) fn entry_path(
    state_dir: &Path,
    domain: &str,
    actor: &str,
    path: &str,
) -> io::Result<PathBuf> {
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
///
/// The screen is [`crate::engine::is_contained_rel`], the repo's rule for a
/// path that arrived from outside, and not the plain containment rule the entry
/// path uses. Two of the three names here are chosen by somebody else - a
/// domain by the local configuration, an actor by whoever holds the account -
/// and a backslash or a colon inside a segment is a separator, a drive marker
/// or a stream marker on Windows. A drive-shaped name is the sharp one: `C:` is
/// a path PREFIX, so `PathBuf::join` replaces the path built so far instead of
/// appending to it, which would point `journal_remove_domain`'s recursive
/// delete at a whole volume. The entry path keeps the looser rule on purpose,
/// because an engram file is whatever a person named it.
pub(crate) fn one_segment<'a>(name: &'a str, what: &str) -> io::Result<&'a str> {
    if !is_contained_rel(name) || name.contains('/') {
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
///
/// Shared with [`crate::overlay_files`] so the sidecar is named in one place.
pub(crate) fn tombstone_of(file: &Path) -> PathBuf {
    let mut name = file
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(TOMBSTONE_SUFFIX);
    file.with_file_name(name)
}

/// Write atomically through [`config::save_bytes`], so a reader never sees a
/// half-written draft and a crash never truncates one. Shared with
/// [`crate::overlay_files`], whose bytes arrive the same way.
pub(crate) fn save(file: &Path, bytes: &[u8]) -> io::Result<()> {
    config::save_bytes(file, bytes).map_err(io::Error::other)
}

/// Remove a file that may not be there. Shared with
/// [`crate::overlay_files`].
pub(crate) fn remove_if_present(file: &Path) -> io::Result<()> {
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
///
/// Shared with [`crate::overlay_files`], which prunes up to its own `files/`
/// folder rather than to the actor's - the actor's folder is this module's and
/// outlives a cleared file.
pub(crate) fn prune_empty(from: Option<&Path>, stop: &Path) {
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

/// What one domain's journal holds, with the honesty flag beside it.
///
/// `unreadable` is the whole reason this is a struct rather than a `Vec`: an
/// empty answer means "nobody is drafting here" only when nothing failed on the
/// way to it, and the callers act on that difference in front of a destructive
/// removal. A read that could not enumerate a folder, or could not read a file
/// it enumerated, comes back with what it did get and this flag set.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct JournalRead {
    /// Every mirrored entry that could be read, ordered by actor then path.
    pub entries: Vec<JournalEntry>,
    /// Whether anything the read tried to reach could not be reached.
    pub unreadable: bool,
}

/// How many drafts one domain's journal holds per actor, without reading a
/// single draft's markdown.
///
/// The counting twin of [`journal_entries`], for the callers that only ever
/// needed a number: the removal preview, the orphan sweep and the two
/// restore early-outs. `unreadable` here covers enumeration alone, which is the
/// only way a COUNT can come out short - a file whose bytes cannot be read is
/// still a draft this domain holds, and it is still counted.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct JournalCounts {
    /// Drafts per actor, ordered by actor.
    pub per_actor: BTreeMap<String, u64>,
    /// Drafts across every actor.
    pub total: u64,
    /// Whether a folder this count needed could not be enumerated.
    pub unreadable: bool,
}

/// Every draft mirrored for one domain, markdown included, ordered by actor and
/// then by path.
///
/// Answers with what it could read and never with an error, and says whether
/// that was everything. Anything that is not a mirrored entry is skipped
/// silently - a file that does not end in `.md` or `.md.tombstone`, the atomic
/// write's temporary sibling among them - but a file that IS one and could not
/// be read is logged and sets `unreadable`.
pub fn journal_entries(state_dir: &Path, domain: &str) -> JournalRead {
    let (entries, unreadable) = walk(state_dir, domain, true);
    JournalRead {
        entries,
        unreadable,
    }
}

/// How many drafts each actor holds in one domain, reading no markdown at all.
pub fn journal_counts(state_dir: &Path, domain: &str) -> JournalCounts {
    let (entries, unreadable) = walk(state_dir, domain, false);
    let mut per_actor: BTreeMap<String, u64> = BTreeMap::new();
    for entry in &entries {
        *per_actor.entry(entry.actor.clone()).or_default() += 1;
    }
    JournalCounts {
        per_actor,
        total: entries.len() as u64,
        unreadable,
    }
}

/// The one walk both readers run. `read_content` is the only difference: with
/// it off, every entry comes back with `content: None` and no file is opened,
/// so a tombstone and a draft are indistinguishable in the result - which is
/// exactly what a count needs and nothing else may use.
fn walk(state_dir: &Path, domain: &str, read_content: bool) -> (Vec<JournalEntry>, bool) {
    let Ok(dir) = domain_dir(state_dir, domain) else {
        // A name that cannot address a journal folder is not a journal that is
        // empty: nothing was read, and a caller about to delete may not be told
        // otherwise.
        return (Vec::new(), true);
    };
    let actors = match std::fs::read_dir(&dir) {
        Ok(actors) => actors,
        // A domain nobody has drafted in has no folder, and that is a certain
        // answer rather than an unknown one. Every other failure is unknown.
        Err(e) if e.kind() == io::ErrorKind::NotFound => return (Vec::new(), false),
        Err(e) => {
            tracing::warn!(
                domain = domain,
                "the overlay journal folder could not be read: {e}"
            );
            return (Vec::new(), true);
        }
    };
    let mut out: Vec<JournalEntry> = Vec::new();
    let mut unreadable = false;
    for actor in actors {
        let actor = match actor {
            Ok(actor) => actor,
            Err(e) => {
                tracing::warn!(domain = domain, "an overlay journal entry was skipped: {e}");
                unreadable = true;
                continue;
            }
        };
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
        unreadable |= collect(&actor.path(), "", &name, read_content, &mut found);
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
    (out, unreadable)
}

/// Walk one actor's folder, appending every mirrored entry under it. Answers
/// whether anything under it could not be read.
///
/// **It descends into the files overlay and takes nothing out of it.** That
/// tree stands at `<actor>/files/` ([`crate::overlay_files`]), and the `is_md`
/// screen below is what keeps it out: every path there is a validated
/// attachment path, so it ends in an allowlisted extension and `.md` is not one
/// of them - a file there can no more be read back as a draft than
/// `<path>.tombstone` can. The coexistence is pinned by
/// `the_journal_walk_never_reads_an_overlay_file_or_its_sidecar_as_a_draft`.
fn collect(
    dir: &Path,
    prefix: &str,
    actor: &str,
    read_content: bool,
    out: &mut Vec<JournalEntry>,
) -> bool {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            tracing::warn!(
                actor = actor,
                "an overlay journal folder could not be read: {e}"
            );
            return true;
        }
    };
    let mut unreadable = false;
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                tracing::warn!(actor = actor, "an overlay journal entry was skipped: {e}");
                unreadable = true;
                continue;
            }
        };
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
            unreadable |= collect(&path, &rel, actor, read_content, out);
            continue;
        }
        let (rel, content) = match rel.strip_suffix(TOMBSTONE_SUFFIX) {
            Some(base) => (base.to_string(), None),
            None if !read_content => (rel, None),
            None => match std::fs::read_to_string(&path) {
                Ok(text) => (rel, Some(text)),
                Err(e) => {
                    // The mirror is the only copy a draft has, so a file that is
                    // there and cannot be read is never passed over in silence.
                    tracing::warn!(
                        actor = actor,
                        path = rel.as_str(),
                        "a mirrored draft could not be read and stays out of the restore: {e}"
                    );
                    unreadable = true;
                    continue;
                }
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
    unreadable
}

/// The convergence record one domain's journal holds.
///
/// Never errors: a domain nobody has pulled yet has no record, and a record
/// this machine cannot read or parse is answered as an empty one with a warning
/// rather than as a failure. The whole of it is derived again by the next pull,
/// so an unreadable record costs a report and never a draft.
pub fn journal_record(state_dir: &Path, domain: &str) -> ConvergenceRecord {
    let Ok(file) = record_path(state_dir, domain) else {
        return ConvergenceRecord::default();
    };
    let bytes = match std::fs::read(&file) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return ConvergenceRecord::default(),
        Err(e) => {
            tracing::warn!(domain, "the convergence record could not be read: {e}");
            return ConvergenceRecord::default();
        }
    };
    match serde_json::from_slice(&bytes) {
        Ok(record) => record,
        Err(e) => {
            tracing::warn!(domain, "the convergence record could not be parsed: {e}");
            ConvergenceRecord::default()
        }
    }
}

/// Write one domain's convergence record, creating the journal folder for it.
///
/// A record with nothing in it removes the file instead of writing an empty
/// one, so "this domain has nothing unsettled" and "nobody has pulled here yet"
/// read the same way they always did: as no record at all.
pub fn journal_save_record(
    state_dir: &Path,
    domain: &str,
    record: &ConvergenceRecord,
) -> io::Result<()> {
    let file = record_path(state_dir, domain)?;
    if record == &ConvergenceRecord::default() {
        return remove_if_present(&file);
    }
    let bytes = serde_json::to_vec_pretty(record).map_err(io::Error::other)?;
    save(&file, &bytes)
}

/// The file one domain's convergence record lives in.
fn record_path(state_dir: &Path, domain: &str) -> io::Result<PathBuf> {
    Ok(domain_dir(state_dir, domain)?.join(RECORD_FILE))
}

/// Drop a whole domain's journal, answering with how many entries it held.
///
/// Called from both removal paths. A domain nobody registers must keep no
/// mirror: the rows and the copy that would bring them back go together, or
/// the next sync resurrects drafts for a domain that no longer exists.
pub fn journal_remove_domain(state_dir: &Path, domain: &str) -> io::Result<u64> {
    let dir = domain_dir(state_dir, domain)?;
    let held = journal_counts(state_dir, domain).total;
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => Ok(held),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(0),
        // A sweep can fail part of the way through, with some actors' folders
        // already gone. The error is the caller's to report, and so is the
        // number that did go: what is left is counted again so nothing claims a
        // removal took nothing when it took some of it.
        Err(e) => {
            let left = journal_counts(state_dir, domain).total;
            Err(io::Error::new(
                e.kind(),
                format!(
                    "{e} ({} of {held} mirrored draft(s) went)",
                    held - left.min(held)
                ),
            ))
        }
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
/// A restored draft is a whole row: it is chunked the way a write verb chunks
/// one, and the domain's pending relations and links are resolved once at the
/// end, so a draft that came back through a wipe is as complete as one that was
/// never wiped.
///
/// **A restored draft stores its full markdown**, frontmatter and all, exactly
/// as a virtual domain's rows do (`Engine::index_markdown`'s `store_full`): a
/// draft is on nobody's disk, so the row is the only place its document
/// survives and a body-only projection would lose the frontmatter for good.
/// The write verbs that come to journal drafts have to store them the same way.
///
/// **A tombstone is rebuilt from the base row it deletes.** Its mirror carries
/// no content of its own (what it records is that this actor deleted this
/// path), so the row comes back standing at that path under the base row's own
/// identity, carrying the base's stored content and none of its child rows: a
/// deletion contributes no observations and no edges to the actor who made it.
/// A tombstone whose base is not there is skipped and its entry left alone:
/// there is nothing to shadow, and clearing it would be convergence rather than
/// restoration, which is a later task's business.
pub async fn restore_into(
    store: &dyn Store,
    state_dir: &Path,
    domain_name: &str,
    domain: DomainId,
    chunk_params: &ChunkParams,
) -> crystalline_index::Result<u64> {
    let mut restored = 0u64;
    // Whose rows came back, so the trailing pass below runs once per actor in
    // that actor's own view rather than once for the domain in nobody's.
    let mut restored_actors: BTreeSet<String> = BTreeSet::new();
    let read = journal_entries(state_dir, domain_name);
    if read.unreadable {
        tracing::warn!(
            domain = domain_name,
            "part of the overlay journal could not be read; the drafts it mirrors there stay \
             out of the index"
        );
    }
    for entry in read.entries {
        if store
            .overlay_entry(domain, &entry.actor, &entry.path)
            .await?
            .is_some()
        {
            continue;
        }
        let record = match &entry.content {
            Some(text) => match draft_record(&entry, text) {
                Some(record) => record,
                None => continue,
            },
            None => match tombstone_record(store, domain_name, domain, &entry).await? {
                Some(record) => record,
                None => continue,
            },
        };
        // The row and its chunks in ONE transaction, exactly as
        // `Engine::index_markdown` writes a draft in the first place. A draft is
        // chunked the way a write verb chunks one or it comes back as a row
        // nothing can ever embed: chunks are written at write time and nothing
        // downstream creates them later, and `chunks_needing_embedding` is
        // unscoped by actor precisely so a draft's chunks reach the same backlog
        // a base row's do. Without the transaction a failure between the two
        // strands the row present and chunkless - and "store rows win" then
        // makes every later restore skip it, so one transient database error
        // costs a draft its embeddings for good. A tombstone is left chunkless
        // on purpose: a deletion's content does not belong in the backlog.
        store.begin().await?;
        let written = async {
            let id = store.upsert_overlay(domain, &entry.actor, &record).await?;
            if !record.tombstone {
                let chunks = chunk_engram(
                    &record.title,
                    record.description.as_deref(),
                    &record.content,
                    chunk_params,
                );
                store.replace_chunks(id, &chunks).await?;
            }
            Ok::<(), crystalline_index::IndexError>(())
        }
        .await;
        match written {
            Ok(()) => store.commit().await?,
            Err(e) => {
                let _ = store.rollback().await;
                return Err(e);
            }
        }
        restored += 1;
        restored_actors.insert(entry.actor.clone());
    }
    // The restored rows' relations and links point at engrams that are already
    // in the index, so they resolve now rather than waiting for a write that
    // may never come. Runs only where something was restored, and it is what
    // keeps the two callers equal: the engine's sync pass has a trailing
    // `resolve_forward_refs` that a restored row would ride on, and the CLI's
    // reindex has already run its own by the time the restore happens.
    //
    // Once per actor, in that actor's own view, because that is how the rows
    // were written: a restore that resolved them against the base alone would
    // rebuild a draft's links onto the pages it was NOT reading, and the
    // mirror exists precisely so a wiped index comes back saying what it said.
    for actor in restored_actors {
        store.reresolve_actor_references(domain, &actor).await?;
    }
    Ok(restored)
}

/// The row one mirrored draft comes back as, or `None` when its markdown no
/// longer parses - which is left in the journal rather than dropped, since the
/// mirror is the only copy there is.
fn draft_record(entry: &JournalEntry, text: &str) -> Option<EngramRecord> {
    let engram = match parse_engram(text) {
        Ok(engram) => engram,
        Err(e) => {
            tracing::warn!(
                actor = entry.actor.as_str(),
                path = entry.path.as_str(),
                "the mirrored draft could not be parsed and was left in the journal: {e}"
            );
            return None;
        }
    };
    let mut record = EngramRecord::from_engram(&engram, &entry.path, virtual_stamp(text));
    // The draft is on nobody's disk, so the row keeps the whole document.
    record.content = text.to_string();
    Some(record)
}

/// The row one mirrored tombstone comes back as: the base row's identity at the
/// same path, flagged. `None` when no base row stands there any more.
async fn tombstone_record(
    store: &dyn Store,
    domain_name: &str,
    domain: DomainId,
    entry: &JournalEntry,
) -> crystalline_index::Result<Option<EngramRecord>> {
    let base = store
        .list_engrams(domain_name, Some(&entry.path), None)
        .await?
        .into_iter()
        .find(|d| d.path == entry.path);
    let Some(base) = base else {
        return Ok(None);
    };
    let content = store
        .engram_content(domain, &entry.path)
        .await?
        .unwrap_or_default();
    let stamp = virtual_stamp(&content);
    Ok(Some(EngramRecord {
        path: entry.path.clone(),
        // The path rather than the base row's permalink, for the reason
        // `Engine::write_overlay_tombstone` gives: one actor holds one row per
        // permalink per domain, and a move leaves that actor holding both a
        // tombstone at the source and an entry at the destination, which
        // inherit the same one. A tombstone answers to no address, so the
        // column carries the row's own identity here. Written the same way by
        // the verb and by this restore, so a deletion means one thing however
        // it got into the index.
        permalink: entry.path.clone(),
        title: base.title,
        engram_type: base.engram_type,
        status: base.status,
        recorded_at: None,
        valid_from: None,
        valid_to: None,
        timestamp: None,
        description: None,
        content,
        metadata: serde_json::Value::Object(serde_json::Map::new()),
        tags: Vec::new(),
        observations: Vec::new(),
        relations: Vec::new(),
        links: Vec::new(),
        stamp,
        actor: String::new(),
        tombstone: true,
    }))
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

        let entries = journal_entries(state, "team").entries;
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
        let entries = journal_entries(state, "team").entries;
        assert_eq!(entries.len(), 3, "still one entry per (actor, path)");
        assert_eq!(entries[1].content, None, "alice's draft became a deletion");

        // And a draft over a tombstone replaces it the other way round.
        journal_write(state, "team", "alice", "top.md", DRAFT).unwrap();
        assert!(!state.join("overlays/team/alice/top.md.tombstone").exists());
        assert_eq!(
            journal_entries(state, "team").entries[1].content.as_deref(),
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
            journal_entries(state, "team").entries,
            vec![JournalEntry {
                actor: "alice".to_string(),
                path: "top.md".to_string(),
                content: Some(DRAFT.to_string()),
            }],
            "only what was not cleared is left"
        );

        // The whole domain goes, count and all.
        assert_eq!(journal_remove_domain(state, "team").unwrap(), 1);
        assert!(journal_entries(state, "team").entries.is_empty());
        assert_eq!(
            journal_remove_domain(state, "team").unwrap(),
            0,
            "a domain with no journal sweeps to nothing"
        );
    }

    /// A read that could not see everything says so, and the two readers
    /// differ in what they even try to read.
    #[test]
    fn journal_reads_report_what_they_could_not_read() {
        let tmp = dir();
        let state = tmp.path();

        journal_write(state, "team", "alice", "good.md", DRAFT).unwrap();
        // A draft whose bytes are not text. The count can still see it - it is
        // enumerated like any other file - but the entry reader cannot hand it
        // to a restore, and that difference is the whole point of the pair.
        std::fs::write(
            state.join("overlays/team/alice/bad.md"),
            [0xff, 0xfe, 0x00, 0x9f],
        )
        .unwrap();

        let counts = journal_counts(state, "team");
        assert_eq!(counts.total, 2, "both files are enumerated");
        assert_eq!(counts.per_actor.get("alice"), Some(&2));
        assert!(
            !counts.unreadable,
            "nothing failed to enumerate, so the count is not in doubt"
        );

        let read = journal_entries(state, "team");
        assert_eq!(read.entries.len(), 1, "only the readable draft comes back");
        assert_eq!(read.entries[0].path, "good.md");
        assert!(
            read.unreadable,
            "and the reader says it could not read everything it tried to"
        );

        // A domain folder that is not a folder: nothing can be enumerated, and
        // an empty answer must not read as `nobody is drafting here`.
        std::fs::remove_dir_all(state.join("overlays/solo")).ok();
        std::fs::write(state.join("overlays/solo"), "not a folder").unwrap();
        let counts = journal_counts(state, "solo");
        assert_eq!(counts.total, 0);
        assert!(
            counts.unreadable,
            "an unreadable journal is not an empty one"
        );
        assert!(journal_entries(state, "solo").unreadable);

        // A domain nobody has ever drafted in is a different answer: empty and
        // certain.
        let counts = journal_counts(state, "never");
        assert_eq!(counts.total, 0);
        assert!(!counts.unreadable);
    }

    /// **The two substrates share an actor folder and cannot read each other.**
    ///
    /// [`crate::overlay_files`] stands at `<actor>/files/`, which this walk
    /// descends into like any other folder. What keeps it out of a restore is
    /// the `is_md` screen and nothing else, so the sharp shape is planted here:
    /// a draft whose own path happens to start with `files/`, beside an overlay
    /// file and an overlay sidecar under the same folder. The draft comes back
    /// and the two files do not.
    #[test]
    fn the_journal_walk_never_reads_an_overlay_file_or_its_sidecar_as_a_draft() {
        let tmp = dir();
        let state = tmp.path();

        journal_write(state, "team", "alice", "files/notes.md", DRAFT).unwrap();
        crate::overlay_files::put(state, "team", "alice", "assets/a.png", b"png bytes").unwrap();
        crate::overlay_files::tombstone(state, "team", "alice", "assets/b.png").unwrap();

        assert!(
            state
                .join("overlays/team/alice/files/assets/a.png")
                .is_file()
                && state
                    .join("overlays/team/alice/files/assets/b.png.tombstone")
                    .is_file()
                && state.join("overlays/team/alice/files/notes.md").is_file(),
            "all three stand under one actor folder"
        );

        let read = journal_entries(state, "team");
        assert_eq!(
            read.entries,
            vec![JournalEntry {
                actor: "alice".to_string(),
                path: "files/notes.md".to_string(),
                content: Some(DRAFT.to_string()),
            }],
            "only the draft is a journal entry; neither the overlay file nor its sidecar is"
        );
        assert!(!read.unreadable);
        assert_eq!(journal_counts(state, "team").total, 1);

        // And the reverse: the files overlay reads none of the journal's own
        // drafts, whatever folder they happen to stand in.
        let files = crate::overlay_files::entries(state, "team", "alice");
        assert_eq!(
            files
                .entries
                .iter()
                .map(|e| e.path.as_str())
                .collect::<Vec<_>>(),
            vec!["assets/a.png", "assets/b.png"],
            "the draft at files/notes.md is not one of this actor's overlay files"
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
            // A drive-shaped name is a path PREFIX on Windows, so `PathBuf::join`
            // would replace the accumulated path with it rather than append -
            // pointing a recursive delete at a whole volume. The actor key comes
            // from an account name somebody else chose, so both names take the
            // repo's rule for untrusted input rather than containment alone.
            ("C:", "alice", "plan.md"),
            ("team", "C:", "plan.md"),
            ("team", "a:b", "plan.md"),
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
            journal_entries(state, "../escape").entries.is_empty(),
            "a refused domain reads as an empty journal rather than a folder above the root"
        );
    }
}
