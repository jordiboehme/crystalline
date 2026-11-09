//! Kept texts: what a co-editing room held when its last save could not land.
//!
//! When the last person leaves a room and the final save is refused or ends
//! in a conflict, nothing would ever save that text again. The server keeps
//! it as a file in its state folder instead of dropping it:
//!
//! ```text
//! <state>/kept/<domain>/<engram|reflection>/<path>.<YYYYMMDDTHHMMSSZ>.md
//! ```
//!
//! `<path>` is the engram's file path in its domain, or the page's path
//! inside `reflections/`. A third kind, `comment`, holds a thread comment
//! a Reflection write could not land (its host lost the domain's host lock
//! mid-write); its `<path>` is the page the comment was on. A text kept for
//! a write rather than a room may name who sent it in a file beside it,
//! `<kept file>.from` (JSON: the sender's account and display name), which
//! [`scan`] reads as [`KeptText::sender`]. A second text kept under the same
//! name in the same second gets `-2`, `-3` and so on after the time. A room over somebody's
//! review draft keeps its text under `<domain>/draft/<owner>/` instead of
//! `<domain>/`, because a draft is private to its owner.
//!
//! A kept text is the last copy of somebody's work, so nothing here deletes
//! one on its own: [`remove`] runs only when a person asks for it. This
//! module is the one place that knows the layout; the room writes through
//! [`write`](fn@write), and `crystalline doctor`, `crystalline kept` and the JSON API
//! read through [`scan`].

use std::io::Write as _;
use std::path::{Path, PathBuf};

use chrono::{DateTime, NaiveDateTime, Utc};
use serde::Serialize;
use sha2::{Digest, Sha256};

/// The folder in the state directory that holds kept texts.
pub const KEPT_DIR: &str = "kept";

/// The time format of a kept file's name, in UTC.
const STAMP: &str = "%Y%m%dT%H%M%SZ";

/// How many hex characters of the path's hash make an id.
const ID_LEN: usize = 12;

/// The folder under a domain's folder that holds drafts' kept texts, one
/// folder per owner.
const DRAFT_DIR: &str = "draft";

/// The folder under the kept folder that holds removed domains' texts, one
/// folder per removal named `<domain>-<time>`. Its leading dot is what
/// `validate_domain_name` refuses, so no domain is ever called this.
pub const REMOVED_DIR: &str = ".removed";

/// Move `domain`'s kept texts aside when the domain is removed, to
/// `<root>/.removed/<domain>-<time>/` (or `-N` after the time when that is
/// taken), so a domain registered later under the same name never reaches
/// them. Nothing is deleted. Answers where they went, or `None` when the
/// domain kept nothing.
pub fn set_aside(root: &Path, domain: &str, at: DateTime<Utc>) -> std::io::Result<Option<PathBuf>> {
    let from = root.join(domain);
    if domain.is_empty()
        || domain.starts_with('.')
        || domain.contains(['/', '\\'])
        || !from.is_dir()
    {
        return Ok(None);
    }
    let removed = root.join(REMOVED_DIR);
    std::fs::create_dir_all(&removed)?;
    let stamp = at.format(STAMP).to_string();
    for n in 1u32.. {
        let to = removed.join(match n {
            1 => format!("{domain}-{stamp}"),
            n => format!("{domain}-{stamp}-{n}"),
        });
        if to.exists() {
            continue;
        }
        std::fs::rename(&from, &to)?;
        return Ok(Some(to));
    }
    unreachable!("a free name is found before the counter runs out")
}

/// The domain a folder under [`REMOVED_DIR`] belonged to:
/// `<domain>-<time>` or `<domain>-<time>-<n>`.
fn removed_domain(folder: &str) -> Option<&str> {
    let folder = match folder.rsplit_once('-') {
        Some((head, n)) if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => head,
        _ => folder,
    };
    let (domain, stamp) = folder.rsplit_once('-')?;
    NaiveDateTime::parse_from_str(stamp, STAMP).ok()?;
    (!domain.is_empty()).then_some(domain)
}

/// Which kind of room a text was kept from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum KeptKind {
    /// An engram's room; the path is the engram's file in its domain.
    Engram,
    /// A Reflection page's room, or a page write; the path is inside
    /// `reflections/`.
    Reflection,
    /// A thread comment on a Reflection page; the path is the page's,
    /// inside `reflections/`.
    Comment,
}

impl KeptKind {
    /// The folder name under the domain's folder, and the wire word.
    pub fn as_str(self) -> &'static str {
        match self {
            KeptKind::Engram => "engram",
            KeptKind::Reflection => "reflection",
            KeptKind::Comment => "comment",
        }
    }

    fn from_folder(name: &str) -> Option<KeptKind> {
        match name {
            "engram" => Some(KeptKind::Engram),
            "reflection" => Some(KeptKind::Reflection),
            "comment" => Some(KeptKind::Comment),
            _ => None,
        }
    }
}

/// One kept text, as [`scan`] finds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct KeptText {
    /// A short, stable id: the start of the hash of the file's path inside
    /// the kept folder. It never changes while the file is there.
    pub id: String,
    /// The domain the room belonged to.
    pub domain: String,
    /// Whether the domain was removed from this machine after the text was
    /// kept: [`set_aside`] moved the texts out of its folder, so a later
    /// domain of the same name never reaches them.
    pub removed: bool,
    /// Whether this is an unfinished keep: the temporary file a keep writes
    /// before it gives the text its kept name, left behind when the
    /// process stopped in between. It may be the only copy, so it is listed
    /// and shown like a kept text, never served over the JSON API and never
    /// deleted on its own.
    pub unfinished: bool,
    /// Whose review draft the room was over, or `None` for the domain's own
    /// text. A draft's kept text is its owner's alone.
    pub owner: Option<String>,
    /// The kind of room.
    pub kind: KeptKind,
    /// The engram's file path in its domain, or the page's path inside
    /// `reflections/`.
    pub path: String,
    /// When the text was kept, from the file name.
    pub kept_at: DateTime<Utc>,
    /// The file's size in bytes.
    pub bytes: u64,
    /// The file on disk. Never sent over the JSON API.
    #[serde(skip)]
    pub file: PathBuf,
    /// Who sent the text, when it was kept for a write that names its
    /// sender ([`record_sender`]); `None` for a room's text, or when the
    /// sender could not be recorded.
    pub sender: Option<Sender>,
}

/// Who sent a kept text: the account it was sent as, which rights are
/// matched on, and the name it was shown under.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct Sender {
    /// The account the sender acted as (an agent's is the account it acts
    /// for).
    pub account: String,
    /// The name the sender is shown under in threads (the login name for
    /// a person).
    pub name: String,
}

/// The end of the name of the file beside a kept text that names its
/// sender.
const SENDER_SUFFIX: &str = ".from";

/// Record who sent the kept text at `file` (as [`write`](fn@write)
/// answered it), in the file beside it. Written to a temporary name and
/// renamed, so a reader never meets half a record. The text is kept
/// already; a failure here leaves it without a sender, and the caller says
/// so.
pub fn record_sender(file: &Path, sender: &Sender) -> std::io::Result<()> {
    let target = sender_file(file);
    let mut temp = target.as_os_str().to_owned();
    temp.push(format!(
        ".{}-{}{TEMP_SUFFIX}",
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let temp = PathBuf::from(temp);
    let json = serde_json::to_string(sender).map_err(std::io::Error::other)?;
    let written = write_new(&temp, json.as_bytes()).and_then(|()| std::fs::rename(&temp, &target));
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    written
}

/// The sender recorded beside `file`, if a whole record is there.
fn read_sender(file: &Path) -> Option<Sender> {
    let text = std::fs::read_to_string(sender_file(file)).ok()?;
    serde_json::from_str(&text).ok()
}

/// The file beside `kept` that names its sender.
fn sender_file(kept: &Path) -> PathBuf {
    let mut name = kept.as_os_str().to_owned();
    name.push(SENDER_SUFFIX);
    PathBuf::from(name)
}

/// Keep `text` for `domain`'s `path` under `root` (the kept folder), named
/// for `at`; with `owner`, in that owner's draft folder. Never overwrites: a
/// name that is taken gets the next free `-N`. Returns the file written.
pub fn write(
    root: &Path,
    domain: &str,
    owner: Option<&str>,
    kind: KeptKind,
    path: &str,
    at: DateTime<Utc>,
    text: &str,
) -> std::io::Result<PathBuf> {
    write_bytes(root, domain, owner, kind, path, at, text.as_bytes())
}

/// [`write`](fn@write) for bytes that need not be text: a draft file beside
/// the pages, kept when it is the last copy. The layout and the name are a
/// text's, so a listing finds it under its path; [`read`] of it fails when the
/// bytes are not UTF-8, and the file itself holds them as they were.
pub fn write_bytes(
    root: &Path,
    domain: &str,
    owner: Option<&str>,
    kind: KeptKind,
    path: &str,
    at: DateTime<Utc>,
    text: &[u8],
) -> std::io::Result<PathBuf> {
    let mut base = root.join(domain);
    if let Some(owner) = owner {
        if owner.is_empty() || matches!(owner, "." | "..") || owner.contains(['/', '\\', ':']) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("'{owner}' is not one folder name"),
            ));
        }
        base = base.join(DRAFT_DIR).join(owner);
    }
    let base = base.join(kind.as_str()).join(path);
    let stamp = at.format(STAMP).to_string();
    let name = base
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let parent = base.parent().unwrap_or(root).to_path_buf();
    // The text goes to a temporary name first, which `parse_name` never
    // takes for a kept text, so a listing never shows a file half written.
    // A `remove` of the folder's last text may prune the folder between its
    // creation and the open: then the folder is made again, a few times.
    // The temporary name carries the process and a counter, so two keeps of
    // one path in one second never meet; one that is taken all the same
    // (another process reusing the id) is passed over for the next.
    let mut attempt = 0;
    let temp = loop {
        let temp = parent.join(format!(
            ".{name}.{stamp}.{}-{}{TEMP_SUFFIX}",
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&parent)?;
        #[cfg(test)]
        tests::before_open(&parent, &temp);
        match write_new(&temp, text) {
            Ok(()) => break temp,
            Err(err)
                if matches!(
                    err.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::AlreadyExists
                ) && attempt < OPEN_TRIES =>
            {
                attempt += 1;
            }
            Err(err) => return Err(err),
        }
    };
    let placed = place(&temp, &base, &name, &stamp, text);
    let _ = std::fs::remove_file(&temp);
    placed
}

/// How many times [`write`] makes a pruned folder again, or passes over a
/// taken temporary name, before it gives up.
const OPEN_TRIES: u32 = 5;

/// The end of a temporary keep's name: `.<name>.<stamp>.<pid>-<n>.keeping`.
const TEMP_SUFFIX: &str = ".keeping";

/// How long a temporary keep stays unchanged before it counts as an
/// unfinished keep rather than one still being written.
pub const UNFINISHED_AFTER: std::time::Duration = std::time::Duration::from_secs(60);

/// The counter in a temporary keep's name, one per keep in this process.
static TEMP_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Write `text` to a new file at `file`; fails when something is there.
fn write_new(file: &Path, text: &[u8]) -> std::io::Result<()> {
    let mut out = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(file)?;
    out.write_all(text)?;
    out.sync_all()
}

/// Give the complete file at `temp` its kept name next to `base`, never over
/// another file: a hard link fails on a taken name, so the next `-N` is
/// tried. A file system without hard links gets the text written straight
/// to the free name instead.
fn place(
    temp: &Path,
    base: &Path,
    name: &str,
    stamp: &str,
    text: &[u8],
) -> std::io::Result<PathBuf> {
    for n in 1u32.. {
        let file = base.with_file_name(match n {
            1 => format!("{name}.{stamp}.md"),
            n => format!("{name}.{stamp}-{n}.md"),
        });
        match std::fs::hard_link(temp, &file) {
            Ok(()) => return Ok(file),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => match write_new(&file, text) {
                Ok(()) => return Ok(file),
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(err) => return Err(err),
            },
        }
    }
    unreachable!("a free name is found before the counter runs out")
}

/// Every kept text under `root`, oldest first (then by domain, kind and
/// path). A missing folder holds none. A file whose name does not follow the
/// layout, a symbolic link and anything that cannot be read are skipped: the
/// scan never fails, so a report that lists kept texts always runs.
pub fn scan(root: &Path) -> Vec<KeptText> {
    let mut found = Vec::new();
    for entry in dir_entries(root) {
        if !entry.is_dir {
            continue;
        }
        if entry.name == REMOVED_DIR {
            for folder in dir_entries(&entry.path) {
                if let Some(domain) = folder
                    .is_dir
                    .then(|| removed_domain(&folder.name))
                    .flatten()
                {
                    let prefix = format!("{REMOVED_DIR}/{}", folder.name);
                    scan_domain(domain, true, &prefix, &folder.path, &mut found);
                }
            }
        } else if !entry.name.starts_with('.') {
            scan_domain(&entry.name, false, &entry.name, &entry.path, &mut found);
        }
    }
    found.sort_by(|a, b| {
        (a.kept_at, &a.domain, &a.owner, a.kind, &a.path, &a.file)
            .cmp(&(b.kept_at, &b.domain, &b.owner, b.kind, &b.path, &b.file))
    });
    found
}

/// The kept texts of one account under `root`, as a name's drafts count them:
/// those kept from its review drafts, and the comments it is the recorded sender
/// of, which its reading right alone reaches. A text set aside with a removed
/// domain or left unfinished is never served, so it does not count.
///
/// Unlike [`scan`] this fails, naming the domain whose folder could not be read,
/// because the caller decides who may take the name: a folder that cannot be
/// read is not a folder that holds nothing. Another account's folder inside it
/// is not asked.
pub fn held_by(root: &Path, account: &str) -> Result<Vec<KeptText>, String> {
    let readable = |dir: &Path| match std::fs::read_dir(dir) {
        Err(e) => e.kind() == std::io::ErrorKind::NotFound,
        Ok(_) => true,
    };
    if !readable(root) {
        return Err(root.display().to_string());
    }
    for entry in dir_entries(root) {
        if !entry.is_dir || entry.name == REMOVED_DIR || entry.name.starts_with('.') {
            continue;
        }
        if !readable(&entry.path.join(DRAFT_DIR).join(account)) {
            return Err(entry.name);
        }
    }
    Ok(scan(root)
        .into_iter()
        .filter(|kept| {
            !kept.removed
                && !kept.unfinished
                && (kept.owner.as_deref() == Some(account)
                    || (kept.kind == KeptKind::Comment
                        && kept.sender.as_ref().is_some_and(|s| s.account == account)))
        })
        .collect())
}

/// The kept text with `id` under `root`. An id is only ever matched against
/// what [`scan`] finds, never joined onto a path.
pub fn find(root: &Path, id: &str) -> Option<KeptText> {
    scan(root).into_iter().find(|kept| kept.id == id)
}

/// The text of `kept`. A copy that is not text (a draft file kept as its
/// bytes) is an error of kind [`std::io::ErrorKind::InvalidData`] whose
/// message says so and where the file is, so a reader can tell a person.
pub fn read(kept: &KeptText) -> std::io::Result<String> {
    String::from_utf8(std::fs::read(&kept.file)?)
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, not_text(kept)))
}

/// What a person is told of a kept copy that is a file, not a text.
pub fn not_text(kept: &KeptText) -> String {
    format!(
        "This kept copy is a file, not text. It is at {}.",
        kept.file.display()
    )
}

/// Whether `kept` is a draft file kept as its bytes rather than a text. A
/// text is kept for a page, an engram or a comment, whose path is markdown; a
/// file beside the pages never is.
pub fn is_file_copy(kept: &KeptText) -> bool {
    !kept.path.ends_with(".md")
}

/// Delete `kept`'s file, then every folder it leaves empty up to (not
/// including) `root`. Only ever on a person's request.
pub fn remove(root: &Path, kept: &KeptText) -> std::io::Result<()> {
    std::fs::remove_file(&kept.file)?;
    let _ = std::fs::remove_file(sender_file(&kept.file));
    let mut dir = kept.file.parent();
    while let Some(d) = dir {
        if d == root || !d.starts_with(root) {
            break;
        }
        // Fails on a folder that still holds something, which ends the walk.
        if std::fs::remove_dir(d).is_err() {
            break;
        }
        dir = d.parent();
    }
    Ok(())
}

/// One domain's folder at `dir` (`prefix` inside the kept folder): its own
/// texts and every owner's draft texts.
fn scan_domain(domain: &str, removed: bool, prefix: &str, dir: &Path, found: &mut Vec<KeptText>) {
    for entry in dir_entries(dir) {
        if entry.is_dir && entry.name == DRAFT_DIR {
            for owner in dir_entries(&entry.path) {
                if owner.is_dir {
                    let prefix = format!("{prefix}/{DRAFT_DIR}/{}", owner.name);
                    let at = Domain {
                        name: domain,
                        removed,
                        owner: Some(&owner.name),
                        prefix: &prefix,
                    };
                    scan_kinds(&at, &owner.path, found);
                }
            }
        }
    }
    let at = Domain {
        name: domain,
        removed,
        owner: None,
        prefix,
    };
    scan_kinds(&at, dir, found);
}

/// Where a scan is: the domain, whether it was removed, whose draft folder
/// (if any), and that folder's path inside the kept folder.
struct Domain<'a> {
    name: &'a str,
    removed: bool,
    owner: Option<&'a str>,
    prefix: &'a str,
}

/// The two kind folders under `dir` (a domain's folder, or one owner's
/// draft folder), walked into `found`.
fn scan_kinds(at: &Domain<'_>, dir: &Path, found: &mut Vec<KeptText>) {
    for kind_dir in dir_entries(dir) {
        let Some(kind) = KeptKind::from_folder(&kind_dir.name) else {
            continue;
        };
        if kind_dir.is_dir {
            walk(&Place { at, kind }, &kind_dir.path, "", found);
        }
    }
}

/// Which kind folder a walk is in.
struct Place<'a> {
    at: &'a Domain<'a>,
    kind: KeptKind,
}

impl Place<'_> {
    /// The file at `rel` inside this kind folder, as a path inside the kept
    /// folder, `/`-separated.
    fn rel(&self, rel: &str) -> String {
        format!("{}/{}/{rel}", self.at.prefix, self.kind.as_str())
    }
}

/// Every kept file under `dir`, the folder `rel` inside `place`, into
/// `found`.
fn walk(place: &Place<'_>, dir: &Path, rel: &str, found: &mut Vec<KeptText>) {
    for entry in dir_entries(dir) {
        let child = if rel.is_empty() {
            entry.name.clone()
        } else {
            format!("{rel}/{}", entry.name)
        };
        if entry.is_dir {
            walk(place, &entry.path, &child, found);
            continue;
        }
        if !entry.is_file {
            continue;
        }
        let (name, kept_at, unfinished) = match parse_name(&entry.name) {
            Some((name, kept_at)) => (name, kept_at, false),
            // A keep in progress holds its temporary name for a moment; only
            // one left this long is an unfinished keep.
            None => match parse_temp_name(&entry.name) {
                Some((name, kept_at)) if entry.idle_for >= UNFINISHED_AFTER => {
                    (name, kept_at, true)
                }
                _ => continue,
            },
        };
        let path = if rel.is_empty() {
            name.to_string()
        } else {
            format!("{rel}/{name}")
        };
        found.push(KeptText {
            id: id_of(&place.rel(&child)),
            domain: place.at.name.to_string(),
            removed: place.at.removed,
            unfinished,
            owner: place.at.owner.map(str::to_string),
            kind: place.kind,
            path,
            kept_at,
            bytes: entry.len,
            sender: read_sender(&entry.path),
            file: entry.path,
        });
    }
}

/// `.<name>.<stamp>.<pid>[-<n>].keeping`, an unfinished keep's temporary
/// name: the original file name and the time.
fn parse_temp_name(file: &str) -> Option<(&str, DateTime<Utc>)> {
    let stem = file.strip_prefix('.')?.strip_suffix(TEMP_SUFFIX)?;
    let (head, process) = stem.rsplit_once('.')?;
    if process.is_empty() || !process.bytes().all(|b| b.is_ascii_digit() || b == b'-') {
        return None;
    }
    let (name, stamp) = head.rsplit_once('.')?;
    let at = NaiveDateTime::parse_from_str(stamp, STAMP).ok()?;
    (!name.is_empty()).then(|| (name, at.and_utc()))
}

/// `<name>.<stamp>.md` or `<name>.<stamp>-<n>.md`: the original file name
/// and the time.
fn parse_name(file: &str) -> Option<(&str, DateTime<Utc>)> {
    let stem = file.strip_suffix(".md")?;
    let (name, tail) = stem.rsplit_once('.')?;
    if name.is_empty() {
        return None;
    }
    let (stamp, counter) = match tail.split_once('-') {
        Some((stamp, n)) => (stamp, Some(n)),
        None => (tail, None),
    };
    if let Some(n) = counter
        && (n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    let at = NaiveDateTime::parse_from_str(stamp, STAMP).ok()?;
    Some((name, at.and_utc()))
}

/// The id of the file at `full`, its path inside the kept folder: the
/// start of its SHA-256.
fn id_of(full: &str) -> String {
    let digest = Sha256::digest(full.as_bytes());
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    hex[..ID_LEN].to_string()
}

struct Entry {
    name: String,
    path: PathBuf,
    is_dir: bool,
    is_file: bool,
    len: u64,
    /// How long ago it last changed; zero when that cannot be told.
    idle_for: std::time::Duration,
}

/// The entries of `dir` with UTF-8 names, by name, without following
/// symbolic links. Empty for a folder that cannot be read.
fn dir_entries(dir: &Path) -> Vec<Entry> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut entries: Vec<Entry> = read
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let meta = std::fs::symlink_metadata(e.path()).ok()?;
            Some(Entry {
                name,
                path: e.path(),
                is_dir: meta.is_dir(),
                is_file: meta.is_file(),
                len: meta.len(),
                // A modified time in the future (a clock that moved) counts
                // as long idle, so an interrupted keep is never hidden.
                idle_for: match meta.modified().map(|m| m.elapsed()) {
                    Ok(Ok(idle)) => idle,
                    Ok(Err(_)) => std::time::Duration::MAX,
                    Err(_) => std::time::Duration::ZERO,
                },
            })
        })
        .collect();
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A comment kept for a refused write is its own kind, names its sender
    /// from the file beside it, and goes with that file when removed.
    #[test]
    fn a_kept_comment_names_its_sender() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let file = write(
            root,
            "eng",
            None,
            KeptKind::Comment,
            "guides/a.md",
            Utc::now(),
            "Is this still true?",
        )
        .unwrap();
        let ada = Sender {
            account: "ada".to_string(),
            name: "Ada".to_string(),
        };
        record_sender(&file, &ada).unwrap();
        let found = scan(root);
        assert_eq!(
            found.len(),
            1,
            "the sender's file is no kept text: {found:?}"
        );
        let kept = &found[0];
        assert_eq!(kept.kind, KeptKind::Comment);
        assert_eq!(kept.path, "guides/a.md");
        assert_eq!(kept.sender.as_ref(), Some(&ada));
        remove(root, kept).unwrap();
        assert!(!sender_file(&file).exists());
        assert!(scan(root).is_empty());
    }
    use chrono::TimeZone;
    use std::cell::Cell;

    thread_local! {
        /// How many more times [`before_open`] removes the folder it is given.
        static PRUNES: Cell<u32> = const { Cell::new(0) };
    }

    thread_local! {
        /// How many more times [`before_open`] takes the temporary name first.
        static TAKES: Cell<u32> = const { Cell::new(0) };
    }

    /// The test seam in [`write`]: right after the folder is made, remove it
    /// again while [`PRUNES`] says so, as a concurrent `remove` would, or put
    /// a file at the temporary name while [`TAKES`] says so, as a second keep
    /// of the same name would.
    pub(super) fn before_open(dir: &Path, temp: &Path) {
        PRUNES.with(|left| {
            if left.get() > 0 {
                left.set(left.get() - 1);
                std::fs::remove_dir(dir).unwrap();
            }
        });
        TAKES.with(|left| {
            if left.get() > 0 {
                left.set(left.get() - 1);
                std::fs::write(temp, "someone else's").unwrap();
            }
        });
    }

    /// Make `file` look last changed two minutes ago.
    fn age(file: &Path) {
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(120);
        std::fs::File::options()
            .write(true)
            .open(file)
            .unwrap()
            .set_modified(old)
            .unwrap();
    }

    /// A temporary keep whose modified time lies in the future (a clock
    /// that moved) is listed: an interrupted keep is never hidden.
    #[test]
    fn a_temporary_keep_from_the_future_is_listed() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("eng/engram");
        std::fs::create_dir_all(&folder).unwrap();
        let file = folder.join(".a.md.20261015T080000Z.42-0.keeping");
        std::fs::write(&file, "left behind").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(3600))
            .unwrap();
        let found = scan(dir.path());
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].unfinished);
    }

    /// A keep in progress holds its temporary name for a moment and is not
    /// listed; the same file left for more than a minute is.
    #[test]
    fn only_a_temporary_keep_left_a_while_is_unfinished() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("eng/engram");
        std::fs::create_dir_all(&folder).unwrap();
        let fresh = folder.join(".a.md.20261015T080000Z.42-0.keeping");
        let aged = folder.join(".b.md.20261015T080000Z.42-1.keeping");
        std::fs::write(&fresh, "being written").unwrap();
        std::fs::write(&aged, "left behind").unwrap();
        age(&aged);
        let found = scan(dir.path());
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].path, "b.md");
        assert!(found[0].unfinished);
    }

    /// A temporary name another keep holds is passed over: the text is kept
    /// under its own name all the same, and the other file is left alone.
    /// Two keeps of one path in one second get two temporary names.
    #[test]
    fn a_taken_temporary_name_is_passed_over() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        TAKES.with(|left| left.set(2));
        let file = write(root, "eng", None, KeptKind::Engram, "a.md", at(8), "mine").unwrap();
        TAKES.with(|left| left.set(0));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "mine");
        for entry in std::fs::read_dir(file.parent().unwrap()).unwrap() {
            let path = entry.unwrap().path();
            if path.to_string_lossy().ends_with(".keeping") {
                age(&path);
            }
        }
        let found = scan(root);
        assert_eq!(
            found.iter().filter(|k| k.unfinished).count(),
            2,
            "{found:?}"
        );
        assert_eq!(found.iter().filter(|k| !k.unfinished).count(), 1);
        let again = write(root, "eng", None, KeptKind::Engram, "a.md", at(8), "again").unwrap();
        assert_ne!(again, file);
    }

    /// A temporary file a crash left behind is listed as an unfinished
    /// keep, with its path, its time and its text, and is never taken for a
    /// finished one.
    #[test]
    fn a_leftover_temporary_file_is_an_unfinished_keep() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("eng/engram/runbooks");
        std::fs::create_dir_all(&folder).unwrap();
        for name in [
            ".deploy.md.20261015T080000Z.42-3.keeping",
            ".deploy.md.20261015T080000Z.42.keeping",
        ] {
            std::fs::write(folder.join(name), "half").unwrap();
            age(&folder.join(name));
        }
        std::fs::write(folder.join(".deploy.md.x.42.keeping"), "not a keep").unwrap();
        let found = scan(dir.path());
        assert_eq!(found.len(), 2, "{found:?}");
        for k in &found {
            assert!(k.unfinished);
            assert_eq!(k.path, "runbooks/deploy.md");
            assert_eq!(k.kept_at, at(8));
            assert_eq!(read(k).unwrap(), "half");
        }
        assert_ne!(found[0].id, found[1].id);
    }

    /// A drop that prunes the folder between its creation and the open does
    /// not lose the text: the folder is made again and the text kept. One
    /// prune more than the tries is an error, never a silent loss. No
    /// temporary file stays behind.
    #[test]
    fn a_folder_pruned_under_a_keep_is_made_again() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        PRUNES.with(|left| left.set(2));
        let file = write(root, "eng", None, KeptKind::Engram, "a.md", at(8), "kept").unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "kept");
        assert_eq!(
            std::fs::read_dir(file.parent().unwrap()).unwrap().count(),
            1
        );
        PRUNES.with(|left| left.set(OPEN_TRIES + 1));
        let err = write(root, "ops", None, KeptKind::Engram, "a.md", at(8), "x").unwrap_err();
        PRUNES.with(|left| left.set(0));
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }

    /// A removed domain's texts move aside whole, are listed as removed
    /// under their domain's name, and a later folder of the same name holds
    /// only its own. A second removal in the same second takes `-2`.
    #[test]
    fn a_removed_domains_texts_are_set_aside() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root,
            "eng",
            Some("ada"),
            KeptKind::Engram,
            "a.md",
            at(8),
            "draft",
        )
        .unwrap();
        write(
            root,
            "eng",
            None,
            KeptKind::Reflection,
            "p.md",
            at(8),
            "page",
        )
        .unwrap();
        let to = set_aside(root, "eng", at(9)).unwrap().unwrap();
        assert_eq!(to, root.join(".removed/eng-20261015T090000Z"));
        assert!(!root.join("eng").exists());
        assert_eq!(set_aside(root, "eng", at(9)).unwrap(), None, "nothing left");
        write(root, "eng", None, KeptKind::Engram, "new.md", at(10), "new").unwrap();
        let found = scan(root);
        let rows: Vec<(&str, bool, Option<&str>, &str)> = found
            .iter()
            .map(|k| {
                (
                    k.domain.as_str(),
                    k.removed,
                    k.owner.as_deref(),
                    k.path.as_str(),
                )
            })
            .collect();
        assert_eq!(
            rows,
            vec![
                ("eng", true, None, "p.md"),
                ("eng", true, Some("ada"), "a.md"),
                ("eng", false, None, "new.md"),
            ]
        );
        let again = set_aside(root, "eng", at(9)).unwrap().unwrap();
        assert_eq!(again, root.join(".removed/eng-20261015T090000Z-2"));
        assert_eq!(scan(root).iter().filter(|k| k.removed).count(), 3);
        assert_eq!(
            removed_domain("my-team-20261015T090000Z-3"),
            Some("my-team")
        );
        assert_eq!(removed_domain("eng"), None);
    }

    fn at(h: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 15, h, 0, 0).unwrap()
    }

    /// Two refusals that map to one kept name both keep their text: the
    /// second never overwrites the first.
    #[test]
    fn a_second_kept_text_under_the_same_name_does_not_overwrite_the_first() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let kind = KeptKind::Reflection;
        let first = write(root, "eng", None, kind, "guides/a.md", at(8), "first").unwrap();
        let second = write(root, "eng", None, kind, "guides/a.md", at(8), "second").unwrap();
        assert_ne!(first, second);
        assert_eq!(
            first,
            root.join("eng/reflection/guides/a.md.20261015T080000Z.md")
        );
        assert_eq!(second.file_name().unwrap(), "a.md.20261015T080000Z-2.md");
        assert_eq!(std::fs::read_to_string(&first).unwrap(), "first");
        assert_eq!(std::fs::read_to_string(&second).unwrap(), "second");
    }

    /// What `write` keeps, `scan` lists: domain, kind, the original path,
    /// the time from the name and the size; oldest first.
    #[test]
    fn scan_lists_what_write_kept() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root,
            "eng",
            None,
            KeptKind::Engram,
            "runbooks/deploy.md",
            at(9),
            "abc",
        )
        .unwrap();
        write(
            root,
            "eng",
            None,
            KeptKind::Reflection,
            "a.md",
            at(8),
            "hello",
        )
        .unwrap();
        write(root, "ops", None, KeptKind::Reflection, "a.md", at(8), "x").unwrap();
        write(
            root,
            "eng",
            None,
            KeptKind::Reflection,
            "a.md",
            at(8),
            "again",
        )
        .unwrap();
        let found = scan(root);
        let rows: Vec<(&str, KeptKind, &str, DateTime<Utc>, u64)> = found
            .iter()
            .map(|k| {
                (
                    k.domain.as_str(),
                    k.kind,
                    k.path.as_str(),
                    k.kept_at,
                    k.bytes,
                )
            })
            .collect();
        assert_eq!(
            rows,
            vec![
                ("eng", KeptKind::Reflection, "a.md", at(8), 5),
                ("eng", KeptKind::Reflection, "a.md", at(8), 5),
                ("ops", KeptKind::Reflection, "a.md", at(8), 1),
                ("eng", KeptKind::Engram, "runbooks/deploy.md", at(9), 3),
            ]
        );
        assert_eq!(read(&found[3]).unwrap(), "abc");
    }

    /// Every file has its own id, and the id stays the same between scans.
    #[test]
    fn ids_are_short_distinct_and_stable() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root,
            "eng",
            None,
            KeptKind::Reflection,
            "guides/a.md",
            at(8),
            "1",
        )
        .unwrap();
        write(
            root,
            "eng",
            None,
            KeptKind::Reflection,
            "ops/a.md",
            at(8),
            "2",
        )
        .unwrap();
        write(
            root,
            "ops",
            None,
            KeptKind::Reflection,
            "guides/a.md",
            at(8),
            "3",
        )
        .unwrap();
        write(
            root,
            "eng",
            None,
            KeptKind::Engram,
            "guides/a.md",
            at(8),
            "4",
        )
        .unwrap();
        let first: Vec<String> = scan(root).into_iter().map(|k| k.id).collect();
        let again: Vec<String> = scan(root).into_iter().map(|k| k.id).collect();
        assert_eq!(first, again);
        let unique: std::collections::HashSet<&String> = first.iter().collect();
        assert_eq!(unique.len(), 4, "{first:?}");
        assert!(
            first
                .iter()
                .all(|id| id.len() == ID_LEN && id.bytes().all(|b| b.is_ascii_hexdigit()))
        );
        let one = &first[1];
        assert_eq!(find(root, one).unwrap().id, *one);
        assert!(find(root, "000000000000").is_none());
    }

    /// A draft room's text is kept in its owner's folder and listed with the
    /// owner; the same path in the domain's own text is another entry with
    /// another id. An owner that is not one folder name is refused.
    #[test]
    fn a_drafts_text_is_kept_under_its_owner() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let kind = KeptKind::Engram;
        let file = write(root, "eng", Some("ada"), kind, "a.md", at(8), "draft").unwrap();
        assert_eq!(
            file,
            root.join("eng/draft/ada/engram/a.md.20261015T080000Z.md")
        );
        write(root, "eng", None, kind, "a.md", at(8), "folder").unwrap();
        let found = scan(root);
        let rows: Vec<(Option<&str>, &str, u64)> = found
            .iter()
            .map(|k| (k.owner.as_deref(), k.path.as_str(), k.bytes))
            .collect();
        assert_eq!(rows, vec![(None, "a.md", 6), (Some("ada"), "a.md", 5)]);
        assert_ne!(found[0].id, found[1].id);
        for owner in ["", ".", "..", "a/b", "a\\b", "c:"] {
            assert!(
                write(root, "eng", Some(owner), kind, "a.md", at(8), "x").is_err(),
                "{owner}"
            );
        }
    }

    /// Names outside the layout, unknown kind folders and loose files are
    /// not kept texts.
    #[test]
    fn scan_skips_what_is_not_a_kept_text() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let folder = root.join("eng/reflection");
        std::fs::create_dir_all(&folder).unwrap();
        for name in [
            "a.md",
            "a.md.20261015T080000Z.txt",
            "a.md.2026-10-15.md",
            ".20261015T080000Z.md",
            "a.md.20261015T080000Z-.md",
            "a.md.20261015T080000Z-x.md",
        ] {
            std::fs::write(folder.join(name), "x").unwrap();
        }
        std::fs::create_dir_all(root.join("eng/other")).unwrap();
        std::fs::write(root.join("eng/other/a.md.20261015T080000Z.md"), "x").unwrap();
        std::fs::write(root.join("loose.md.20261015T080000Z.md"), "x").unwrap();
        assert!(scan(root).is_empty(), "{:?}", scan(root));
        assert!(scan(&root.join("missing")).is_empty());
    }

    /// `remove` deletes the one file and the folders it leaves empty, never
    /// the kept folder itself and never a neighbour.
    #[test]
    fn remove_deletes_one_file_and_its_empty_folders() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root,
            "eng",
            None,
            KeptKind::Reflection,
            "guides/deep/a.md",
            at(8),
            "1",
        )
        .unwrap();
        write(root, "eng", None, KeptKind::Reflection, "b.md", at(9), "2").unwrap();
        let found = scan(root);
        remove(root, &found[0]).unwrap();
        assert!(!root.join("eng/reflection/guides").exists());
        assert!(root.join("eng/reflection").is_dir());
        let left = scan(root);
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].path, "b.md");
        remove(root, &left[0]).unwrap();
        assert!(root.is_dir(), "the kept folder stays");
        assert!(!root.join("eng").exists());
    }
}
